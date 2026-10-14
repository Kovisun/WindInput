//! `DictLayer` 适配（设计 §4）：快照 + 增量表叠加成候选。
//!
//! 叠加规则：Delete 剔除、SetWeight 覆盖、Upsert 并入或覆盖、Incr / Touch 不影响候选。
//! 候选字段照 `wind-dict/src/store_layer.rs::record_to_candidate`：`meta.is_temp_dict`、
//! `meta.raw_weight`、`natural_order`（快照全局序号；delta 里新来的排在同权快照条目之后）、
//! `boundary`。简拼召回按 trait 约定返回**超集**（声母组 + 首字符的无边界兜底组），判据留在
//! 引擎侧；`search_abbrev_exact` 只给声母组（不含无边界词）。

use crate::delta::{DeltaOp, Entry, group_of};
use crate::engine::UserData;
use crate::record::Target;
use std::collections::BTreeMap;
use std::sync::Arc;
use wind_candidate::{Candidate, better};
use wind_dict::{DictLayer, LayerType};

pub struct UserDataTempLayer {
    ud: Arc<UserData>,
    schema: String,
}

/// delta 里新来的条目（快照里没有）的自然序：排在同权的快照条目之后。
const DELTA_ORDER: i32 = i32::MAX - 1;

impl UserDataTempLayer {
    pub fn new(ud: Arc<UserData>, schema: &str) -> Self {
        Self {
            ud,
            schema: schema.to_string(),
        }
    }

    fn candidate(
        code: &str,
        text: &str,
        weight: i32,
        boundary: u64,
        order: i32,
        is_prefix: bool,
    ) -> Candidate {
        let mut c = Candidate {
            text: text.to_string(),
            code: code.to_string(),
            weight,
            boundary,
            is_prefix,
            natural_order: order,
            ..Default::default()
        };
        c.meta.raw_weight = weight;
        c.meta.is_temp_dict = true;
        c
    }

    /// 快照命中 `(code, text, weight, boundary, entry_idx)` 与 delta 条目叠加成候选。
    /// `query_code` 非空时命中的 code 等于它的 `is_prefix = false`，其余 true。
    fn overlay(
        hits: Vec<(String, String, i32, u64, u32)>,
        delta: Vec<&Entry>,
        query_code: Option<&str>,
    ) -> Vec<Candidate> {
        // key = (code, text) → (weight, boundary, order)
        let mut map: BTreeMap<(String, String), (i32, u64, i32)> = hits
            .into_iter()
            .map(|(c, t, w, b, idx)| ((c, t), (w, b, idx as i32)))
            .collect();
        let mut ops = delta;
        ops.sort_by_key(|e| e.seq);
        for op in ops {
            let key = (op.code.clone(), op.text.clone());
            match op.op {
                DeltaOp::Upsert { weight, .. } => {
                    let order = map.get(&key).map(|v| v.2).unwrap_or(DELTA_ORDER);
                    map.insert(key, (weight, op.boundary, order));
                }
                DeltaOp::Delete => {
                    map.remove(&key);
                }
                DeltaOp::SetWeight(w) => {
                    if let Some(v) = map.get_mut(&key) {
                        v.0 = w;
                    }
                }
                DeltaOp::Incr(_) | DeltaOp::Touch(_) => {}
            }
        }
        map.into_iter()
            .map(|((code, text), (w, b, order))| {
                let is_prefix = query_code.is_some_and(|q| q != code);
                Self::candidate(&code, &text, w, b, order, is_prefix)
            })
            .collect()
    }

    fn sort_trunc(mut v: Vec<Candidate>, limit: usize) -> Vec<Candidate> {
        v.sort_by(better);
        if limit > 0 {
            v.truncate(limit);
        }
        v
    }

    fn abbrev_groups(&self, groups: &[String], limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let snap = v.snapshots.get(&(Target::Temp, self.schema.clone()));
        let mut hits = Vec::new();
        let mut ops: Vec<&Entry> = Vec::new();
        for g in groups {
            if let Some(s) = snap {
                hits.extend(s.abbrev(g, 0));
            }
            ops.extend(v.delta.by_group(Target::Temp, &self.schema, g));
        }
        // 快照命中的键在 delta 里可能还有别的操作（如 Delete）：按键补齐全部操作
        let mut keys: Vec<(String, String)> =
            hits.iter().map(|h| (h.0.clone(), h.1.clone())).collect();
        keys.sort();
        keys.dedup();
        for (code, text) in &keys {
            ops.extend(v.delta.ops_for(Target::Temp, &self.schema, code, text));
        }
        ops.sort_by_key(|e| e.seq);
        ops.dedup_by_key(|e| e.seq);
        Self::sort_trunc(Self::overlay(hits, ops, None), limit)
    }
}

impl DictLayer for UserDataTempLayer {
    fn name(&self) -> &str {
        "userdata-temp"
    }

    fn layer_type(&self) -> LayerType {
        LayerType::Temp
    }

    fn search(&self, code: &str, limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let hits: Vec<(String, String, i32, u64, u32)> = v
            .snapshots
            .get(&(Target::Temp, self.schema.clone()))
            .map(|s| {
                s.exact(code)
                    .into_iter()
                    .map(|(t, w, b, idx)| (code.to_string(), t, w, b, idx))
                    .collect()
            })
            .unwrap_or_default();
        let ops = v.delta.exact(Target::Temp, &self.schema, code);
        Self::sort_trunc(Self::overlay(hits, ops, Some(code)), limit)
    }

    fn search_prefix(&self, prefix: &str, limit: usize) -> Vec<Candidate> {
        let v = self.ud.view();
        let ops = v.delta.prefix(Target::Temp, &self.schema, prefix);
        // 每段多取「delta 覆盖的条数」：被删条目占名额、被调高的条目可能不在前 limit 里
        let take = if limit == 0 { 0 } else { limit + ops.len() };
        let hits = v
            .snapshots
            .get(&(Target::Temp, self.schema.clone()))
            .map(|s| s.prefix(prefix, take))
            .unwrap_or_default();
        Self::sort_trunc(Self::overlay(hits, ops, Some(prefix)), limit)
    }

    fn search_abbrev(&self, abbrev: &str, limit: usize) -> Vec<Candidate> {
        let mut groups = vec![abbrev.to_string()];
        if let Some(c) = abbrev.chars().next() {
            groups.push(group_of(&c.to_string(), 0));
        }
        self.abbrev_groups(&groups, limit)
    }

    fn search_abbrev_exact(&self, abbrev: &str, limit: usize) -> Vec<Candidate> {
        self.abbrev_groups(&[abbrev.to_string()], limit)
            .into_iter()
            .filter(|c| c.boundary != 0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Dirs, UserData};
    use crate::record::{OpKind, Payload, Target};
    use std::sync::Arc;
    use wind_dict::DictLayer;

    fn names(v: Vec<wind_candidate::Candidate>) -> Vec<String> {
        v.into_iter().map(|c| c.text).collect()
    }

    #[test]
    fn layer_sees_snapshot_delta_and_deletes() {
        let d = std::env::temp_dir().join(format!("wind_layer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ud = Arc::new(
            UserData::open(Dirs {
                state_dir: d,
                dir_id: "x".into(),
            })
            .unwrap(),
        );
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihao",
            "你好",
            Payload::Weight(10),
            0b101,
        )
        .unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihao",
            "拟好",
            Payload::Weight(20),
            0b101,
        )
        .unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "wubi",
            "五笔",
            Payload::Weight(1),
            0,
        )
        .unwrap();
        ud.fold(Target::Temp, "py").unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Delete,
            "nihao",
            "拟好",
            Payload::None,
            0b101,
        )
        .unwrap();
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            "nihaoma",
            "你好吗",
            Payload::Weight(1),
            0b100101,
        )
        .unwrap();
        let layer = UserDataTempLayer::new(ud, "py");
        assert_eq!(layer.layer_type(), wind_dict::LayerType::Temp);
        assert_eq!(
            names(layer.search("nihao", 10)),
            vec!["你好"],
            "快照里的 + delta 删除生效"
        );
        let pre = layer.search_prefix("niha", 10);
        assert_eq!(
            names(pre.clone()),
            vec!["你好", "你好吗"],
            "快照 + delta 新增，按权重降序"
        );
        assert!(pre.iter().all(|c| c.is_prefix), "码都比前缀长");
        let pre = layer.search_prefix("nihao", 10);
        assert_eq!(names(pre.clone()), vec!["你好", "你好吗"]);
        assert!(
            !pre[0].is_prefix && pre[1].is_prefix,
            "恰等于前缀的不算前缀命中"
        );
        assert_eq!(
            names(layer.search_abbrev("nh", 10)),
            vec!["你好"],
            "简拼：快照里被删的不出现"
        );
        assert_eq!(
            names(layer.search_abbrev("nhm", 10)),
            vec!["你好吗"],
            "简拼：delta 新增能召回"
        );
        assert_eq!(
            names(layer.search_abbrev("w", 10)),
            vec!["五笔"],
            "无边界词走兜底组"
        );
        assert!(
            layer.search_abbrev_exact("w", 10).is_empty(),
            "exact 不含无边界词"
        );
        assert!(layer.has_longer_code("niha"));
        let c = &layer.search("nihao", 10)[0];
        assert!(c.meta.is_temp_dict && c.meta.raw_weight == 10 && c.boundary == 0b101);
    }
}

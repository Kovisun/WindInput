//! 引擎门面（设计 §3.4–§3.6）：打开（锁、设备、state、各目标快照、回放日志）、写入即发布、折回。
//!
//! 读路径无锁：按键线程 [`UserData::view`] 克隆一个 `Arc<View>` 后不再碰任何锁。写路径由一把
//! 写锁串行化（写者之间互斥、读者不等）：追加日志与发布快照在调用线程上同步完成，保证
//! 「写后立即可读」。折回在调用线程上做（P1 再放到独立线程），与写入靠同一把写锁协调：
//! 取「折回起点」与「水位」是一个原子动作，折回期间新到的操作留在 delta 里。

use crate::delta::{Delta, DeltaOp, Entry, entry_from_record};
use crate::device::{DeviceOutcome, Lock};
use crate::journal::Journal;
use crate::record::{OpKind, Payload, Record, Target};
use crate::snapshot::{Snapshot, SnapshotEntry};
use crate::state::{State, TargetState};
use anyhow::bail;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

pub struct Dirs {
    /// 本机状态目录（`<localdata>/state/`，设计 §2.1）。
    pub state_dir: PathBuf,
    /// 用户目录 `meta.toml` 里的 dir_id。
    pub dir_id: String,
}

/// 一个不可变的读视图：各 (目标, 数据方案) 的快照 + 未折回的增量表。
pub struct View {
    pub snapshots: BTreeMap<(Target, String), Arc<Snapshot>>,
    pub delta: Arc<Delta>,
}

pub struct UserData {
    dirs: Dirs,
    /// 只读实例为 `None`：它不打开日志写端，免得 open 时的截尾截掉写者正在写的那一条。
    journal: Mutex<Option<Journal>>,
    state_path: PathBuf,
    view: RwLock<Arc<View>>,
    state: Mutex<State>,
    lock: Option<Lock>,
    /// 写者之间互斥；apply 持它跨「追加 + 发布」，fold 持它跨「取水位 + 取起点」与「换入」。
    write_lock: Mutex<()>,
}

const TARGETS: [Target; 7] = [
    Target::User,
    Target::Temp,
    Target::Draft,
    Target::Freq,
    Target::Assoc,
    Target::Phrases,
    Target::Shadow,
];

fn target_name(t: Target) -> &'static str {
    match t {
        Target::User => "user",
        Target::Temp => "temp",
        Target::Draft => "draft",
        Target::Freq => "freq",
        Target::Assoc => "assoc",
        Target::Phrases => "phrases",
        Target::Shadow => "shadow",
    }
}

fn target_key(t: Target, schema: &str) -> String {
    format!("{}:{schema}", target_name(t))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn dir_id16(dir_id: &str) -> [u8; 16] {
    let mut b = [0u8; 16];
    for (i, x) in dir_id.bytes().take(16).enumerate() {
        b[i] = x;
    }
    b
}

type Snapshots = BTreeMap<(Target, String), Arc<Snapshot>>;

/// 枚举 `state_dir/<schema>/<target>` 已有的快照。打不开（sidecar 坏、对不上）的当零段，
/// 并记进第二个返回值：这些目标的水位不可信，要从日志整份回放。
fn enumerate_snapshots(state_dir: &std::path::Path) -> (Snapshots, BTreeSet<(Target, String)>) {
    let mut out = BTreeMap::new();
    let mut broken = BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(state_dir) else {
        return (out, broken);
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let Some(schema) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if schema == "journal" {
            continue;
        }
        for t in TARGETS {
            match crate::snapshot::open(&p, target_name(t)) {
                Ok(Some(s)) => {
                    out.insert((t, schema.clone()), Arc::new(s));
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        "快照 {}/{} 打不开，当零段并整份回放: {err}",
                        p.display(),
                        target_name(t)
                    );
                    broken.insert((t, schema.clone()));
                }
            }
        }
    }
    (out, broken)
}

/// 要回放的日志：本设备的 `.wjl` 与所有 `.wjl.sealed`（换号封存的旧日志，未折回的记录仍要
/// 可见），按 seq 合并。缺失的跳过。
fn replay_all(journal_dir: &std::path::Path, current: Option<&std::path::Path>) -> Vec<Record> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(journal_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_str().is_some_and(|s| s.ends_with(".wjl.sealed")))
        .collect();
    paths.sort();
    paths.extend(current.filter(|p| p.is_file()).map(|p| p.to_path_buf()));
    let mut out = Vec::new();
    for p in paths {
        match Journal::replay(&p, 0) {
            Ok(rs) => out.extend(rs),
            Err(err) => tracing::warn!("日志 {} 读不出，跳过: {err}", p.display()),
        }
    }
    out.sort_by_key(|r| r.seq);
    out
}

/// 把 delta 里某键的操作按 seq 叠加到快照条目上（折回语义，设计 §3.5 / 计划 Task 8）。
fn apply_ops(
    base: Option<SnapshotEntry>,
    ops: &[&Entry],
    code: &str,
    text: &str,
) -> Option<SnapshotEntry> {
    let mut cur = base;
    for op in ops {
        match op.op {
            DeltaOp::Upsert { weight, kind } => match &mut cur {
                Some(e) => {
                    e.weight = weight;
                    e.flags = kind;
                    e.boundary = op.boundary;
                }
                None => {
                    cur = Some(SnapshotEntry {
                        code: code.to_string(),
                        text: text.to_string(),
                        weight,
                        boundary: op.boundary,
                        count: 0,
                        created_at: now_ms(),
                        last_used: 0,
                        flags: kind,
                    })
                }
            },
            DeltaOp::Delete => cur = None,
            DeltaOp::SetWeight(w) => {
                if let Some(e) = &mut cur {
                    e.weight = w;
                }
            }
            DeltaOp::Incr(c) => {
                if let Some(e) = &mut cur {
                    e.count = e.count.saturating_add(c);
                }
            }
            DeltaOp::Touch(t) => {
                if let Some(e) = &mut cur {
                    e.last_used = t;
                }
            }
        }
    }
    cur
}

/// 旧快照 + delta（该目标、该方案、seq ≤ upto）→ 按 (code, text) 排好序的新快照条目。
fn merge_for_fold(
    snapshot: Option<&Snapshot>,
    delta: &Delta,
    target: Target,
    schema: &str,
    upto: u64,
) -> Vec<SnapshotEntry> {
    let mut map: BTreeMap<(String, String), SnapshotEntry> = BTreeMap::new();
    if let Some(s) = snapshot {
        s.for_each(&mut |e| {
            map.insert((e.code.clone(), e.text.clone()), e);
        });
    }
    // delta 里涉及的键
    let mut keys: Vec<(String, String)> = delta
        .prefix(target, schema, "")
        .into_iter()
        .filter(|e| e.seq <= upto)
        .map(|e| (e.code.clone(), e.text.clone()))
        .collect();
    keys.sort();
    keys.dedup();
    for (code, text) in keys {
        let ops: Vec<&Entry> = delta
            .ops_for(target, schema, &code, &text)
            .into_iter()
            .filter(|e| e.seq <= upto)
            .collect();
        let base = map.remove(&(code.clone(), text.clone()));
        if let Some(e) = apply_ops(base, &ops, &code, &text) {
            map.insert((code, text), e);
        }
    }
    map.into_values().collect()
}

/// 快照命中 + delta 操作 → 候选 `(text, weight)`，权重降序、同权按 text。
fn overlay(snapshot_hits: Vec<(String, i32, u64, u32)>, ops: Vec<&Entry>) -> Vec<(String, i32)> {
    let mut map: BTreeMap<String, i32> = snapshot_hits
        .into_iter()
        .map(|(t, w, _, _)| (t, w))
        .collect();
    for op in ops {
        match op.op {
            DeltaOp::Upsert { weight, .. } => {
                map.insert(op.text.clone(), weight);
            }
            DeltaOp::Delete => {
                map.remove(&op.text);
            }
            DeltaOp::SetWeight(w) => {
                if let Some(v) = map.get_mut(&op.text) {
                    *v = w;
                }
            }
            DeltaOp::Incr(_) | DeltaOp::Touch(_) => {}
        }
    }
    let mut v: Vec<(String, i32)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

impl UserData {
    pub fn open(dirs: Dirs) -> anyhow::Result<UserData> {
        let lock = crate::device::try_lock(&dirs.state_dir)?;
        let journal_dir = dirs.state_dir.join("journal");
        // 只读实例不碰 device.toml：换号会把写者正在追加的日志改名封存。
        let device_id = if lock.is_some() {
            match crate::device::load_or_renew(&dirs.state_dir, &dirs.dir_id, &journal_dir)? {
                DeviceOutcome::Same(d) => Some(d.device_id),
                DeviceOutcome::Renewed {
                    new,
                    sealed_journal,
                } => {
                    if let Some(s) = sealed_journal {
                        tracing::info!("设备标识已更新，旧日志已封存: {}", s.display());
                    }
                    Some(new.device_id)
                }
            }
        } else {
            crate::device::load_existing(&dirs.state_dir)
        };
        let jpath =
            device_id.map(|id| journal_dir.join(format!("{}.wjl", crate::device::hex(&id))));
        let mut journal = match (&lock, &jpath, device_id) {
            (Some(_), Some(p), Some(id)) => Some(Journal::open(p, id, dir_id16(&dirs.dir_id))?),
            _ => None,
        };
        let state_path = journal_dir.join("state");
        let mut state = State::load(&state_path).unwrap_or_default();

        let (snapshots, broken) = enumerate_snapshots(&dirs.state_dir);
        let records = replay_all(&journal_dir, jpath.as_deref());
        // 续号取所有已知水位之上：state、各快照内嵌水位、各折回水位、日志尾（含封存日志）。
        // 少了任何一个，删掉 journal/ 或换号之后新记录的 seq 就会落到水位之下，重开即被跳过。
        let floor = [
            state.next_seq,
            snapshots.values().map(|s| s.max_seq + 1).max().unwrap_or(0),
            state
                .targets
                .values()
                .map(|t| t.folded_seq + 1)
                .max()
                .unwrap_or(0),
            records.last().map(|r| r.seq + 1).unwrap_or(0),
        ]
        .into_iter()
        .max()
        .unwrap_or(1);
        if let Some(j) = journal.as_mut() {
            j.bump_next_seq(floor);
            state.next_seq = j.next_seq();
        }

        // 回放：每条记录只在 seq 高于「该目标快照内嵌水位」与「state 记的折回水位」两者的
        // 较大者时进 delta——state 丢了也不会把已折回的记录再应用一遍（§3.6）。快照坏了的
        // 目标水位不可信，整份回放。
        let mut delta = Delta::default();
        for r in records {
            let key = (r.target, r.schema.clone());
            let wm = if broken.contains(&key) {
                0
            } else {
                let snap_wm = snapshots.get(&key).map(|s| s.max_seq).unwrap_or(0);
                let state_wm = state
                    .targets
                    .get(&target_key(r.target, &r.schema))
                    .map(|t| t.folded_seq)
                    .unwrap_or(0);
                snap_wm.max(state_wm)
            };
            if r.seq > wm
                && let Some(e) = entry_from_record(&r)
            {
                delta = delta.with(e);
            }
        }
        Ok(UserData {
            dirs,
            journal: Mutex::new(journal),
            state_path,
            view: RwLock::new(Arc::new(View {
                snapshots,
                delta: Arc::new(delta),
            })),
            state: Mutex::new(state),
            lock,
            write_lock: Mutex::new(()),
        })
    }

    /// 克隆当前视图的 Arc；读锁只持这一瞬。
    pub fn view(&self) -> Arc<View> {
        self.view.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 只读模式：另一个实例持有锁（§3.7）。查询正常，写入返回 Err。
    pub fn readonly(&self) -> bool {
        self.lock.is_none()
    }

    fn publish(&self, v: View) {
        *self.view.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(v);
    }

    /// 追加日志 + 发布新视图，返回 seq。调用线程上同步完成（写后立即可读）。
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        target: Target,
        schema: &str,
        kind: OpKind,
        code: &str,
        text: &str,
        payload: Payload,
        boundary: u64,
    ) -> anyhow::Result<u64> {
        if self.readonly() {
            bail!(
                "只读模式：另一个实例持有 {}",
                self.dirs.state_dir.join("lock").display()
            );
        }
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let seq = {
            let mut guard = self.journal.lock().unwrap_or_else(|e| e.into_inner());
            let Some(j) = guard.as_mut() else {
                bail!("只读模式");
            };
            let seq = j.next_seq();
            let rec = Record {
                seq,
                ts_ms: now_ms(),
                kind,
                target,
                schema: schema.into(),
                code: code.into(),
                text: text.into(),
                payload,
                boundary,
            };
            j.append(&rec)?;
            if let Some(e) = entry_from_record(&rec) {
                let cur = self.view();
                self.publish(View {
                    snapshots: cur.snapshots.clone(),
                    delta: Arc::new(cur.delta.with(e)),
                });
            }
            seq
        };
        Ok(seq)
    }

    /// 旧快照 + delta → 新快照 → 换入 → 摘 delta → state（设计 §3.5 次序）。
    pub fn fold(&self, target: Target, schema: &str) -> anyhow::Result<()> {
        if self.readonly() {
            bail!("只读模式");
        }
        // 水位与起点是一个原子动作：apply 持写锁跨「追加 + 发布」，所以这里拿到的视图
        // 一定含有全部 seq ≤ upto 的操作。
        let (upto, base) = {
            let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let upto = match self
                .journal
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                Some(j) => j.next_seq() - 1,
                None => bail!("只读模式"),
            };
            (upto, self.view())
        };
        let key = (target, schema.to_string());
        let merged = merge_for_fold(
            base.snapshots.get(&key).map(|s| s.as_ref()),
            &base.delta,
            target,
            schema,
            upto,
        );
        let dir = self.dirs.state_dir.join(schema);
        let name = target_name(target);
        let new_snap = if merged.is_empty() {
            crate::snapshot::remove(&dir, name);
            None
        } else {
            Some(Arc::new(crate::snapshot::build(
                &dir,
                name,
                upto,
                merged.into_iter(),
            )?))
        };
        {
            let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let cur = self.view();
            let mut snapshots = cur.snapshots.clone();
            match new_snap {
                Some(s) => {
                    snapshots.insert(key.clone(), s);
                }
                None => {
                    snapshots.remove(&key);
                }
            }
            // 只摘 seq ≤ upto 的该目标条目：折回期间新到的操作留下（Review Focus 4）
            let delta = Arc::new(cur.delta.drop_folded_for_schema(target, schema, upto));
            self.publish(View { snapshots, delta });
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.targets.insert(
            target_key(target, schema),
            TargetState {
                folded_seq: upto,
                text_fp: None,
                snapshot_fp: String::new(),
            },
        );
        st.next_seq = upto + 1;
        st.save(&self.state_path)
    }

    /// 调试 / 适配层用：快照 + delta 叠加后的候选 `(text, weight)`。
    pub fn query_exact(&self, target: Target, schema: &str, code: &str) -> Vec<(String, i32)> {
        let v = self.view();
        let hits = v
            .snapshots
            .get(&(target, schema.to_string()))
            .map(|s| s.exact(code))
            .unwrap_or_default();
        overlay(hits, v.delta.exact(target, schema, code))
    }

    /// 某条的使用计数：快照里的 count + delta 里的 Incr 之和。
    pub fn query_count(&self, target: Target, schema: &str, code: &str, text: &str) -> u32 {
        let v = self.view();
        let base = v
            .snapshots
            .get(&(target, schema.to_string()))
            .and_then(|s| {
                s.exact(code)
                    .into_iter()
                    .find(|h| h.0 == text)
                    .map(|h| s.meta_of(h.3).0)
            })
            .unwrap_or(0);
        v.delta
            .ops_for(target, schema, code, text)
            .iter()
            .map(|e| match e.op {
                DeltaOp::Incr(c) => c,
                _ => 0,
            })
            .fold(base, |a, b| a.saturating_add(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(ud: &UserData, code: &str, text: &str) {
        ud.apply(
            Target::Temp,
            "py",
            OpKind::Upsert,
            code,
            text,
            Payload::Weight(1),
            0,
        )
        .unwrap();
    }

    /// 折回写到一半崩溃（新 wdat 已写、sidecar 未写）：重开后已折回的与未折回的都在。
    #[test]
    fn crash_mid_fold_loses_nothing() {
        let d = std::env::temp_dir().join(format!("wind_ud_halffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let dirs = || Dirs {
            state_dir: d.clone(),
            dir_id: "dir".into(),
        };
        let ud = UserData::open(dirs()).unwrap();
        for (c, t) in [("a", "甲"), ("b", "乙"), ("c", "丙")] {
            up(&ud, c, t);
        }
        ud.fold(Target::Temp, "py").unwrap();
        up(&ud, "d", "丁");
        crate::snapshot::FAIL_BEFORE_META.with(|f| f.set(true));
        assert!(ud.fold(Target::Temp, "py").is_err());
        drop(ud);
        let ud = UserData::open(dirs()).unwrap();
        for (c, t) in [("a", "甲"), ("b", "乙"), ("c", "丙"), ("d", "丁")] {
            assert_eq!(
                ud.query_exact(Target::Temp, "py", c),
                vec![(t.to_string(), 1)],
                "{c}"
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

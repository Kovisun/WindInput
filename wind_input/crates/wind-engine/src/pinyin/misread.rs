//! 误读表（设计 `docs/design/pinyin-correction-and-hints.md` §6）：常见读错的音 → 正确读音。
//!
//! 用途两个：
//! - **提示**：候选文字 + 用户打的拼音双重命中表项 ⇒ 按表标正确拼音（`HintKind::Misread`），
//!   与模糊 / 纠错来源的提示冲突时表优先。
//! - **召回**：整串音节等于某条的错误读音 ⇒ 词库读不出也以低权重放进候选。
//!
//! 内置一份（`schemas/pinyin_misread.yaml`）+ 用户目录追加一份（`<userdata>/data/pinyin_misread.yaml`），
//! 两份格式相同；用户表同键（文字 + 错误读音）覆盖内置。

use std::collections::HashMap;
use tracing::warn;

/// 内置表在 `schemas/` 下的相对路径，也是用户追加表在用户数据目录下的文件名。
pub const MISREAD_FILE: &str = wind_store::MISREAD_FILE;

/// meta 头与列表体的分隔行（与 librime `.dict.yaml`、字符类文件一致）。
const HEAD_BODY_SEP: &str = "...";

/// 一条误读：`wrong` / `correct` 一字一音节、与 `text` 字数相同，且两者不同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MisreadEntry {
    pub text: String,
    pub wrong: Vec<String>,
    pub correct: Vec<String>,
}

impl MisreadEntry {
    /// 错误读音连写（`xinxin`），召回的键。
    fn wrong_code(&self) -> String {
        self.wrong.concat()
    }
}

/// 文件里的一条（列表体的一个 mapping）。未知字段（`note`、`group` 等）忽略。
#[derive(serde::Deserialize)]
struct RawEntry {
    text: String,
    wrong: OneOrMany,
    right: String,
}

/// `wrong` 可写一个读音，也可写读音列表（同一个词有几种常见读错法）。
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

/// 误读表：条目 + 两张索引（文字 → 条目、错误读音连写 → 条目）。相等 = 条目逐条相同。
#[derive(Debug, Default)]
pub struct MisreadTable {
    entries: Vec<MisreadEntry>,
    by_text: HashMap<String, Vec<usize>>,
    by_wrong: HashMap<String, Vec<usize>>,
}

impl PartialEq for MisreadTable {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl MisreadTable {
    /// 解析 YAML 误读表：meta 头（`name` / `version` / `description`，引擎不读）、`...` 行、
    /// 列表体（每项 `{text, wrong, right}`，读音以空格分隔音节、不带声调，`ü` 记作 `v`）。
    ///
    /// 坏条目（缺字段、音节非纯字母、音节数与字数不等、错误读音与正确读音相同）warn 跳过；
    /// 列表体整体不是合法 YAML 时逐行解析，只丢坏的那几行。
    pub fn parse(src: &str, origin: &str) -> Self {
        let body = body_of(src);
        let items: Vec<serde_yaml::Value> = match serde_yaml::from_str(body) {
            Ok(serde_yaml::Value::Sequence(items)) => items,
            Ok(serde_yaml::Value::Null) => Vec::new(),
            Ok(_) => {
                warn!("误读表 {origin} 的列表体不是列表，已忽略");
                Vec::new()
            }
            Err(e) => {
                warn!("误读表 {origin} 不是合法 YAML（{e}），改为逐行解析");
                body.lines()
                    .filter(|l| l.trim_start().starts_with('-'))
                    .filter_map(|l| {
                        let v = serde_yaml::from_str::<Vec<serde_yaml::Value>>(l).ok();
                        if v.is_none() {
                            warn!("误读表 {origin} 有一行不是合法 YAML，已跳过");
                        }
                        v
                    })
                    .flatten()
                    .collect()
            }
        };
        let mut entries = Vec::new();
        for (no, item) in items.into_iter().enumerate() {
            match entries_of(item) {
                Some(es) => entries.extend(es),
                // 日志不带条目内容（词库内容不进日志），序号足够定位。
                None => warn!("误读表 {origin} 第 {} 条格式不对，已跳过", no + 1),
            }
        }
        Self::from_entries(entries)
    }

    /// 用户表同键（文字 + 错误读音）覆盖内置，其余追加。
    pub fn merge(builtin: Self, user: Self) -> Self {
        let mut entries = builtin.entries;
        for u in user.entries {
            match entries
                .iter_mut()
                .find(|e| e.text == u.text && e.wrong == u.wrong)
            {
                Some(e) => *e = u,
                None => entries.push(u),
            }
        }
        Self::from_entries(entries)
    }

    /// 文字 + 用户打的音节（连写比较，切法不论）双重命中的条目。
    pub fn lookup(&self, text: &str, typed: &[&str]) -> Option<&MisreadEntry> {
        let ids = self.by_text.get(text)?;
        let typed = typed.concat();
        ids.iter()
            .map(|&i| &self.entries[i])
            .find(|e| e.wrong_code() == typed)
    }

    /// 错误读音连写恰为 `typed_code` 的条目（召回用）。
    pub fn recall(&self, typed_code: &str) -> impl Iterator<Item = &MisreadEntry> {
        self.by_wrong
            .get(typed_code)
            .into_iter()
            .flatten()
            .map(|&i| &self.entries[i])
    }

    /// 表里有没有这个文字（热路径先零分配判掉）。
    pub fn has_text(&self, text: &str) -> bool {
        self.by_text.contains_key(text)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn from_entries(entries: Vec<MisreadEntry>) -> Self {
        let mut by_text: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_wrong: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            by_text.entry(e.text.clone()).or_default().push(i);
            by_wrong.entry(e.wrong_code()).or_default().push(i);
        }
        Self {
            entries,
            by_text,
            by_wrong,
        }
    }
}

/// `...` 行之后的列表体；没有分隔行时整份都是列表体。
fn body_of(src: &str) -> &str {
    let mut at = 0;
    for line in src.split_inclusive('\n') {
        at += line.len();
        if line.trim_end() == HEAD_BODY_SEP {
            return &src[at..];
        }
    }
    src
}

/// 一项展开成条目（`wrong` 是列表时每个读音一条）；任一处不合格整项作废。
fn entries_of(item: serde_yaml::Value) -> Option<Vec<MisreadEntry>> {
    let raw: RawEntry = serde_yaml::from_value(item).ok()?;
    let text = raw.text.trim();
    let n = text.chars().count();
    let correct = syllables(&raw.right).filter(|c| c.len() == n)?;
    let wrongs = match raw.wrong {
        OneOrMany::One(w) => vec![w],
        OneOrMany::Many(ws) if !ws.is_empty() => ws,
        OneOrMany::Many(_) => return None,
    };
    wrongs
        .iter()
        .map(|w| {
            let wrong = syllables(w).filter(|w| w.len() == n && *w != correct)?;
            Some(MisreadEntry {
                text: text.to_string(),
                wrong,
                correct: correct.clone(),
            })
        })
        .collect()
}

/// 空格分隔的音节，小写、`ü` → `v`；空或含非字母 ⇒ `None`。
fn syllables(col: &str) -> Option<Vec<String>> {
    let out: Vec<String> = col
        .split_whitespace()
        .map(|s| s.to_lowercase().replace('ü', "v"))
        .collect();
    let ok = !out.is_empty()
        && out
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase()));
    ok.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syls(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    fn table(body: &str) -> MisreadTable {
        MisreadTable::parse(&format!("---\nname: t\nversion: \"1\"\n...\n{body}"), "t")
    }

    #[test]
    fn parses_flow_entries_after_meta_head() {
        let t = table(
            "# 形声字\n\
             - {text: 莘莘, wrong: xin xin, right: shen shen}\n\
             - {text: 酗酒, wrong: xiong jiu, right: xu jiu, note: 酗读xù, group: 形声}\n",
        );
        assert_eq!(t.len(), 2);
        let e = t.lookup("莘莘", &["xin", "xin"]).expect("应收");
        assert_eq!(e.correct, syls("shen shen"));
        assert!(
            t.lookup("酗酒", &["xiongjiu"]).is_some(),
            "切法不论、未知字段忽略"
        );
    }

    #[test]
    fn wrong_list_expands_to_one_entry_each() {
        let t = table("- {text: 炽热, wrong: [zhi re, zhi le], right: chi re}\n");
        assert_eq!(t.len(), 2);
        assert!(t.lookup("炽热", &["zhire"]).is_some());
        assert!(t.lookup("炽热", &["zhile"]).is_some());
    }

    #[test]
    fn bad_entries_are_skipped() {
        let t = table(
            "- {text: 馄饨, wrong: hun dun}\n\
             - {text: 炽热, wrong: zhi, right: chi re}\n\
             - {text: 龋齿, wrong: yu chi, right: qu}\n\
             - {text: 呆板, wrong: dai ban, right: dai ban}\n\
             - {text: 惬意, wrong: [], right: qie yi}\n\
             - {text: 酗酒, wrong: xiong9 jiu, right: xu jiu}\n\
             - 一行字符串\n\
             - {text: 莘莘, wrong: xin xin, right: shen shen}\n",
        );
        assert_eq!(
            t.len(),
            1,
            "缺字段、音节数不等、错读同正读、空列表、非字母、非映射都跳过"
        );
        assert!(t.has_text("莘莘"));
    }

    #[test]
    fn broken_yaml_loses_only_bad_lines() {
        let t = table(
            "- {text: 莘莘, wrong: xin xin, right: shen shen}\n\
             - {text: 馄饨, wrong: hun dun, right: [hun tun\n\
             - {text: 酗酒, wrong: xiong jiu, right: xu jiu}\n",
        );
        assert_eq!(t.len(), 2);
        assert!(t.has_text("酗酒"));
    }

    #[test]
    fn without_meta_head_whole_file_is_body() {
        let t = MisreadTable::parse("- {text: 莘莘, wrong: xin xin, right: shen shen}\n", "t");
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn user_table_overrides_builtin() {
        let builtin = table(
            "- {text: 莘莘, wrong: xin xin, right: shen shen}\n\
             - {text: 馄饨, wrong: hun dun, right: hun tun}\n",
        );
        let user = table(
            "- {text: 馄饨, wrong: hun dun, right: hun tun, note: 用户改}\n\
             - {text: 炽热, wrong: zhi re, right: chi re}\n\
             - {text: 莘莘, wrong: xin xin, right: xin xin}\n",
        );
        let t = MisreadTable::merge(builtin, user);
        assert_eq!(t.len(), 3, "同键覆盖、新键追加、坏项跳过");
        assert!(t.lookup("炽热", &["zhi", "re"]).is_some());
        assert_eq!(
            t.lookup("莘莘", &["xin", "xin"]).unwrap().correct,
            syls("shen shen"),
            "用户坏项不覆盖内置"
        );
    }

    #[test]
    fn recall_by_whole_wrong_code() {
        let t = table(
            "- {text: 堤防, wrong: ti fang, right: di fang}\n\
             - {text: 提防, wrong: ti fang, right: di fang}\n\
             - {text: 莘莘, wrong: xin xin, right: shen shen}\n",
        );
        let mut got: Vec<&str> = t.recall("tifang").map(|e| e.text.as_str()).collect();
        got.sort();
        assert_eq!(got, ["堤防", "提防"]);
        assert_eq!(t.recall("xin").count(), 0, "只认整串");
    }

    #[test]
    fn lookup_needs_both_text_and_reading() {
        let t = table("- {text: 莘莘, wrong: xin xin, right: shen shen}\n");
        assert!(t.lookup("莘莘", &["shen", "shen"]).is_none());
        assert!(t.lookup("欣欣", &["xin", "xin"]).is_none());
    }

    #[test]
    fn umlaut_is_written_as_v() {
        let t = table("- {text: 戏谑, wrong: xi nüe, right: xi xue}\n");
        assert!(t.lookup("戏谑", &["xinve"]).is_some());
    }

    fn builtin_src() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../data/schemas/pinyin_misread.yaml"
        );
        std::fs::read_to_string(path).expect("内置误读表应存在")
    }

    /// 内置表每项都收得进来（坏项会被跳过，条数对不上就是有坏项），且没有重复的「文字 + 错误读音」。
    #[test]
    fn builtin_table_parses_cleanly() {
        let src = builtin_src();
        let items: Vec<serde_yaml::Value> =
            serde_yaml::from_str(body_of(&src)).expect("内置表应是合法 YAML 列表");
        let expanded: usize = items
            .iter()
            .map(|v| match v.get("wrong") {
                Some(serde_yaml::Value::Sequence(ws)) => ws.len(),
                _ => 1,
            })
            .sum();
        let t = MisreadTable::parse(&src, "builtin");
        assert_eq!(t.len(), expanded, "内置表有坏项");
        let mut keys = std::collections::HashSet::new();
        let dup: Vec<String> = t
            .entries
            .iter()
            .filter(|e| !keys.insert((e.text.clone(), e.wrong_code())))
            .map(|e| e.text.clone())
            .collect();
        assert!(dup.is_empty(), "重复条目：{dup:?}");
        assert!(t.len() >= 400, "内置表 {} 条", t.len());
    }

    /// 内置表对照随包词库（`build_dev/data` 的 cn_dicts，不含错音表 `corrections`）：每条的正确读音
    /// 是词库登记的读音，错误读音**不是**词库登记的读音（否则那是合法读法，不该标错）。
    /// 读大词库较慢且依赖本地数据，故 `#[ignore]`；改表后手动跑：
    /// `cargo test -p wind-engine --lib builtin_table_matches_dict_readings -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn builtin_table_matches_dict_readings() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../");
        let table = MisreadTable::parse(&builtin_src(), "builtin");
        let dicts = format!("{root}build_dev/data/schemas/pinyin/cn_dicts");
        let mut readings: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for entry in std::fs::read_dir(&dicts).expect("需要 build_dev/data 词库") {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".dict.yaml") || name.starts_with("corrections") {
                continue;
            }
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                let mut cols = line.split('\t');
                if let (Some(t), Some(code)) = (cols.next(), cols.next())
                    && table.has_text(t)
                {
                    readings
                        .entry(t.to_string())
                        .or_default()
                        .insert(code.to_string());
                }
            }
        }
        let has = |e: &MisreadEntry, r: &[String]| {
            readings
                .get(&e.text)
                .is_some_and(|set| set.contains(&r.join(" ")))
        };
        let missing: Vec<String> = table
            .entries
            .iter()
            .filter(|e| !has(e, &e.correct))
            .map(|e| format!("{} {}", e.text, e.correct.join(" ")))
            .collect();
        let legit_wrong: Vec<String> = table
            .entries
            .iter()
            .filter(|e| has(e, &e.wrong))
            .map(|e| format!("{} {}", e.text, e.wrong.join(" ")))
            .collect();
        println!(
            "核对 {} 条：词库缺正确读音 {} 条，错误读音是词库读音 {} 条",
            table.len(),
            missing.len(),
            legit_wrong.len()
        );
        assert!(missing.is_empty(), "词库里没有这些正确读音：{missing:?}");
        assert!(
            legit_wrong.is_empty(),
            "这些「错误读音」词库有登记：{legit_wrong:?}"
        );
    }
}

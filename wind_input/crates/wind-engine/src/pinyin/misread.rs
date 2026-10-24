//! 误读表（设计 `docs/design/pinyin-correction-and-hints.md` §6.2）：常见读错的音 → 正确读音。
//!
//! 用途两个：
//! - **提示**：候选文字 + 用户打的拼音双重命中表项 ⇒ 按表标正确拼音（`HintKind::Misread`），
//!   与自动推断（模糊 / 纠错）冲突时表优先；白名单条目（无正确读音）压掉自动推断的提示。
//! - **召回**：整串音节等于某条的错误读音 ⇒ 词库读不出也以低权重放进候选（白名单不召回）。
//!
//! 内置一份（`schemas/pinyin_misread.txt`）+ 用户目录追加一份（`<userdata>/data/pinyin_misread.txt`），
//! 用户表同键（文字 + 错误读音）覆盖内置。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tracing::warn;
use wind_candidate::{HintKind, PinyinHint};
use wind_dict::cached::ReverseIndex;

use super::fuzzy::{FuzzyConfig, FuzzyMatcher};

/// 内置表在 `schemas/` 下的相对路径，也是用户追加表在用户数据目录下的文件名。
pub const MISREAD_FILE: &str = "pinyin_misread.txt";

/// 一条误读：`wrong` / `correct` 一字一音节、与 `text` 字数相同。`correct = None` 为白名单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MisreadEntry {
    pub text: String,
    pub wrong: Vec<String>,
    pub correct: Option<Vec<String>>,
}

impl MisreadEntry {
    /// 错误读音连写（`xinxin`），召回的键。
    fn wrong_code(&self) -> String {
        self.wrong.concat()
    }
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
    /// 行格式：文字<Tab>错误拼音<Tab>正确拼音（空格分隔音节；第三列空 = 白名单）；# 注释；坏行 warn 跳过。
    ///
    /// 坏行：不足三列、音节不是纯字母、音节数与字数不等。`ü` 按词库写法记作 `v`。
    pub fn parse(src: &str, origin: &str) -> Self {
        let mut entries = Vec::new();
        for (no, line) in src.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            match parse_line(line) {
                Some(e) => entries.push(e),
                // 日志不带行内容（词库内容不进日志），行号足够定位。
                None => warn!("误读表 {origin} 第 {} 行格式不对，已跳过", no + 1),
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

    /// 文字 + 用户打的音节（连写比较，切法不论）双重命中的条目（含白名单）。
    pub fn lookup(&self, text: &str, typed: &[&str]) -> Option<&MisreadEntry> {
        let ids = self.by_text.get(text)?;
        let typed = typed.concat();
        ids.iter()
            .map(|&i| &self.entries[i])
            .find(|e| e.wrong_code() == typed)
    }

    /// 错误读音连写恰为 `typed_code` 的非白名单条目（召回用）。
    pub fn recall(&self, typed_code: &str) -> impl Iterator<Item = &MisreadEntry> {
        self.by_wrong
            .get(typed_code)
            .into_iter()
            .flatten()
            .map(|&i| &self.entries[i])
            .filter(|e| e.correct.is_some())
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

/// 自动推断误读（设计 §6.1）：候选文字是词库词，而用户打的音节与它**所有**登记读音都不同 ⇒
/// 取权重最高的读音，逐字标出不同处（`调(diao)研`）。
///
/// `readings`：该词全部登记读音（一字一音节）与权重。不标：单字、字数 ≠ 音节数、不在词库、
/// 某个读音就是用户打的、表里有这条（表优先，含白名单）、差异能被已开的模糊规则解释（归模糊来源）。
pub fn infer(
    text: &str,
    typed: &[&str],
    readings: &[(Vec<String>, u32)],
    table: &MisreadTable,
    fuzzy: &FuzzyConfig,
) -> Vec<PinyinHint> {
    let n = text.chars().count();
    if n < 2 || typed.len() != n || table.lookup(text, typed).is_some() {
        return Vec::new();
    }
    let same_len = || readings.iter().filter(|(r, _)| r.len() == n);
    if same_len().next().is_none() || readings.iter().any(|(r, _)| r.iter().eq(typed.iter())) {
        return Vec::new();
    }
    let explained_by_fuzzy = |r: &[String]| {
        r.iter().zip(typed).all(|(c, u)| {
            c == u
                || FuzzyMatcher::fuzzy_spelling_variants(c, fuzzy)
                    .iter()
                    .any(|v| v == u)
        })
    };
    if same_len().any(|(r, _)| explained_by_fuzzy(r)) {
        return Vec::new();
    }
    // 同权取 `readings` 里靠前的那条（`max_by_key` 取最后一个最大值，故反向找）；读音来自反查索引时
    // 即码长短、字典序小的在前，与词库登记次序无关。
    let Some((best, _)) = same_len().rev().max_by_key(|(_, w)| *w) else {
        return Vec::new();
    };
    best.iter()
        .zip(typed)
        .enumerate()
        .filter(|(_, (c, u))| c != *u)
        .map(|(i, (c, _))| PinyinHint {
            char_index: i as u16,
            correct: c.clone(),
            kind: HintKind::Misread,
        })
        .collect()
}

/// 一个方案的「词 → 读音」索引槽（拼音方案的反查索引）：后台建好后放进来，引擎每次查时读；
/// 空 = 未就绪或已释放，推断静默不标。克隆共享同一个槽。
#[derive(Clone, Default)]
pub struct ReadingSlot(Arc<RwLock<Option<Arc<ReverseIndex>>>>);

impl std::fmt::Debug for ReadingSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReadingSlot(ready={})", self.get().is_some())
    }
}

impl ReadingSlot {
    pub fn get(&self) -> Option<Arc<ReverseIndex>> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, idx: Option<Arc<ReverseIndex>>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = idx;
    }

    /// 槽表之外还有人（引擎）持有这个槽。
    pub fn in_use(&self) -> bool {
        Arc::strong_count(&self.0) > 1
    }
}

/// 管理器持有的各方案读音索引槽（方案 id → 槽）。建拼音引擎时按方案取槽注入。
#[derive(Clone, Default, Debug)]
pub struct ReadingSlots(Arc<Mutex<HashMap<String, ReadingSlot>>>);

impl ReadingSlots {
    /// 取 `schema_id` 的槽，没有就建一个空的。
    pub fn slot(&self, schema_id: &str) -> ReadingSlot {
        self.lock()
            .entry(schema_id.to_string())
            .or_default()
            .clone()
    }

    /// 已登记（建过拼音引擎）的方案 id。
    pub fn ids(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, ReadingSlot>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// 建拼音引擎时注入的误读来源：误读表（`None` = 不召回不标）与读音索引槽。
#[derive(Clone, Default, Debug)]
pub struct MisreadInputs {
    pub table: Option<Arc<MisreadTable>>,
    pub readings: ReadingSlots,
}

fn parse_line(line: &str) -> Option<MisreadEntry> {
    let mut cols = line.split('\t');
    let text = cols.next()?.trim();
    let wrong = syllables(cols.next()?)?;
    let correct_col = cols.next()?;
    let n = text.chars().count();
    if text.is_empty() || wrong.len() != n {
        return None;
    }
    let correct = if correct_col.trim().is_empty() {
        None
    } else {
        let c = syllables(correct_col)?;
        if c.len() != n {
            return None;
        }
        Some(c)
    };
    Some(MisreadEntry {
        text: text.to_string(),
        wrong,
        correct,
    })
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

    #[test]
    fn parse_skips_bad_lines() {
        let t = MisreadTable::parse(
            "# 注释\n\
             莘莘\txin xin\tshen shen\n\
             馄饨\thun dun\n\
             炽热\tzhi\tchi re\n\
             龋齿\tyu chi\tqu\n\
             \n\
             酗酒\txiong jiu\txu jiu\n",
            "t",
        );
        assert_eq!(t.len(), 2, "两列行、音节数不等的跳过，其余照收");
        assert!(t.lookup("莘莘", &["xin", "xin"]).is_some());
        assert!(t.lookup("酗酒", &["xiongjiu"]).is_some(), "切法不论");
        assert!(!t.has_text("馄饨"));
        assert!(!t.has_text("炽热"));
        assert!(!t.has_text("龋齿"));
    }

    #[test]
    fn whitelist_entry_has_no_correct() {
        let t = MisreadTable::parse("关卡\tguan ka\t\n", "t");
        let e = t.lookup("关卡", &["guan", "ka"]).expect("白名单也能查到");
        assert_eq!(e.correct, None);
        assert_eq!(t.recall("guanka").count(), 0, "白名单不召回");
    }

    #[test]
    fn user_table_overrides_builtin() {
        let builtin = MisreadTable::parse(
            "莘莘\txin xin\tshen shen\n馄饨\thun dun\thun tun\n",
            "builtin",
        );
        let user = MisreadTable::parse("馄饨\thun dun\t\n炽热\tzhi re\tchi re\n", "user");
        let t = MisreadTable::merge(builtin, user);
        assert_eq!(t.len(), 3);
        assert_eq!(
            t.lookup("馄饨", &["hun", "dun"]).unwrap().correct,
            None,
            "用户表同键覆盖内置"
        );
        assert_eq!(
            t.lookup("莘莘", &["xin", "xin"]).unwrap().correct,
            Some(syls("shen shen"))
        );
        assert!(t.lookup("炽热", &["zhi", "re"]).is_some());
    }

    #[test]
    fn recall_by_whole_wrong_code() {
        let t = MisreadTable::parse(
            "堤防\tti fang\tdi fang\n提防\tti fang\tdi fang\n莘莘\txin xin\tshen shen\n",
            "t",
        );
        let mut got: Vec<&str> = t.recall("tifang").map(|e| e.text.as_str()).collect();
        got.sort();
        assert_eq!(got, ["堤防", "提防"]);
        assert_eq!(t.recall("xin").count(), 0, "只认整串");
    }

    #[test]
    fn lookup_needs_both_text_and_reading() {
        let t = MisreadTable::parse("莘莘\txin xin\tshen shen\n", "t");
        assert!(t.lookup("莘莘", &["shen", "shen"]).is_none());
        assert!(t.lookup("欣欣", &["xin", "xin"]).is_none());
    }

    #[test]
    fn umlaut_is_written_as_v() {
        let t = MisreadTable::parse("戏谑\txi nüe\txi xue\n", "t");
        assert!(t.lookup("戏谑", &["xinve"]).is_some());
    }

    /// 内置表每条的正确读音都是随包词库登记的读音（`build_dev/data` 的 cn_dicts，不含错音表
    /// `corrections`）。读大词库较慢且依赖本地数据，故 `#[ignore]`；改表后手动跑：
    /// `cargo test -p wind-engine --lib builtin_table_matches_dict_readings -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn builtin_table_matches_dict_readings() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../");
        let src = std::fs::read_to_string(format!("{root}data/schemas/pinyin_misread.txt"))
            .expect("内置误读表应存在");
        let table = MisreadTable::parse(&src, "builtin");
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
        let checked: Vec<&MisreadEntry> = table
            .entries
            .iter()
            .filter(|e| e.correct.is_some())
            .collect();
        let missing: Vec<String> = checked
            .iter()
            .filter(|e| {
                let want = e.correct.as_ref().unwrap().join(" ");
                !readings.get(&e.text).is_some_and(|r| r.contains(&want))
            })
            .map(|e| format!("{} {}", e.text, e.correct.as_ref().unwrap().join(" ")))
            .collect();
        println!("核对 {} 条，词库缺读音 {} 条", checked.len(), missing.len());
        assert!(missing.is_empty(), "词库里没有这些正确读音：{missing:?}");
    }

    fn infer_with(
        typed: &str,
        readings: &[(&str, u32)],
        table: &str,
        fuzzy: &FuzzyConfig,
    ) -> Vec<PinyinHint> {
        let typed: Vec<&str> = typed.split(' ').collect();
        let readings: Vec<(Vec<String>, u32)> =
            readings.iter().map(|(r, w)| (syls(r), *w)).collect();
        let table = MisreadTable::parse(table, "t");
        infer("调研", &typed, &readings, &table, fuzzy)
    }

    fn hint(i: u16, c: &str) -> PinyinHint {
        PinyinHint {
            char_index: i,
            correct: c.to_string(),
            kind: HintKind::Misread,
        }
    }

    #[test]
    fn infer_marks_tiaoyan() {
        let got = infer_with(
            "tiao yan",
            &[("diao yan", 3784)],
            "",
            &FuzzyConfig::default(),
        );
        assert_eq!(got, [hint(0, "diao")]);
    }

    #[test]
    fn infer_picks_heaviest_reading() {
        let got = infer_with(
            "tiao yan",
            &[("zhou yan", 10), ("diao yan", 3784)],
            "",
            &FuzzyConfig::default(),
        );
        assert_eq!(got, [hint(0, "diao")]);
    }

    #[test]
    fn infer_skips_matching_reading() {
        let got = infer_with(
            "tiao yan",
            &[("diao yan", 3784), ("tiao yan", 1)],
            "",
            &FuzzyConfig::default(),
        );
        assert!(got.is_empty());
    }

    #[test]
    fn infer_skips_single_char() {
        let table = MisreadTable::default();
        let r = vec![(syls("diao"), 100)];
        assert!(infer("调", &["tiao"], &r, &table, &FuzzyConfig::default()).is_empty());
    }

    #[test]
    fn infer_skips_count_mismatch() {
        let f = FuzzyConfig::default();
        assert!(
            infer_with("tiaoyan", &[("diao yan", 3784)], "", &f).is_empty(),
            "音节数≠字数"
        );
        assert!(
            infer_with("tiao yan", &[("diao yan er", 3784)], "", &f).is_empty(),
            "读音字数对不上的不算登记读音"
        );
    }

    #[test]
    fn infer_skips_unknown_word() {
        assert!(infer_with("tiao yan", &[], "", &FuzzyConfig::default()).is_empty());
    }

    #[test]
    fn infer_whitelist_suppresses() {
        let got = infer_with(
            "tiao yan",
            &[("diao yan", 3784)],
            "调研\ttiao yan\t\n",
            &FuzzyConfig::default(),
        );
        assert!(got.is_empty());
    }

    #[test]
    fn infer_defers_to_fuzzy() {
        let fuzzy = FuzzyConfig {
            in_ing: true,
            ..Default::default()
        };
        let table = MisreadTable::default();
        let r = vec![(syls("ping guo"), 3000)];
        assert!(infer("苹果", &["pin", "guo"], &r, &table, &fuzzy).is_empty());
        assert_eq!(
            infer("苹果", &["pin", "guo"], &r, &table, &FuzzyConfig::default()),
            [hint(0, "ping")],
            "模糊没开时照标"
        );
    }

    #[test]
    fn reading_slots_share_per_schema() {
        let slots = ReadingSlots::default();
        let a = slots.slot("pinyin");
        assert!(a.get().is_none());
        slots
            .slot("pinyin")
            .set(Some(Arc::new(ReverseIndex::default())));
        assert!(a.get().is_some(), "同方案取到的是同一个槽");
        assert!(slots.slot("shuangpin").get().is_none());
    }

    /// 内置表每行都收得进来（坏行会被静默跳过，条数对不上就是有坏行）。
    #[test]
    fn builtin_table_parses_cleanly() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../data/schemas/pinyin_misread.txt"
        );
        let src = std::fs::read_to_string(path).expect("内置误读表应存在");
        let lines = src
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .count();
        let t = MisreadTable::parse(&src, "builtin");
        assert_eq!(t.len(), lines, "内置表有坏行");
        assert!(t.len() >= 50);
    }
}

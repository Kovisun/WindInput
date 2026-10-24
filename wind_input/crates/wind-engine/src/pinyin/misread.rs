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
use tracing::warn;

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

/// 误读表：条目 + 两张索引（文字 → 条目、错误读音连写 → 条目）。
#[derive(Debug, Default)]
pub struct MisreadTable {
    entries: Vec<MisreadEntry>,
    by_text: HashMap<String, Vec<usize>>,
    by_wrong: HashMap<String, Vec<usize>>,
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

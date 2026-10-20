//! 英文词频名次表：给**没写权重**的英文词条按常用程度赋权。
//!
//! 出厂英文主库 `en.dict.yaml` 从上游原样下载，词条没有权重列（全是 0），前缀补全于是
//! 退化成字母序——打 `produc` 先出 produce / produced，而不是 product。上游文件不改，
//! 权重在**加载期**补上：名次表（`schemas/english/en_freq.txt`，一行一词、行序即名次）
//! 里查得到的词，按名次映射到 `1..=RANK_WEIGHT_MAX`。
//!
//! ## 为什么在加载期、并且要进 wdat 指纹
//!
//! 前缀查询在 wdat 里按**存盘权重**做分支限界取前 N（`datformat::search_prefix`），查询期
//! 再改权重已经晚了——前 N 早按旧权重截好了。所以名次必须在建 wdat 之前写进条目，
//! 名次表因此是 wdat 的第二个源：改了它，指纹变、缓存重建（`CachedDict::load_english_at`）。
//!
//! ## 只填 0 权重的条目，值域压在 [`RANK_WEIGHT_MAX`] 以下
//!
//! - 写了权重的条目（扩展库的 `iPhone 999`、`shell 2 / she'll 1` 这类消歧对）一律不动；
//! - 名次权重上限 799，低于英文临时词的初始权重（协调器 `LEARN_ADD_WEIGHT` = 800）与手工
//!   加词的 1200：用户词、临时词与系统词的先后和加名次之前一样（之前系统词全是 0）；
//! - 查不到名次的词保持 0，排在所有有名次的词之后，彼此仍按文件顺序。

use std::collections::HashMap;
use std::path::Path;

/// 名次权重的上界（第 1 名取这个值）。理由见模块文档「值域压在…以下」。
pub const RANK_WEIGHT_MAX: i32 = 799;

/// 名次表：小写词 → 名次（1 起）。
pub struct FreqRanks {
    ranks: HashMap<String, u32>,
}

impl FreqRanks {
    /// 读名次表。文件不在返回 `None`（不赋权，行为同未引入名次表）；读失败告警后同样 `None`。
    pub fn load(path: &Path) -> Option<Self> {
        if !path.is_file() {
            return None;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => Some(Self::parse(&text)),
            Err(e) => {
                tracing::warn!("英文词频名次表 {} 读取失败：{}", path.display(), e);
                None
            }
        }
    }

    /// 一行一词，`#` 开头为注释，空行跳过；重复的词以首次出现为准。
    pub fn parse(text: &str) -> Self {
        let mut ranks = HashMap::new();
        let mut rank = 0u32;
        for line in text.lines() {
            let w = line.trim();
            if w.is_empty() || w.starts_with('#') {
                continue;
            }
            rank += 1;
            ranks.entry(w.to_lowercase()).or_insert(rank);
        }
        Self { ranks }
    }

    /// `text` 的名次权重（大小写不敏感）；不在表里返回 `None`。
    ///
    /// 按名次线性映射到 `1..=RANK_WEIGHT_MAX`：名次越靠前权重越高，相邻几十名可能同权
    /// （同权时按文件顺序，不影响常用词与生僻词的先后）。
    pub fn weight_of(&self, text: &str) -> Option<i32> {
        let rank = *self.ranks.get(&text.to_lowercase())?;
        let n = self.ranks.len().max(1) as i64;
        let span = (RANK_WEIGHT_MAX - 1) as i64;
        Some(RANK_WEIGHT_MAX - ((rank as i64 - 1) * span / n) as i32)
    }

    pub fn len(&self) -> usize {
        self.ranks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_comments_and_keeps_first_rank() {
        let r = FreqRanks::parse("# 头注释\nthe\n\nof\nThe\nand\n");
        assert_eq!(r.len(), 3, "注释、空行不算，重复的 The 不另占名次");
        assert_eq!(r.weight_of("the"), Some(RANK_WEIGHT_MAX), "第 1 名取上界");
        assert!(r.weight_of("of") > r.weight_of("and"));
        assert_eq!(r.weight_of("THE"), r.weight_of("the"), "大小写不敏感");
        assert_eq!(r.weight_of("missing"), None);
    }

    #[test]
    fn weights_stay_in_band_and_never_increase() {
        let words: Vec<String> = (0..20_000).map(|i| format!("w{i}")).collect();
        let r = FreqRanks::parse(&words.join("\n"));
        let ws: Vec<i32> = words.iter().map(|w| r.weight_of(w).unwrap()).collect();
        assert_eq!(ws[0], RANK_WEIGHT_MAX);
        assert!(
            *ws.last().unwrap() >= 1,
            "末名也要大于 0，否则与无名次的词分不开"
        );
        assert!(ws.windows(2).all(|p| p[0] >= p[1]), "名次靠后权重不得更高");
    }
}

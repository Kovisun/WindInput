//! 「下一词」表（[`AssocSource::Bigram`](crate::AssocSource::Bigram) 的数据）：上文 → 常接的后继。
//!
//! 数据是离线从 n-gram 模型蒸馏出的纯文本（`scripts/lm/distill_assoc_bigram.py`），
//! 每行 `上文<TAB>后继1 后继2 …`，按上文 UTF-8 字节序升序，后继按模型分值降序；
//! `#` 开头是注释。[`SENTENCE_END`] 是模型里的句末记号。
//!
//! 整份文本留在内存里，只额外建一张行首偏移表，查找是二分——两万多行、一两 MB，
//! 不必解析成哈希表（手机上省几 MB 常驻）。

/// 模型里的句末记号：排名越靠前，说明上文后面越常直接断句。
pub const SENTENCE_END: &str = "$";

/// 上文最多看几个字。与蒸馏参数 `--max-context-chars` 一致：更长的上文表里没有。
pub const MAX_CONTEXT_CHARS: usize = 4;

#[derive(Debug)]
pub struct AssocBigram {
    text: String,
    /// 每条数据行的起始字节偏移，按上文升序（即文件顺序）
    lines: Vec<u32>,
}

impl AssocBigram {
    /// 解析文本。行序不对（上文没按字节序升序）返回 `Err`——二分会静默查不到，
    /// 宁可在加载时就报出来。
    pub fn parse(text: String) -> Result<Self, String> {
        let mut lines = Vec::new();
        let mut prev: Option<&str> = None;
        let mut pos = 0usize;
        for line in text.split_inclusive('\n') {
            let start = pos;
            pos += line.len();
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let Some((ctx, _)) = line.split_once('\t') else {
                return Err(format!("第 {} 字节处的行缺少制表符", start));
            };
            if prev.is_some_and(|p| p >= ctx) {
                return Err(format!(
                    "上文没有按升序排列：「{}」在「{ctx}」之前",
                    prev.unwrap_or("")
                ));
            }
            prev = Some(ctx);
            lines.push(u32::try_from(start).map_err(|_| "文件超过 4GB".to_string())?);
        }
        Ok(Self { text, lines })
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn line(&self, i: usize) -> (&str, &str) {
        let start = self.lines[i] as usize;
        let rest = &self.text[start..];
        let line = rest
            .split('\n')
            .next()
            .unwrap_or(rest)
            .trim_end_matches('\r');
        line.split_once('\t').unwrap_or((line, ""))
    }

    fn find(&self, ctx: &str) -> Option<usize> {
        let (mut lo, mut hi) = (0usize, self.lines.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.line(mid).0.cmp(ctx) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    /// 按上文查后继：先整段（不超过 [`MAX_CONTEXT_CHARS`] 字时），查不到再逐个缩短到
    /// 末尾的 3、2、1 个字。返回命中的那段上文与后继。
    ///
    /// 上文是「刚上屏的那段」：整句上屏时（「我们今天」）表里没有整句，末尾的「今天」
    /// 才是要接的词。
    pub fn lookup<'a>(&'a self, ctx: &'a str) -> Option<(&'a str, Vec<&'a str>)> {
        let n = ctx.chars().count();
        for keep in (1..=n.min(MAX_CONTEXT_CHARS)).rev() {
            let at = ctx.char_indices().nth(n - keep).map_or(0, |(i, _)| i);
            let tail = &ctx[at..];
            if let Some(i) = self.find(tail) {
                return Some((tail, self.successors(i)));
            }
        }
        None
    }

    /// 带光标前文查：上文 = `preceding`（上屏前光标前的正文）+ `commit`（这次上屏的）。
    ///
    /// 逐字上屏时整词被拆开了（五笔打「今」「天」两个字），只拿 `commit`「天」去查，
    /// 接的是「天」的后继而不是「今天」的。故先试**向前文借字**凑成表里的上文，借得越多越先试；
    /// 都不成再退回 [`Self::lookup`]。
    ///
    /// 借字有个反例：前文「今天」、上屏「下」（要打「下午」），借一个字就是表里的「天下」。
    /// 判据：被借走的那几个字，在前文里已经和更前面的字连成了一个词（「今天」在表里）——
    /// 那它们属于那个词，不该再借给这次上屏。
    pub fn lookup_after<'a>(&'a self, preceding: &str, commit: &'a str) -> Option<Vec<&'a str>> {
        let n = commit.chars().count();
        let pre: Vec<char> = preceding.chars().collect();
        let max_borrow = MAX_CONTEXT_CHARS.saturating_sub(n).min(pre.len());
        for k in (1..=max_borrow).rev() {
            let borrowed: String = pre[pre.len() - k..].iter().collect();
            let Some(i) = self.find(&format!("{borrowed}{commit}")) else {
                continue;
            };
            let taken = (k + 1..=MAX_CONTEXT_CHARS.min(pre.len())).any(|j| {
                self.find(&pre[pre.len() - j..].iter().collect::<String>())
                    .is_some()
            });
            if !taken {
                return Some(self.successors(i));
            }
        }
        self.lookup(commit).map(|(_, succ)| succ)
    }

    fn successors(&self, i: usize) -> Vec<&str> {
        self.line(i)
            .1
            .split(' ')
            .filter(|s| !s.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> AssocBigram {
        // 按字节序排好：今(E4BB8A) < 我(E68891) < 谢(E8B0A2)
        AssocBigram::parse("# 注释\n今天\t$ 是 我 晚上\n我\t的 $ 不\n谢谢\t你 大家 $\n".into())
            .unwrap()
    }

    #[test]
    fn exact_and_suffix_lookup() {
        let t = table();
        assert_eq!(t.len(), 3);
        assert_eq!(
            t.lookup("今天").unwrap(),
            ("今天", vec!["$", "是", "我", "晚上"])
        );
        // 整句上屏：取末尾在表里的那段
        assert_eq!(t.lookup("我们今天").unwrap().0, "今天");
        assert_eq!(t.lookup("你和我").unwrap(), ("我", vec!["的", "$", "不"]));
        assert!(t.lookup("中").is_none());
        assert!(t.lookup("").is_none());
    }

    #[test]
    fn borrows_from_preceding_text() {
        // parse 要求按上文字节序升序，这里排好再拼
        let mut rows = [
            ("今天", "是 晚上"),
            ("天下", "第一"),
            ("下", "午 面"),
            ("天", "气"),
            ("中国人", "民"),
            ("国人", "的"),
            ("中国", "人"),
        ];
        rows.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let text: String = rows.iter().map(|(c, s)| format!("{c}\t{s}\n")).collect();
        let t = AssocBigram::parse(text).unwrap();
        // 逐字上屏「今」「天」：借「今」凑成「今天」
        assert_eq!(t.lookup_after("我们今", "天").unwrap(), vec!["是", "晚上"]);
        // 「今天」已成词，「天」不借给「下」
        assert_eq!(t.lookup_after("今天", "下").unwrap(), vec!["午", "面"]);
        // 借得多的先试：「中国人」而不是「国人」
        assert_eq!(t.lookup_after("中国", "人").unwrap(), vec!["民"]);
        // 没有前文 = 只看上屏的
        assert_eq!(t.lookup_after("", "天").unwrap(), vec!["气"]);
        assert!(t.lookup_after("今天", "鬻").is_none());
    }

    #[test]
    fn unsorted_input_is_rejected() {
        assert!(AssocBigram::parse("我\t的\n今天\t是\n".into()).is_err());
    }

    #[test]
    fn crlf_and_blank_lines_are_tolerated() {
        let t = AssocBigram::parse("今天\t是 我\r\n\n我\t的\r\n".into()).unwrap();
        assert_eq!(t.lookup("今天").unwrap().1, vec!["是", "我"]);
        assert_eq!(t.lookup("我").unwrap().1, vec!["的"]);
    }
}

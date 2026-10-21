//! 模糊音提示：把用户敲的拼音串与候选词库读音逐音节对齐，找出「读错的音节」。
//!
//! 对齐用 DP 把 `typed` 切成与词库音节同样多的段，每段按 Equal / Fuzzy / Prefix /
//! FuzzyPrefix 判类（先命中先得），取总代价最小的切法。只有 `Fuzzy` 段产出提示。

use super::fuzzy::{FuzzyConfig, FuzzyMatcher};
use wind_candidate::FuzzyHint;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Equal,
    Fuzzy,
    Prefix,
    FuzzyPrefix,
}

impl Kind {
    fn cost(self) -> u32 {
        match self {
            Kind::Equal | Kind::Prefix => 0,
            Kind::Fuzzy | Kind::FuzzyPrefix => 1,
        }
    }
}

/// 判一段用户输入 `u` 相对词库音节 `c` 的类别；`variants` 是 `c` 的拼写级模糊变体。
fn classify(u: &str, c: &str, variants: &[String], last_open: bool) -> Option<Kind> {
    if u == c {
        return Some(Kind::Equal);
    }
    let is_prefix = u.len() < c.len() && c.starts_with(u);
    if variants.iter().any(|v| v == u) {
        // D10：输入末尾的半截音节（`pin` 对 `ping`）可能只是还没敲完，不当作读错。
        return Some(if last_open && is_prefix {
            Kind::Prefix
        } else {
            Kind::Fuzzy
        });
    }
    if is_prefix {
        return Some(Kind::Prefix);
    }
    if variants
        .iter()
        .any(|v| u.len() < v.len() && v.starts_with(u))
    {
        return Some(Kind::FuzzyPrefix);
    }
    None
}

/// 对齐 `typed` 与 `dict_syllables`，返回读错的音节（`char_index` 升序）。
///
/// `input_ends_here`：`typed` 的末尾就是用户当前输入的末尾。对齐失败返回空。
pub(crate) fn align_hints(
    typed: &str,
    dict_syllables: &[&str],
    cfg: &FuzzyConfig,
    input_ends_here: bool,
) -> Vec<FuzzyHint> {
    if !cfg.hint || dict_syllables.is_empty() || typed.is_empty() || !typed.is_ascii() {
        return Vec::new();
    }
    let n = dict_syllables.len();
    let m = typed.len();
    let group_cfg = cfg.without_shared_key();
    let variants: Vec<Vec<String>> = dict_syllables
        .iter()
        .map(|c| FuzzyMatcher::fuzzy_spelling_variants(c, &group_cfg))
        .collect();

    // best[i][p]：前 i 个音节消费了 typed[..p] 的最优 (代价, Fuzzy 段数)。
    type Score = (u32, u32);
    let mut best: Vec<Vec<Option<Score>>> = vec![vec![None; m + 1]; n + 1];
    // 回溯：(上一位置, 本段类别)
    let mut from: Vec<Vec<Option<(usize, Kind)>>> = vec![vec![None; m + 1]; n + 1];
    best[0][0] = Some((0, 0));
    for i in 0..n {
        for p in 0..m {
            let Some(base) = best[i][p] else { continue };
            for q in (p + 1)..=m {
                let last_open = input_ends_here && i + 1 == n && q == m;
                let Some(kind) = classify(&typed[p..q], dict_syllables[i], &variants[i], last_open)
                else {
                    continue;
                };
                let score = (
                    base.0 + kind.cost(),
                    base.1 + u32::from(kind == Kind::Fuzzy),
                );
                if best[i + 1][q].is_none_or(|b| score < b) {
                    best[i + 1][q] = Some(score);
                    from[i + 1][q] = Some((p, kind));
                }
            }
        }
    }
    if best[n][m].is_none() {
        return Vec::new();
    }
    let mut hints = Vec::new();
    let mut p = m;
    for i in (1..=n).rev() {
        let (prev, kind) = from[i][p].expect("best 非空则回溯链完整");
        if kind == Kind::Fuzzy {
            hints.push(FuzzyHint {
                char_index: (i - 1) as u16,
                correct: dict_syllables[i - 1].to_string(),
            });
        }
        p = prev;
    }
    hints.reverse();
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> FuzzyConfig {
        FuzzyConfig {
            in_ing: true,
            n_l: true,
            sh_s: true,
            hint: true,
            ..Default::default()
        }
    }

    fn h(i: u16, s: &str) -> FuzzyHint {
        FuzzyHint {
            char_index: i,
            correct: s.into(),
        }
    }

    #[test]
    fn pinguo_marks_ping() {
        assert_eq!(
            align_hints("pinguo", &["ping", "guo"], &cfg(), true),
            vec![h(0, "ping")]
        );
    }

    #[test]
    fn exact_marks_nothing() {
        assert!(align_hints("pingguo", &["ping", "guo"], &cfg(), true).is_empty());
    }

    #[test]
    fn abbrev_marks_nothing() {
        assert!(align_hints("pg", &["ping", "guo"], &cfg(), true).is_empty());
    }

    #[test]
    fn trailing_prefix_not_marked() {
        assert!(align_hints("pin", &["ping"], &cfg(), true).is_empty());
        assert_eq!(
            align_hints("pin", &["ping"], &cfg(), false),
            vec![h(0, "ping")]
        );
    }

    #[test]
    fn lan_nan_marked() {
        assert_eq!(
            align_hints("lan", &["nan"], &cfg(), true),
            vec![h(0, "nan")]
        );
    }

    #[test]
    fn half_last_syllable() {
        assert!(align_hints("pinggu", &["ping", "guo"], &cfg(), true).is_empty());
    }

    #[test]
    fn sentence_mixed_abbrev() {
        let mut c = cfg();
        c.en_eng = true;
        assert_eq!(
            align_hints(
                "jintiantqzhenhao",
                &["jin", "tian", "tian", "qi", "zheng", "hao"],
                &c,
                true
            ),
            vec![h(4, "zheng")]
        );
    }

    #[test]
    fn shared_key_not_marked() {
        let mut c = cfg();
        c.shared_key.insert("diu".into(), vec!["dui".into()]);
        assert!(align_hints("diu", &["dui"], &c, true).is_empty());
    }

    #[test]
    fn illegal_spelling_marked() {
        let c = FuzzyConfig {
            in_ing: true,
            hint: true,
            ..Default::default()
        };
        assert_eq!(align_hints("din", &["ding"], &c, false), vec![h(0, "ding")]);
    }

    #[test]
    fn hint_off_returns_empty() {
        let mut c = cfg();
        c.hint = false;
        assert!(align_hints("pinguo", &["ping", "guo"], &c, true).is_empty());
    }

    #[test]
    fn misaligned_returns_empty() {
        assert!(align_hints("pinguo", &["ping"], &cfg(), true).is_empty());
    }
}

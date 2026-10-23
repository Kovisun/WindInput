//! 字母换位纠错（`gogn` → `gong`）：规则表与错误拼写生成。
//!
//! 生成方向**从合法音节出发**：对每个标准音节，把规则的「正确片段」换成「错误片段」，
//! 得到用户可能打出的错误拼写，登记「错误拼写 → 正确音节」。不对用户输入做串替换。
//!
//! 产物沿用模糊错音串的两条约束（见 [`super::fuzzy::fuzzy_spellings`]）：
//! - **只收非法拼写**：产物本身是合法音节的丢弃（`dui`→`diu` 不收）；
//! - **不收元音开头的拼写**：它能接在任何音节后面，会改动合法输入的切分。
//!
//! 接线：[`super::fuzzy::FuzzyConfig::correction`] 持一份；错误拼写并入
//! [`super::fuzzy::fuzzy_spellings`] 进切分层，[`super::fuzzy::FuzzyMatcher::fuzzy_variants_scored`]
//! 对错误拼写追加 `(正确音节, correction_edits())`。双拼不启用（规则是全拼字母层的）。

use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::fuzzy::VALID_SYLLABLES;
use super::syllable::STANDARD_SYLLABLES;

/// 内置规则（一条 = 设置端一个开关，见设计 §5.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectionRule {
    /// `ng` 打成 `gn`：agn/egn/ign/ogn/iagn/uagn/iogn。
    GnNg,
    /// 漏 `g`：on→ong、ion→iong。
    OnOng,
    /// 韵母字母颠倒。
    VowelSwap,
    /// 旧式拼写 uei/iou/uen → ui/iu/un。
    LegacySpelling,
    /// h 错位：hz/hc/hs、zih/cih/sih。
    HMisplaced,
    /// `ng` 打成 `mg`。
    MgNg,
    /// `ng` 打成 nag/neg/nig/nog。
    NBeforeG,
    /// 简拼冲突类：qai/xai→qia/xia、nu→un、na/ne/ni→an/en/in。
    AiIa,
}

/// 罚分以「处数」表达，与模糊音共用 0.5^处数：7 处 ≈ ×0.008（对齐 librime 纠错 ×0.01）。
/// 实验期从环境变量 WIND_CORRECTION_EDITS 读（Task 5 扫参后固化并删掉环境变量）。
pub const CORRECTION_EDITS: usize = 7;

static CORRECTION_EDITS_RESOLVED: LazyLock<usize> = LazyLock::new(|| {
    std::env::var("WIND_CORRECTION_EDITS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(CORRECTION_EDITS)
});

/// 纠错音节计几处改动：环境变量 `WIND_CORRECTION_EDITS`（只读一次），缺省或解析失败取
/// [`CORRECTION_EDITS`]。
pub(crate) fn correction_edits() -> usize {
    *CORRECTION_EDITS_RESOLVED
}

/// 片段在音节里的匹配方式。
#[derive(Clone, Copy)]
enum Site {
    /// 韵母恰为该片段（声母 = 去掉韵母后的前缀，`y`/`w` 算声母）。
    Final,
    /// 音节以该片段结尾。
    Suffix,
    /// 声母恰为该片段。
    Initial,
    /// 整个音节恰为该片段。
    Whole,
}

/// 一条片段替换：`correct` 片段改成 `wrong`；`initials` 非空时只对这些声母生效。
struct Edit {
    site: Site,
    correct: &'static str,
    wrong: &'static str,
    initials: &'static [&'static str],
}

const fn e(site: Site, correct: &'static str, wrong: &'static str) -> Edit {
    Edit {
        site,
        correct,
        wrong,
        initials: &[],
    }
}

const fn ei(
    site: Site,
    correct: &'static str,
    wrong: &'static str,
    initials: &'static [&'static str],
) -> Edit {
    Edit {
        site,
        correct,
        wrong,
        initials,
    }
}

use Site::{Final, Initial, Suffix, Whole};

// 各规则的片段集合，按设计 §5.2「覆盖」列。
const GN_NG: &[Edit] = &[e(Suffix, "ng", "gn")];
const ON_ONG: &[Edit] = &[e(Suffix, "ong", "on")];
const VOWEL_SWAP: &[Edit] = &[
    ei(Final, "ai", "ia", &["w", "g", "h", "k"]),
    ei(Final, "ei", "ie", &["w", "f", "g", "h", "k", "z"]),
    ei(Final, "ie", "ei", &["j", "q", "x"]),
    e(Final, "ao", "oa"),
    ei(Final, "ou", "uo", &["y", "p", "f", "m"]),
    e(Final, "ui", "iu"),
    e(Final, "iu", "ui"),
    e(Final, "iao", "ioa"),
    e(Final, "iao", "oia"),
    e(Final, "ua", "au"),
    e(Final, "uai", "aui"),
    e(Final, "uang", "aung"),
    e(Final, "iang", "aing"),
    e(Final, "ue", "eu"),
];
const LEGACY_SPELLING: &[Edit] = &[
    e(Final, "ui", "uei"),
    e(Final, "iu", "iou"),
    e(Final, "un", "uen"),
];
const H_MISPLACED: &[Edit] = &[
    e(Initial, "zh", "hz"),
    e(Initial, "ch", "hc"),
    e(Initial, "sh", "hs"),
    e(Whole, "zhi", "zih"),
    e(Whole, "chi", "cih"),
    e(Whole, "shi", "sih"),
];
const MG_NG: &[Edit] = &[e(Suffix, "ng", "mg")];
const N_BEFORE_G: &[Edit] = &[
    e(Suffix, "ang", "nag"),
    e(Suffix, "eng", "neg"),
    e(Suffix, "ing", "nig"),
    e(Suffix, "ong", "nog"),
];
const AI_IA: &[Edit] = &[
    e(Whole, "qia", "qai"),
    e(Whole, "xia", "xai"),
    e(Final, "un", "nu"),
    e(Final, "an", "na"),
    e(Final, "en", "ne"),
    e(Final, "in", "ni"),
];

fn edits_of(rule: CorrectionRule) -> &'static [Edit] {
    match rule {
        CorrectionRule::GnNg => GN_NG,
        CorrectionRule::OnOng => ON_ONG,
        CorrectionRule::VowelSwap => VOWEL_SWAP,
        CorrectionRule::LegacySpelling => LEGACY_SPELLING,
        CorrectionRule::HMisplaced => H_MISPLACED,
        CorrectionRule::MgNg => MG_NG,
        CorrectionRule::NBeforeG => N_BEFORE_G,
        CorrectionRule::AiIa => AI_IA,
    }
}

/// 声母表（与 `fuzzy.rs` 同口径：按长度降序、`y`/`w` 算声母）。
const INITIALS: &[&str] = &[
    "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r",
    "z", "c", "s", "y", "w",
];

fn split_initial(syl: &str) -> (&str, &str) {
    INITIALS
        .iter()
        .find_map(|i| syl.strip_prefix(i).map(|rest| (&syl[..i.len()], rest)))
        .unwrap_or(("", syl))
}

impl Edit {
    /// 对音节 `syl` 施加本条替换，不适用时 `None`。
    fn apply(&self, syl: &str) -> Option<String> {
        let (initial, final_) = split_initial(syl);
        if !self.initials.is_empty() && !self.initials.contains(&initial) {
            return None;
        }
        match self.site {
            Final => (final_ == self.correct).then(|| format!("{initial}{}", self.wrong)),
            Suffix => syl
                .strip_suffix(self.correct)
                .map(|head| format!("{head}{}", self.wrong)),
            Initial => (initial == self.correct).then(|| format!("{}{final_}", self.wrong)),
            Whole => (syl == self.correct).then(|| self.wrong.to_string()),
        }
    }
}

/// 错误拼写可收否：非合法音节、非元音开头、非空。
fn acceptable(wrong: &str) -> bool {
    !wrong.is_empty()
        && !wrong.starts_with(['a', 'e', 'i', 'o', 'u', 'v'])
        && !VALID_SYLLABLES.contains(wrong)
}

/// 纠错规则表：错误拼写 → 正确音节（及反向索引）。
#[derive(Debug, Clone, Default)]
pub struct CorrectionRules {
    /// 错误拼写 → 正确音节（升序去重）。
    by_wrong: BTreeMap<String, Vec<String>>,
    /// 正确音节 → 错误拼写（升序去重）。
    by_correct: BTreeMap<String, Vec<String>>,
}

impl CorrectionRules {
    /// 按配置生成；`enabled = false` ⇒ 空。自定义规则语法错或一条可用错误拼写都生成不出来
    /// 时 `warn` 一次并跳过该条。
    pub fn from_config(c: &wind_config::CorrectionConfig) -> Self {
        let mut out = Self::default();
        if !c.enabled {
            return out;
        }
        let t = &c.rules;
        let toggles = [
            (CorrectionRule::GnNg, t.gn_ng),
            (CorrectionRule::OnOng, t.on_ong),
            (CorrectionRule::VowelSwap, t.vowel_swap),
            (CorrectionRule::LegacySpelling, t.legacy_spelling),
            (CorrectionRule::HMisplaced, t.h_misplaced),
            (CorrectionRule::MgNg, t.mg_ng),
            (CorrectionRule::NBeforeG, t.n_before_g),
            (CorrectionRule::AiIa, t.ai_ia),
        ];
        for (rule, on) in toggles {
            if !on {
                continue;
            }
            for edit in edits_of(rule) {
                for syl in STANDARD_SYLLABLES {
                    if let Some(wrong) = edit.apply(syl) {
                        out.insert(wrong, syl);
                    }
                }
            }
        }
        for raw in &c.custom {
            let (wrong, correct) = match parse_custom(raw) {
                Ok(p) => p,
                Err(why) => {
                    tracing::warn!("拼写纠错自定义规则无效，已跳过: {raw:?}（{why}）");
                    continue;
                }
            };
            let mut produced = false;
            for syl in STANDARD_SYLLABLES {
                for (at, _) in syl.match_indices(correct.as_str()) {
                    let wrong_spelling =
                        format!("{}{wrong}{}", &syl[..at], &syl[at + correct.len()..]);
                    produced |= out.insert(wrong_spelling, syl);
                }
            }
            if !produced {
                tracing::warn!("拼写纠错自定义规则生成不出可用的错误拼写，已跳过: {raw:?}");
            }
        }
        out
    }

    /// 登记一条（不可收的拼写丢弃），返回是否收下。
    fn insert(&mut self, wrong: String, correct: &str) -> bool {
        if !acceptable(&wrong) {
            return false;
        }
        let cs = self.by_wrong.entry(wrong.clone()).or_default();
        if let Err(i) = cs.binary_search_by(|x| x.as_str().cmp(correct)) {
            cs.insert(i, correct.to_string());
        }
        let ws = self.by_correct.entry(correct.to_string()).or_default();
        if let Err(i) = ws.binary_search(&wrong) {
            ws.insert(i, wrong);
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        self.by_wrong.is_empty()
    }

    /// 用户打的拼写 `typed` 可纠成哪些合法音节。
    pub fn corrections_of(&self, typed: &str) -> &[String] {
        self.by_wrong.get(typed).map_or(&[], Vec::as_slice)
    }

    /// 全部错误拼写（只含非法、非零声母），供第二棵 trie。
    pub fn spellings(&self) -> impl Iterator<Item = &str> {
        self.by_wrong.keys().map(String::as_str)
    }

    /// 正确音节 `syl` 的全部错误拼写（`align` 判类用）。
    pub fn wrong_spellings_of(&self, syl: &str) -> &[String] {
        self.by_correct.get(syl).map_or(&[], Vec::as_slice)
    }
}

/// 解析自定义规则 `错误写法=正确写法`，返回 `(错误写法, 正确写法)`；无效条目返回 Err(原因)，
/// 调用方 warn 后跳过。两侧须为非空小写字母串且不相同。
pub fn parse_custom(rule: &str) -> Result<(String, String), String> {
    let (wrong, correct) = rule.split_once('=').ok_or_else(|| "缺少 `=`".to_string())?;
    let (wrong, correct) = (wrong.trim(), correct.trim());
    let letters = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    if !letters(wrong) || !letters(correct) {
        return Err("两侧须为非空小写字母".to_string());
    }
    if wrong == correct {
        return Err("两侧相同".to_string());
    }
    Ok((wrong.to_string(), correct.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pinyin::fuzzy::VALID_SYLLABLES;
    use wind_config::{CorrectionConfig, CorrectionRuleToggles};

    fn rules(f: impl FnOnce(&mut CorrectionConfig)) -> CorrectionRules {
        let mut c = CorrectionConfig::default();
        f(&mut c);
        CorrectionRules::from_config(&c)
    }

    #[test]
    fn gn_ng_fixes_ogn() {
        assert_eq!(rules(|_| {}).corrections_of("gogn"), ["gong"]);
    }
    #[test]
    fn gn_ng_fixes_ign() {
        assert_eq!(rules(|_| {}).corrections_of("dign"), ["ding"]);
    }
    #[test]
    fn on_ong_fixes_on() {
        assert_eq!(rules(|_| {}).corrections_of("gon"), ["gong"]);
    }
    #[test]
    fn vowel_swap_fixes_ia_after_h() {
        assert_eq!(rules(|_| {}).corrections_of("hia"), ["hai"]);
    }
    #[test]
    fn legacy_off_by_default() {
        assert!(rules(|_| {}).corrections_of("liou").is_empty());
    }
    #[test]
    fn legacy_on_fixes_liou() {
        assert_eq!(
            rules(|c| c.rules.legacy_spelling = true).corrections_of("liou"),
            ["liu"]
        );
    }
    #[test]
    fn disabled_yields_nothing() {
        assert!(rules(|c| c.enabled = false).is_empty());
    }
    #[test]
    fn never_registers_legal_syllable() {
        let r = rules(|c| {
            c.rules = CorrectionRuleToggles {
                gn_ng: true,
                on_ong: true,
                vowel_swap: true,
                legacy_spelling: true,
                h_misplaced: true,
                mg_ng: true,
                n_before_g: true,
                ai_ia: true,
            }
        });
        for s in r.spellings() {
            assert!(!VALID_SYLLABLES.contains(s), "{s}");
            assert!(!s.starts_with(['a', 'e', 'o']));
        }
    }
    #[test]
    fn custom_rule_applies_alone() {
        let r = rules(|c| {
            c.rules.gn_ng = false;
            c.custom = vec!["ogn=ong".into()];
        });
        assert_eq!(r.corrections_of("gogn"), ["gong"]);
        assert!(r.corrections_of("dign").is_empty());
    }
    /// 声母限定照 §5.2 表：`ai←ia` 只对 w/g/h/k，`ei←ie` 只对 j/q/x，`ie←ei` 只对 w/f/g/h/k/z。
    #[test]
    fn vowel_swap_respects_initial_restrictions() {
        let r = rules(|_| {});
        assert_eq!(r.corrections_of("kia"), ["kai"]);
        assert!(
            r.corrections_of("bia").is_empty(),
            "b 不在 ai←ia 的声母表里"
        );
        assert_eq!(r.corrections_of("jei"), ["jie"]);
        assert_eq!(r.corrections_of("zie"), ["zei"]);
        assert!(r.corrections_of("bie").is_empty(), "bie 合法");
        assert_eq!(r.corrections_of("yuo"), ["you"]);
        assert!(r.corrections_of("duo").is_empty(), "duo 合法");
        assert_eq!(r.corrections_of("boa"), ["bao"]);
        assert_eq!(r.corrections_of("xioa"), ["xiao"]);
        assert_eq!(r.corrections_of("xoia"), ["xiao"]);
        assert_eq!(r.corrections_of("guia"), Vec::<String>::new());
        assert_eq!(r.corrections_of("gaui"), ["guai"]);
        assert_eq!(r.corrections_of("giu"), ["gui"]);
        assert_eq!(r.corrections_of("jui"), ["jiu"]);
        assert_eq!(r.wrong_spellings_of("xiao"), ["xioa", "xoia"]);
    }

    /// 出厂关的规则各自开了才生效。
    #[test]
    fn off_by_default_rules_need_their_toggle() {
        let base = rules(|_| {});
        for (typed, correct, on) in [
            (
                "hzong",
                "zhong",
                (|c: &mut CorrectionConfig| c.rules.h_misplaced = true)
                    as fn(&mut CorrectionConfig),
            ),
            ("zih", "zhi", |c| c.rules.h_misplaced = true),
            ("bimg", "bing", |c| c.rules.mg_ng = true),
            ("bnag", "bang", |c| c.rules.n_before_g = true),
            ("xai", "xia", |c| c.rules.ai_ia = true),
            ("dnu", "dun", |c| c.rules.ai_ia = true),
            ("duei", "dui", |c| c.rules.legacy_spelling = true),
        ] {
            assert!(base.corrections_of(typed).is_empty(), "{typed}");
            assert_eq!(rules(on).corrections_of(typed), [correct], "{typed}");
        }
    }

    /// 生成不出可用错误拼写的自定义规则整条跳过，不影响其余规则。
    #[test]
    fn custom_rule_without_output_is_skipped() {
        let r = rules(|c| {
            c.custom = vec!["xyz=qqq".into(), "dui=diu".into(), "bad".into()];
        });
        assert_eq!(r.spellings().count(), rules(|_| {}).spellings().count());
    }

    #[test]
    fn custom_rule_invalid_is_rejected() {
        assert!(parse_custom("ogn").is_err());
        assert!(parse_custom("=ong").is_err());
    }
}

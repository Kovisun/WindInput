//! 拼音编码栏按**高亮候选**的切分显示，不是按首选算一次就不动。
//!
//! `pinan` 有 `pi|nan` 与 `pin|an` 两条切法：高亮「平安」显示 `pin'an`，移到「皮」应变
//! `pi'nan`，移回首选恢复。每条候选自带切分（`CandidateMeta::preedit_split`，引擎按它自己走的
//! 边界算，只覆盖前缀的候选尾部按最大匹配接着切），协调器在高亮变化时换用高亮那条的。
//! 主路、临拼、快捷输入（mix 文本透镜）同一套；双拼仍按击键两两分段，不随全拼切分变。
//!
//! 每例同时断言首选与它的编码栏、高亮移到指定候选后的编码栏；高亮移动的回执（编码嵌在宿主里
//! 时回传的组合串）一并核对。位次在 Coordinator 层取（引擎位次 ≠ 界面位次）。
//!
//! ⚠️ 词典缺失时整族静默跳过（判据是**耗时 0.00s**，不是通过条数）。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;

const VK_BACKTICK: u32 = 0xC0;
const VK_SEMICOLON: u32 = 0xBA;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_data() -> bool {
    let d = data_dir();
    d.join("schemas/pinyin/cn_dicts/base.dict.yaml").exists()
        && d.join("schemas/wubi86.schema.toml").exists()
        && d.join("schemas/shuangpin.schema.toml").exists()
}

fn key(k: u32) -> KeyEventData {
    KeyEventData {
        key_code: k,
        scan_code: 0,
        modifiers: 0,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    }
}

fn press(coord: &Coordinator, k: u32) -> KeyAction {
    coord.handle_key_event_policed(&key(k))
}

fn cfg(active: &str) -> Config {
    let mut c = Config::default();
    c.schema.available = vec![
        active.into(),
        "pinyin".into(),
        "shuangpin".into(),
        "english".into(),
    ];
    c.schema.available.dedup();
    c.schema.active = active.into();
    c.input.default.chinese_mode = true;
    c
}

fn in_ing(mut c: Config) -> Config {
    c.schema.pinyin.fuzzy.enabled = true;
    c.schema.pinyin.fuzzy.in_ing = true;
    c
}

/// 双拼作临拼目标（`schema.primary_pinyin`），开全拼降级支路。
fn shuangpin_target(mut c: Config) -> Config {
    c.schema.primary_pinyin = "shuangpin".into();
    c.schema.pinyin.shuangpin.allow_full_pinyin = true;
    c
}

/// 先按 `enter`（进 overlay 的引导键），再逐键打 `input`。
fn typed(c: Config, enter: Option<u32>, input: &str) -> Arc<Coordinator> {
    let coord = Coordinator::new_headless(c, Some(&data_dir()));
    if let Some(k) = enter {
        press(&coord, k);
    }
    for ch in input.chars() {
        press(&coord, (ch.to_ascii_uppercase() as u32) & 0xFF);
    }
    coord
}

fn highlighted(coord: &Coordinator) -> String {
    let (_, sel, _) = coord.debug_page_info();
    coord.debug_page_texts()[sel].clone()
}

/// 把高亮从当前位置逐个下移到 `want`，返回（最后一次按键前的编码栏, 最后一次按键的回执）。
fn highlight(coord: &Coordinator, want: &str) -> (String, KeyAction) {
    let all = coord.debug_all_candidate_texts();
    let cur = all.iter().position(|t| *t == highlighted(coord)).unwrap();
    let target = all
        .iter()
        .position(|t| t == want)
        .unwrap_or_else(|| panic!("候选里没有「{want}」：{:?}", &all[..all.len().min(20)]));
    assert!(target > cur, "「{want}」应在当前高亮之后");
    let mut before = coord.debug_preedit();
    let mut last = KeyAction::Consumed;
    for _ in cur..target {
        before = coord.debug_preedit();
        last = press(coord, VK_DOWN);
    }
    assert_eq!(highlighted(coord), want, "前提：高亮已移到「{want}」");
    (before, last)
}

fn composed(act: &KeyAction) -> Option<&str> {
    match act {
        KeyAction::UpdateComposition { text, .. } => Some(text),
        _ => None,
    }
}

/// 首选是 `top`、编码栏是 `preedit`。
fn assert_top(coord: &Coordinator, top: &str, preedit: &str) {
    let all = coord.debug_all_candidate_texts();
    assert_eq!(
        all.first().map(String::as_str),
        Some(top),
        "前提：首选应是「{top}」，实际: {:?}",
        &all[..all.len().min(8)]
    );
    assert_eq!(coord.debug_preedit(), preedit, "首选「{top}」的编码栏");
}

/// 高亮移到 `cand` 后编码栏是 `preedit`；形态变了时回执须把同一串回传宿主（出厂编码嵌在宿主里）。
fn assert_at(coord: &Coordinator, cand: &str, preedit: &str) {
    let (before, act) = highlight(coord, cand);
    assert_eq!(coord.debug_preedit(), preedit, "高亮「{cand}」时的编码栏");
    if before != preedit {
        assert_eq!(
            composed(&act),
            Some(preedit),
            "高亮移到「{cand}」形态变了，须回传新组合串"
        );
    }
}

/// 报障原例。开 in↔ing：首选「平安」走 `pin|an`，「皮」走 `pi|nan`，移回首选恢复。
/// 对照：关模糊音时首选是整句「拼按」（同样 `pin|an`），「皮」照样按自己的切分。
#[test]
fn main_pinan_follows_highlight() {
    if !has_data() {
        return;
    }
    let coord = typed(in_ing(cfg("pinyin")), None, "pinan");
    assert_top(&coord, "平安", "pin'an");
    assert_at(&coord, "皮", "pi'nan");
    let act = press(&coord, VK_UP);
    assert_eq!(highlighted(&coord), "平安");
    assert_eq!(coord.debug_preedit(), "pin'an", "移回首选恢复");
    assert_eq!(composed(&act), Some("pin'an"));

    let plain = typed(cfg("pinyin"), None, "pinan");
    assert_top(&plain, "拼按", "pin'an");
    assert_at(&plain, "皮", "pi'nan");
}

/// 非模糊的歧义切分：整句与子短语走 `xi|an`，「先交」「先」走 `xian|…`。只覆盖前缀的候选
/// 尾部按最大匹配接着切（`xian'jiao` + `tong'da'xue`）。
#[test]
fn main_xianjiaotongdaxue_prefix_candidates() {
    if !has_data() {
        return;
    }
    let coord = typed(cfg("pinyin"), None, "xianjiaotongdaxue");
    assert_top(&coord, "西安交通大学", "xi'an'jiao'tong'da'xue");
    assert_at(&coord, "西安交通", "xi'an'jiao'tong'da'xue");
    assert_at(&coord, "先交", "xian'jiao'tong'da'xue");
    assert_at(&coord, "先", "xian'jiao'tong'da'xue");
    assert_at(&coord, "西安", "xi'an'jiao'tong'da'xue");
}

/// 带残码：「你会」只吃 `ni` + 声母 `h`，剩下 `aom` 另切（`ao'm`）；「你好」「你」的尾部
/// 照旧是 `hao'm`。
#[test]
fn main_partial_candidates_keep_trailing_code() {
    if !has_data() {
        return;
    }
    let coord = typed(cfg("pinyin"), None, "nihaom");
    assert_top(&coord, "你好吗", "ni'hao'm");
    assert_at(&coord, "你好", "ni'hao'm");
    assert_at(&coord, "你会", "ni'h'ao'm");
    assert_at(&coord, "你", "ni'hao'm");
}

/// 单音节与两字词同码：`xian` 首选「先」原样显示，高亮「西安」变 `xi'an`。五笔拼音混输下
/// 首选的切分与原始码同形（拆分串为空），别的拼音候选照样跟随。
#[test]
fn main_xian_single_syllable_vs_word() {
    if !has_data()
        || !data_dir()
            .join("schemas/wubi86_pinyin.schema.toml")
            .exists()
    {
        return;
    }
    for schema in ["pinyin", "wubi86_pinyin"] {
        let coord = typed(cfg(schema), None, "xian");
        assert_top(&coord, "先", "xian");
        assert_at(&coord, "西安", "xi'an");
        press(&coord, VK_DOWN);
        assert_eq!(coord.debug_preedit(), "xian", "{schema}：移离「西安」恢复");
    }
}

/// 双拼编码栏按击键两两分段：高亮移到哪条候选都是 `xm'jn`，不随全拼切分变。
#[test]
fn main_shuangpin_keeps_keystroke_pairs() {
    if !has_data() {
        return;
    }
    let coord = typed(cfg("shuangpin"), None, "xmjn");
    assert_top(&coord, "先交", "xm'jn");
    for cand in ["线脚", "先", "线"] {
        assert_at(&coord, cand, "xm'jn");
    }
}

/// 临拼与主路同一套：只读前缀 + 高亮那条的切分。
#[test]
fn temp_pinyin_pinan_follows_highlight() {
    if !has_data() {
        return;
    }
    let coord = typed(in_ing(cfg("wubi86")), Some(VK_BACKTICK), "pinan");
    assert_eq!(coord.debug_active_mode(), Some("temp_pinyin"));
    assert_top(&coord, "平安", "`pin'an");
    assert_at(&coord, "皮", "`pi'nan");
    let act = press(&coord, VK_UP);
    assert_eq!(coord.debug_preedit(), "`pin'an", "移回首选恢复");
    assert_eq!(composed(&act), Some("`pin'an"));
}

/// 临拼目标是双拼：全拼降级与简拼的形态随高亮切换，对齐主路
/// （`input_flow.rs` 的 `shuangpin_preedit_follows_highlight_between_domains`）。
/// 修前临拼只读引擎的 `preedit_display`：「再见」配着双拼切法 `za'ij'ia'n`，移到「问」
/// 也停在简拼切法。
#[test]
fn temp_pinyin_shuangpin_target_switches_forms() {
    if !has_data() {
        return;
    }
    let coord = typed(
        shuangpin_target(cfg("wubi86")),
        Some(VK_BACKTICK),
        "zaijian",
    );
    assert_top(&coord, "再见", "`zai'jian");
    assert_at(&coord, "在家", "`za'ij'ia'n");

    let coord = typed(shuangpin_target(cfg("wubi86")), Some(VK_BACKTICK), "wfwt");
    assert_top(&coord, "无法无天", "`w'f'w't");
    assert_at(&coord, "问", "`wf'wt");
}

/// 快捷输入的文本透镜：按合并后的高亮候选显示。
#[test]
fn mix_text_lens_pinan_follows_highlight() {
    if !has_data() {
        return;
    }
    let coord = typed(in_ing(cfg("wubi86")), Some(VK_SEMICOLON), "pinan");
    assert_eq!(coord.debug_active_mode(), Some("mix"));
    assert_top(&coord, "平安", ";pin'an");
    assert_at(&coord, "皮", ";pi'nan");
    let act = press(&coord, VK_UP);
    assert_eq!(coord.debug_preedit(), ";pin'an", "移回首选恢复");
    assert_eq!(composed(&act), Some(";pin'an"));
}

/// 光标在缓冲中间（`pin|an`，左移两次）：形态切换后插入符按新形态换算，回传宿主的位置与
/// 自绘编码栏的位置一致。`pin'an` 里在 `pin` 之后，`pi'nan` 里在 `pi'n` 之后。
#[test]
fn caret_tracks_form_switch() {
    if !has_data() {
        return;
    }
    let cases: [(Option<u32>, &str); 3] = [
        (None, ""),
        (Some(VK_BACKTICK), "`"),
        (Some(VK_SEMICOLON), ";"),
    ];
    for (enter, prefix) in cases {
        let active = if enter.is_some() { "wubi86" } else { "pinyin" };
        let coord = typed(in_ing(cfg(active)), enter, "pinan");
        press(&coord, VK_LEFT);
        press(&coord, VK_LEFT);
        let p = prefix.len();
        assert_eq!(coord.debug_preedit(), format!("{prefix}pin'an"));
        assert_eq!(coord.debug_ui_caret_bytes(), p + 3, "{prefix:?}：pin| 'an");

        match press(&coord, VK_DOWN) {
            KeyAction::UpdateComposition { text, caret_pos } => {
                assert_eq!(text, format!("{prefix}pi'nan"));
                assert_eq!(
                    caret_pos as usize,
                    p + 4,
                    "{prefix:?}：回传的光标在 pi'n| 之后"
                );
            }
            other => panic!("{prefix:?}：形态变了应回传组合串，实得 {other:?}"),
        }
        assert_eq!(highlighted(&coord), "皮");
        assert_eq!(
            coord.debug_ui_caret_bytes(),
            p + 4,
            "{prefix:?}：自绘编码栏的插入符同样在 pi'n| 之后"
        );
    }
}

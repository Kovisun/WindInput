//! 下一词学习（v0.3「手机习惯适配」Stage 4）：从用户实际上屏的**相邻两个词**学
//! 「A 之后常接 B」，下次上屏 A 时联想首位给 B。
//!
//! 与联想历史同表（`assoc_history`）：前缀延伸行（「荷」→「荷枪实弹」）上屏补剩余，
//! 下一词行（「我们」→「一直」）整词上屏。夹具：wubi86，`trwu` =「我们」、`ggfh` =「一直」。
//!
//! 词典缺失时自动跳过（`build_dev/data` 不存在时整族静默跳过）。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn dict_ready() -> bool {
    data_dir()
        .join("schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
}

fn press(c: &Coordinator, vk: u32) -> KeyAction {
    c.handle_key_event(&KeyEventData {
        key_code: vk,
        scan_code: 0,
        modifiers: 0,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    })
}

/// 逐字母敲，返回最后一键的动作（满码自动上屏就发生在那一键上）。
fn type_code(c: &Coordinator, code: &str) -> KeyAction {
    let mut last = KeyAction::Consumed;
    for ch in code.chars() {
        last = press(c, (ch.to_ascii_uppercase() as u32) & 0xFF);
    }
    last
}

fn committed(act: &KeyAction) -> Option<&str> {
    match act {
        KeyAction::InsertText { text, .. } => Some(text),
        KeyAction::CommitThenDeferComposition { commit_text, .. } => Some(commit_text),
        _ => None,
    }
}

fn coord(
    tag: &str,
    kind: &str,
    at_full: bool,
    seed: impl FnOnce(&wind_store::Store),
) -> (Arc<Coordinator>, Arc<wind_store::Store>) {
    coord_with(tag, kind, at_full, seed, |_| {})
}

fn coord_with(
    tag: &str,
    kind: &str,
    at_full: bool,
    seed: impl FnOnce(&wind_store::Store),
    tweak: impl FnOnce(&mut Config),
) -> (Arc<Coordinator>, Arc<wind_store::Store>) {
    let store_path = std::env::temp_dir().join(format!("wind_assoc_user_{tag}.redb"));
    let _ = std::fs::remove_file(&store_path);
    let store = Arc::new(wind_store::Store::open(&store_path).unwrap());
    seed(&store);
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.input.symbol.smart_mode = false;
    cfg.input.association.kind = kind.to_string();
    cfg.input.association.mode = "continuous".to_string();
    cfg.schema.codetable.auto_commit_at_full = at_full;
    // 联想档内按选词次数重排只在调频开着时生效（`assoc_prefix_words` 查 `enabled`）。
    // `Config::default()` 不读 data/config.toml，码表调频默认关，故显式打开。
    cfg.schema.codetable.frequency.enabled = true;
    tweak(&mut cfg);
    let c = Coordinator::new_headless_with_store(cfg, Some(&data_dir()), store.clone());
    // 生产由启动后的预热线程建索引（系统反查索引 + 用户词文本索引），headless 不跑那个线程。
    c.prewarm_indexes();
    (c, store)
}

const VK_SPACE: u32 = 0x20;
const VK_COMMA: u32 = 0xBC;
const VK_1: u32 = 0x31;

/// 打码 + 空格上屏首选，返回上屏文本。
fn commit_word(c: &Coordinator, code: &str) -> String {
    type_code(c, code);
    let act = press(c, VK_SPACE);
    committed(&act).unwrap_or_default().to_string()
}

fn smart(tag: &str) -> (Arc<Coordinator>, Arc<wind_store::Store>) {
    coord(tag, "smart", false, |_| {})
}

#[test]
fn next_word_learned_from_adjacent_commits() {
    if !dict_ready() {
        eprintln!("!!! 跳过：build_dev 词库不存在");
        return;
    }
    let (c, _) = smart("next_learn");
    for _ in 0..2 {
        assert_eq!(commit_word(&c, "trwu"), "我们", "前提：trwu 首选「我们」");
        assert_eq!(commit_word(&c, "ggfh"), "一直", "前提：ggfh 首选「一直」");
    }
    commit_word(&c, "trwu");
    let texts = c.debug_assoc_texts();
    assert_eq!(
        texts.first().map(String::as_str),
        Some("一直"),
        "两次「我们一直」之后，上屏「我们」联想首位应是「一直」，实得 {texts:?}"
    );
}

/// 下一词候选整词上屏（不是当成前缀延伸只补剩余），且选中后继续强化。
#[test]
fn picking_next_word_commits_whole_word() {
    if !dict_ready() {
        return;
    }
    let (c, _) = smart("next_pick");
    commit_word(&c, "trwu");
    commit_word(&c, "ggfh");
    commit_word(&c, "trwu");
    assert_eq!(
        c.debug_assoc_texts().first().map(String::as_str),
        Some("一直")
    );
    let act = press(&c, VK_1);
    assert_eq!(
        committed(&act),
        Some("一直"),
        "数字键选下一词应整词上屏，实得 {act:?}"
    );
}

/// 中间隔了标点就不相邻：「我们，一直」不该学成「我们 → 一直」。
#[test]
fn punctuation_breaks_adjacency() {
    if !dict_ready() {
        return;
    }
    let (c, _) = smart("next_punct");
    for _ in 0..2 {
        commit_word(&c, "trwu");
        press(&c, VK_COMMA);
        commit_word(&c, "ggfh");
    }
    commit_word(&c, "trwu");
    let texts = c.debug_assoc_texts();
    assert!(
        !texts.iter().any(|t| t == "一直"),
        "隔着标点不该学到「我们 → 一直」，实得 {texts:?}"
    );
}

/// 失焦（换了输入框）也是断点。
#[test]
fn focus_change_breaks_adjacency() {
    if !dict_ready() {
        return;
    }
    let (c, _) = smart("next_focus");
    for _ in 0..2 {
        commit_word(&c, "trwu");
        c.note_word_boundary();
        commit_word(&c, "ggfh");
    }
    commit_word(&c, "trwu");
    assert!(!c.debug_assoc_texts().iter().any(|t| t == "一直"));
}

/// 联想关着不学：不让没开联想的用户默默攒数据。
#[test]
fn assoc_off_learns_nothing() {
    if !dict_ready() {
        return;
    }
    let (c, _) = coord("next_off", "off", false, |_| {});
    for _ in 0..2 {
        commit_word(&c, "trwu");
        commit_word(&c, "ggfh");
    }
    // 打开联想再看：关着时若偷偷记了，这里就会冒出来
    let (c2, _) = coord_with("next_off", "smart", false, |_| {}, |_| {});
    drop(c);
    commit_word(&c2, "trwu");
    assert!(!c2.debug_assoc_texts().iter().any(|t| t == "一直"));
}

/// 词语联想档只出「上文的延长」：学到的下一词不混进去（智能联想才出）。
#[test]
fn word_kind_does_not_show_next_words() {
    if !dict_ready() {
        return;
    }
    let (c, _) = coord("next_word_kind", "word", false, |_| {});
    for _ in 0..2 {
        commit_word(&c, "trwu");
        commit_word(&c, "ggfh");
    }
    commit_word(&c, "trwu");
    let texts = c.debug_assoc_texts();
    assert!(
        texts.iter().all(|t| t.starts_with("我们")),
        "词语联想档应只有以「我们」开头的词，实得 {texts:?}"
    );
}

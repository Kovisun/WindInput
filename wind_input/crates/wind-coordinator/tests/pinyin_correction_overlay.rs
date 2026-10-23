//! 特殊模式 / overlay 路径（快捷输入的 `$primary_pinyin` 成员）同样施加 D6：纠错候选不压过
//! 完整的非纠错候选占首位。这两条路不重排，D6 若只接在主路会漏。
//!
//! 夹具同 `pinyin_correction.rs::corrected_never_takes_top_when_complete_exists`：自定义 `in=ian`
//! 让 `din` 可纠成 `dian`，in/ing 模糊让它同时读作 `ding`；用户词「点」(dian) 权重极高。
//! 真实词库在 `build_dev/data`，缺失时跳过。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;

const VK_SEMICOLON: u32 = 0xBA;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_data() -> bool {
    let d = data_dir();
    ["wubi86", "pinyin", "english"]
        .iter()
        .all(|s| d.join(format!("schemas/{s}.schema.toml")).exists())
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

#[test]
fn quick_input_pinyin_member_applies_d6() {
    if !has_data() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("wind_corr_overlay_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(wind_store::Store::open(dir.join("u.redb")).unwrap());
    store
        .add_user_word("pinyin", "dian", "点", 1_000_000_000, 0)
        .unwrap();
    let mut c = Config::default();
    c.schema.available = vec!["wubi86".into(), "pinyin".into(), "english".into()];
    c.schema.active = "wubi86".into();
    c.schema.primary_pinyin = "pinyin".into();
    c.input.default.chinese_mode = true;
    c.input.correction.custom = vec!["in=ian".into()];
    c.schema.pinyin.fuzzy.enabled = true;
    c.schema.pinyin.fuzzy.in_ing = true;
    let coord = Coordinator::new_headless_with_store(c, Some(&data_dir()), store);
    coord.handle_key_event(&key(VK_SEMICOLON));
    for ch in "din".chars() {
        coord.handle_key_event(&key(ch.to_ascii_uppercase() as u32));
    }
    let all = coord.debug_all_candidate_texts();
    drop(coord);
    let _ = std::fs::remove_dir_all(&dir);
    let ding = all.iter().position(|t| t == "定");
    let dian = all.iter().position(|t| t == "点");
    assert!(ding.is_some() && dian.is_some(), "{all:?}");
    assert!(ding < dian, "纠错的「点」压过了完整的「定」：{all:?}");
}

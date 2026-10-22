//! 码表方案英文候选的位次下限（`schema.codetable.english_merge.position`）经
//! `EngineManager::active_english_position` 送到协调器的 `place_english_after_common_exact`。
//! 词库缺失时跳过（worktree 里要自备 `build_dev/data`）。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;
use wind_store::Store;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn key(key_code: u32) -> KeyEventData {
    KeyEventData {
        key_code,
        scan_code: 0,
        modifiers: 0,
        event_type: EVENT_KEY_DOWN,
        toggles: 0,
        event_seq: 0,
        prev_char: 0,
    }
}

/// 打 `git`（五笔有一串中文候选，英文精确命中 git），返回英文 `git` 在首页的下标与首页条数。
fn english_index(position: usize, tag: &str) -> (usize, usize) {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["wubi86".into()];
    cfg.schema.active = "wubi86".into();
    cfg.input.default.chinese_mode = true;
    cfg.schema.codetable.english_merge.enable = true;
    cfg.schema.codetable.english_merge.position = position;
    let path = std::env::temp_dir().join(format!("wind_ct_en_pos_{tag}.redb"));
    let _ = std::fs::remove_file(&path);
    let store = Arc::new(Store::open(&path).unwrap());
    let coord = Coordinator::new_headless_with_store(cfg, Some(&data_dir()), store);
    for c in "git".chars() {
        coord.handle_key_event(&key(c.to_ascii_uppercase() as u32));
    }
    let page = coord.debug_page_texts();
    let i = page
        .iter()
        .position(|t| t == "git")
        .unwrap_or_else(|| panic!("首页应有英文候选 git，实际 {page:?}"));
    (i, page.len())
}

#[test]
fn codetable_english_position_floors_placement() {
    let d = data_dir();
    if !d.join("schemas/wubi86.schema.toml").exists()
        || !d.join("schemas/english/en.dict.yaml").exists()
    {
        eprintln!("跳过：缺少五笔方案或英文词库");
        return;
    }
    let (natural, len) = english_index(1, "p1");
    assert!(
        natural + 2 < len,
        "前提：英文后面还得有中文候选，才看得出位次下限（下标 {natural} / {len}）"
    );
    let floor = natural + 3; // 1 起的位次 ⇒ 下标 natural + 2
    let (placed, _) = english_index(floor, "pn");
    assert_eq!(placed, floor - 1, "英文应被压到第 {floor} 位");
    // 下限低于让位结果时不往前拉。
    assert_eq!(english_index(natural.max(1), "pl").0, natural);
}

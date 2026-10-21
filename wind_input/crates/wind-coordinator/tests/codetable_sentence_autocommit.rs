//! 码表整句 × 满码自动上屏的端到端验证（论坛 t350）。
//!
//! 整句候选的 `code` 是整串，超码长时它常是唯一一条 `code == input`，曾被满码上屏当成
//! 「唯一全码」在第 5 码就上屏（`trntw` → 「我人」），句子永远组不起来。
//!
//! 五笔 86 出厂词库：`trnt` 有「我 / 特性」两条重码，故第 4 码不触发满码上屏，
//! 能走进超码长区间。
//!
//! ⚠️ 依赖 `build_dev/data` 真实词库；缺失时**静默跳过**（判据是耗时 0.00s）。

use std::path::PathBuf;
use std::sync::Arc;
use wind_bridge::handler::{KeyAction, KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;
use wind_store::Store;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_data() -> bool {
    data_dir()
        .join("schemas/wubi86/wubi86_jidian.dict.yaml")
        .exists()
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

/// 用例结束时删掉临时目录。
struct TempRoot(PathBuf);
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 整句 + 满码上屏经方案级 override 打开；目录名带 pid，并行的 `cargo test` 不互相删库。
fn coord() -> (Arc<Coordinator>, TempRoot) {
    let root = std::env::temp_dir().join(format!("wind_sentence_ac_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ov = root.join("overrides");
    std::fs::create_dir_all(&ov).unwrap();
    std::fs::write(
        ov.join("wubi86.toml"),
        "[engine.codetable]\nsentence_input = true\nauto_commit_at_full = true\n",
    )
    .unwrap();
    let store = Arc::new(Store::open(root.join("user_data.db")).unwrap());
    let mut c = Config::default();
    c.schema.available = vec!["wubi86".into()];
    c.schema.active = "wubi86".into();
    c.input.default.chinese_mode = true;
    let coord =
        Coordinator::new_headless_with_store_override(c, Some(&data_dir()), store, Some(ov));
    (coord, TempRoot(root))
}

#[test]
fn sentence_survives_auto_commit_at_full() {
    if !has_data() {
        eprintln!("跳过：缺 build_dev 词库");
        return;
    }
    let (coord, _dir) = coord();
    // 我 trnt / 们 wuny：连打 8 码，期间任何一键都不得上屏。
    for ch in "trntwuny".chars() {
        let action = coord.handle_key_event(&key(u32::from(ch.to_ascii_uppercase())));
        assert!(
            !matches!(action, KeyAction::InsertText { .. }),
            "第 {ch} 键不该自动上屏，实际 {action:?}"
        );
    }
    // 只钉「句子还在、以我们开头」：`wuny` 末尾的 `y` 怎么切归解码质量，不归本条。
    let page = coord.debug_page_texts();
    assert!(
        page.first().is_some_and(|t| t.starts_with("我们")),
        "整句应仍在组，实际 {page:?}"
    );
}

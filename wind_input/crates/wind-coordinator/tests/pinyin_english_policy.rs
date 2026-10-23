//! 拼音方案英文候选的界面位次（`schema.pinyin.english_merge`），真实词库、协调器界面序：
//!
//! - 吃满整串的拼音真词（含简拼解出来的 bus→不是）算进英文让过的开头段；拼凑句不算；
//! - `exact_position` 出厂按方案自动：全拼 1、双拼 2；
//! - 中文侧为空的前缀兜底页里，与输入相同的词排最前。
//!
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

fn ready(schema: &str) -> bool {
    let d = data_dir();
    let ok = d.join(format!("schemas/{schema}.schema.toml")).exists()
        && d.join("schemas/english/en.dict.yaml").exists();
    if !ok {
        eprintln!("跳过：缺少 {schema} 方案或英文词库");
    }
    ok
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

fn config(schema: &str, f: impl FnOnce(&mut Config)) -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec![schema.into()];
    cfg.schema.active = schema.into();
    cfg.input.default.chinese_mode = true;
    cfg.schema.pinyin.english_merge.enable = true;
    f(&mut cfg);
    cfg
}

/// 打 `input`，返回协调器排好的全部候选文本（界面序）。
fn typed(cfg: Config, input: &str, tag: &str) -> Vec<String> {
    let path = std::env::temp_dir().join(format!("wind_py_en_policy_{tag}.redb"));
    let _ = std::fs::remove_file(&path);
    let store = Arc::new(Store::open(&path).unwrap());
    let coord = Coordinator::new_headless_with_store(cfg, Some(&data_dir()), store);
    for c in input.chars() {
        coord.handle_key_event(&key(c.to_ascii_uppercase() as u32));
    }
    coord.debug_all_candidate_texts()
}

fn index_of(list: &[String], word: &str, input: &str) -> usize {
    list.iter().position(|t| t == word).unwrap_or_else(|| {
        panic!(
            "{input} 应有候选 {word}，实际 {:?}",
            &list[..list.len().min(12)]
        )
    })
}

/// 简拼真词吃满整串（bus→不是、rug→如果）时，英文让到它后面。
#[test]
fn abbrev_real_word_leads_english() {
    if !ready("pinyin") {
        return;
    }
    for (input, word) in [("bus", "不是"), ("rug", "如果")] {
        let list = typed(config("pinyin", |_| {}), input, input);
        assert_eq!(
            list[0],
            word,
            "{input} 首选应是「{word}」，实际 {:?}",
            &list[..4]
        );
        assert_eq!(
            list[1],
            input,
            "{input} 英文应在第 2，实际 {:?}",
            &list[..4]
        );
    }
}

/// 拼凑句不算开头段：hello / world 英文仍第 1；hen 的英文位次不受影响。
#[test]
fn synthesized_sentence_still_yields_to_english() {
    if !ready("pinyin") {
        return;
    }
    for input in ["hello", "world"] {
        let list = typed(config("pinyin", |_| {}), input, input);
        assert_eq!(
            list[0],
            input,
            "{input} 英文应仍第 1，实际 {:?}",
            &list[..4]
        );
    }
    let hen = typed(config("pinyin", |_| {}), "hen", "hen");
    let i = index_of(&hen, "hen", "hen");
    // 开头只有常用精确单字（很 恨 狠 痕…）能排在它前面。
    assert!(
        hen[..i].iter().all(|t| t.chars().count() == 1),
        "hen 前面只该有精确单字，实际 {:?}",
        &hen[..=i]
    );
}

/// 双拼出厂下英文不当首选、排第 2；全拼 hello 仍第 1。
#[test]
fn shuangpin_english_not_first_by_default() {
    if !ready("shuangpin") || !ready("pinyin") {
        return;
    }
    for input in ["the", "good", "this"] {
        let list = typed(config("shuangpin", |_| {}), input, &format!("sp_{input}"));
        assert_ne!(
            list[0],
            input,
            "双拼 {input} 首选不该是英文，实际 {:?}",
            &list[..4]
        );
        assert_eq!(
            list[1],
            input,
            "双拼 {input} 英文应在第 2，实际 {:?}",
            &list[..4]
        );
    }
    let hello = typed(config("pinyin", |_| {}), "hello", "qp_hello");
    assert_eq!(hello[0], "hello");
}

/// 显式写了 `exact_position` 时全拼双拼都用它。
#[test]
fn explicit_exact_position_applies_to_both() {
    if !ready("shuangpin") || !ready("pinyin") {
        return;
    }
    let set3 = |c: &mut Config| c.schema.pinyin.english_merge.exact_position = 3;
    let sp = typed(config("shuangpin", set3), "the", "sp_the_3");
    assert_eq!(sp[2], "the", "双拼显式 3 时英文在第 3，实际 {:?}", &sp[..4]);
    let qp = typed(config("pinyin", set3), "hello", "qp_hello_3");
    assert_eq!(
        qp[2],
        "hello",
        "全拼显式 3 时英文在第 3，实际 {:?}",
        &qp[..4]
    );
}

/// 中文侧为空的前缀兜底页：与输入相同的词排最前（hi 打出 hi 第 1）。
#[test]
fn prefix_fallback_puts_exact_word_first() {
    if !ready("pinyin") {
        return;
    }
    let list = typed(config("pinyin", |_| {}), "hi", "hi");
    assert_eq!(list[0], "hi", "hi 应排第 1，实际 {list:?}");
}

/// `complete_min_length` 开着、`max_count` 出厂 2：world 只多补 1 条，且补全不当首选。
#[test]
fn completion_capped_by_max_count() {
    if !ready("pinyin") {
        return;
    }
    let cfg = config("pinyin", |c| {
        c.schema.pinyin.english_merge.complete_min_length = 5
    });
    let list = typed(cfg, "world", "world_complete");
    assert_eq!(list[0], "world", "实际 {:?}", &list[..4]);
    let english: Vec<&String> = list
        .iter()
        .filter(|t| t.to_ascii_lowercase().starts_with("world"))
        .collect();
    assert_eq!(
        english.len(),
        2,
        "完整单词 + 补全合计 2 条，实际 {english:?}"
    );
}

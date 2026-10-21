//! 系统词模糊命中不依赖「猜中」的那条切分。
//!
//! `pinan` 在音节图上有 `pi|nan` 与 `pin|an` 两条等长切分。模糊变体若只按其中一条逐音节展开，
//! 另一条上的 `pin|an → ping|an`（平安）就永远查不到，用户只能打 `pin'an`。本文件钉的是
//! 「每条切分上的变体都要查」，并附「模糊音关着不出」的对照，证明命中来自模糊路径。
//!
//! 位次在 **Coordinator 层**取（协调器会按消费长度等再排一次，引擎位次 ≠ 界面位次）。
//!
//! ⚠️ 词典缺失时整族静默跳过（判据是**耗时 0.00s**，不是通过条数）。

use std::path::PathBuf;
use wind_bridge::handler::{KeyEventData, MessageHandler};
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_ipc::protocol::EVENT_KEY_DOWN;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn has_pinyin() -> bool {
    data_dir()
        .join("schemas/pinyin/cn_dicts/base.dict.yaml")
        .exists()
}

fn candidates_for(input: &str, apply: impl FnOnce(&mut Config)) -> Vec<String> {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["pinyin".into()];
    cfg.schema.active = "pinyin".into();
    cfg.input.default.chinese_mode = true;
    apply(&mut cfg);
    let coord = Coordinator::new_headless(cfg, Some(&data_dir()));
    for c in input.chars() {
        coord.handle_key_event(&KeyEventData {
            key_code: (c.to_ascii_uppercase() as u32) & 0xFF,
            scan_code: 0,
            modifiers: 0,
            event_type: EVENT_KEY_DOWN,
            toggles: 0,
            event_seq: 0,
            prev_char: 0,
        });
    }
    coord.debug_all_candidate_texts()
}

/// 开关打开时 `input` 的界面候选前 `top` 项含 `want`，模糊音关着时全列表不含。
fn assert_fuzzy_recalls(input: &str, want: &str, top: usize, on: fn(&mut Config)) {
    let with = candidates_for(input, |cfg| {
        cfg.schema.pinyin.fuzzy.enabled = true;
        on(cfg);
    });
    let head = &with[..with.len().min(top)];
    assert!(
        head.iter().any(|t| t == want),
        "开模糊音后 {input} 前 {top} 项须有「{want}」，实际: {head:?}"
    );
    let without = candidates_for(input, |_| {});
    assert!(
        !without.iter().any(|t| t == want),
        "模糊音关着时 {input} 不该出「{want}」，否则本测试证明不了模糊路径生效"
    );
}

/// 报障原例：`pi|nan` 与 `pin|an` 等长，平安在后一条切分上。
#[test]
fn in_ing_pinan_recalls_pingan() {
    if !has_pinyin() {
        return;
    }
    assert_fuzzy_recalls("pinan", "平安", 5, |cfg| {
        cfg.schema.pinyin.fuzzy.in_ing = true
    });
}

/// 多切分展开后，变体码会撞上切法不同的词：只开 an↔ang 时 `xi|an → xi|ang` 拼出 `xiang`，
/// 而「想」是单音节 `xiang`，不是 `xi|ang`。词条边界与变体边界同域，对不上就不能进图，
/// 否则 `kandaoxian` 的整句成了「看到想」（`xian → xiang` 要开 ian↔iang 才算）。
#[test]
fn an_ang_does_not_borrow_words_across_segmentations() {
    if !has_pinyin() {
        return;
    }
    let got = candidates_for("kandaoxian", |cfg| {
        cfg.schema.pinyin.fuzzy.enabled = true;
        cfg.schema.pinyin.fuzzy.an_ang = true;
    });
    let head = &got[..got.len().min(5)];
    assert!(
        !head.iter().any(|t| t.contains('想')),
        "只开 an↔ang 时 kandaoxian 前 5 项不该出现「想」，实际: {head:?}"
    );
}

/// 韵母组换成 an↔ang：`fa|nan`（发难）与 `fan|an → fang|an`（方案）。
#[test]
fn an_ang_fanan_recalls_fangan() {
    if !has_pinyin() {
        return;
    }
    assert_fuzzy_recalls("fanan", "方案", 5, |cfg| {
        cfg.schema.pinyin.fuzzy.an_ang = true
    });
}

/// 整串已有精确词时（`ji|nan` 济南），平安那类整句兜不住：`jin|an → jing|an`（静安）只能靠
/// 词候选那一路查出来，而它此前只按 `maximum_match` 那一条切分展开变体。
#[test]
fn in_ing_jinan_recalls_jingan() {
    if !has_pinyin() {
        return;
    }
    assert_fuzzy_recalls("jinan", "静安", 9, |cfg| {
        cfg.schema.pinyin.fuzzy.in_ing = true
    });
}

/// 同上，另一组真实竞争：`xi|nan`（西南）占着整串，`xin|an → xing|an`（兴安）只能靠词候选。
#[test]
fn in_ing_xinan_recalls_xingan() {
    if !has_pinyin() {
        return;
    }
    assert_fuzzy_recalls("xinan", "兴安", 9, |cfg| {
        cfg.schema.pinyin.fuzzy.in_ing = true
    });
}

/// 前缀子码同口径：`jinanshi` 的前缀 `jinan` 也要按全部切分展开（静安）。
#[test]
fn in_ing_prefix_jinanshi_recalls_jingan() {
    if !has_pinyin() {
        return;
    }
    assert_fuzzy_recalls("jinanshi", "静安", 20, |cfg| {
        cfg.schema.pinyin.fuzzy.in_ing = true
    });
}

/// 词候选那一路同样核对边界：只开 an↔ang 时 `xian` 的 `xi|an → xi|ang` 不能借来单音节的「想」。
#[test]
fn an_ang_xian_word_candidates_do_not_borrow_xiang() {
    if !has_pinyin() {
        return;
    }
    let got = candidates_for("xian", |cfg| {
        cfg.schema.pinyin.fuzzy.enabled = true;
        cfg.schema.pinyin.fuzzy.an_ang = true;
    });
    let head = &got[..got.len().min(30)];
    assert!(
        !head.iter().any(|t| t == "想"),
        "只开 an↔ang 时 xian 前 30 项不该有「想」，实际: {head:?}"
    );
}

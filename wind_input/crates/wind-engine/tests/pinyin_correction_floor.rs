//! 换位纠错只能**加**候选：关纠错时有的候选文字，开纠错后必须仍在（前 50 条）。
//!
//! 纠错拼写注册成切分边后，曾挤掉同一跨度上的合法解释（`xign` 的「习惯」、`yuo` 的「与哦」）。
//!
//! 词库不存在时跳过。

use std::path::PathBuf;

use wind_config::Config;
use wind_engine::EngineManager;

fn data_dir() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data");
    p.join("schemas/pinyin/cn_dicts/base.dict.yaml")
        .exists()
        .then_some(p)
}

fn texts(dir: &std::path::Path, correction: bool, input: &str, limit: usize) -> Vec<String> {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["pinyin".to_string()];
    cfg.schema.active = "pinyin".to_string();
    cfg.input.correction.enabled = correction;
    let mgr = EngineManager::new(&cfg, Some(dir));
    mgr.convert_with("pinyin", input, limit)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[test]
fn correction_never_removes_candidates() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：无 build_dev/data 词库");
        return;
    };
    let mut lost = Vec::new();
    // 前 8 条是回归现场；其后是排查中各机制各自的现场（混合简拼降级、同文占位、纠错召回）。
    let inputs = [
        "yuo", "yone", "xign", "gogn", "xiong", "zhong", "nage", "liou", "zhon", "boa", "gognping",
        "zhonguo", "hia", "xignfu",
    ];
    for input in inputs {
        let off = texts(&dir, false, input, 50);
        let on = texts(&dir, true, input, 50);
        for t in &off {
            if !on.contains(t) {
                lost.push(format!("{input}: {t}"));
            }
        }
    }
    assert!(lost.is_empty(), "开纠错后消失的候选: {lost:?}");
}

/// 守卫不能靠「纠错什么都不出」空过：该加的纠错候选仍在。
#[test]
fn correction_still_adds_candidates() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：无 build_dev/data 词库");
        return;
    };
    // `zhonguo` 关纠错时已有 125 条（前缀 `zhong` 的字），纠错候选只占剩余名额，
    // 窗口放得下全部非纠错候选才轮得到它。
    for (input, text, limit) in [
        ("gogn", "工", 50),
        ("gognping", "公平", 50),
        ("hia", "还", 50),
        ("zhonguo", "中国", 300),
    ] {
        let t = text.to_string();
        assert!(!texts(&dir, false, input, limit).contains(&t), "{input}");
        assert!(texts(&dir, true, input, limit).contains(&t), "{input}");
    }
}

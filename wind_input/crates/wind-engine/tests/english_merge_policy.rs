//! 码表方案英文混入的上屏否决在**真实词库**下的验证。
use std::path::PathBuf;
use wind_candidate::{Candidate, CandidateSource};
use wind_config::Config;
use wind_engine::EngineManager;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data")
}

fn ready(schema: &str) -> bool {
    let dir = data_dir();
    let ok = dir.join(format!("schemas/{schema}.schema.toml")).exists()
        && dir.join("schemas/english/en.dict.yaml").exists();
    if !ok {
        eprintln!("跳过：{schema} 方案或英文词库不存在");
    }
    ok
}

fn make_config(schema: &str) -> Config {
    let mut cfg = Config::default();
    cfg.schema.available = vec![schema.to_string()];
    cfg.schema.active = schema.to_string();
    cfg
}

fn english(r: &wind_engine::ConvertResult) -> Vec<&Candidate> {
    r.candidates
        .iter()
        .filter(|c| c.source == CandidateSource::English)
        .collect()
}

fn wubi(f: impl FnOnce(&mut Config)) -> EngineManager {
    let mut cfg = make_config("wubi86");
    cfg.schema.codetable.english_merge.enable = true;
    f(&mut cfg);
    EngineManager::new(&cfg, Some(&data_dir()))
}

/// ⑤ 漏洞：开满码自动上屏时，英文词的 4 码前缀（此刻无精确命中、列表里没有英文）须被
/// 英文否决。`abse` 是五笔唯一全码「荫棚」，也是 absent / absence 的前缀。
#[test]
fn codetable_block_commit_vetoes_prefix_at_full_code() {
    if !ready("wubi86") {
        return;
    }
    let build = |block: bool| {
        wubi(|c| {
            c.schema.codetable.auto_commit_at_full = true;
            c.schema.codetable.english_merge.block_commit = block;
        })
    };
    assert!(
        build(false).convert("abse", 50).should_commit,
        "前提：关掉否决时 abse 满码上屏，否则测不到否决"
    );
    let guarded = build(true).convert("abse", 50);
    assert!(
        english(&guarded).is_empty(),
        "前提：出厂口径下 abse 不显示英文（无精确命中、不补前缀）"
    );
    assert!(
        !guarded.should_commit,
        "abse 是 absent 的前缀，满码上屏须被否决，否则 absent 敲不完"
    );
    // 否决须叠「英文确有词条」：英文里没有以它为前缀的满码照常上屏。
    let plain = build(true).convert("aaaa", 50);
    assert_eq!(
        plain.should_commit,
        build(false).convert("aaaa", 50).should_commit
    );
}

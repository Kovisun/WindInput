//! 误读自动推断（设计 `docs/design/pinyin-correction-and-hints.md` §6.1）：整条是词库词、而用户打的
//! 音节与它所有登记读音都不同 ⇒ 按权重最高的读音标出读错的字（`tiaoyan` → 「调(diao)研」）。
//! 读音索引后台建，未就绪时不标；随配置热重载建 / 放。
//!
//! 自造夹具；方案 id 每个用例各取一个，免得并行用例共用同一份反查索引缓存。

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use wind_bridge::handler::{CaretData, KeyEventData, MessageHandler};
use wind_config::{Config, HintScope};
use wind_coordinator::Coordinator;
use wind_coordinator::web_host::WebDataHost;
use wind_ipc::protocol::{EVENT_KEY_DOWN, caret_source};
use wind_ui_types::{CandidateItem, UiCommand};

const DICT: &str = "调\tdiao\t800\n调\ttiao\t500\n研\tyan\t500\n调研\tdiao yan\t3784\n";

struct Fixture {
    dir: PathBuf,
    schema: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(tag: &str) -> Fixture {
    let schema = format!("mrinfer{tag}");
    let dir = std::env::temp_dir().join(format!("wind_mrinfer_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = dir.join("schemas");
    std::fs::create_dir_all(schemas.join(&schema)).unwrap();
    std::fs::write(
        schemas.join(format!("{schema}/mini.dict.yaml")),
        format!("---\nname: mini\nversion: \"1\"\nsort: by_weight\n...\n{DICT}"),
    )
    .unwrap();
    std::fs::write(
        schemas.join(format!("{schema}.schema.toml")),
        format!(
            "[schema]\nid = \"{schema}\"\nname = \"{schema}\"\n[engine]\ntype = \"pinyin\"\n\
             [[dictionaries]]\nid = \"mini\"\npath = \"{schema}/mini.dict.yaml\"\n\
             type = \"rime_pinyin\"\ndefault = true\n"
        ),
    )
    .unwrap();
    Fixture { dir, schema }
}

fn config(f: &Fixture, misread: bool, hint: bool) -> Config {
    let mut c = Config::default();
    c.schema.available = vec![f.schema.clone()];
    c.schema.active = f.schema.clone();
    c.input.default.chinese_mode = true;
    let h = &mut c.input.correction.hint;
    h.enabled = hint;
    h.scope = HintScope::All;
    h.misread = misread;
    // 重载读盘时 `Config::load` 已规整过，用例给的也得是这个形态，否则方案段次次算「变了」。
    c.normalize();
    c
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

fn type_str(coord: &Coordinator, s: &str) {
    for ch in s.chars() {
        coord.handle_key_event(&key(ch.to_ascii_uppercase() as u32));
    }
}

fn clear(coord: &Coordinator) {
    coord.handle_key_event(&key(0x1B)); // Esc
}

fn shown(coord: &Coordinator, rx: &Receiver<UiCommand>) -> Vec<CandidateItem> {
    coord.handle_caret_update(&CaretData {
        x: 100,
        y: 200,
        height: 20,
        composition_start_x: 100,
        composition_start_y: 200,
        source: caret_source::TSF_SELECTION,
        composition_rect: None,
    });
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            UiCommand::UpdateCandidates { candidates, .. } => Some(candidates),
            _ => None,
        })
        .last()
        .expect("应下发候选")
}

/// 打 `input`、等后台读音索引、清掉重打一次，取这次的候选页。
fn typed(coord: &Coordinator, rx: &Receiver<UiCommand>, input: &str) -> Vec<CandidateItem> {
    type_str(coord, input);
    coord.wait_misread_readings();
    clear(coord);
    let _ = rx.try_iter().count();
    type_str(coord, input);
    let items = shown(coord, rx);
    clear(coord);
    items
}

fn styled<'a>(items: &'a [CandidateItem], text: &str) -> &'a str {
    items
        .iter()
        .find(|c| c.text == text)
        .unwrap_or_else(|| {
            panic!(
                "应有「{text}」：{:?}",
                items.iter().map(|c| c.text.as_str()).collect::<Vec<_>>()
            )
        })
        .text_styled
        .as_str()
}

fn open(f: &Fixture, misread: bool, hint: bool) -> (Arc<Coordinator>, Receiver<UiCommand>) {
    Coordinator::new_headless_with_ui(config(f, misread, hint), Some(&f.dir))
}

#[test]
fn tiaoyan_shows_diao_hint() {
    let f = fixture("on");
    let (coord, rx) = open(&f, true, true);
    let items = typed(&coord, &rx, "tiaoyan");
    assert_eq!(styled(&items, "调研"), "调(diao)研");
    let items = typed(&coord, &rx, "diaoyan");
    assert_eq!(styled(&items, "调研"), "", "打对了不标");
    let items = typed(&coord, &rx, "tiao");
    assert_eq!(styled(&items, "调"), "", "单字不标");
}

#[test]
fn no_infer_when_misread_unchecked() {
    let f = fixture("off");
    let (coord, rx) = open(&f, false, true);
    let items = typed(&coord, &rx, "tiaoyan");
    assert_eq!(styled(&items, "调研"), "");
    assert!(!coord.engine_mgr().misread_readings_ready(&f.schema));
}

static RELOAD_CFG: Mutex<Option<Config>> = Mutex::new(None);

fn reload_overlay(cfg: &mut Config) {
    if let Some(c) = RELOAD_CFG.lock().unwrap().clone() {
        *cfg = c;
    }
}

/// 读音索引随热重载：关「误读」即释放、不再标；再打开就重建、又能标。
#[test]
fn readings_follow_reload() {
    let f = fixture("reload");
    let (coord, rx) = open(&f, true, true);
    let items = typed(&coord, &rx, "tiaoyan");
    assert_eq!(styled(&items, "调研"), "调(diao)研");
    assert!(coord.engine_mgr().misread_readings_ready(&f.schema));

    assert!(coord.set_config_overlay(reload_overlay));
    *RELOAD_CFG.lock().unwrap() = Some(config(&f, true, false));
    coord.reload_user_config();
    coord.wait_misread_readings();
    assert!(
        !coord.engine_mgr().misread_readings_ready(&f.schema),
        "关掉正文提示且无模板引用：释放"
    );

    *RELOAD_CFG.lock().unwrap() = Some(config(&f, true, true));
    coord.reload_user_config();
    let items = typed(&coord, &rx, "tiaoyan");
    assert_eq!(styled(&items, "调研"), "调(diao)研", "重新打开后重建");
}

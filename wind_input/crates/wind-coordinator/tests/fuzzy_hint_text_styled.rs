//! 模糊音提示的显示正文：协调器按 `meta.fuzzy_hints` 与 `hint_scope` 生成
//! `CandidateItem.text_styled`（「苹(ping)果」，`(ping)` 段角色 `fuzzy_hint`），上屏文字不变。
//!
//! 自造夹具，不依赖 `build_dev/data`：`pinguo` 精确命中「拼过」，in/ing 模糊命中「苹果」「平过」。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use wind_bridge::handler::{CaretData, KeyAction, KeyEventData, MessageHandler};
use wind_config::{Config, FuzzyHintScope};
use wind_coordinator::Coordinator;
use wind_ipc::protocol::{EVENT_KEY_DOWN, caret_source};
use wind_ui_types::{CandidateItem, UiCommand};

const DICT: &str = "拼过\tpin guo\t5000\n苹果\tping guo\t3000\n平过\tping guo\t100\n";

struct Fixture {
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(tag: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("wind_fuzzy_hint_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = dir.join("schemas");
    std::fs::create_dir_all(schemas.join("fh")).unwrap();
    std::fs::write(
        schemas.join("fh/mini.dict.yaml"),
        format!("---\nname: mini\nversion: \"1\"\nsort: by_weight\n...\n{DICT}"),
    )
    .unwrap();
    std::fs::write(
        schemas.join("fh.schema.toml"),
        "[schema]\nid = \"fh\"\nname = \"fh\"\n[engine]\ntype = \"pinyin\"\n\
         [[dictionaries]]\nid = \"mini\"\npath = \"fh/mini.dict.yaml\"\n\
         type = \"rime_pinyin\"\ndefault = true\n",
    )
    .unwrap();
    Fixture { dir }
}

fn open(f: &Fixture, hint: bool, scope: FuzzyHintScope) -> (Arc<Coordinator>, Receiver<UiCommand>) {
    let mut c = Config::default();
    c.schema.available = vec!["fh".into()];
    c.schema.active = "fh".into();
    c.input.default.chinese_mode = true;
    let fz = &mut c.schema.pinyin.fuzzy;
    fz.enabled = true;
    fz.in_ing = true;
    fz.hint = hint;
    fz.hint_scope = scope;
    Coordinator::new_headless_with_ui(c, Some(&f.dir))
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

/// 放行首显闸门后取最近一次下发的候选页。
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

fn item<'a>(items: &'a [CandidateItem], text: &str) -> &'a CandidateItem {
    items
        .iter()
        .find(|c| c.text == text)
        .unwrap_or_else(|| panic!("应有候选「{text}」：{:?}", texts(items)))
}

fn texts(items: &[CandidateItem]) -> Vec<&str> {
    items.iter().map(|c| c.text.as_str()).collect()
}

fn pinguo(tag: &str, hint: bool, scope: FuzzyHintScope) -> Vec<CandidateItem> {
    let f = fixture(tag);
    let (coord, rx) = open(&f, hint, scope);
    type_str(&coord, "pinguo");
    shown(&coord, &rx)
}

fn hint_span(c: &CandidateItem) -> Vec<(&str, Option<&str>)> {
    let s = c.text_styled.as_str();
    c.text_styled
        .spans()
        .iter()
        .map(|sp| (&s[sp.start as usize..sp.end as usize], sp.role))
        .collect()
}

#[test]
fn scope_first_marks_only_first_fuzzy() {
    let items = pinguo("first", true, FuzzyHintScope::First);
    assert_eq!(
        items[0].text,
        "拼过",
        "前提：首选是精确命中：{:?}",
        texts(&items)
    );
    assert!(items[0].text_styled.is_empty(), "精确命中不标");
    let pg = item(&items, "苹果");
    assert_eq!(pg.text_styled.as_str(), "苹(ping)果");
    assert_eq!(hint_span(pg), vec![("(ping)", Some("fuzzy_hint"))]);
    let later = item(&items, "平过");
    assert!(
        later.text_styled.is_empty(),
        "first：第一个之后的模糊候选不标：{:?}",
        later.text_styled
    );
}

#[test]
fn scope_all_marks_every_fuzzy() {
    let items = pinguo("all", true, FuzzyHintScope::All);
    assert!(item(&items, "拼过").text_styled.is_empty());
    assert_eq!(item(&items, "苹果").text_styled.as_str(), "苹(ping)果");
    assert_eq!(item(&items, "平过").text_styled.as_str(), "平(ping)过");
}

#[test]
fn hint_off_text_styled_empty() {
    let items = pinguo("off", false, FuzzyHintScope::All);
    assert!(
        items.iter().any(|c| c.text == "苹果"),
        "{:?}",
        texts(&items)
    );
    assert!(items.iter().all(|c| c.text_styled.is_empty()));
}

#[test]
fn commit_text_unchanged() {
    let f = fixture("commit");
    let (coord, rx) = open(&f, true, FuzzyHintScope::First);
    type_str(&coord, "pinguo");
    let items = shown(&coord, &rx);
    let pos = items
        .iter()
        .position(|c| c.text == "苹果")
        .expect("应有「苹果」");
    assert_eq!(
        items[pos].text_styled.as_str(),
        "苹(ping)果",
        "前提：带提示"
    );
    let action = coord.handle_key_event(&key('1' as u32 + pos as u32));
    match action {
        KeyAction::InsertText { text, .. } => assert_eq!(text, "苹果"),
        other => panic!("应上屏：{other:?}"),
    }
}

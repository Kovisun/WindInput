//! emoji 附加模块（`EMOJI_MODULES`）与它散落在别处的几份清单必须一致。
//!
//! - 出厂 `data/config.toml` 的 `[input.emoji.modules]`：键与默认值。少一个键，设置页那个开关
//!   读不到值；默认值不一致，出厂行为取决于哪层先被读到。
//! - `config_schema` 的登记：漏登记的键 `config set` 认不出来，设置端也写不进去。
//! - 三套构建脚本的 `others.txt` 剔除清单（`著名表演艺术家` 等）：漏一处，那个平台的安装包
//!   就带上了本不该分发的内容，且没有任何运行期症状。
//!
//! ⚠️ 读仓库里的真实出厂文件，不是夹具。

use std::path::{Path, PathBuf};
use wind_config::EMOJI_MODULES;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/wind_input/crates/wind-config
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("仓库根")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("读 {rel}: {e}"))
}

#[test]
fn factory_config_lists_every_module_with_its_default() {
    let cfg: toml::Value = toml::from_str(&read("data/config.toml")).expect("解析出厂配置");
    let table = cfg
        .get("input")
        .and_then(|v| v.get("emoji"))
        .and_then(|v| v.get("modules"))
        .and_then(|v| v.as_table())
        .expect("出厂配置缺 [input.emoji.modules]");
    let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut ids: Vec<&str> = EMOJI_MODULES.iter().map(|m| m.id).collect();
    ids.sort_unstable();
    assert_eq!(keys, ids, "出厂配置的模块键与 EMOJI_MODULES 不一致");
    for m in EMOJI_MODULES {
        assert_eq!(
            table[m.id].as_bool(),
            Some(m.default_on),
            "{} 的出厂值与注册表不一致",
            m.id
        );
    }
}

#[test]
fn every_module_key_is_registered_in_config_schema() {
    for m in EMOJI_MODULES {
        let key = format!("input.emoji.modules.{}", m.id);
        assert!(
            wind_config::config_schema::field(&key).is_some(),
            "config_schema 没登记 {key}"
        );
    }
}

/// `FROST_OTHERS_EXCLUDE="a b"`（bash）
fn bash_list(rel: &str) -> Vec<String> {
    let src = read(rel);
    let line = src
        .lines()
        .find(|l| l.starts_with("FROST_OTHERS_EXCLUDE=\""))
        .unwrap_or_else(|| panic!("{rel} 里找不到 FROST_OTHERS_EXCLUDE"));
    line["FROST_OTHERS_EXCLUDE=\"".len()..]
        .trim_end_matches('"')
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[test]
fn build_scripts_exclude_the_same_others_sections() {
    let sh = bash_list("scripts/dev.sh");
    assert!(sh.iter().any(|s| s == "著名表演艺术家"));
    assert_eq!(
        bash_list("scripts/mac/dev.sh"),
        sh,
        "mac/dev.sh 与 dev.sh 不一致"
    );
    let ps = read("scripts/dev.ps1");
    let start = ps
        .find("$FrostOthersExclude = @(")
        .expect("dev.ps1 里找不到 $FrostOthersExclude");
    let body = &ps[start + "$FrostOthersExclude = @(".len()..];
    let body = &body[..body.find(')').expect("缺右括号")];
    let ps_list: Vec<String> = body
        .split(',')
        .map(|s| s.trim().trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(ps_list, sh, "dev.ps1 与 dev.sh 不一致");
    // 被剔除的分组不能同时被某个模块引用：那个模块会永远是空的
    for m in EMOJI_MODULES {
        for s in m.sections {
            assert!(
                !sh.iter().any(|x| x == s),
                "{} 引用了被剔除的分组 {s}",
                m.id
            );
        }
    }
}

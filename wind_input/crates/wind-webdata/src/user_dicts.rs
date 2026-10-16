//! `userDicts.*`：用户分库（设计 `user-dict-libraries-and-external-edit.md` §3）的列表、条目、
//! 增删改名、导入与开关。文件操作在 `wind_store::user_dicts`，开关与 `schema.setDictEnabled`
//! 同一实现；改了文件或开关都失效用到该数据方案的方案（同 `dict.reload` 的 `dicts:<数据方案>`）。

use super::{WebDataRpc, parse_sort, resolve_engine_type, store_text, str_param, ui_text};
use serde_json::{Value, json};
use wind_store::user_dicts::ID_PREFIX;
use wind_store::user_words::USER_DICTS_CHANGED;

/// 方案 id、它的数据方案（拼音族折叠到 `pinyin`）与 store。混输没有自己的分库（随成员方案走）。
fn target<'a, H: WebDataRpc + ?Sized>(
    h: &'a H,
    params: &'a Value,
) -> anyhow::Result<(&'a str, String, &'a wind_store::Store)> {
    let schema_id = str_param(params, "schemaId")?;
    let s = h
        .engine_mgr()
        .schema_merged(schema_id)
        .ok_or_else(|| anyhow::anyhow!("方案不存在: {schema_id}"))?;
    if resolve_engine_type(&s) == "mixed" {
        anyhow::bail!("混输方案没有自己的分库，请在它的码表 / 拼音方案下管理");
    }
    let store = h
        .user_store()
        .ok_or_else(|| anyhow::anyhow!("无持久化存储"))?;
    Ok((schema_id, h.engine_mgr().data_schema_id(schema_id), store))
}

/// 写操作的前置：只读模式拒绝。
fn writable<'a, H: WebDataRpc + ?Sized>(
    h: &'a H,
    params: &'a Value,
) -> anyhow::Result<(&'a str, String, &'a wind_store::Store)> {
    if let Some(r) = h.storage_readonly_reason() {
        anyhow::bail!("{r}");
    }
    target(h, params)
}

fn changed<H: WebDataRpc + ?Sized>(h: &H, ds: &str) {
    h.apply_user_text_changes(&[format!("{USER_DICTS_CHANGED}{ds}")]);
}

/// `userDicts.list {schemaId}` → `[{name, label, enabled, entries, file, issues?}]`，按名排序。
pub(super) fn list<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let (schema_id, ds, store) = target(h, params)?;
    let items: Vec<Value> = store
        .list_user_dicts(&ds, schema_id)?
        .into_iter()
        .map(|l| {
            let mut v = json!({
                "name": l.name,
                "label": l.label,
                "enabled": l.enabled,
                "entries": l.entries,
                "file": l.file,
            });
            if l.unreadable {
                v["issues"] = json!([{ "kind": "isolated" }]);
            } else if l.bad_lines > 0 {
                v["issues"] = json!([{ "kind": "bad_lines", "lines": l.bad_lines }]);
            }
            v
        })
        .collect();
    Ok(Value::Array(items))
}

/// `userDicts.entries {schemaId, name, offset, limit, query?, sortBy?, sortOrder?}` →
/// `{items: [{code, text, weight, enabled}], total}`（同 `dict.listPaged`）。
pub(super) fn entries<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let (_, ds, store) = target(h, params)?;
    let name = str_param(params, "name")?;
    let all = store.user_dict_entries(&ds, name)?;
    let query = params
        .get("query")
        .or_else(|| params.get("prefix"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let limit = super::usize_param(params, "limit", 50);
    let offset = super::usize_param(params, "offset", 0);
    // 同 `dict.listPaged`：编码前缀命中在前，词条内容 / 编码中段包含在后
    let mut rows = if query.is_empty() {
        all
    } else {
        let (code_q, _) = wind_store::wdict::split_spaced_code(query);
        let code_q = code_q.to_lowercase();
        let q = query.to_lowercase();
        let (mut head, tail): (Vec<_>, Vec<_>) = all
            .into_iter()
            .filter(|e| {
                e.code.to_lowercase().contains(&code_q) || e.text.to_lowercase().contains(&q)
            })
            .partition(|e| e.code.to_lowercase().starts_with(&code_q));
        head.extend(tail);
        head
    };
    if let Some((by, desc)) = parse_sort(params, &["code", "text", "weight"]) {
        rows.sort_by(|a, b| {
            let o = match by {
                "weight" => a.weight.cmp(&b.weight),
                "text" => a.text.cmp(&b.text),
                _ => a.code.cmp(&b.code),
            };
            if desc { o.reverse() } else { o }
        });
    }
    let total = rows.len();
    let items: Vec<Value> = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|e| {
            json!({
                "code": wind_store::wdict::join_code_by_boundary(&e.code, e.boundary),
                "text": ui_text(&e.text),
                "weight": e.weight,
                "enabled": true,
            })
        })
        .collect();
    Ok(json!({ "items": items, "total": total }))
}

/// `userDicts.removeEntry {schemaId, name, code, text}` → `{ok, removed}`。
pub(super) fn remove_entry<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let (_, ds, store) = writable(h, params)?;
    let name = str_param(params, "name")?;
    let (code, text) = (str_param(params, "code")?, str_param(params, "text")?);
    let removed = store.remove_user_dict_entry(&ds, name, code, &store_text(text))?;
    if removed {
        changed(h, &ds);
    }
    Ok(json!({ "ok": true, "removed": removed }))
}

/// `userDicts.create {schemaId, name}` → `{ok, file}`。
pub(super) fn create<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let (_, ds, store) = writable(h, params)?;
    let name = str_param(params, "name")?;
    store.create_user_dict(&ds, name)?;
    changed(h, &ds);
    Ok(
        json!({ "ok": true, "file": format!("{ds}/{}/{name}.dict.yaml", wind_store::user_dicts::LIBS_DIR) }),
    )
}

/// `userDicts.rename {schemaId, name, newName}` → `{ok}`。
pub(super) fn rename<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let (_, ds, store) = writable(h, params)?;
    let r = store.rename_user_dict(
        &ds,
        str_param(params, "name")?,
        str_param(params, "newName")?,
    );
    // 失败也失效：文件可能已改名、只是后面同步开关时出错，引擎不能留着旧路径
    changed(h, &ds);
    r?;
    Ok(json!({ "ok": true }))
}

/// `userDicts.delete {schemaId, name}` → `{ok}`。
pub(super) fn delete<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let (_, ds, store) = writable(h, params)?;
    let r = store.delete_user_dict(&ds, str_param(params, "name")?);
    // 失败也失效：文件可能已删、只是后面改开关时出错，引擎不能拿着已删文件的旧数据
    changed(h, &ds);
    r?;
    Ok(json!({ "ok": true }))
}

/// `userDicts.importAs {schemaId, name, path | content}` → `{ok, imported, skipped, format}`。
/// 第三方 / 自有文件（Rime、TSV、WindDict 的 words 段）转成分库的 Rime 格式整写。纯词表
/// （无编码）先经 `dict.encodeWords` 出码、拼成 TSV 再以 `content` 传进来。
pub(super) fn import_as<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let (schema_id, ds, store) = writable(h, params)?;
    let name = str_param(params, "name")?;
    wind_store::user_dicts::validate_name(name)?;
    let content = super::dict_source_text(params)?;
    let content = content.as_ref();
    let (fmt, rows, skipped) =
        wind_store::import_formats::parse_words_auto(content, h.code_policy_for(schema_id))
            .map_err(|e| anyhow::anyhow!(e))?;
    if fmt == wind_store::import_formats::DictFormat::WindDict
        && let Some(src) = wind_store::wdict::read_header_field(content, "engine_type")
        && !src.is_empty()
        && Some(src.as_str())
            != h.engine_mgr()
                .schema_merged(schema_id)
                .as_ref()
                .map(resolve_engine_type)
    {
        anyhow::bail!("该文件的词库类型与当前方案不一致，导入会导致编码错乱，已阻止。");
    }
    let parsed = rows.len();
    let (rows, st) = h.apply_pinyin_entry_contract(schema_id, rows, true);
    let imported = store.import_user_dict(&ds, name, &rows)?;
    changed(h, &ds);
    Ok(json!({
        "ok": true,
        "imported": imported,
        "skipped": skipped + (parsed - imported),
        "format": fmt.as_str(),
        "rejected": st.rejected,
    }))
}

/// `userDicts.setEnabled {schemaId, name, enabled}`：`schema.setDictEnabled` 的薄封装。
pub(super) fn set_enabled<H: WebDataRpc + ?Sized>(h: &H, params: &Value) -> anyhow::Result<Value> {
    let p = json!({
        "id": str_param(params, "schemaId")?,
        "dictId": format!("{ID_PREFIX}{}", str_param(params, "name")?),
        "enabled": params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
    });
    h.web_schema_set_dict_enabled(&p)
}

/// `schema.setDictEnabled` 对 `user:` 前缀的 id：写 `dicts.toml`，生效路径与扩展库相同。
pub(super) fn set_dict_enabled<H: WebDataRpc + ?Sized>(
    h: &H,
    schema_id: &str,
    dict_id: &str,
    enabled: bool,
) -> anyhow::Result<Value> {
    if let Some(r) = h.storage_readonly_reason() {
        anyhow::bail!("{r}");
    }
    let name = dict_id.strip_prefix(ID_PREFIX).unwrap_or(dict_id);
    let store = h
        .user_store()
        .ok_or_else(|| anyhow::anyhow!("无持久化存储"))?;
    let ds = h.engine_mgr().data_schema_id(schema_id);
    store.set_user_dict_enabled(&ds, schema_id, name, enabled)?;
    let live = h
        .engine_mgr()
        .set_dict_enabled_live(schema_id, dict_id, enabled);
    Ok(json!({ "ok": true, "live": live }))
}

#[cfg(test)]
mod tests {
    use crate::WebDataRpc;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;
    use wind_config::Config;
    use wind_coordinator::Coordinator;
    use wind_coordinator::web_host::WebDataHost;
    use wind_store::Store;

    struct Fx {
        dir: PathBuf,
        ct: String,
        py: String,
        c: Arc<Coordinator>,
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            if let Some(cache) = Config::cache_dir() {
                let _ = std::fs::remove_dir_all(cache.join(&self.ct));
                let _ = std::fs::remove_dir_all(cache.join(&self.py));
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 码表方案 `ct`（工 a / 式 aa）与拼音方案 `py`（你好 ni hao）。
    fn fx(tag: &str) -> Fx {
        let pid = std::process::id();
        let (ct, py) = (
            format!("zz_wdudl_ct_{tag}_{pid}"),
            format!("zz_wdudl_py_{tag}_{pid}"),
        );
        let dir = std::env::temp_dir().join(format!("wind_wdudl_{tag}_{pid}"));
        let _ = std::fs::remove_dir_all(&dir);
        let s = dir.join("schemas");
        for (id, ty, dty, body) in [
            (
                &ct,
                "codetable",
                "rime_codetable",
                "工\ta\t100\n式\taa\t10\n",
            ),
            (
                &py,
                "pinyin",
                "rime_pinyin",
                "你好\tni hao\t100\n后\thou\t100\n",
            ),
        ] {
            std::fs::create_dir_all(s.join(id)).unwrap();
            std::fs::write(
                s.join(format!("{id}.schema.toml")),
                format!(
                    "[schema]\nid = \"{id}\"\nname = \"测\"\n[engine]\ntype = \"{ty}\"\n\
                     [engine.codetable]\nmax_code_length = 4\n\
                     [[dictionaries]]\nid = \"main\"\npath = \"{id}/main.dict.yaml\"\ntype = \"{dty}\"\ndefault = true\n"
                ),
            )
            .unwrap();
            std::fs::write(
                s.join(format!("{id}/main.dict.yaml")),
                format!("---\nname: main\ncolumns: [text, code, weight]\n...\n{body}"),
            )
            .unwrap();
        }
        let store = Arc::new(Store::open(dir.join("u.redb")).unwrap());
        let mut cfg = Config::default();
        cfg.schema.available = vec![ct.clone(), py.clone()];
        cfg.schema.active = ct.clone();
        let c = Coordinator::new_headless_with_store_override(
            cfg,
            Some(&dir),
            store,
            Some(dir.join("ov")),
        );
        Fx { dir, ct, py, c }
    }

    impl Fx {
        fn rpc(&self, m: &str, p: Value) -> anyhow::Result<Value> {
            self.c.web_data_rpc(m, &p)
        }
        fn texts(&self, id: &str, input: &str) -> Vec<String> {
            self.c
                .engine_mgr()
                .convert_with(id, input, 20)
                .candidates
                .into_iter()
                .map(|c| c.text)
                .collect()
        }
        fn root(&self) -> PathBuf {
            self.c.user_store().unwrap().user_text_dir().to_path_buf()
        }
    }

    /// 端到端：建分库 → 外部写入词 → 查到（同权排在系统库前）→ 关掉查不到 → 改文件 + reload 后新词可查。
    #[test]
    fn create_write_query_toggle_reload() {
        let f = fx("e2e");
        let ct = f.ct.clone();
        let r = f
            .rpc("userDicts.create", json!({"schemaId": ct, "name": "我的"}))
            .unwrap();
        let file = f.root().join(r["file"].as_str().unwrap());
        assert!(file.is_file(), "{r}");
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("戈\ta\t100\n");
        std::fs::write(&file, &text).unwrap();
        f.rpc("dict.reload", json!({"paths": [file.to_string_lossy()]}))
            .unwrap();
        assert_eq!(f.texts(&ct, "a")[..2], ["戈", "工"]);

        f.rpc(
            "userDicts.setEnabled",
            json!({"schemaId": ct, "name": "我的", "enabled": false}),
        )
        .unwrap();
        assert!(!f.texts(&ct, "a").contains(&"戈".to_string()));
        let l = f.rpc("userDicts.list", json!({"schemaId": ct})).unwrap();
        assert_eq!(l[0]["enabled"], false, "{l}");
        f.rpc(
            "schema.setDictEnabled",
            json!({"id": ct, "dictId": "user:我的", "enabled": true}),
        )
        .unwrap();
        assert_eq!(f.texts(&ct, "a")[0], "戈");
        let ov = f.dir.join("ov").join(format!("{ct}.toml"));
        assert!(
            !std::fs::read_to_string(&ov)
                .unwrap_or_default()
                .contains("user:"),
            "分库开关不进 schema_overrides"
        );

        text.push_str("新词\tb\t5\n");
        std::fs::write(&file, &text).unwrap();
        let r = f.rpc("dict.reload", json!({})).unwrap();
        assert!(
            r["reloaded"]
                .as_array()
                .unwrap()
                .contains(&json!(format!("dicts:{ct}"))),
            "{r}"
        );
        assert_eq!(f.texts(&ct, "b"), ["新词"]);
    }

    #[test]
    fn list_entries_remove_and_paging() {
        let f = fx("entries");
        let ct = f.ct.clone();
        let tsv = f.dir.join("in.txt");
        std::fs::write(&tsv, "jj\t甲\t3\nyy\t乙\t9\nbb\t丙\t1\n坏行\n").unwrap();
        let r = f
            .rpc(
                "userDicts.importAs",
                json!({"schemaId": ct, "name": "导入", "path": tsv.to_string_lossy()}),
            )
            .unwrap();
        assert_eq!(
            (r["imported"].clone(), r["format"].clone()),
            (json!(3), json!("tsv")),
            "{r}"
        );
        assert!(
            f.rpc(
                "userDicts.importAs",
                json!({"schemaId": ct, "name": "导入", "path": tsv.to_string_lossy()}),
            )
            .is_err(),
            "已存在"
        );
        assert_eq!(f.texts(&ct, "yy"), ["乙"], "导入后即可查");

        let l = f.rpc("userDicts.list", json!({"schemaId": ct})).unwrap();
        assert_eq!(
            l,
            json!([{"name": "导入", "label": "导入", "enabled": true, "entries": 3,
                    "file": format!("{ct}/dicts/导入.dict.yaml")}])
        );
        let page = |p: Value| {
            let mut p = p;
            p["schemaId"] = json!(ct);
            p["name"] = json!("导入");
            f.rpc("userDicts.entries", p).unwrap()
        };
        let r = page(json!({"offset": 0, "limit": 2, "sortBy": "weight", "sortOrder": "desc"}));
        assert_eq!(r["total"], 3);
        assert_eq!(
            r["items"][0],
            json!({"code": "yy", "text": "乙", "weight": 9, "enabled": true})
        );
        assert_eq!(r["items"].as_array().unwrap().len(), 2);
        assert_eq!(
            page(json!({"offset": 5, "limit": 2}))["items"],
            json!([]),
            "越界给空页"
        );
        assert_eq!(page(json!({"query": "甲"}))["items"][0]["text"], "甲");

        let r = f
            .rpc(
                "userDicts.removeEntry",
                json!({"schemaId": ct, "name": "导入", "code": "yy", "text": "乙"}),
            )
            .unwrap();
        assert_eq!(r["removed"], true);
        assert!(f.texts(&ct, "yy").is_empty(), "删词后失效重建");
        assert_eq!(page(json!({}))["total"], 2);
        assert!(
            f.rpc(
                "userDicts.entries",
                json!({"schemaId": ct, "name": "无", "offset": 0, "limit": 10}),
            )
            .is_err(),
            "不存在"
        );
    }

    #[test]
    fn create_rename_delete_and_bad_names() {
        let f = fx("names");
        let ct = f.ct.clone();
        for bad in ["", "a/b", ".x", "a:b", "x."] {
            assert!(
                f.rpc("userDicts.create", json!({"schemaId": ct, "name": bad}))
                    .is_err(),
                "{bad:?}"
            );
        }
        f.rpc("userDicts.create", json!({"schemaId": ct, "name": "a"}))
            .unwrap();
        f.rpc("userDicts.create", json!({"schemaId": ct, "name": "b"}))
            .unwrap();
        assert!(
            f.rpc("userDicts.create", json!({"schemaId": ct, "name": "a"}))
                .is_err(),
            "重名"
        );
        f.rpc(
            "userDicts.setEnabled",
            json!({"schemaId": ct, "name": "a", "enabled": false}),
        )
        .unwrap();
        assert!(
            f.rpc(
                "userDicts.rename",
                json!({"schemaId": ct, "name": "a", "newName": "b"}),
            )
            .is_err(),
            "改成已有的名字"
        );
        f.rpc(
            "userDicts.rename",
            json!({"schemaId": ct, "name": "a", "newName": "甲 乙"}),
        )
        .unwrap();
        let l = f.rpc("userDicts.list", json!({"schemaId": ct})).unwrap();
        let names: Vec<(&str, &str, bool)> = l
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["name"].as_str().unwrap(),
                    v["label"].as_str().unwrap(),
                    v["enabled"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [("b", "b", true), ("甲 乙", "甲 乙", false)],
            "开关随改名"
        );
        f.rpc("userDicts.delete", json!({"schemaId": ct, "name": "甲 乙"}))
            .unwrap();
        assert!(
            f.rpc("userDicts.delete", json!({"schemaId": ct, "name": "甲 乙"}))
                .is_err()
        );
        assert_eq!(
            f.rpc("userDicts.list", json!({"schemaId": ct}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            f.rpc("userDicts.list", json!({"schemaId": "不存在的方案"}))
                .is_err()
        );
    }

    /// 拼音族折叠到 `pinyin`；开关按方案 id 记。
    #[test]
    fn pinyin_libs_fold_and_switch_per_schema() {
        let f = fx("py");
        let py = f.py.clone();
        let tsv = "hou ji bo fa\t厚积薄发\t5\n";
        let r = f
            .rpc(
                "userDicts.importAs",
                json!({"schemaId": py, "name": "成语", "content": tsv}),
            )
            .unwrap();
        assert_eq!(r["imported"], 1, "{r}");
        assert!(f.root().join("pinyin/dicts/成语.dict.yaml").is_file());
        let l = f.rpc("userDicts.list", json!({"schemaId": py})).unwrap();
        assert_eq!(l[0]["file"], "pinyin/dicts/成语.dict.yaml");
        assert_eq!(f.texts(&py, "houjibofa")[0], "厚积薄发");
        let e = f
            .rpc(
                "userDicts.entries",
                json!({"schemaId": py, "name": "成语", "offset": 0, "limit": 10}),
            )
            .unwrap();
        assert_eq!(e["items"][0]["code"], "hou ji bo fa");
        f.rpc(
            "userDicts.setEnabled",
            json!({"schemaId": py, "name": "成语", "enabled": false}),
        )
        .unwrap();
        assert!(!f.texts(&py, "houjibofa").contains(&"厚积薄发".to_string()));
        let meta = std::fs::read_to_string(f.root().join("pinyin/dicts.toml")).unwrap();
        assert!(meta.contains(&format!("[schemas.{py}]")), "{meta}");
    }

    /// 删分库时文件已删、后面改开关文件失败：照样失效，引擎不能拿着已删文件的旧数据。
    #[test]
    fn delete_invalidates_even_when_switch_file_fails() {
        let f = fx("del_fail");
        let ct = f.ct.clone();
        f.rpc(
            "userDicts.importAs",
            json!({"schemaId": ct, "name": "删", "content": "zz\t自造\t1\n"}),
        )
        .unwrap();
        assert_eq!(f.texts(&ct, "zz"), ["自造"]);
        // 开关文件读不了（是个目录）
        std::fs::create_dir_all(f.root().join(&ct).join("dicts.toml")).unwrap();
        assert!(
            f.rpc("userDicts.delete", json!({"schemaId": ct, "name": "删"}))
                .is_err()
        );
        assert!(!f.root().join(&ct).join("dicts/删.dict.yaml").exists());
        assert!(f.texts(&ct, "zz").is_empty(), "{:?}", f.texts(&ct, "zz"));
    }

    /// 备份还原把分库写回后，用到它的方案失效重建（还原出的词立即可查）。
    #[test]
    fn restore_brings_libs_back_and_rebuilds() {
        let f = fx("restore");
        let ct = f.ct.clone();
        f.rpc(
            "userDicts.importAs",
            json!({"schemaId": ct, "name": "备", "content": "zz\t自造\t1\n"}),
        )
        .unwrap();
        let pkg = f.dir.join("b.zip");
        f.rpc("backup.create", json!({"path": pkg.to_string_lossy()}))
            .unwrap();
        f.rpc("userDicts.delete", json!({"schemaId": ct, "name": "备"}))
            .unwrap();
        assert!(f.texts(&ct, "zz").is_empty());
        f.rpc(
            "backup.restore",
            json!({"path": pkg.to_string_lossy(), "sections": ["dict"], "strategy": "merge"}),
        )
        .unwrap();
        assert_eq!(f.texts(&ct, "zz"), ["自造"]);
    }
}

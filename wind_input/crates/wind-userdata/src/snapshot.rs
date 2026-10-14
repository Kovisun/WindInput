//! 快照段（设计 §3.1）：wdat（带简拼区，含无边界兜底组）+ 定长 sidecar `.meta`。
//!
//! sidecar 头 32 字节：`"WUDM" | version u16 | reserved u16 | max_seq u64 | entry_count u32 |
//! gen u64 | reserved [u8;4]`，其后每条 24 字节 `count u32 | created_at i64 | last_used i64 |
//! flags u8 | pad [u8;3]`，按 wdat 条目的**全局序号**对齐（DAT 键序 × 叶内序）。`max_seq` 是快照
//! 内嵌的折回水位：state 丢了也能只回放更大的 seq（§3.6）。
//!
//! **sidecar 是提交点**：wdat 按代号落成 `<name>-<gen>.wdat`（每次折回一个新路径，从不覆盖
//! 正被映射的文件，也就不需要 `reader_pool::replacing`），sidecar 头里记 gen 指向它，
//! sidecar 原子写完才算换段。崩溃在两步之间只留下一个无人引用的新 wdat，旧段原样可读；
//! 无人引用的代号文件在下一次换段后清掉（Windows 上仍被映射的删不掉，留给再下一次）。
//!
//! 简拼区：每个条目按 [`crate::delta::group_of`] 的规则得一个键（有边界 ⇒ 首字母串；无边界
//! ⇒ `\u{1}` + 首字符），同码同键只留一个、权重取其中最高——这就是 `wind-store`
//! `USER_ABBREV` 兜底组在段里的等价物。同码异边界（xian 下的「先」与「西安」）各得各的键。

use anyhow::bail;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wind_dict::datformat::{WdatReader, WdatStreamWriter};

#[cfg(test)]
thread_local! {
    /// 故障注入：下一次 `build` 在 wdat 落盘后、sidecar 落盘前失败（模拟崩溃在两步之间）。
    pub(crate) static FAIL_BEFORE_META: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

const META_MAGIC: &[u8; 4] = b"WUDM";
const META_HEADER: usize = 32;
const META_ROW: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotEntry {
    pub code: String,
    pub text: String,
    pub weight: i32,
    pub boundary: u64,
    pub count: u32,
    pub created_at: i64,
    pub last_used: i64,
    pub flags: u8,
}

pub struct Snapshot {
    pub reader: Arc<WdatReader>,
    meta: memmap2::Mmap,
    pub max_seq: u64,
    pub entry_count: u32,
    /// code → 该码首条目的全局序号（叶内第 i 条 = 首序号 + i）。
    first_idx: BTreeMap<String, u32>,
}

/// 从**按 (code, text) 排好序**的条目流建段：`<dir>/<name>.wdat` + `<dir>/<name>.meta`。
pub fn build(
    dir: &Path,
    name: &str,
    max_seq: u64,
    entries: impl Iterator<Item = SnapshotEntry>,
) -> anyhow::Result<Snapshot> {
    std::fs::create_dir_all(dir)?;
    let mut sw = WdatStreamWriter::new();
    let mut meta: Vec<u8> = Vec::new();
    // (简拼键, code, 该 code 最高权重)
    let mut abbrevs: Vec<(String, String, i32)> = Vec::new();
    let mut n = 0u32;

    fn flush(
        sw: &mut WdatStreamWriter,
        meta: &mut Vec<u8>,
        abbrevs: &mut Vec<(String, String, i32)>,
        code: &str,
        g: &mut Vec<SnapshotEntry>,
    ) {
        g.sort_by_key(|e| std::cmp::Reverse(e.weight));
        // 已按权重降序：同键第一次出现时的权重就是该键下最高的
        let mut keys: Vec<String> = Vec::new();
        for e in g.iter() {
            let key = crate::delta::group_of(code, e.boundary);
            if !key.is_empty() && !keys.contains(&key) {
                abbrevs.push((key.clone(), code.to_string(), e.weight));
                keys.push(key);
            }
        }
        sw.push_code(
            code,
            g.iter()
                .enumerate()
                .map(|(i, e)| (e.text.as_str(), e.weight, i as u32, e.boundary)),
        );
        for e in g.iter() {
            meta.extend_from_slice(&e.count.to_le_bytes());
            meta.extend_from_slice(&e.created_at.to_le_bytes());
            meta.extend_from_slice(&e.last_used.to_le_bytes());
            meta.push(e.flags);
            meta.extend_from_slice(&[0u8; 3]);
        }
        g.clear();
    }

    let mut cur: Option<String> = None;
    let mut group: Vec<SnapshotEntry> = Vec::new();
    for e in entries {
        if cur.as_deref() != Some(e.code.as_str()) {
            if let Some(c) = cur.take() {
                flush(&mut sw, &mut meta, &mut abbrevs, &c, &mut group);
            }
            cur = Some(e.code.clone());
        }
        group.push(e);
        n += 1;
    }
    if let Some(c) = cur.take() {
        flush(&mut sw, &mut meta, &mut abbrevs, &c, &mut group);
    }
    abbrevs.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.cmp(&a.2)).then(a.1.cmp(&b.1)));
    let mut i = 0;
    while i < abbrevs.len() {
        let k = abbrevs[i].0.clone();
        let mut codes: Vec<(&str, i32)> = Vec::new();
        while i < abbrevs.len() && abbrevs[i].0 == k {
            codes.push((abbrevs[i].1.as_str(), abbrevs[i].2));
            i += 1;
        }
        sw.push_abbrev_sorted(&k, codes.into_iter());
    }

    if n == 0 {
        // 零段：没有文件就是零段（open 返回 None），不写空 wdat
        remove(dir, name);
        bail!("空快照不落盘");
    }
    let mut generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    while wdat_path(dir, name, generation).exists() {
        generation += 1;
    }
    let wdat = wdat_path(dir, name, generation);
    sw.build().emit(&wdat)?;
    std::fs::File::open(&wdat)?.sync_all()?;
    #[cfg(test)]
    if FAIL_BEFORE_META.with(|f| f.replace(false)) {
        bail!("故障注入：wdat 已写、sidecar 未写");
    }
    let mut head = Vec::with_capacity(META_HEADER + meta.len());
    head.extend_from_slice(META_MAGIC);
    head.extend_from_slice(&1u16.to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes());
    head.extend_from_slice(&max_seq.to_le_bytes());
    head.extend_from_slice(&n.to_le_bytes());
    head.extend_from_slice(&generation.to_le_bytes());
    head.extend_from_slice(&[0u8; 4]);
    head.extend_from_slice(&meta);
    crate::state::atomic_write(&meta_path(dir, name), &head)?;
    remove_stale_wdats(dir, name, Some(generation));
    open(dir, name)?.ok_or_else(|| anyhow::anyhow!("刚建好的段打不开"))
}

fn meta_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.meta"))
}

fn wdat_path(dir: &Path, name: &str, generation: u64) -> PathBuf {
    dir.join(format!("{name}-{generation:016x}.wdat"))
}

/// 删掉 `<name>-<gen>.wdat` 里除 `keep` 以外的代号文件（尽力而为：Windows 上仍被映射的删不掉）。
fn remove_stale_wdats(dir: &Path, name: &str, keep: Option<u64>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{name}-");
    for e in rd.flatten() {
        let file = e.file_name();
        let Some(f) = file.to_str() else { continue };
        let Some(hex) = f
            .strip_prefix(&prefix)
            .and_then(|r| r.strip_suffix(".wdat"))
        else {
            continue;
        };
        let Ok(g) = u64::from_str_radix(hex, 16) else {
            continue;
        };
        if hex.len() == 16 && Some(g) != keep {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// 删段：先删 sidecar（提交点，删了即零段），再删代号文件。
pub fn remove(dir: &Path, name: &str) {
    let _ = std::fs::remove_file(meta_path(dir, name));
    remove_stale_wdats(dir, name, None);
}

/// sidecar 不存在 ⇒ `Ok(None)`（零段）；在但头坏、指向的 wdat 不在或条目数对不上 ⇒ `Err`
/// （调用方当零段，并对该目标整份回放日志）。
pub fn open(dir: &Path, name: &str) -> anyhow::Result<Option<Snapshot>> {
    let meta_path = meta_path(dir, name);
    if !meta_path.is_file() {
        return Ok(None);
    }
    let meta = unsafe { memmap2::Mmap::map(&std::fs::File::open(&meta_path)?)? };
    if meta.len() < META_HEADER || &meta[..4] != META_MAGIC {
        bail!("sidecar 头损坏: {}", meta_path.display());
    }
    let generation = u64::from_le_bytes(meta[20..28].try_into()?);
    let wdat = wdat_path(dir, name, generation);
    if !wdat.is_file() {
        bail!("sidecar 指向的段不在: {}", wdat.display());
    }
    let reader = wind_dict::reader_pool::open_wdat(&wdat)?;
    let max_seq = u64::from_le_bytes(meta[8..16].try_into()?);
    let entry_count = u32::from_le_bytes(meta[16..20].try_into()?);
    let mut first_idx = BTreeMap::new();
    let mut total = 0u32;
    for (code, leaf) in reader.codes() {
        first_idx.insert(code, total);
        total += reader.leaf_entries(leaf).len() as u32;
    }
    if total != entry_count || meta.len() < META_HEADER + entry_count as usize * META_ROW {
        bail!("sidecar 与 wdat 对不上: {} 条 vs {} 条", entry_count, total);
    }
    Ok(Some(Snapshot {
        reader,
        meta,
        max_seq,
        entry_count,
        first_idx,
    }))
}

impl Snapshot {
    /// 第 `entry_idx` 条的 `(count, created_at, last_used, flags)`；越界给零。
    pub fn meta_of(&self, entry_idx: u32) -> (u32, i64, i64, u8) {
        let o = META_HEADER + entry_idx as usize * META_ROW;
        let Some(row) = self.meta.get(o..o + META_ROW) else {
            return (0, 0, 0, 0);
        };
        (
            u32::from_le_bytes(row[0..4].try_into().unwrap()),
            i64::from_le_bytes(row[4..12].try_into().unwrap()),
            i64::from_le_bytes(row[12..20].try_into().unwrap()),
            row[20],
        )
    }

    fn idx_of(&self, code: &str, order: i32) -> u32 {
        self.first_idx.get(code).copied().unwrap_or(0) + order.max(0) as u32
    }

    /// `(text, weight, boundary, entry_idx)`，同码按权重降序（建段时的叶内序）。
    pub fn exact(&self, code: &str) -> Vec<(String, i32, u64, u32)> {
        self.reader
            .search(code)
            .into_iter()
            .map(|e| (e.text, e.weight, e.boundary, self.idx_of(code, e.order)))
            .collect()
    }

    /// `(code, text, weight, boundary, entry_idx)`。
    pub fn prefix(&self, prefix: &str, limit: usize) -> Vec<(String, String, i32, u64, u32)> {
        self.reader
            .search_prefix(prefix, limit)
            .into_iter()
            .map(|e| {
                let idx = self.idx_of(&e.code, e.order);
                (e.code, e.text, e.weight, e.boundary, idx)
            })
            .collect()
    }

    /// 简拼：简拼区给出全拼码，再回主表装配候选；按权重降序截到 `limit`。
    pub fn abbrev(&self, abbrev: &str, limit: usize) -> Vec<(String, String, i32, u64, u32)> {
        let mut out = Vec::new();
        for a in self.reader.search_abbrev(abbrev, 0) {
            let code = a.text;
            for (text, weight, boundary, idx) in self.exact(&code) {
                out.push((code.clone(), text, weight, boundary, idx));
            }
        }
        out.sort_by(|x, y| y.2.cmp(&x.2).then(x.0.cmp(&y.0)));
        if limit > 0 {
            out.truncate(limit);
        }
        out
    }

    /// 全量遍历（DAT 键序 × 叶内序 = 全局序号顺序），供折回合并用。
    pub fn for_each(&self, f: &mut dyn FnMut(SnapshotEntry)) {
        let mut idx = 0u32;
        for (code, leaf) in self.reader.codes() {
            for (text, weight, _order, boundary) in self.reader.leaf_entries(leaf) {
                let (count, created_at, last_used, flags) = self.meta_of(idx);
                f(SnapshotEntry {
                    code: code.clone(),
                    text: text.to_string(),
                    weight,
                    boundary,
                    count,
                    created_at,
                    last_used,
                    flags,
                });
                idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn se(code: &str, text: &str, w: i32, b: u64, count: u32) -> SnapshotEntry {
        SnapshotEntry {
            code: code.into(),
            text: text.into(),
            weight: w,
            boundary: b,
            count,
            created_at: 100,
            last_used: 200,
            flags: 0,
        }
    }

    #[test]
    fn build_open_query_and_meta_roundtrip() {
        let d = std::env::temp_dir().join(format!("wind_snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut es = vec![
            se("nihao", "你好", 10, 0b101, 3),
            se("ni", "你", 50, 0b1, 9),
            se("nihao", "拟好", 5, 0b101, 1),
            se("wubi", "五笔", 7, 0, 2),
        ];
        es.sort_by(|a, b| {
            (a.code.as_str(), a.text.as_str()).cmp(&(b.code.as_str(), b.text.as_str()))
        });
        let s = build(&d, "temp", 42, es.into_iter()).unwrap();
        assert_eq!(s.max_seq, 42);
        assert_eq!(s.entry_count, 4);
        drop(s);
        let s = open(&d, "temp").unwrap().expect("应能打开");
        assert_eq!(s.max_seq, 42);
        let hits = s.exact("nihao");
        assert_eq!(
            hits.iter().map(|h| h.0.as_str()).collect::<Vec<_>>(),
            vec!["你好", "拟好"],
            "同码按权重降序"
        );
        let (count, created, used, _) = s.meta_of(hits[0].3);
        assert_eq!((count, created, used), (3, 100, 200));
        let (count2, _, _, _) = s.meta_of(hits[1].3);
        assert_eq!(count2, 1, "sidecar 与条目一一对齐");
        // 简拼命中的是**候选**：nihao 下两条（你好、拟好）都算
        assert_eq!(s.abbrev("nh", 10).len(), 2, "多音节简拼");
        assert_eq!(s.abbrev("nh", 1).len(), 1, "limit 截断");
        assert_eq!(s.abbrev("\u{1}w", 10).len(), 1, "无边界兜底组");
        assert_eq!(s.prefix("ni", 10).len(), 3);
        let mut seen = Vec::new();
        s.for_each(&mut |e| seen.push((e.code.clone(), e.count)));
        assert_eq!(seen.len(), 4);
        assert!(seen.contains(&("wubi".to_string(), 2)));
        assert!(
            open(&d, "nope").unwrap().is_none(),
            "没有文件 = 零段，不是错误"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 同码异边界：xian 下「先」(x) 与「西安」(x'a) 两条，两个简拼都要召回。
    #[test]
    fn abbrev_keys_cover_every_boundary_under_one_code() {
        let d = std::env::temp_dir().join(format!("wind_snap_ab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let es = vec![
            se("xian", "先", 100, 0b1, 0),
            se("xian", "西安", 50, 0b101, 0),
        ];
        let s = build(&d, "t", 1, es.into_iter()).unwrap();
        assert!(s.abbrev("x", 10).iter().any(|h| h.1 == "先"));
        assert!(
            s.abbrev("xa", 10).iter().any(|h| h.1 == "西安"),
            "低权重条目的边界也要建键"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 崩溃落在「新 wdat 已写、新 sidecar 未写」之间：旧段必须原样可读。
    #[test]
    fn crash_between_wdat_and_meta_keeps_previous_snapshot() {
        let d = std::env::temp_dir().join(format!("wind_snap_half-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let old = vec![
            se("a", "甲", 1, 0, 1),
            se("b", "乙", 1, 0, 1),
            se("c", "丙", 1, 0, 1),
        ];
        drop(build(&d, "t", 3, old.into_iter()).unwrap());
        FAIL_BEFORE_META.with(|f| f.set(true));
        assert!(build(&d, "t", 4, vec![se("d", "丁", 1, 0, 1)].into_iter()).is_err());
        let s = open(&d, "t").unwrap().expect("旧段应仍可打开");
        assert_eq!((s.max_seq, s.entry_count), (3, 3));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn mismatched_sidecar_is_an_error_not_a_silent_zero() {
        let d = std::env::temp_dir().join(format!("wind_snap_bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = build(&d, "t", 1, vec![se("a", "甲", 1, 0, 1)].into_iter()).unwrap();
        drop(s);
        std::fs::write(d.join("t.meta"), b"WUDM").unwrap();
        assert!(open(&d, "t").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

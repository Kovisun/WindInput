//! 文本目标的文件层（设计 §3.5 第 1 步的读、写、复核）。引擎侧的折回 / 同步 / 整文件替换在
//! `engine.rs`（要动视图与 state），这里只管文件：指纹、读、带复核的提交。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

type Hook = Box<dyn Fn() + Send + Sync>;

static BEFORE_RECHECK: Mutex<Option<Hook>> = Mutex::new(None);

/// 测试钩子：提交时「tmp 已写好、复核原文件指纹之前」调用——借此模拟编辑器恰在折回期间保存。
#[doc(hidden)]
pub fn set_before_rename_hook(h: Option<Hook>) {
    *BEFORE_RECHECK.lock().unwrap_or_else(|e| e.into_inner()) = h;
}

/// 内容指纹：`"{len:x}-{crc32:08x}"`；文件不存在为 `"absent"`。
pub fn fingerprint(bytes: Option<&[u8]>) -> String {
    match bytes {
        Some(b) => format!("{:x}-{:08x}", b.len(), crc32fast::hash(b)),
        None => "absent".to_string(),
    }
}

/// 读文件：不存在 ⇒ `Ok(None)`；其他错误（权限、被占用）⇒ `Err`（调用方本轮跳过，不当成删除）。
pub fn read_text(path: &Path) -> anyhow::Result<Option<(Vec<u8>, i64)>> {
    match std::fs::read(path) {
        Ok(b) => {
            let mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            Ok(Some((b, mtime)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("读不了 {}：{e}", path.display())),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// §3.5 第 1 步的写回：tmp → fsync → （`expect_fp` 给出时）复核原文件指纹未变 → 原文件存在则留
/// `.bak` → rename → fsync 目录（Windows 对目标再 `sync_all`）。复核不过 ⇒ 删 tmp、`Err`。
pub fn commit_text(path: &Path, bytes: &[u8], expect_fp: Option<&str>) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无父目录: {}", path.display()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = with_suffix(path, ".tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
    }
    if let Some(h) = BEFORE_RECHECK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        h();
    }
    if let Some(fp) = expect_fp {
        let now = read_text(path)?;
        if fingerprint(now.as_ref().map(|(b, _)| b.as_slice())) != fp {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("{} 在折回期间被修改，本轮放弃", path.display());
        }
    }
    if path.is_file() {
        std::fs::copy(path, with_suffix(path, ".bak"))?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::File::open(path).and_then(|f| f.sync_all());
    }
    Ok(())
}

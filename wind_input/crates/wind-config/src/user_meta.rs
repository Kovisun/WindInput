//! `<userdata>/meta.toml`：用户目录内数据格式的版本与目录 id（设计 §2.4、§7.1）。
//!
//! **降级保护必须先于任何新格式发布**：新版本写出新格式后用户回退，旧版本若照常在
//! 旁边继续写 redb，两边就分叉。读到比自己认识的更高的版本号 ⇒ 只读模式，一个字节都不写。

use std::path::Path;

/// 本版本认识的格式版本。1 = 纯 redb。每个 P 阶段迁移完成 +1。
pub const KNOWN_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserMeta {
    pub format_version: u32,
    /// 随数据目录生成的随机 id（32 位十六进制）。随目录漫游 / 复制；与本机 `device.toml`
    /// 里记的 `dir_id` 对不上就说明目录被复制或换了目录（设计 §3.7）。
    pub dir_id: String,
}

#[derive(Debug)]
pub enum MetaCheck {
    Ok(UserMeta),
    /// 数据由更新版本写入：只读模式，`found` 给提示用。
    Incompatible {
        found: u32,
        known: u32,
    },
    /// 文件在但读不出 / 解析不出：不覆盖（那可能是更新版本的格式），按只读处理。
    Unreadable(String),
}

pub fn load_or_init(user_dir: &Path) -> MetaCheck {
    let path = user_dir.join("meta.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<UserMeta>(&text) {
            Ok(m) if m.format_version > KNOWN_FORMAT_VERSION => MetaCheck::Incompatible {
                found: m.format_version,
                known: KNOWN_FORMAT_VERSION,
            },
            Ok(m) => MetaCheck::Ok(m),
            Err(e) => MetaCheck::Unreadable(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let m = UserMeta {
                format_version: KNOWN_FORMAT_VERSION,
                dir_id: uuid::Uuid::new_v4().simple().to_string(),
            };
            let _ = std::fs::create_dir_all(user_dir);
            match toml::to_string(&m) {
                Ok(s) => {
                    let tmp = user_dir.join("meta.toml.tmp");
                    if std::fs::write(&tmp, s).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                        MetaCheck::Ok(m)
                    } else {
                        // 写不进用户目录：不阻断启动，但没有稳定的 dir_id，按只读对待更稳
                        MetaCheck::Unreadable("meta.toml 写入失败".into())
                    }
                }
                Err(e) => MetaCheck::Unreadable(e.to_string()),
            }
        }
        Err(e) => MetaCheck::Unreadable(e.to_string()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wind_meta_{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_meta_is_initialised_with_known_version_and_random_dir_id() {
        let d = dir("init");
        let MetaCheck::Ok(m) = load_or_init(&d) else {
            panic!("应初始化")
        };
        assert_eq!(m.format_version, KNOWN_FORMAT_VERSION);
        assert_eq!(m.dir_id.len(), 32);
        let MetaCheck::Ok(again) = load_or_init(&d) else {
            panic!()
        };
        assert_eq!(again.dir_id, m.dir_id, "第二次读到同一个 dir_id");
    }

    #[test]
    fn newer_version_is_incompatible_and_file_is_left_untouched() {
        let d = dir("newer");
        let text = "format_version = 99\ndir_id = \"abc\"\n";
        std::fs::write(d.join("meta.toml"), text).unwrap();
        assert!(matches!(
            load_or_init(&d),
            MetaCheck::Incompatible {
                found: 99,
                known: 1
            }
        ));
        assert_eq!(
            std::fs::read_to_string(d.join("meta.toml")).unwrap(),
            text,
            "只读：一字不改"
        );
    }

    #[test]
    fn unparsable_meta_is_reported_not_overwritten() {
        let d = dir("bad");
        std::fs::write(d.join("meta.toml"), "not toml ==").unwrap();
        assert!(matches!(load_or_init(&d), MetaCheck::Unreadable(_)));
        assert_eq!(
            std::fs::read_to_string(d.join("meta.toml")).unwrap(),
            "not toml =="
        );
    }
}

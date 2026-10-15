//! wind-store: 基于 redb 的持久化存储
//!
//! 与 Go 版本 `wind_input/internal/store/` 对齐。
//! 使用 redb 替代 bbolt，保持相同的 bucket 语义。

pub mod abbrev_index;
pub mod assoc_history;
pub mod charsets;
pub mod common_chars;
pub mod completion;
pub mod dict_export;
pub mod draft_words;
pub mod freq;
pub mod import_formats;
mod last_good;
mod learned_text;
pub mod legacy_retire;
mod machine_file;
pub mod migrate_p1;
pub mod migrate_p2;
pub mod migrate_p3;
pub mod migrate_p4;
pub mod migration;
pub mod phrase_text;
pub mod phrases;
pub mod quick_format;
pub mod shadow;
pub mod stat_collector;
pub mod stats;
pub mod store;
pub mod store_layer;
pub mod temp_words;
pub mod user_words;
pub use wind_userdata::issues;
pub use wind_wdict as wdict;

pub use store::Store;

/// 用户目录下装用户数据文件的子目录名（设计 §2.1，P5 起；此前叫 `user`）。
pub const DATA_DIR_NAME: &str = "data";
pub use store_layer::{StoreDraftLayer, StoreTempLayer, StoreUserLayer};

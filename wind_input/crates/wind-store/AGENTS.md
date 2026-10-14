<!-- Parent: ../../AGENTS.md -->
<!-- Updated: 2026-06-29 -->

# wind-store

## Purpose

用户数据门面。Shadow 规则（`user/<schema>/shadow.dict.yaml`）与全局用户短语（`user/phrases.dict.yaml`）自 P3 起是文本小表（`wind_userdata::text::table::TextTable`，内存持有、按组补丁）；**临时词、草稿、词频、联想历史自 P1b 起委托给 `wind-userdata` 的 `Runtime`，用户词自 P2b 起真源是 `user/<schema>/user.dict.yaml`、同样委托 `Runtime`**（门面方法签名不变，调用方与测试零改动；设计 `docs/design/user-data-storage-redesign.md` §6、计划 `user-data-storage-plan-p1b.md` / `-p2b.md`）。三个查询层 `StoreUserLayer` / `StoreTempLayer` / `StoreDraftLayer` 也在本 crate，都是引擎层的薄包装。

## Key Files

| File | Description |
|------|-------------|
| `src/store.rs` | `Store` 核心：redb `Database` 包装；7 张扁平 table 定义；`with_db` 闭包是所有读写的唯一安全入口；`pause`/`resume` 释放并重开文件锁（Windows 热替换）；版本迁移框架 |
| `src/user_words.rs` | 用户词：门面委托 `Runtime`（文本 + 引擎）；`count` / `created_at` 恒 0；`sync_user_text(s)` / `reload_user_text_paths`（`dict.reload`）/ `flush_user_data`（`dict.flush`）；旧表编解码只剩迁移用 |
| `src/temp_words.rs` | 临时学习词：门面委托 `Runtime`；`promote_temp_word` 先写用户词再删临时词 |
| `src/abbrev_index.rs` | 用户词 / 临时词简拼：包引擎层 `search_abbrev`（redb 倒排表 P2 起退役） |
| `src/draft_words.rs` / `src/assoc_history.rs` | 草稿、联想历史：门面委托 `Runtime`；`dec_draft` 只剩迁移读旧表用 |
| `src/store_layer.rs` | 三个 `DictLayer`：包 `UserDataUserLayer` / `UserDataTempLayer` / `UserDataDraftLayer` |
| `src/migrate_p1.rs` | 首次打开时把旧表 temp_words / draft_words / freq / assoc_history 合并进引擎，完成写 `state/migrated-p1`；`legacy_counts` 供 `examples/p1_migrate_check.rs` 比对 |
| `src/migrate_p2.rs` | 首次打开时把旧表 user_words 迁成文本（文件不在整写、在则逐条合并），完成写 `state/migrated-p2`；`examples/p2_migrate_check.rs` 逐码比对 |
| `src/phrases.rs` | 用户短语（**全局，不分方案**）：叠加模型——系统短语只在内存（`set_system_phrases`，协调器每次启动交进来），文件只记用户行与 `overrides_system=1` 的修改行；`set_phrase_enabled` 软删除 |
| `src/shadow.rs` | `ShadowRecord`（pinned/deleted）规则落 `user/<schema>/shadow.dict.yaml`：同编码一组行，pin 在前（newest first）、delete 在后，`position` 是槽位；`get_shadow_rules` 只读内存（按键路径零 IO）；`cand_id` 精准去重（动态短语 R2） |
| `src/migrate_p3.rs` | Shadow 打开时迁（标记 `migrated-p3-shadow`）；短语在首次 `set_system_phrases` 时迁（要 TOML 区分系统行改没改过，标记 `migrated-p3-phrases`）；`format_version_reached` 汇总 1–4 |
| `src/freq.rs` | `FreqRecord`；门面委托 `Runtime`；`FreqProfile` 拼音衰减分；legacy `FreqTracker`（过渡期文件式，待移除）；`dec_freq` 只剩迁移读旧表用 |
| `src/migration.rs` | 版本迁移框架；当前 `CURRENT_VERSION=1`，全新库直接打版本号 |
| `src/stat_collector.rs` / `src/stats.rs` | 每日打字统计，写 `STATS_DAILY` table |

## For AI Agents

### Working In This Directory

- **临时词 / 草稿 / 词频 / 联想历史不在 redb 里了**（P1b）：这四组门面方法委托 `self.runtime()`，**不经 `with_db`**——按键路径不再碰 redb Mutex（`temp_hot_path_does_not_touch_redb` / `draft_freq_assoc_hot_paths_do_not_touch_redb` 用 `pause()` 守着）。旧表 `TEMP_WORDS` / `TEMP_ABBREV` / `DRAFT_WORDS` / `FREQ` / `ASSOC_HIST` 只读保留到 P4，**不要再往里写**；`words_generation` 系列代次仍由门面推进，时机与旧实现逐条一致。
- **状态目录**：`Store::open(path)` 把引擎状态放在 `<path>.state/`，redb 文件此前不存在时先清空它（测试反复用同一路径不串数据）；生产走 `Store::open_with_state(path, StateLoc)`（协调器传 `Config::state_dir()/runtime/<dir_id>`——按用户目录分开，换目录不串数据），从不清空。迁移**合并**而非替换：重做时引擎已有条目为准、count 取大，上一轮失败后的写入不丢；`migrated_p1()` 为真协调器才升 `format_version`。
- **迁移以标记为准**：`state/migrated-p1` / `migrated-p2` 在就不迁；不在就整体重做（合并，重跑不翻倍）。P1 完成升 `format_version` 到 2，P1 + P2 都完成升到 3，旧版本随之只读。
- **用户词文本位置**：`StateLoc.text_dir`（生产 `<userdata>/user`，派生模式 `<redb>.user/`，随新库清空）。`Store` 打开时把「目录下有文本的 ∪ 引擎里有用户词的」方案都 `bind_text`；其余在首次访问时 `ensure_user_text` 按需绑定（文本隔离 / 读不了只 WARN，绑定照样登记）。`lowercase` 恒 false，`syllable_codes` = 方案为 `pinyin`。改动停手 30 秒由后台节拍（`spawn_userdata_ticker`，协调器起）写回；手改文件在切方案 / 配置重载 / `dict.reload` 时 `sync`，变了推进代次。
- **redb 只剩覆盖表 / 小文件 / META**（P4 处理）：页缓存回收那套测试（`drop_page_cache.rs` 等）用补全历史表 `COMPLETION` 做负载。
- **小表写入是同步的整文件提交**（几毫秒）：短语 / Shadow 都是用户逐条的显式动作；批量（导入、还原、迁移）在内存里攒好再**一次**提交，别逐条调 `pin_shadow` / `add_phrase`。手改文件经 `sync_user_texts`（返回值含 `"phrases"` 时协调器重建短语层）生效。

- **所有读写必须经 `Store::with_db`**：不要直接访问 `Store.db` 字段。`with_db` 持 `Mutex` 锁并检查暂停态——`pause()` 后 `with_db` 返回 `"store is paused"` 错误，绕过它会破坏 pause/resume 语义或导致 panic。

- **redb 的读缓存是只涨不落的高水位，全额计进进程私有内存**：redb 2.x 不是 mmap，页是堆上的 `Arc<[u8]>`；每次读未命中就插入，只有越过上限才淘汰，唯一的整体清空是库文件扩容，刷盘时脏页还会被直接晋升进读缓存。上界 ≈ `min(读缓存配额, 库文件大小)`——实测 19 万词的库全表扫后常驻 41 MB 不还，50 万词约 90 MB，这是「导入大词库后内存高」的正主（`tests/redb_cache_high_water.rs`）。回收分两种：**批量写**（导入、清空、备份还原、删方案）成功后立即调 `Store::drop_page_cache`；**全表扫描**（设置页列表 / 导出 / 统计、后台建 `UserTextIndex` / `UserAssocIndex`）做完只标记（`mark_scan_pending`，建索引走带 1 万行门槛的 `mark_scan_pending_rows`），由回收线程在库空闲 3 秒后回收并整理堆——打字中自动造词反复触发重建，当场回收会让按键线程陪等关库重开；连续 60 秒没人碰库则无条件兜底回收一次。`drop_page_cache` **绝不能进按键链路**，它要关库重开 `Database`（关库刷盘，本机约 6 ms）。
- **要条数就用 `count_*`，要一页就用 `for_each_*`，别 `list(..).len()` 或先全收再 `skip/take`**：19 万条的库翻第一页会先造 38 万个 `String`。`for_each_user_word` 的回调收 `UserWordView`（`code`/`text` 直接借 redb 的页），只对真正要留的那几条调 `to_record()`。⚠️ 新增计数/遍历 API 时，**口径必须与对应的 `search_*` 逐条一致**（含「解不出的坏记录算不算」），否则症状是「说有 N 条、翻到最后一页只有 N-1 条」，而且只在库里真有坏记录时才出现——守门在 `tests/counts_match_listings.rs`，它不比具体数字，只比两条路彼此相等。
- **这两件事是分开的**：流式遍历省的是 Rust 侧的物化峰值，**不省 redb 读缓存**（range 迭代照样把走过的叶子页读进缓存，key 与 value 同页）。只改其中一样就以为内存问题解决了，是这条最容易踩的坑。
- **复合 key 编码是范围查询的基础，不可改**：用户词/临时词/词频统一用 `schema\0code\0text` 三段；Shadow 用 `schema\0code` 两段；短语用 `code\0text`（全局，无 schema 前缀）。前缀范围查用 `t.range(scan..)` + 前缀 break，依赖此编码结构——改分隔符或字段顺序会静默读到跨方案数据。

- **短语软删除走 `set_phrase_enabled`，不走 Shadow delete**：短语候选的屏蔽语义是"用户在设置 UI 可恢复的禁用"，存 `PhraseRecord.enabled=false`；`Shadow.delete` 只用于系统词库/用户词候选。两条路径不混用，否则设置 UI 的启用开关无法正确反映状态（见 `shadow.rs` 顶部注释）。

- **Shadow 有两套实现，新代码用 redb 路径**：`ShadowStore`（内存+JSON 文件）是 coordinator 过渡期残留；`Store::pin_shadow` / `delete_shadow` / `remove_shadow_rule` 是 redb 正式实现。两者共用 `ShadowRecord.apply_*` 方法保持逻辑一致，但持久化后端不同。

- **`FreqTracker` 是 legacy，不要在新代码中引用**：`freq.rs` 内注释已标明"coordinator 接通 redb 词频后移除"。新的频率记录走 `Store::record_freq` / `get_freq`（redb，按方案隔离）。

### Testing Requirements

- `wind-store` 无 `windows` crate 依赖，可在任意 host 直接运行：`cargo test -p wind-store`。
- 各测试在 `std::env::temp_dir()` 创建带唯一名的 `.redb` 临时文件，测后删除；并发运行时文件名已区分，不会互相干扰。

## Dependencies

### Internal

- `wind-userdata` — 运行时引擎（四组数据的后端）
- `wind-dict` / `wind-candidate` — `DictLayer` 与候选类型（`store_layer.rs`）
- `wind-wdict` — 交换格式（`pub use wind_wdict as wdict` 保持原路径）
- `wind-cmdbar` / `wind-utils`

### External

- `redb` — 嵌入式 KV 数据库（替代 Go 版的 bbolt）
- `serde` / `serde_json` — Shadow 规则与短语的 JSON 序列化
- `uuid` — 设备 ID 生成（Meta 表）
- `tokio` — 异步运行时（stat 相关异步路径）
- `chrono` — 日期处理（每日统计 key）
- `anyhow` / `thiserror` / `tracing` — 错误处理与日志

## 全局约束

提交前跑 `cargo fmt`；日志 INFO 级不得含用户输入/词库内容（见根 `AGENTS.md`）。

<!-- MANUAL: 此行以下为人工补充区，重新生成时保留 -->

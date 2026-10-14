<!-- Parent: ../../AGENTS.md -->
<!-- Updated: 2026-10-14 -->

# wind-userdata

## Purpose

用户数据引擎（设计：[docs/design/user-data-storage-redesign.md](../../../docs/design/user-data-storage-redesign.md) §3）：
「快照 + 日志 + 增量表」三种物理形态各管一类变化速度。P0 只交付引擎与 `DictLayer` 适配，
**尚未接进 `wind-engine`**（P1 把临时词 / 草稿 / 计数迁进来；P2 文本层在同一台引擎上多一步
「按行补丁写回」）。

## Key Files

| File | Description |
|------|-------------|
| `src/record.rs` | 日志记录帧：`len u32 \| crc32 u32 \| body`，body 含 seq / ts / kind / target / **boundary** / schema / code / text / payload。decode 对半帧与篡改一律 `Err`，绝不 panic |
| `src/journal.rs` | 按设备的追加日志 `.wjl`：一条一次 `write(2)`，无用户态缓冲；open 时截掉坏尾巴、从最后一条完整记录续号；超长字段（>u16）写前拒；裁剪是方法，重写 + rename 后把自己的 fd 换到新文件，**永不 mmap** |
| `src/state.rs` | 折回水位 `state`（toml）与真源级原子写 `atomic_write`（tmp → fsync → rename → fsync 目录）。state 读不出 = 「水位未知」，由快照内嵌 `max_seq` 兜底，绝不整份回放 |
| `src/device.rs` | `device.toml`（device_id / 机器指纹 / dir_id）：任一不符就换号并把旧日志封存为 `.wjl.sealed`；`state/lock` 用 `File::try_lock` 做单写者，拿不到即只读（只读实例走 `load_existing`，不换号不写盘） |
| `src/delta.rs` | 两级增量表：冻结层 `Arc<[Entry]>` 有序 + 尾巴 ≤128 条；同键多操作按 seq 各自保留；声母分组索引（规则同 `wind-store::abbrev_index::group_of`，含无边界兜底组 `\u{1}` + 首字符） |
| `src/snapshot.rs` | 快照段：代号命名的 `<name>-<gen>.wdat`（`WdatStreamWriter`，简拼区按条目建键、含兜底组）+ 定长 sidecar `<name>.meta`（count / created_at / last_used / flags，按 wdat 全局条目序号对齐；头内嵌 `max_seq` 与 gen）。**sidecar 是提交点** |
| `src/engine.rs` | 门面 `UserData`：`open`（锁、设备、state、枚举快照、回放日志）、`apply`（追加 + 发布，写后立即可读）、`fold`（旧快照 + delta → 新快照 → 换入 → 摘 delta → state）、`query_exact` / `query_count` |
| `src/layer.rs` | `UserDataTempLayer: DictLayer`：快照命中 + delta 叠加成候选；前缀多取 delta 覆盖条数；简拼返回声母组 + 兜底组的超集 |
| `tests/engine_crash_paths.rs` | 引擎级：写后可读 / 重开回放、折回后不再回放、state 缺失按快照水位、折回期间新到的操作保留、第二实例只读且不写盘、删 journal/ 不倒号、快照坏整份回放、换号后封存日志仍可见且续号。折回半写崩溃在 `engine.rs` 单测（需 `snapshot::FAIL_BEFORE_META` 故障注入） |

## For AI Agents

### Working In This Directory

- **读路径无锁**：按键线程 `UserData::view()` 克隆一个 `Arc<View>` 后不再碰锁。消费方只借用，不把 `Arc<View>` 或其中的 reader 存进长寿结构——否则折回后的旧段释放不掉。
- **写路径一把写锁**：`apply` 持它跨「追加 + 发布」，`fold` 持它跨「取水位 + 取起点」与「换入」。这保证折回起点视图含全部 `seq ≤ 水位` 的操作；折回只摘 `seq ≤ 水位` 的该 (target, schema) 条目，期间新到的留下。改这两处的持锁范围前先看 `fold_keeps_ops_that_arrived_during_fold`。
- **回放水位取两者较大**：快照内嵌 `max_seq`、state 里的 `folded_seq`；快照打不开（不是缺失）的目标水位不可信，整份回放。`next_seq` 取 state、各快照 `max_seq`+1、各 `folded_seq`+1、日志尾（含封存日志）四者最大（`Journal::bump_next_seq`）——删掉 `journal/` 或换号后序号都不会倒退。
- **封存日志照常回放**：换号时旧日志改名 `.wjl.sealed`，open 把它们与当前日志按 seq 合并回放，未折回的记录仍可见；新日志接着旧号编。
- **换段不覆盖**：快照每次折回落一个新代号文件，sidecar 原子写完才算换入；崩溃在两步之间旧段原样可读。别改回「同名覆盖 wdat」——那会让 `reader_pool` 按 (大小, mtime) 复用旧 reader，也会让半写状态把整个目标清零。
- **只读实例零写入**：拿不到锁就不开日志写端、不换号、不截尾（尾巴可能是写者正写的那条）。
- **记录带 boundary**：拍平的码推不回音节边界，而 delta 的简拼分组索引要它。写入端（P1 的协调器）必须传真实位图；位图是**字节**位置，`nihaoma` = `0b100101`（0、2、5），不是 `0b10101`——计划阶段的夹具就错过一次。
- **空快照不落盘**：`snapshot::build` 遇零条目 bail，`fold` 据此 `snapshot::remove`（先删 sidecar 再删代号文件）；`open` 对「sidecar 不存在」给 `None`（零段），对「头坏 / 指向的 wdat 不在 / 条目数对不上」给 `Err`（当零段并整份回放）。
- 日志记录格式 v1 **尚未发布**，改字段直接改编码即可；一旦随版本发出去就要加版本分支。
- 目标键 `"{target}:{schema}"` 既是 state 的 key，也是快照目录 `state/<schema>/<target>.{wdat,meta}`；`schema` 是**数据方案 id**（拼音族折叠为 `pinyin`）。

### Testing Requirements

- `cargo test -p wind-userdata`（Linux 原生可跑；`File::try_lock` 需工具链 ≥ 1.89）。
- 测试目录名带 pid；用 `TMPDIR` 隔离并发会话。`fold_keeps_ops_that_arrived_during_fold` 是竞态用例，改写锁后连跑 10 次。

## Dependencies

### Internal

- `wind-dict` — `WdatStreamWriter` / `WdatReader` / `reader_pool`、`DictLayer` / `LayerType`
- `wind-candidate` — `Candidate`、排序 `better`

### External

- `crc32fast` — 记录帧校验
- `memmap2` — sidecar 映射（快照 wdat 经 `reader_pool`）
- `toml` / `serde` — state、device.toml
- `uuid` — device_id
- `anyhow` / `tracing`

## 全局约束

提交前跑 `cargo fmt -p wind-userdata`；日志 INFO 级不得含词条内容（见根 `AGENTS.md`）。

<!-- MANUAL: 此行以下为人工补充区，重新生成时保留 -->

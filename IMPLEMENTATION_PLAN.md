# C2-98 词库统计 实施计划

## Stage 1: wind-dict 同码分组枚举 + stats 纯函数
**Goal**: `cached::for_each_code_group`（多库 k 路归并、保留 order/boundary）；`stats.rs` 计算 P1–P3 全部口径。
**Success Criteria**: 小夹具钉精确数值：分档/去重/其他、码长分布、简码分级、重码组/涉及/最大/Top、单字全码重码、分字集未收/静重/加权选重率/平均码长、拼音多音字/音节/零权重。
**Tests**: `cargo test -p wind-dict stats`、`for_each_code_group` 跨层归并与 Memory/Mmap 一致。
**Status**: Complete

## Stage 2: wind-engine `EngineManager::dict_stats`
**Goal**: 复用 `enabled_dict_specs` / `split_codetable_dicts` 的选库口径逐库加载；字频（白霜 8105+41448）、通规字集；缓存键 = 各层 digest + 字频源摘要 + 算法版本。
**Success Criteria**: 夹具方案出精确数；缓存命中/失效；真实五笔86/白霜冒烟与原型对照（有 build_dev/data 时）。
**Tests**: `cargo test -p wind-engine dict_stats`。
**Status**: Not Started

## Stage 3: RPC `dict.systemStats`
**Goal**: `wind-coordinator/src/dict_stats_api.rs`（混输并排两成员、用户词/临时词实时计数），webdata 分派转发。
**Success Criteria**: RPC 回包形状测试；分派表测试；不入 FULL_SCAN_METHODS。
**Tests**: `cargo test -p wind-coordinator -p wind-webdata`。
**Status**: Not Started

## Stage 4: 设置端对话框
**Goal**: 中栏末行「词库统计」+ 左栏右键菜单 → 同一对话框；基础区 + 可展开高级区 + 重码 Top；加载中/失败态；mock。
**Success Criteria**: UI 测试覆盖入口、加载、失败、混输并排；全量测试绿。
**Tests**: wind-setting `cargo test`。
**Status**: Not Started

## Stage 5: 文档与收尾
**Goal**: 文档站补口径说明；删本文件。
**Status**: Not Started

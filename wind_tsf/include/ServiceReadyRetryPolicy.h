// 收到 SERVICE_READY 后补发 IME_ACTIVATED 失败时的**重试次数与间隔**纯判据。
//
// 从 `CLangBarItemButton::_MsgWndProc` 的 WM_SERVICE_READY / TIMER_ID_SERVICE_READY 分支
// 抽出来只为可测（不含 Win32 头，g++ 可跑，见 tests/service_ready_retry_policy_test.cpp）。
//
// # 为什么要重试
//
// 服务就绪时给所有已连上 push 管道的客户端一起补发 SERVICE_READY，每个焦点非空的 TSF 线程
// （含后台宿主）同时去连主管道；服务端一次只挂一个监听实例，宿主一多，前台那个常拿到
// ERROR_PIPE_BUSY，IME_ACTIVATED 发不出去。SERVICE_READY 只来一次，服务端又只在收到
// IME_ACTIVATED 时显示工具栏——不重试的话工具栏要等到用户按键才出来。
//
// # 为什么带抖动
//
// N 个宿主同一时刻失败，固定间隔重试会再次同时撞上；按线程 id 与次数散开 0–200ms。
#pragma once

#include <cstdint>

namespace wind
{
namespace svcready
{

/// 失败后最多重试几次（不含首次）。
constexpr uint32_t kMaxRetries = 10;
constexpr uint32_t kBaseDelayMs = 300;
constexpr uint32_t kJitterMs = 200;

/// 已失败 `failures` 次（含本次）后是否还要再试。
inline bool ShouldRetry(uint32_t failures)
{
    return failures >= 1 && failures <= kMaxRetries;
}

/// 第 `failures` 次失败后的重试间隔，落在 [kBaseDelayMs, kBaseDelayMs + kJitterMs]。
/// `seed` 取线程 id：同一线程每次不同，不同线程彼此错开。
inline uint32_t RetryDelayMs(uint32_t seed, uint32_t failures)
{
    uint32_t h = seed * 2654435761u ^ failures * 40503u;
    h ^= h >> 15;
    h *= 2246822519u;
    h ^= h >> 13;
    return kBaseDelayMs + h % (kJitterMs + 1);
}

} // namespace svcready
} // namespace wind

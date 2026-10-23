// CtrlAltCleanupPolicy 判据测试（会话中未登记的 Ctrl/Alt 组合处理完后是否交还宿主）
//
// 跑法（纯 C++17、不含 Win32 头）：
//   g++ -std=c++17 -Wall -Wextra -I../include -o ctrl_alt_cleanup_test ctrl_alt_cleanup_policy_test.cpp && ./ctrl_alt_cleanup_test
//
// 变异检验：去掉 committedByResponse 项 ⇒ TestCommitKeepsKey 红；去掉 isComposing 项
// ⇒ TestCompositionKeepsKey 红。

#include "CtrlAltCleanupPolicy.h"

#include <cstdio>

namespace
{
using wind::ctrlalt::ShouldReturnToHost;

int g_failures = 0;

void Expect(bool cond, const char* what)
{
    if (!cond)
    {
        std::printf("FAIL: %s\n", what);
        ++g_failures;
    }
}

// Ctrl+S / Ctrl+C：服务端只清了组合 ⇒ 键归宿主。
void TestCleanupReturnsKey()
{
    Expect(ShouldReturnToHost(false, false, false), "只清组合时应交还宿主");
}

// 候选操作 / 快捷输入里无候选时按选词键：组合仍在 ⇒ 不交还。
void TestCompositionKeepsKey()
{
    Expect(!ShouldReturnToHost(false, true, false), "组合仍在时不交还");
    Expect(!ShouldReturnToHost(true, false, false), "候选仍在时不交还");
}

// 快捷输入选词键整段上屏：组合已收掉，但键是被服务端用掉的 ⇒ 不交还。
void TestCommitKeepsKey()
{
    Expect(!ShouldReturnToHost(false, false, true), "上屏了候选时不交还");
}

} // namespace

int main()
{
    TestCleanupReturnsKey();
    TestCompositionKeepsKey();
    TestCommitKeepsKey();

    if (g_failures == 0)
    {
        std::printf("ctrl_alt_cleanup_policy_test: all passed\n");
        return 0;
    }
    std::printf("ctrl_alt_cleanup_policy_test: %d failure(s)\n", g_failures);
    return 1;
}

# macOS 提示类浮层与菜单改为服务端自绘

状态：实施中（分支 `feat/mac-overlay-raster`）。目标：macOS 与 Linux 同构——状态气泡 / Toast /
悬停提示 / 候选右键菜单与主菜单，都在服务进程按主题光栅化，`.app` 只贴图、落位、计时、转发鼠标。

## 为什么

候选窗与软键盘早已是服务端自绘，唯独这四类在 `.app` 里用 AppKit 原生控件排字（`TooltipPanel` /
`StatusBubblePanel` / `ToastPanel` / `NSMenu`），服务端只发文本与 hex 颜色。结果是主题的背景图、
渐变、图层、软阴影、边框、字号偏移、悬停提示右键菜单等能力在 macOS 上拿不到，每加一个主题字段
就要在 Swift 侧再抄一遍（`[tooltip] font_size` 就是这样漏掉的）。

## 做法

复用 Linux 已有的整条链路，不新造协议：

- 渲染：`overlay_linux.rs` / `menu_linux.rs` 改名为 `overlay_host.rs` / `menu_host.rs`，条件从
  「linux + ext_presenter」放宽为 `ext_presenter`（macOS 恒真）。`Forwarder` 里 macOS 与 Linux 的
  并行分支合一。
- 协议：`CMD_OVERLAY_FRAME`（kind = tooltip / status / toast / menu 各级）+ 各层独立 SHM 段
  （`overlay_shm_name`，macOS 名长不超过 31 字节上限）。
- 落位：与 Linux 同构，由宿主（`.app`）按屏幕几何落位、计时；服务端只给首选点 / 备选点 / 锚点。
- 交互：对齐 `wind_linux/include/OverlayInput.h` 的语义（见 `linux-port.md` §5b）。
- 菜单：复用 `popup_menu` 整套，指针事件由 `.app` 转 `CMD_MENU_POINTER`。

## 保留原生的部分

菜单栏状态项（`NSStatusItem`）与系统输入源菜单（`InputController.menu`，由 IMK 在别的进程里绘制）
无法自绘，继续用 `UnifiedMenuBuilder` + 精简菜单树。

## 分期

| 期 | 内容 |
|---|---|
| P0 | 放宽 cfg、改名；CoreText 后端补字重；Forwarder 合一 |
| P1 | `.app` 通用 `OverlayPresenter`（tooltip / status / toast），删三个原生 Panel 与旧 `CMD_*_SHOW` |
| P2 | 鼠标交互：悬停保持与宽限、tooltip / 状态气泡右键菜单、气泡拖动、Toast 点击关闭 |
| P3 | 菜单自绘：指针捕获、键盘转发、防卡死自愈 |

## 验证条件

本机（Linux）没有 Swift 工具链：Rust 侧靠 `linux-host` 形态的单测 + `aarch64-apple-darwin` 交叉
check；Swift 侧只能靠 CI（`swift test`）与真机。Retina / 多屏 / 全屏 App 的 Space / Stage Manager
必须真机看一遍。

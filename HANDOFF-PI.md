# netspeed — 交接文档（给 Pi）

> 原维护者（Hermes）不再负责本项目。你（Pi）接手后自主排查与修复，遇到需要用户决策的点再报告。
>
> **重要**：`COMPARE-TRAFFICMONITOR.md` 是对照开源项目 TrafficMonitor 的对比分析，包含修复右键菜单问题的关键结论（窗口嵌入任务栏、背景避免近黑、菜单调用简化），优先阅读并按其中优先级执行。

## 项目概况

- **源码**：`D:\dev\projects\netspeed`（Rust，Win32 GDI 手绘，无框架）
- **可执行文件**：`C:\Users\Administrator\netspeed.exe`（单文件，release 编译后拷贝）
- **构建**：`cd /d/dev/projects/netspeed && cargo build --release && cp target/release/netspeed.exe C:/Users/Administrator/netspeed.exe`
- **git**：已有提交 `b2db6e1`（修复右键菜单闪黑框）与 `d62ffb7`（初始）；工作区有**未提交改动**（调试日志 + WM_CONTEXTMENU/WM_RBUTTONDOWN 处理 + WM_ERASEBKGND 填背景）
- **调试日志**：程序会写 `C:\Users\Administrator\netspeed-debug.log`（当前版本的调试埋点，修复后应移除）

## 功能目标（用户要求）

1. 任务栏上直接显示：下行下载网速（绿箭头）、上行上传网速（橙箭头），右侧 CPU/内存
2. 窗口：160×42 逻辑像素（150% DPI 下物理 107×28），无边框，`WS_EX_TOOLWINDOW | WS_EX_TOPMOST`，背景采样任务栏颜色（不透明 GDI + ClearType，保文字锐利）
3. 右键菜单：**开机自启（带勾选）/ 退出**，菜单要能弹出、显示在最顶层、点击外部自动关闭、不闪黑框
4. 开机自启默认开启（注册表 Run 项 NetSpeed）
5. 单实例；窗口位置：任务栏通知区左侧

## 当前核心问题（未解决）

**真实物理右键点击 → 菜单不弹出。** 已确认的事实：

- 真实右键点击时窗口**确实收到了 `WM_RBUTTONUP`**（debug log 出现 `RBUTTONUP`），且 `show_context_menu()` 被调用
- 但 debug log **没有出现 `menu_ret=` 行** → 说明 `TrackPopupMenu` 没有返回，模态循环可能在运行，但菜单窗口不可见/不可枚举（`EnumWindows` 找不到可见 `#32768`）
- `GetLastActivePopup(hwnd) == hwnd` → 主窗口没有活动弹出窗口
- **关键异常**：`WindowFromPoint(窗口中心)` 返回 `0x0`！窗口区域下竟然没有窗口——窗口可能被任务栏遮挡，或命中测试被禁
- 窗口 rect：`(2221,1402)-(2328,1430)`（物理像素），任务栏 rect `(0,1392)-(2560,1440)`，托盘区 `(2332,1392)-(2560,1440)`。窗口在任务栏内、托盘左侧，位置正确
- 截图分析：窗口区域有内容（std≈15 vs 背景 std≈1.7），说明窗口**确实渲染了文字**，肉眼可见
- exstyle=`0x88`（TOOLWINDOW|TOPMOST，无 NOACTIVATE），style=`0x94000000`
- `PostMessage(WM_RBUTTONUP)` 在**旧版本**曾成功弹出 `#32768` 菜单（后来改动后未再复测成功）

## 已尝试但放弃/未生效的方案（不要再重复）

1. **自绘菜单窗口**（`WS_EX_NOACTIVATE` 独立 popup，定时器检测外部点击）：能显示，但**创建瞬间闪黑框**，且跨窗口捕获不可靠 → 已弃用
2. **隐藏 owner + TrackPopupMenu**（owner 用 `WS_EX_TOOLWINDOW` 不显示）：菜单不显示
3. **当前方案**：主窗口不加 `WS_EX_NOACTIVATE`，`SetForegroundWindow(hwnd)` + `PostMessage(WM_NULL)` 后直接 `TrackPopupMenu(menu, TPM_RIGHTBUTTON|TPM_RETURNCMD|TPM_NOANIMATION, ..., hwnd)` → 真实点击仍不弹

## 建议排查方向

1. **先确认 TrackPopupMenu 到底卡在哪**：在 `SetForegroundWindow` 前后、`TrackPopupMenu` 调用前后各加一行日志（时间戳），区分是"卡在模态循环"还是"根本没进函数"
2. **怀疑窗口被任务栏遮挡/命中测试异常**：试试 `SetWindowPos(hwnd, HWND_TOPMOST, ...)` 强制置顶后右键；或检查是否与任务栏窗口（Shell_TrayWnd）有父子关系冲突
3. **对比参考**：TrafficMonitor 的做法——它用自己的对话框窗口 + `TrackPopupMenu`（可见窗口直接作 owner），能正常弹。我们的差异点：窗口无边框、`WS_POPUP`、位置在任务栏上
4. 若 `TrackPopupMenu` 模态循环在跑但菜单不可见，试 `TPM_LEFTALIGN` 或指定不同坐标（比如屏幕中央）验证是否是**坐标导致菜单位置出屏**
5. 菜单弹出坐标当前用 `GetCursorPos`（物理像素），`TrackPopupMenu` 在 DPI 150% 下可能需 `SetProcessDpiAwareness` 已在代码里做了——检查坐标单位是否一致

## 注意事项

- **不要移除** `WM_ERASEBKGND` 里的背景填充（防黑框）；但里面 debug 埋点要清理
- 源码含 Unicode 文本/箭头，patch 工具可能误判二进制，用精确字符串替换
- 验证菜单用 `PostMessage(WM_RBUTTONUP)`（`mouse_event` 模拟物理点击会被 Windows 拦截导致假阴性）——注意这点在**当前版本**也需要复测是否仍成立
- 用户偏好：解决问题要**深挖根因**，不要反复表面尝试；改完直接构建+部署+启动实机查看；汇报要结论+证据，不要过程流水账
- 项目 git 状态：提交前把 debug log 写入代码清掉

---

## 修复记录（Pi，2026-08-10）— 右键菜单不弹出的根因与修复

### 根因（已实机复现并验证）

**`is_autostart()`/`ensure_autostart()`/`clear_autostart()` 在窗口过程（wndproc）内同步
`spawn("reg.exe")` 子进程**（`std::process::Command::output()`），在 TrackPopupMenu 之前执行。
后果分两种（都实测到）：

1. **wndproc 死锁/卡死**：GUI 子系统进程启动 `reg.exe` 需要拉起控制台宿主（conhost），
   在 RDP/远程会话中会长时间停滞，UI 线程卡在 `Command::output()`，窗口失去响应、
   后续点击消息不再被派发。
2. **TrackPopupMenu 静默失效**：即使 `reg` 最终返回（实测慢至 226ms），TrackPopupMenu
   进入模态循环但**不创建任何 #32768 菜单窗口**，永不返回、菜单不可见。

### 修复

三个函数全部改为直接调用 Win32 注册表 API，**UI 线程不再产生任何子进程**：

- `read_autostart()`：`RegOpenKeyExW + RegQueryValueExW`
- `ensure_autostart()`：`RegOpenKeyExW(KEY_WRITE) + RegSetValueExW`
- `clear_autostart()`：`RegOpenKeyExW(KEY_WRITE) + RegDeleteValueW`

### 验证（物理右键 = SendInput 模拟的真实输入，含激活路径）

- 物理右键 → 窗口激活（成为前台）→ 原生 `#32768` 菜单弹出，位于任务栏上方
- 菜单 z-order 第 4 位（高于 Shell_TrayWnd 第 7 位，其上仅系统隐形窗口）→ 最顶层 ✓
- 菜单项：开机自启（勾选状态正确、点击切换注册表）✓、退出（进程正常退出）✓
- 点击外部 → 菜单自动关闭 ✓；无闪黑框（原生菜单 + WM_ERASEBKGND 背景填充）✓
- 开机自启默认开启（启动时写 Run\NetSpeed）✓

### 经验教训 / 注意

- **wndproc 热路径内严禁 spawn 子进程**（尤其 reg.exe / 控制台程序）；本机用 Win32 API 直读。
- 之前"窗口被任务栏遮挡 / WindowFromPoint 返回 0"等判断，多为 DPI 虚拟化坐标换算错误
  与模态循环卡死后的**假象**，并非根因。
- TrafficMonitor 对比文档（已删除）里的 **Win11 深色任务栏 + 纯黑背景抑制右键菜单**
  是其官方注释提到的已知坑：若将来用户反馈深色模式下菜单不弹，检查采样背景是否为纯黑
  （0x000000），可参考其做法把背景色至少设为 1。当前 DARK_BG=0x302C2C 与采样色均非纯黑，
  实测不触发。

---

## 追加记录（Pi，2026-08-10，第 2 轮）— COMPARE 方案的逐项验证与收尾

### 逐项验证结果（真实右键 = SetCursorPos + mouse_event，已验证有效）

1. **近黑背景抑制假设：否定**。把采样背景强制改为 0x202020（近黑）后编译运行，
   真实右键仍正常弹出原生 #32768 菜单。当前系统为浅色任务栏（采样色 ≈ 0xFEF9F8），
   深色/近黑背景不是本机菜单问题的成因。
2. **SetParent 嵌入任务栏：不可行，已放弃**（有实机证据）：
   - 嵌入 TrayNotifyWnd：我们的窗口位于托盘左侧，client x = -166 落在父窗口
     client 区之外 → 命中测试/鼠标输入完全不达窗口。
   - 嵌入 Shell_TrayWnd：位置正确、WindowFromPoint/RealChildWindowFromPoint 均命中，
     但 **鼠标消息被 Win11 25H2 XAML 任务栏拦截，WM_RBUTTONDOWN 永远收不到**；
     只有 PostMessage 直接发消息才能弹菜单。嵌入还会让 FindWindowW 找不到窗口
     （不再是顶级窗口），且 explorer 重启会销毁子窗口。
   - 结论：菜单不弹与遮挡/嵌入无关；嵌入反而破坏输入，保持独立顶级窗口 + TOPMOST。
3. **TrackPopupMenu 简化：已采纳**。去掉 SetForegroundWindow + WM_NULL 预热，
   flags 改为 `TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD`（对齐 TrafficMonitor），
   实测真实右键菜单正常弹出、z-order 第 4 位（高于 Shell_TrayWnd 第 7 位）、
   点击外部自动关闭、开机自启勾选切换、退出正常。
4. **背景钳制非纯黑：已采纳**。`refresh_taskbar_color` 采样后每通道 `max(0x10)`，
   防御 Win11 深色任务栏 + 纯黑背景的已知坑。
5. **顺带修复单实例 bug**：原 `CreateMutexW` 未检查 `ERROR_ALREADY_EXISTS`，
   导致多实例可同时运行；现正确退出第二实例（实测启动第二个实例后仅剩一个进程）。

### 收尾

- 已删除全部调试埋点（netspeed-debug.log 不再写入）。
- 验证输入注意：本会话中 SendInput 偶发不送达（Start 菜单/任务栏菜单都不响应），
  而 `SetCursorPos + mouse_event` 稳定有效；远程会话下两者行为可能不一致，
  判定菜单行为请以能稳定复现的注入方式为准。

---

## 追加记录（Pi，2026-08-10，第 3 轮）— 收尾：嵌入重试被否，回退独立顶层窗口

两个并发代理留下的未提交改动（SetParent 嵌入任务栏 + 大量调试埋点）经审查与实测后**回退**：

- **嵌入方案再次实测否决**：临时埋点确认，嵌入 Shell_TrayWnd 后真实右键**收不到任何消息**
  （WM_RBUTTONDOWN/UP/CONTEXTMENU 均无），任务栏 XAML 弹出它自己的菜单（Xaml_WindowedPopupClass）。
  与第 2 轮结论一致（Win11 25H2 XAML 任务栏吞掉子窗口鼠标按钮输入）。最终保留
  **独立顶级窗口 + WS_EX_TOPMOST/HWND_TOPMOST**（round-2 已验证方案）。
- **顺带发现**：WS_POPUP 窗口经 SetParent 后 `GetParent()` 返回 owner（NULL）而非父窗口，
  会触发每 3 秒重复 SetParent（旧进程 16h 日志里 19196 次 "embed: ok"）；正确写法是
  `GetAncestor(hwnd, GA_PARENT)`。此坑在嵌入代码回退后已无关紧要，留档备查。
- 保留的三项改进：背景钳制 `clamp_bg()`（每通道 ≥0x10，采样与 fallback 均生效）、
  TPM 简化（`TPM_LEFTALIGN|TPM_RIGHTBUTTON|TPM_RETURNCMD`，去掉菜单关闭后的 PostMessage(WM_NULL)）、
  单实例 mutex 检查 ERROR_ALREADY_EXISTS。
- 清理：全部 dlog/tlog 埋点、仓库根目录临时 ps1 脚本、netspeed-debug.log 不再生成（实测确认）。
- 验证（真实输入 = SetCursorPos+mouse_event；本会话键盘注入可靠，鼠标按钮注入约 40% 概率被
  RDP 吞掉，判定以多次复现为准）：
  - 真实右键 → 原生 #32768 菜单弹出，z-order #7 > Shell_TrayWnd #13（最顶层）✓
  - 点击外部关闭 ✓；开机自启项点击切换注册表（双向）✓；退出项点击进程正常结束 ✓
  - 窗口区域右键连拍 9 帧：0 近黑像素，最小通道 56（文字本身），无闪黑框 ✓
  - 窗口 160×42 物理像素（3840×2160 @ 144DPI），托盘左侧 6px、垂直居中 ✓
- **DPI 陷阱**：PowerShell（DPI-unaware）的 GetWindowRect/GetPixel/GetSystemMetrics 坐标会被
  1.5x 虚拟化（160×42 窗口显示为 107×28）；验证物理坐标前须先 `SetProcessDpiAwareness(2)`。

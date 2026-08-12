# netspeed — 会话交接存档

更新：2026-08-12（上下文压缩前）

## 项目状态：✅ 功能完成，Win10 测试中

Rust 单 exe 任务栏网速/CPU/内存监控（TrafficMonitor 类），D2D+DComposition 渲染。

## 源码与部署

- 源码：`D:\dev\projects\netspeed`（git 已推 GitHub：https://github.com/zengge23/netspeed，SSH 认证 zengge23 可用）
- 部署：`C:\Users\Administrator\netspeed.exe`（运行中实例，开机自启引用）
- Win10 测试包：`C:\Users\Administrator\netspeed-v0.3.0-win10test.exe`（**用户正在 Win10 测，未反馈结果**）
- 归档：`D:\dev\projects\netspeed\release-archive\`（历史中间版 exe，保留备查）
- 构建：`cargo build --release` 零警告；`hermes verify --json` 全 PASS

## 最终架构（已定型，勿回退）

- 渲染：D2D → DXGI swapchain（PREMULTIPLIED）→ DirectComposition；透明背景 + GRAYSCALE AA；**每帧 `ctx.Clear()`**（FillRectangle(alpha=0) 不清屏会重影）
- 嵌入：SetParent(Shell_TrayWnd) + **保留 WS_POPUP**（不强制 WS_CHILD）；定位 HWND_TOP + 父客户区坐标（子窗口不能用 HWND_TOPMOST）
- 闪烁修复：REFRESH_TIMER(100ms) 已删；REPOS_TIMER=1s（位置比较跳过 SetWindowPos）；PAINT 由 DIRTY 门控；text format 缓存（FORMAT_LEFT/RIGHT/ARROW static）
- Win11 定位：锚 `TrayNotifyWnd.left - WINDOW_W - gap`，`gap = 小组件开(TaskbarDa=1) ? 250 : 6`（避让天气图标，XAML 渲染 GDI 枚举不到）
- Win10 定位：MSTaskSwWClass/MSTaskListWClass 右侧（EnumChildWindows 递归找）+ 托盘左缘钳制 + 通用避让遍历（枚举子窗口取最左避让位）
- 位置 rescue：锚点 None 时回 LAST_X/LAST_Y（上次成功位置）；fast-path 与目标差 ≤40px 才算 in place
- 小组件开关即时响应：watch_taskbar_da 线程 RegNotifyChangeKeyValue 监听 TaskbarDa → PostMessage(WM_APP_REPOS=0x8001) → reposition
- 网速：`total_received()/total_transmitted()` 自算差值（sysinfo 0.33 的 received() 是增量勿二次相减）；启动锁定网卡不切换
- 菜单：WM_RBUTTONUP + WM_CONTEXTMENU 双分支 + MENU_OPEN 防重入；TPM_LEFTALIGN|TPM_RIGHTBUTTON 无 RETURNCMD，命令走 WM_COMMAND（1=开机自启 2=退出）
- 图标/版本：embed-resource + resources/netspeed.rc（1 ICON + VERSIONINFO 0.3.0）

## 布局常量（最终）

WINDOW_W=177、WINDOW_H（运行时：Win11 42 / Win10 36）；ROW_LEFT=3、ARROW_RIGHT=24、SPEED_LEFT=26、SPEED_RIGHT=94（右对齐）、DIVIDER_X=98、STATUS_LEFT=106、STATUS_LABEL_RIGHT=132、STATUS_RIGHT=173（数值右对齐）；两行均分 h2=h/2、row_h=h2-3、up_top=2、down_top=h2+1

## 待办/未决

1. **Win10 测试结果未反馈**（netspeed-v0.3.0-win10test.exe）——用户测完可能报位置/网速/菜单问题
2. 开机自启默认写入注册表 Run（右键菜单可切换）
3. 版本号 0.3.0（Cargo.toml / VERSIONINFO / 交付命名 三处同步）

## 环境备忘

- crates.io 直连 SSL 失败 → 项目级 `.cargo/config.toml` rsproxy.cn 镜像（已随仓库公开，用户 OK）
- vision_analyze 401 不可用 → 截图像素分析用 zlib 手动解 PNG
- PIL 沙箱半损坏 → GDI BitBlt 直采验证闪烁；闪烁阈值：窗口 7434px，diff>2000 才全窗闪
- 用户有 Win10 虚拟机测兼容性；本机 Win11 25H2（3840×2160，Python 逻辑坐标 ×1.5 = 物理）
- 单实例互斥锁：hermes verify 会起 debug 实例占锁 → verify 后需手动重启 release 实例再交付
- 用户 Telegram @zengge（838144185）；GitHub zengge23

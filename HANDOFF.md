# netspeed — 会话交接存档

更新：2026-08-12（图表渐变填充完成，待提交）

## 项目状态：✅ 功能完成，图表风格已定为渐变填充

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
- **窗口宽动态**：WINDOW_W 是 static mut（285 有图 / 177 无图），右键「显示图表」切换时重建 renderer + SetWindowPos resize + reposition（自适应不留空白）
- **走势图 = 渐变填充**（用户选定）：PathGeometry 闭合路径（底部→曲线→底部）+ 垂直 LinearGradientBrush（顶部 alpha .55 → 底部透明）+ 曲线描边（Round cap/join）；GradientStopCollection 用 ctx 6 参版本（D2D1_COLOR_SPACE_SRGB/PREMULTIPLIED），FillGeometry 需 geom.cast::<ID2D1Geometry>()；CreateStrokeStyle 在 d2d_factory 用 PROPERTIES1
- **菜单 6s 自动关闭**：show_context_menu spawn 线程 sleep 6s → 若 MENU_OPEN 则 PostMessage(#32768, WM_KEYDOWN VK_ESCAPE) 关闭 TrackPopupMenu 模态循环
- Win11 定位：锚 `TrayNotifyWnd.left - WINDOW_W - gap`，`gap = 小组件开(TaskbarDa=1) ? 250 : 6`（避让天气图标，XAML 渲染 GDI 枚举不到）
- Win10 定位：MSTaskSwWClass/MSTaskListWClass 右侧（EnumChildWindows 递归找）+ 托盘左缘钳制 + 通用避让遍历（枚举子窗口取最左避让位）
- 位置 rescue：锚点 None 时回 LAST_X/LAST_Y（上次成功位置）；fast-path 与目标差 ≤40px 才算 in place
- 小组件开关即时响应：watch_taskbar_da 线程 RegNotifyChangeKeyValue 监听 TaskbarDa → PostMessage(WM_APP_REPOS=0x8001) → reposition
- 网速：`total_received()/total_transmitted()` 自算差值（sysinfo 0.33 的 received() 是增量勿二次相减）；启动锁定网卡不切换
- 菜单：WM_RBUTTONUP + WM_CONTEXTMENU 双分支 + MENU_OPEN 防重入；TPM_LEFTALIGN|TPM_RIGHTBUTTON 无 RETURNCMD，命令走 WM_COMMAND（1=开机自启 2=退出）
- 图标/版本：embed-resource + resources/netspeed.rc（1 ICON + VERSIONINFO 0.3.0）

## 布局常量（动态）

WINDOW_W（运行时：有图 285 / 无图 177）、WINDOW_H（运行时：Win11 42 / Win10 36）；ROW_LEFT=3、ARROW_RIGHT=24、SPEED_LEFT=26；有图布局：speed_right=88、divider=142、status 146-175/215、网速图 90-138、CPU/内存图 219-281；无图布局：speed_right=94、divider=98、status 106-134/173（无图区）；两行均分 h2=h/2、row_h=h2-3、up_top=2、down_top=h2+1、graph_half=h2-3、graph_top1=2、graph_top2=h2+1

## 待办/未决

1. Win10 实机回归：悬停面板位置（主窗口上方 y=top-H-6，Win10 36px 任务栏下是否贴顶）+ 图表开关自适应
2. 候选功能未做：节日彩蛋、今日流量大字版、丢包率检测、延迟/速度日统计、多网卡切换、点击表情切换模式、ping 目标自定义、网络心情日报
3. 版本号 0.4.0（Cargo.toml / VERSIONINFO / 交付命名 三处同步）——已发版

## 环境备忘

- crates.io 直连 SSL 失败 → 项目级 `.cargo/config.toml` rsproxy.cn 镜像（已随仓库公开，用户 OK）
- vision_analyze 401 不可用 → 截图像素分析用 zlib 手动解 PNG
- PIL 沙箱半损坏 → GDI BitBlt 直采验证闪烁；闪烁阈值：窗口 7434px，diff>2000 才全窗闪
- 用户有 Win10 虚拟机测兼容性；本机 Win11 25H2（3840×2160，Python 逻辑坐标 ×1.5 = 物理）
- 单实例互斥锁：hermes verify 会起 debug 实例占锁 → verify 后需手动重启 release 实例再交付
- 用户 Telegram @zengge（838144185）；GitHub zengge23

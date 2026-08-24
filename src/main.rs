#![windows_subsystem = "windows"]
// Global mutable state is accessed only from the single GUI thread (message
// loop + timers); the project predates Rust 2024's static_mut_refs lint.
#![allow(static_mut_refs)]

use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct3D10::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::UI::HiDpi::*;

// Window width is runtime: 347 with trend graphs, 219 compact without
// (toggled via the context menu's 显示图表 item). The left 42px hold the
// latency dot + value; everything right of x=53 is the ORIGINAL layout
// shifted wholesale (no column is stretched or squeezed).
const WINDOW_W_GRAPH: i32 = 367;
const WINDOW_W_COMPACT: i32 = 245;
static mut WINDOW_W: i32 = WINDOW_W_GRAPH;

// Window height is platform-dependent: Win11 taskbar ≈48px → 42; Win10
// classic taskbar ≈40px → 36. Set once at startup (see detect_taskbar_type).
static mut WINDOW_H: i32 = 42;

// Compact two-row layout tokens (physical px at 96 DPI)
const ROW_LEFT: i32 = 79;
const ARROW_RIGHT: i32 = 92;
const SPEED_LEFT: i32 = 94;
// Latency status dot + mood emoji + value, left edge of the window.
// Dot 6px, mood emoji 16px wide, value text right-aligned; the text zone
// is wide enough that "1234ms" never collides with the arrow column.
// Latency mood emoji + value, left edge of the window. No status dot —
// the mood emoji itself carries the color cue. Value text right-aligned;
// the text zone is wide enough that "1234ms" never collides with the
// arrow column.
const LAT_TEXT_LEFT: f32 = 32.0;
const LAT_TEXT_RIGHT: f32 = 76.0;
// Mood emoji column: big glyph (28px, window-height limit) leading the
// latency value.
const EMOJI_LEFT: f32 = 0.0;
const EMOJI_RIGHT: f32 = 28.0;
// Network trend graph, immediately right of the speed number.
const GRAPH_UP_LEFT: i32 = 158;
const GRAPH_UP_RIGHT: i32 = 206;
// CPU/memory trend graph, immediately right of the percentage value.
const GRAPH_CPU_LEFT: i32 = 287;
const GRAPH_CPU_RIGHT: i32 = 349;
// Color tokens — Dark theme (system dark taskbar)
const DARK_BG: COLORREF = COLORREF(0x00302C2C);
const DARK_DIVIDER: COLORREF = COLORREF(0x00606060);
const DARK_DOWN: COLORREF = COLORREF(0x009EDB6C); // #6cdb9e
const DARK_UP: COLORREF = COLORREF(0x006BB3FF);   // #ffb36b
const DARK_IDLE: COLORREF = COLORREF(0x007A7A7A);
const DARK_UNIT: COLORREF = COLORREF(0x00B0B0B0);
const DARK_WARNING: COLORREF = COLORREF(0x006060E8);
// Color tokens — Light theme (system light taskbar)
const LIGHT_BG: COLORREF = COLORREF(0x00DFECF2); // ≈ taskbar light bg RGB(242,236,223)
const LIGHT_DIVIDER: COLORREF = COLORREF(0x00A8A8AC);
const LIGHT_DOWN: COLORREF = COLORREF(0x003A8A3A); // softer green on light taskbar
const LIGHT_UP: COLORREF = COLORREF(0x00385CB8);   // softer orange-red on light taskbar
const LIGHT_IDLE: COLORREF = COLORREF(0x00909090);
const LIGHT_UNIT: COLORREF = COLORREF(0x00787878);
const LIGHT_WARNING: COLORREF = COLORREF(0x003838C8);
const CLASS_NAME: &str = "NetSpeedTaskbarWnd\0";
const REPOS_TIMER: usize = 2;
const PAINT_TIMER: usize = 3;
// Custom message: widgets button (TaskbarDa) changed → reposition now.
const WM_APP_REPOS: u32 = 0x8000 + 1;

static mut DOWN: f64 = 0.0;
static mut UP: f64 = 0.0;
static mut CPU_USAGE: f32 = 0.0;
static mut MEMORY_USAGE: f32 = 0.0;
// Adaptive peak for the speed graph (network speed has no fixed 0-100% scale).
// Decays slowly so the curve stays visible at low activity.
static mut MAX_DOWN: f64 = 1024.0;
static mut MAX_UP: f64 = 1024.0;
// History buffers for the scrolling trend graph (TrafficMonitor style:
// TASKBAR_GRAPH_STEP=5 → every 5 polls the accumulated average is written,
// then the curve scrolls one column). Values are 0-100 percent.
const GRAPH_STEP: usize = 5;
const GRAPH_LEN: usize = 128;
static mut HIST_DOWN: [u8; GRAPH_LEN] = [0; GRAPH_LEN];
static mut HIST_UP: [u8; GRAPH_LEN] = [0; GRAPH_LEN];
static mut HIST_CPU: [u8; GRAPH_LEN] = [0; GRAPH_LEN];
static mut HIST_MEM: [u8; GRAPH_LEN] = [0; GRAPH_LEN];
static mut HIST_HEAD: usize = 0;
static mut HIST_ACC: [f64; 4] = [0.0; 4]; // down/up/cpu/mem accumulators
static mut HIST_COUNT: usize = 0;
static mut LIGHT_THEME: bool = false;
static mut EMBEDDED: bool = false;
static mut MENU_OPEN: bool = false;
// Whether the scrolling trend graphs are drawn (toggle via context menu).
static mut SHOW_GRAPHS: bool = true;
// Network latency detection (toggle via context menu). LATENCY_MS: -1 = fail.
static mut NET_DETECT: bool = true;
static mut LATENCY_MS: i32 = -1;
// Autostart remembered in netspeed.ini. The registry Run key is the source
// of truth, but in restricted environments (no HKCU write access) the write
// silently fails, so the last user choice is kept here as a fallback.
static mut AUTOSTART_INI: bool = false;
static mut DIRTY: bool = true;
// 渐变温度计配色（右键菜单切换；关 = 固定主题色 up/down）
static mut GRADIENT_COLOR: bool = true;
// 悬停详情面板（右键菜单切换）
static mut SHOW_PANEL: bool = true;
// Main window handle. Set once at creation; a background thread (see
// watch_main_window) checks it and force-quits this process if the window
// is ever destroyed (Explorer crash on lock-screen destroys the taskbar and
// our child window, but the process lingers because the hover panel keeps
// the message loop alive — which the watchdog, keyed off the mutex, mistakes
// for a healthy instance).
static mut MAIN_HWND: HWND = HWND(0);
// True = Win11 XAML taskbar (parent Shell_TrayWnd, anchor TrayNotifyWnd);
// False = Win10 classic taskbar (parent ReBarWindow32, anchor start button).
static mut TASKBAR_WIN11: bool = true;
// Last successfully applied window position (physical). Used as a rescue
// anchor when TrayNotifyWnd is briefly unavailable during a taskbar
// re-layout — we go back to where we were, not to a guessed spot.
static mut LAST_X: i32 = 0;
static mut LAST_Y: i32 = 0;
static mut RENDERER: Option<D2DRenderer> = None;
// Cached DirectWrite text formats — creating a DWriteFactory + format on
// every render (up to 2x/sec) causes intermittent font-loading races, which
// showed up as arrows rendering smaller on some frames.
static mut FORMAT_LEFT: Option<IDWriteTextFormat> = None;
static mut FORMAT_RIGHT: Option<IDWriteTextFormat> = None;
static mut FORMAT_ARROW: Option<IDWriteTextFormat> = None;
// 12px right-aligned format for ≥1000ms latency values ("1234ms").
static mut FORMAT_SMALL: Option<IDWriteTextFormat> = None;
// Mood emoji (Segoe UI Emoji, 16px) — rendered with ENABLE_COLOR_FONT.
static mut FORMAT_EMOJI: Option<IDWriteTextFormat> = None;
// ─── Hover detail panel ──────────────────────────────────────
// Second topmost popup shown while the mouse is over the taskbar window:
// current speeds, latency + mood, CPU/mem, NIC name, today's totals.
static mut PANEL_HWND: HWND = HWND(0);
static mut PANEL_VISIBLE: bool = false;
static mut PANEL_TRACKING: bool = false; // TrackMouseEvent armed
static mut IFACE_NAME: String = String::new(); // active NIC (from net thread)
static mut TODAY_DOWN: f64 = 0.0;  // bytes received today
static mut TODAY_UP: f64 = 0.0;    // bytes sent today
static mut TODAY_DATE: i32 = -1;   // yyyymmdd the counters were last reset on
const PANEL_W: i32 = 290;
const PANEL_H: i32 = 142;

// ─── D2D + DirectComposition renderer (TrafficMonitor Win11 path) ──
//
// The Win11 XAML taskbar composites its children through DirectComposition;
// plain GDI child-window content is NOT shown on top of the taskbar. So we
// render with D2D into a DXGI swapchain, attach that swapchain to a
// DirectComposition visual rooted at our (embedded) HWND, and commit. This
// mirrors CTaskBarDlg's D2D1_WITH_DCOMPOSITION path exactly.

struct D2DRenderer {
    // Held alive for their lifetime; not read after construction.
    #[allow(dead_code)]
    d3d10: ID3D10Device1,
    #[allow(dead_code)]
    dxgi_factory: IDXGIFactory2,
    swapchain: IDXGISwapChain1,
    dcomp: IDCompositionDevice,
    #[allow(dead_code)]
    target: IDCompositionTarget,
    #[allow(dead_code)]
    visual: IDCompositionVisual,
    #[allow(dead_code)]
    d2d_factory: ID2D1Factory1,
    #[allow(dead_code)]
    d2d_device: ID2D1Device,
    ctx: ID2D1DeviceContext,
    bitmap: ID2D1Bitmap1,
    h: u32,
}

impl D2DRenderer {
    /// Create the full D3D10 → DXGI → DComposition → D2D pipeline for a
    /// window of the given (physical) size. Returns None on any failure.
    fn new(hwnd: HWND, w: u32, h: u32) -> Option<D2DRenderer> {
        unsafe {
            // 0) Pick the first DXGI adapter (mirrors TrafficMonitor).
            let factory0: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let adapter: IDXGIAdapter1 = factory0.EnumAdapters1(0).ok()?;

            // 1) D3D10 device (hardware first, software fallback).
            let mut d3d10: Option<ID3D10Device1> = None;
            let hr = D3D10CreateDevice1(
                &adapter,
                D3D10_DRIVER_TYPE_HARDWARE,
                None,
                D3D10_CREATE_DEVICE_BGRA_SUPPORT.0 as u32,
                D3D10_FEATURE_LEVEL_10_1,
                0x20, // D3D10_1_SDK_VERSION
                Some(&mut d3d10),
            );
            let device = if hr.is_ok() && d3d10.is_some() {
                d3d10.unwrap()
            } else {
                let mut sw: Option<ID3D10Device1> = None;
                let hr2 = D3D10CreateDevice1(
                    &adapter,
                    D3D10_DRIVER_TYPE_REFERENCE,
                    None,
                    D3D10_CREATE_DEVICE_BGRA_SUPPORT.0 as u32,
                    D3D10_FEATURE_LEVEL_10_1,
                    0x20,
                    Some(&mut sw),
                );
                if hr2.is_err() || sw.is_none() {
                    return None;
                }
                sw.unwrap()
            };

            // 2) DXGI factory + composition swapchain.
            let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory2(0).ok()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w,
                Height: h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swapchain = dxgi_factory.CreateSwapChainForComposition(&device, &desc, None).ok()?;

            // 3) DComposition device + target + visual rooted at our HWND.
            let dxgi_device: IDXGIDevice = device.cast().ok()?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device).ok()?;
            let topmost = (GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as i32) != 0;
            let target = dcomp.CreateTargetForHwnd(hwnd, topmost).ok()?;
            let visual = dcomp.CreateVisual().ok()?;
            let _ = target.SetRoot(&visual);
            let _ = visual.SetContent(&swapchain);

            // 4) D2D device context over the swapchain buffer.
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi_device).ok()?;
            let ctx = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE).ok()?;

            // 5) Bitmap bound to the swapchain back buffer. Passing the
            // properties explicitly fails with E_INVALIDARG on some drivers;
            // let D2D adopt the surface's own format (TrafficMonitor passes
            // D2D1_BITMAP_PROPERTIES to CreateSharedBitmap, not the surface
            // variant).
            let surface: IDXGISurface = swapchain.GetBuffer(0).ok()?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: core::mem::ManuallyDrop::new(None),
            };
            let bitmap = match ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props)) {
                Ok(b) => b,
                Err(_) => ctx.CreateBitmapFromDxgiSurface(&surface, None).ok()?,
            };

            Some(D2DRenderer {
                d3d10: device,
                dxgi_factory,
                swapchain,
                dcomp,
                target,
                visual,
                d2d_factory,
                d2d_device,
                ctx,
                bitmap,
                h,
            })
        }
    }

    /// Render the current stats into the swapchain and commit to screen.
    /// Returns false if the GPU device was lost (e.g. after a driver update
    /// or display change) — the caller should rebuild the renderer.
    fn render(&mut self) -> bool {
        unsafe {
            self.ctx.SetTarget(&self.bitmap);
            self.ctx.BeginDraw();
            // Transparent background + explicit GRAYSCALE AA. ClearType
            // (subpixel) rendering on a transparent layer is what caused the
            // ghosting — grayscale AA composites cleanly over the taskbar.
            self.ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            let (_, div, down_a, up_a, idle, unit, warn) = if LIGHT_THEME {
                (LIGHT_BG, LIGHT_DIVIDER, LIGHT_DOWN, LIGHT_UP, LIGHT_IDLE, LIGHT_UNIT, LIGHT_WARNING)
            } else {
                (DARK_BG, DARK_DIVIDER, DARK_DOWN, DARK_UP, DARK_IDLE, DARK_UNIT, DARK_WARNING)
            };
            let h = self.h as f32;
            // Clear() overwrites the whole target (unlike FillRectangle with
            // alpha=0, which blends to "dst = 0*a + dst*(1-a)" and leaves old
            // frame content in the premultiplied buffer — that residue is the
            // ghosting you see when numbers change). Clear guarantees a clean
            // transparent layer every frame.
            let clear_color = d2d_color(COLORREF(0), 0.0);
            self.ctx.Clear(Some(&clear_color));

            let (down, up, cpu_usage, memory_usage) = (DOWN, UP, CPU_USAGE, MEMORY_USAGE);
            let (down_val, down_unit) = fmt_speed(down);
            let (up_val, up_unit) = fmt_speed(up);
            // 渐变温度计配色开关：开 = log 尺度色带（青蓝→橙红），关 = 固定主题色
            let down_color = if down > 0.0 {
                if GRADIENT_COLOR { speed_color(down) } else { down_a }
            } else {
                idle
            };
            let up_color = if up > 0.0 {
                if GRADIENT_COLOR { speed_color(up) } else { up_a }
            } else {
                idle
            };

            // Cached text formats (created once; see static FORMAT_*).
            if FORMAT_LEFT.is_none() {
                FORMAT_LEFT = create_text_format(&"Segoe UI".encode_utf16().collect::<Vec<_>>(), 15.0, DWRITE_TEXT_ALIGNMENT_LEADING);
            }
            if FORMAT_RIGHT.is_none() {
                FORMAT_RIGHT = create_text_format(&"Segoe UI".encode_utf16().collect::<Vec<_>>(), 15.0, DWRITE_TEXT_ALIGNMENT_TRAILING);
            }
            if FORMAT_ARROW.is_none() {
                FORMAT_ARROW = create_text_format(&"Segoe UI".encode_utf16().collect::<Vec<_>>(), 20.0, DWRITE_TEXT_ALIGNMENT_LEADING);
            }
            if FORMAT_SMALL.is_none() {
                FORMAT_SMALL = create_text_format(&"Segoe UI".encode_utf16().collect::<Vec<_>>(), 12.0, DWRITE_TEXT_ALIGNMENT_TRAILING);
            }
            if FORMAT_EMOJI.is_none() {
                FORMAT_EMOJI = create_text_format(&"Segoe UI Emoji".encode_utf16().collect::<Vec<_>>(), 28.0, DWRITE_TEXT_ALIGNMENT_LEADING);
            }
            if let (Some(f), Some(rf), Some(af)) = (FORMAT_LEFT.as_ref(), FORMAT_RIGHT.as_ref(), FORMAT_ARROW.as_ref()) {
                let row_left = ROW_LEFT as f32;
                let arrow_right = ARROW_RIGHT as f32;
                let speed_left = SPEED_LEFT as f32;
                // Layout adapts to graph toggle: with graphs the window is
                // wide and each value has a graph beside it; without graphs
                // it is compact and values sit next to the divider.
                let (speed_right, divider_x, status_left, status_label_right, status_right,
                     graph_up_left, graph_up_w, graph_cpu_left, graph_cpu_w) =
                    if SHOW_GRAPHS {
                        (156.0, 210.0, 214.0, 243.0, 283.0,
                         GRAPH_UP_LEFT as f32, (GRAPH_UP_RIGHT - GRAPH_UP_LEFT) as f32,
                         GRAPH_CPU_LEFT as f32, (GRAPH_CPU_RIGHT - GRAPH_CPU_LEFT) as f32)
                    } else {
                        (162.0, 166.0, 174.0, 200.0, 241.0,
                         0.0, 0.0, 0.0, 0.0)
                    };
                // Layout: two rows fill the window; text vertically centered
                // per row. Each value has its own trend graph immediately to
                // its right: row1 = [↑speed][up graph] | [CPU][cpu graph],
                // row2 = [↓speed][down graph] | [MEM][mem graph].
                let h2 = h / 2.0;
                let row_h = h2 - 3.0;         // per-row height
                let up_top = 2.0;              // top margin
                let down_top = h2 + 1.0;       // second row
                let graph_half = h2 - 3.0;     // each row's graph band height
                let graph_top1 = 2.0;          // row1 band
                let graph_top2 = h2 + 1.0;     // row2 band
                // Arrow glyph is 20px tall; the row is ~16px. Give the arrow
                // layout rect extra height so it is never clipped (clipping is
                // what made the arrow look "sometimes big, sometimes small").
                let arrow_top = up_top - 2.0;
                let arrow_h = row_h + 4.0;

                // Arrows.
                let ub = self.ctx.CreateSolidColorBrush(&d2d_color(up_color, 1.0), None).ok();
                let db = self.ctx.CreateSolidColorBrush(&d2d_color(down_color, 1.0), None).ok();
                if let (Some(ub), Some(db)) = (ub.as_ref(), db.as_ref()) {
                    self.ctx.DrawText(
                        &"↑".encode_utf16().collect::<Vec<_>>(), af,
                        &D2D_RECT_F { left: row_left, top: arrow_top, right: arrow_right, bottom: arrow_top + arrow_h },
                        ub, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                    self.ctx.DrawText(
                        &"↓".encode_utf16().collect::<Vec<_>>(), af,
                        &D2D_RECT_F { left: row_left, top: down_top - 2.0, right: arrow_right, bottom: down_top - 2.0 + arrow_h },
                        db, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                }

                // Latency status dot (left edge, one per row): green <50ms,
                // yellow 50-150ms, red >150ms or unreachable. Hidden when the
                // detection toggle is off.
                if NET_DETECT {
                    let dot_color = if LATENCY_MS < 0 {
                        COLORREF(0x00E05A5A) // red: unreachable
                    } else if LATENCY_MS < 50 {
                        COLORREF(0x0058A858) // green
                    } else if LATENCY_MS < 150 {
                        COLORREF(0x00D8B838) // yellow
                    } else {
                        COLORREF(0x00E05A5A) // red: high latency
                    };
                    if let Some(dotb) = self.ctx.CreateSolidColorBrush(&d2d_color(dot_color, 1.0), None).ok().as_ref() {
                        // Single latency indicator, vertically centered on the
                        // whole window (both rows share the same network path,
                        // so one value is enough).
                        // Mood emoji ("网络心情") leading the latency value:
                        // 😄 flying / 🙂 smooth / 😐 middling / 😫 struggling.
                        // Big glyph (22px) — the emoji itself is the status
                        // indicator (no separate dot). Rendered with
                        // ENABLE_COLOR_FONT so Segoe UI Emoji draws in full
                        // color; falls back to monochrome on drivers without
                        // color-font support.
                        if let Some(ef) = FORMAT_EMOJI.as_ref() {
                            let mood = net_mood();
                            let mood_utf: Vec<u16> = mood.encode_utf16().collect();
                            self.ctx.DrawText(
                                &mood_utf,
                                ef,
                                &D2D_RECT_F {
                                    left: EMOJI_LEFT,
                                    top: h / 2.0 - 14.0,
                                    right: EMOJI_RIGHT,
                                    bottom: h / 2.0 + 14.0,
                                },
                                dotb,
                                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                        // Latency value text right of the dot. Unit always
                        // shown; ≥1000ms switches to a smaller 12px format so
                        // "1234ms" fits without touching the arrow column.
                        // Right-aligned so short values hug the right edge.
                        let lat_text = if LATENCY_MS < 0 {
                            "--".to_string()
                        } else {
                            format!("{}ms", LATENCY_MS)
                        };
                        let lat_utf: Vec<u16> = lat_text.encode_utf16().collect();
                        let lat_fmt = if LATENCY_MS >= 1000 {
                            FORMAT_SMALL.as_ref()
                        } else {
                            Some(rf)
                        };
                        if let Some(lat_fmt) = lat_fmt {
                            self.ctx.DrawText(
                                &lat_utf,
                                lat_fmt,
                                &D2D_RECT_F {
                                    left: LAT_TEXT_LEFT,
                                    top: up_top,
                                    right: LAT_TEXT_RIGHT,
                                    bottom: h - up_top,
                                },
                                dotb,
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                    }
                }

                // Speeds (right-aligned so long values grow left, never into
                // the divider).
                if let Some(sb) = self.ctx.CreateSolidColorBrush(&d2d_color(up_color, 1.0), None).ok().as_ref() {
                    let up_text = format!("{} {}", up_val.trim(), up_unit);
                    let down_text = format!("{} {}", down_val.trim(), down_unit);
                    self.ctx.DrawText(
                        &up_text.encode_utf16().collect::<Vec<_>>(), rf,
                        &D2D_RECT_F { left: speed_left, top: up_top, right: speed_right, bottom: up_top + row_h },
                        sb, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                    let sb2 = self.ctx.CreateSolidColorBrush(&d2d_color(down_color, 1.0), None).ok();
                    if let Some(sb2) = sb2.as_ref() {
                        self.ctx.DrawText(
                            &down_text.encode_utf16().collect::<Vec<_>>(), rf,
                            &D2D_RECT_F { left: speed_left, top: down_top, right: speed_right, bottom: down_top + row_h },
                            sb2, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }

                // Divider.
                if let Some(dvb) = self.ctx.CreateSolidColorBrush(&d2d_color(div, 1.0), None).ok().as_ref() {
                    self.ctx.FillRectangle(
                        &D2D_RECT_F { left: divider_x, top: 7.0, right: divider_x + 1.0, bottom: h - 7.0 },
                        dvb,
                    );
                }

                // CPU / memory labels + values.
                let cpu_color = if cpu_usage >= 85.0 { warn } else { unit };
                let mem_color = if memory_usage >= 85.0 { warn } else { unit };
                if let Some(lb) = self.ctx.CreateSolidColorBrush(&d2d_color(unit, 1.0), None).ok().as_ref() {
                    self.ctx.DrawText(
                        &"CPU".encode_utf16().collect::<Vec<_>>(), f,
                        &D2D_RECT_F { left: status_left, top: up_top, right: status_label_right, bottom: up_top + row_h },
                        lb, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                    self.ctx.DrawText(
                        &"内存".encode_utf16().collect::<Vec<_>>(), f,
                        &D2D_RECT_F { left: status_left, top: down_top, right: status_label_right, bottom: down_top + row_h },
                        lb, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
                let cb = self.ctx.CreateSolidColorBrush(&d2d_color(cpu_color, 1.0), None).ok();
                let mb = self.ctx.CreateSolidColorBrush(&d2d_color(mem_color, 1.0), None).ok();
                if let (Some(cb), Some(mb)) = (cb.as_ref(), mb.as_ref()) {
                    let cpu_text = format!("{:>2.0}%", cpu_usage);
                    let mem_text = format!("{:>2.0}%", memory_usage);
                    self.ctx.DrawText(
                        &cpu_text.encode_utf16().collect::<Vec<_>>(), rf,
                        &D2D_RECT_F { left: status_label_right, top: up_top, right: status_right, bottom: up_top + row_h },
                        cb, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                    self.ctx.DrawText(
                        &mem_text.encode_utf16().collect::<Vec<_>>(), rf,
                        &D2D_RECT_F { left: status_label_right, top: down_top, right: status_right, bottom: down_top + row_h },
                        mb, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL,
                    );
                }

                // ── Trend graph with gradient fill (user-chosen style):
                // build a closed path from the bottom-left up over the curve
                // points and back to the bottom-right, fill it with a
                // vertical gradient (curve color at top → transparent at
                // bottom), then stroke the curve itself on top.
                let draw_graph = |hist: &[u8; GRAPH_LEN], color: COLORREF, gx: f32, gw: f32, gy: f32, gh: f32| {
                    let head = HIST_HEAD;
                    let cols = (gw as usize).min(GRAPH_LEN);
                    if cols < 2 { return; }
                    // Curve points (newest at right edge, scrolling left).
                    let pts: Vec<D2D_POINT_2F> = (0..cols)
                        .map(|i| {
                            let v = hist[(head + GRAPH_LEN - 1 - i) % GRAPH_LEN] as f32;
                            let x = gx + (cols - 1 - i) as f32;
                            let y = gy + gh - v * gh / 100.0;
                            D2D_POINT_2F { x, y }
                        })
                        .collect();
                    // Closed path: bottom-left → curve → bottom-right.
                    let path = self.d2d_factory.CreatePathGeometry().ok();
                    if let Some(geom) = path.as_ref() {
                        if let Ok(sink) = geom.Open() {
                            sink.BeginFigure(D2D_POINT_2F { x: pts[0].x, y: gy + gh }, D2D1_FIGURE_BEGIN_FILLED);
                            for p in pts.iter() { sink.AddLine(*p); }
                            sink.AddLine(D2D_POINT_2F { x: pts[cols-1].x, y: gy + gh });
                            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                            let _ = sink.Close();
                            // Vertical gradient: color (alpha .55) → transparent.
                            let stops = [
                                D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(color, 0.55) },
                                D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(color, 0.0) },
                            ];
                            let col = self.ctx.CreateGradientStopCollection(
                                &stops,
                                D2D1_COLOR_SPACE_SRGB,
                                D2D1_COLOR_SPACE_SRGB,
                                D2D1_BUFFER_PRECISION_8BPC_UNORM,
                                D2D1_EXTEND_MODE_CLAMP,
                                D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
                            ).ok();
                            let g: windows::core::Result<ID2D1Geometry> = geom.cast();
                            if let Ok(g) = g {
                                if let Some(col) = col.as_ref() {
                                    let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                                        startPoint: D2D_POINT_2F { x: gx, y: gy },
                                        endPoint: D2D_POINT_2F { x: gx, y: gy + gh },
                                        ..Default::default()
                                    };
                                    let brush = self.ctx.CreateLinearGradientBrush(&props, None, col).ok();
                                    if let Some(brush) = brush.as_ref() {
                                        self.ctx.FillGeometry(&g, brush, None);
                                    }
                                } else {
                                    // Gradient unavailable → translucent solid fill.
                                    if let Some(sb) = self.ctx.CreateSolidColorBrush(&d2d_color(color, 0.4), None).ok().as_ref() {
                                        self.ctx.FillGeometry(&g, sb, None);
                                    }
                                }
                            }
                        }
                    }
                    // Stroke the curve itself.
                    if let Some(gb) = self.ctx.CreateSolidColorBrush(&d2d_color(color, 1.0), None).ok().as_ref() {
                        let line = self.d2d_factory.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES1 {
                            startCap: D2D1_CAP_STYLE_ROUND,
                            endCap: D2D1_CAP_STYLE_ROUND,
                            lineJoin: D2D1_LINE_JOIN_ROUND,
                            ..Default::default()
                        }, None).ok();
                        if let Some(line) = line.as_ref() {
                            for wnd in pts.windows(2) {
                                self.ctx.DrawLine(wnd[0], wnd[1], gb, 1.0, line);
                            }
                        }
                    }
                };
                if SHOW_GRAPHS {
                    // Row 1: upload graph (left) + CPU graph (right).
                    draw_graph(&HIST_UP, up_color, graph_up_left, graph_up_w, graph_top1, graph_half);
                    draw_graph(&HIST_CPU, cpu_color, graph_cpu_left, graph_cpu_w, graph_top1, graph_half);
                    // Row 2: download graph (left) + memory graph (right).
                    draw_graph(&HIST_DOWN, down_color, graph_up_left, graph_up_w, graph_top2, graph_half);
                    draw_graph(&HIST_MEM, mem_color, graph_cpu_left, graph_cpu_w, graph_top2, graph_half);
                }
            }

            let _ = self.ctx.EndDraw(None, None);
            // Present/Commit can fail with DXGI_ERROR_DEVICE_REMOVED /
            // DXGI_ERROR_DEVICE_RESET after a driver update, GPU reset, or
            // display change (common on wake/lock-screen). These are
            // IGNORED with `let _`, so the renderer keeps drawing to a dead
            // device and the window goes blank yet stays interactive. Surface
            // that condition so the caller knows to rebuild the pipeline.
            let present_ok = self.swapchain.Present(1, 0).is_ok();
            let _ = self.dcomp.Commit();
            !present_ok
        }
    }
}

/// (Re)build the D2D/DXGI/DComposition renderer for the main window.
/// Used at startup and to recover from GPU device loss (driver update,
/// display change, wake) — without a fresh pipeline the window goes blank
/// yet stays interactive, which the watchdog can't detect.
unsafe fn rebuild_renderer(hwnd: HWND) {
    RENDERER = None;
    let renderer = D2DRenderer::new(hwnd, WINDOW_W as u32, WINDOW_H as u32);
    if let Some(mut r) = renderer {
        r.render();
        RENDERER = Some(r);
    }
}

/// Render the current frame; if the GPU device was lost, rebuild the whole
/// D3D/DXGI/DComposition pipeline once and retry. Present returns
/// DXGI_ERROR_DEVICE_REMOVED/RESET after a driver swap, so this is the
/// recovery path for the "window blank but clickable" state.
unsafe fn render_or_rebuild(hwnd: HWND) {
    if let Some(renderer) = RENDERER.as_mut() {
        if renderer.render() {
            // Device lost — drop everything and recreate it fresh.
            rebuild_renderer(hwnd);
        }
    }
}

fn d2d_color(c: COLORREF, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((c.0 & 0xff) as f32) / 255.0,
        g: (((c.0 >> 8) & 0xff) as f32) / 255.0,
        b: (((c.0 >> 16) & 0xff) as f32) / 255.0,
        a: alpha,
    }
}

fn create_text_format(font_name: &[u16], size: f32, align: DWRITE_TEXT_ALIGNMENT) -> Option<IDWriteTextFormat> {
    unsafe {
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let format = dw
            .CreateTextFormat(
                windows::core::PCWSTR(font_name.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                windows::core::PCWSTR([0u16].as_ptr()),
            )
            .ok()?;
        // Vertical centering within the layout rect, and never wrap — text
        // that overflows just clips instead of breaking onto a second line.
        let _ = format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let _ = format.SetTextAlignment(align);
        Some(format)
    }
}

// ─── Theme detection ─────────────────────────────────────────

/// Read HKCU\...\Themes\Personalize\SystemUsesLightTheme.
/// Returns true when the system (taskbar) uses the light theme.
fn system_light_theme() -> bool {
    unsafe {
        let mut key = HKEY::default();
        let subkey = windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, subkey, 0, KEY_READ, &mut key);
        if r != ERROR_SUCCESS { return false; }
        let name = windows::core::w!("SystemUsesLightTheme");
        let mut data: u32 = 0;
        let mut size: u32 = std::mem::size_of::<u32>() as u32;
        let r2 = RegQueryValueExW(key, name, None, None, Some(&mut data as *mut u32 as *mut u8), Some(&mut size));
        let _ = RegCloseKey(key);
        if r2 != ERROR_SUCCESS { return false; }
        data != 0
    }
}

// ─── Speed formatting ──────────────────────────────────────────

/// "渐变温度计"配色：速度 → 颜色，log 尺度从青蓝→绿→黄→橙红平滑过渡。
/// 0 速度由调用方保留 idle 灰；速度越高色温越暖。深浅主题各一套锚点。
fn speed_color(speed: f64) -> COLORREF {
    speed_color_for(speed, unsafe { LIGHT_THEME })
}

/// speed_color with an explicit theme — the hover panel sits on a WHITE card
/// regardless of the system theme, so it must use the light (deep) anchors.
fn speed_color_for(speed: f64, light: bool) -> COLORREF {
    let bands: [(u8, u8, u8); 4] = if light {
        [(40, 120, 190), (40, 150, 90), (190, 150, 50), (210, 80, 50)]
    } else {
        [(120, 210, 255), (120, 235, 160), (240, 210, 100), (255, 130, 90)]
    };
    // log10: 100 B/s≈2.0 → 100 MB/s≈8.0
    let t = ((speed.max(1.0).log10() - 2.0) / 6.0).clamp(0.0, 1.0);
    let seg = t * 3.0;
    let i = (seg as usize).min(2);
    let f = seg - i as f64;
    let lerp = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * f).round() as u8;
    let (r1, g1, b1) = bands[i];
    let (r2, g2, b2) = bands[i + 1];
    COLORREF(((lerp(b1, b2) as u32) << 16) | ((lerp(g1, g2) as u32) << 8) | lerp(r1, r2) as u32)
}

fn fmt_speed(s: f64) -> (String, String) {
    // Unified format: always 1 decimal so digits look consistent across units
    if s < 1024.0 { (format!("{:.1}", s), "B/s".to_string()) }
    else if s < 1048576.0 { (format!("{:.1}", s / 1024.0), "K/s".to_string()) }
    else if s < 1073741824.0 { (format!("{:.1}", s / 1048576.0), "M/s".to_string()) }
    else { (format!("{:.1}", s / 1073741824.0), "G/s".to_string()) }
}

/// "网络心情" — a quick mood read of the network, driven by latency and
/// current throughput: 😄 flying (low latency + fast), 🙂 smooth,
/// 😐 middling, 😫 struggling/unreachable.
fn net_mood() -> &'static str {
    unsafe {
        let lat = LATENCY_MS;
        let speed = DOWN.max(UP);
        if lat < 0 {
            "😫"
        } else if lat < 50 && speed > 1048576.0 {
            "😄"
        } else if lat < 50 {
            "🙂"
        } else if lat < 150 {
            "😐"
        } else {
            "😫"
        }
    }
}

fn read_cpu_usage(previous: &mut Option<(u64, u64, u64)>) -> f32 {
    let mut idle = windows::Win32::Foundation::FILETIME::default();
    let mut kernel = windows::Win32::Foundation::FILETIME::default();
    let mut user = windows::Win32::Foundation::FILETIME::default();
    unsafe {
        let _ = GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user));
    }

    let to_u64 = |t: windows::Win32::Foundation::FILETIME| {
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    };
    let current = (to_u64(idle), to_u64(kernel), to_u64(user));
    let result = previous.map(|old| {
        let idle_delta = current.0.saturating_sub(old.0);
        let kernel_delta = current.1.saturating_sub(old.1);
        let user_delta = current.2.saturating_sub(old.2);
        let total = kernel_delta.saturating_add(user_delta);
        if total == 0 { 0.0 } else { ((total - idle_delta) as f64 * 100.0 / total as f64) as f32 }
    }).unwrap_or(0.0);
    *previous = Some(current);
    result.clamp(0.0, 100.0)
}

// ─── Window procedure ─────────────────────────────────────────

// WM_MOUSELEAVE is defined in the Win32_UI_Controls module in windows 0.57
// (not WindowsAndMessaging); we don't need the whole Controls feature just
// for one message id, so define it locally.
const WM_MOUSELEAVE: u32 = 0x02A3;
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            // Data polling lives in net_thread; position is reasserted by
            // REPOS_TIMER. REFRESH_TIMER (100ms) used to call reposition()
            // every tick — that hammered the taskbar and caused the periodic
            // full-window flicker, so it is gone.
            SetTimer(hwnd, PAINT_TIMER, 500, None);
            SetTimer(hwnd, REPOS_TIMER, 1000, None);
            // Watch TaskbarDa (widgets toggle) — reposition immediately when
            // it changes instead of waiting for the next REPOS tick.
            watch_taskbar_da(hwnd);
            LRESULT(0)
        }
        WM_APP_REPOS => {
            unsafe { reposition(hwnd); }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == PAINT_TIMER => {
            // Render via D2D → swapchain → DirectComposition (the only path
            // the XAML taskbar composites). Only when data actually changed —
            // every Present+Commit on a DComposition child forces a reblend
            // of the surface, which flickers when numbers are static.
            if unsafe { DIRTY } {
                render_or_rebuild(hwnd);
                unsafe { DIRTY = false; }
            }
            // Refresh the hover panel while it is visible (values move).
            if unsafe { PANEL_VISIBLE } {
                let _ = InvalidateRect(unsafe { PANEL_HWND }, None, true);
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == REPOS_TIMER => {
            // Reassert position/z-order periodically (taskbar can be
            // re-laid-out by Explorer). When already embedded AND in place,
            // reposition() returns immediately without touching the window —
            // touching a DComposition child (even a GetParent check inside
            // embed_into_taskbar) can trigger a taskbar reblend, the flicker
            // the user still sees occasionally.
            if !unsafe { MENU_OPEN } {
                reposition(hwnd);
            }
            // Theme may have changed — re-check and repaint if needed
            let light = system_light_theme();
            unsafe {
                if light != LIGHT_THEME {
                    LIGHT_THEME = light;
                    DIRTY = true;
                }
            }
            if unsafe { DIRTY } {
                render_or_rebuild(hwnd);
                unsafe { DIRTY = false; }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // Never show the busy/hourglass cursor over this window
            if let Ok(cursor) = LoadCursorW(None, windows::core::PCWSTR(32512 as *const u16)) {
                SetCursor(cursor);
            }
            LRESULT(1)
        }
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            // Arm mouse-leave tracking once per entry, then show the detail
            // panel above the taskbar window.
            if !unsafe { PANEL_TRACKING } {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                unsafe { PANEL_TRACKING = true; }
            }
            show_panel(hwnd);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            unsafe { PANEL_TRACKING = false; }
            hide_panel();
            LRESULT(0)
        }
        // WM_CONTEXTMENU is the "official" context-menu message; keep it in
        // case some input path sends it directly. show_context_menu() is
        // re-entrant-safe (MENU_OPEN guard), so double dispatch can't stack
        // two menus.
        WM_CONTEXTMENU => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            // Commands from the popup menu (ID 1 = 开机自启, ID 2 = 退出,
            // ID 3 = 显示图表).
            let id = (wp.0 as u32) & 0xFFFF;
            if id == 1 {
                if is_autostart() {
                    clear_autostart();
                    // Keep the ini mirror in sync: if we only clear the
                    // registry, is_autostart() would still see the old
                    // AUTOSTART_INI and the toggle could never turn off.
                    unsafe { AUTOSTART_INI = false; }
                } else {
                    ensure_autostart();
                    unsafe { AUTOSTART_INI = true; }
                }
                // Remember the choice in netspeed.ini too, so the checked
                // state survives even where HKCU writes are blocked.
                save_config();
            } else if id == 2 {
                // User-initiated exit: kill the watchdog too, else it would
                // immediately relaunch us.
                unsafe { stop_watchdog(); }
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            } else if id == 3 {
                unsafe {
                    SHOW_GRAPHS = !SHOW_GRAPHS;
                    WINDOW_W = if SHOW_GRAPHS { WINDOW_W_GRAPH } else { WINDOW_W_COMPACT };
                    // Recreate the D2D pipeline for the new width, resize the
                    // window and reposition it (compact mode drops the graph
                    // columns; wide mode restores them).
                    rebuild_renderer(hwnd);
                    reposition(hwnd);
                    DIRTY = true;
                }
                save_config();
            } else if id == 4 {
                unsafe {
                    NET_DETECT = !NET_DETECT;
                    if !NET_DETECT { LATENCY_MS = -1; }
                    DIRTY = true;
                }
                save_config();
            } else if id == 5 {
                // 渐变温度计配色开关
                unsafe {
                    GRADIENT_COLOR = !GRADIENT_COLOR;
                    DIRTY = true;
                }
                save_config();
            } else if id == 6 {
                // 悬停详情面板开关
                unsafe {
                    SHOW_PANEL = !SHOW_PANEL;
                    if !SHOW_PANEL { hide_panel(); }
                }
                save_config();
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            // Content lives in the DComposition visual, not the GDI surface.
            LRESULT(1)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, PAINT_TIMER);
            let _ = KillTimer(hwnd, REPOS_TIMER);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ─── Hover detail panel ───────────────────────────────────────
// A second topmost popup shown while the mouse is over the taskbar window.
// It renders with plain GDI (it is NOT embedded in the taskbar, so the
// DComposition constraint does not apply) and shows speeds, latency/mood,
// CPU/mem, the active NIC and today's traffic totals.
const PANEL_CLASS: windows::core::PCWSTR = windows::core::w!("NetSpeedPanelWnd");

fn show_panel(owner: HWND) {
    unsafe {
        if !SHOW_PANEL || PANEL_HWND.0 == 0 || PANEL_VISIBLE {
            return;
        }
        // Position: centered above the owner window, just above the taskbar.
        let mut wr = RECT::default();
        if GetWindowRect(owner, &mut wr).is_err() {
            return;
        }
        let x = wr.left + (wr.right - wr.left - PANEL_W) / 2;
        let y = wr.top - PANEL_H - 6;
        // Win11 card feel: subtle 100ms fade-in (AW_BLEND). AnimateWindow
        // itself reveals the window from the hidden state; calling it AFTER
        // ShowWindow was observed to leave the panel invisible, so the order
        // is AnimateWindow first, then reposition (position is unaffected by
        // the animation, and SWP_SHOWWINDOW alone was unreliable here).
        // Fallback: AnimateWindow can fail (returns 0) on rapid show/hide
        // cycles — then force ShowWindow so the panel still appears.
        let animated = AnimateWindow(PANEL_HWND, 100, AW_BLEND);
        if animated.is_err() {
            let _ = ShowWindow(PANEL_HWND, SW_SHOWNOACTIVATE);
        }
        SetWindowPos(
            PANEL_HWND,
            HWND_TOPMOST,
            x,
            y,
            PANEL_W,
            PANEL_H,
            SWP_NOACTIVATE,
        )
        .ok();
        // Force a fresh paint (values may have changed while hidden).
        let _ = InvalidateRect(PANEL_HWND, None, true);
        PANEL_VISIBLE = true;
    }
}

fn hide_panel() {
    unsafe {
        if PANEL_HWND.0 == 0 || !PANEL_VISIBLE {
            return;
        }
        let _ = ShowWindow(PANEL_HWND, SW_HIDE);
        PANEL_VISIBLE = false;
    }
}

/// Format an absolute byte count (used by the panel for today's totals).
fn fmt_bytes(b: f64) -> String {
    if b >= 1073741824.0 {
        format!("{:.2}G", b / 1073741824.0)
    } else if b >= 1048576.0 {
        format!("{:.1}M", b / 1048576.0)
    } else if b >= 1024.0 {
        format!("{:.0}K", b / 1024.0)
    } else {
        format!("{:.0}B", b)
    }
}

unsafe extern "system" fn panel_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            // Double-buffer to avoid flicker.
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            // Panel background: Win11 Mica light card (user wants the panel
            // to match Windows 11's look; #F3F3F3 is the light Mica surface).
            let bg = CreateSolidBrush(COLORREF(0x00F3F3F3));
            FillRect(mem, &rc, bg);
            let _ = DeleteObject(bg);
            // Card border (subtle). FrameRect strokes only — Rectangle()
            // would FILL the interior with the border brush, wiping the bg.
            let border = CreateSolidBrush(COLORREF(0x00E0E0E0));
            let _ = FrameRect(mem, &rc, border);
            let _ = DeleteObject(border);

            // Text formatting: Segoe UI 13px; values in white-ish.
            let f = CreateFontW(
                -15,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                CLEARTYPE_QUALITY.0 as u32,
                DEFAULT_PITCH.0 as u32,
                windows::core::w!("Segoe UI Variable"),
            );
            let old_font = SelectObject(mem, f);
            // Panel is a white card: dark text regardless of system theme.
            let text_col = COLORREF(0x00202020);
            let dim_col = COLORREF(0x00808080);

            let (down, up) = (unsafe { DOWN }, unsafe { UP });
            let (dv, du) = fmt_speed(down);
            let (uv, uu) = fmt_speed(up);
            let down_col = if down > 0.0 { speed_color_for(down, true) } else { dim_col };
            let up_col = if up > 0.0 { speed_color_for(up, true) } else { dim_col };
            let cpu = unsafe { CPU_USAGE };
            let mem_usage = unsafe { MEMORY_USAGE };
            let lat = unsafe { LATENCY_MS };
            let mood = net_mood();
            let iface = unsafe { IFACE_NAME.clone() };
            let t_down = fmt_bytes(unsafe { TODAY_DOWN });
            let t_up = fmt_bytes(unsafe { TODAY_UP });

            // Line helper: label (dim) + value (colored/white).
            let line = |y: i32, label: &str, value: &str, value_col: COLORREF| {
                let _ = SetTextColor(mem, dim_col);
                let _ = SetBkMode(mem, TRANSPARENT);
                let lw: Vec<u16> = label.encode_utf16().collect();
                let _ = TextOutW(mem, 10, y, &lw);
                let _ = SetTextColor(mem, value_col);
                let vw: Vec<u16> = value.encode_utf16().collect();
                let _ = TextOutW(mem, 74, y, &vw);
            };

            // Row 1: speeds.
            let _ = SetTextColor(mem, dim_col);
            let _ = SetBkMode(mem, TRANSPARENT);
            let lab: Vec<u16> = "下载".encode_utf16().collect();
            let _ = TextOutW(mem, 10, 10, &lab);
            let _ = SetTextColor(mem, down_col);
            let txt = format!("{} {}", dv.trim(), du);
            let vw: Vec<u16> = txt.encode_utf16().collect();
            let _ = TextOutW(mem, 74, 10, &vw);

            let _ = SetTextColor(mem, dim_col);
            let lab: Vec<u16> = "上传".encode_utf16().collect();
            let _ = TextOutW(mem, 10, 32, &lab);
            let _ = SetTextColor(mem, up_col);
            let txt = format!("{} {}", uv.trim(), uu);
            let vw: Vec<u16> = txt.encode_utf16().collect();
            let _ = TextOutW(mem, 74, 32, &vw);

            // Row 2: latency + mood.
            let lat_txt = if unsafe { NET_DETECT } {
                if lat < 0 {
                    "检测失败".to_string()
                } else {
                    format!("{}ms", lat)
                }
            } else {
                "已关闭".to_string()
            };
            line(54, "延迟", &lat_txt, text_col);
            let _ = SetTextColor(mem, dim_col);
            let lab: Vec<u16> = "心情".encode_utf16().collect();
            let _ = TextOutW(mem, 200, 54, &lab);
            let _ = SetTextColor(mem, text_col);
            let mw: Vec<u16> = mood.encode_utf16().collect();
            let _ = TextOutW(mem, 254, 54, &mw);

            // Row 3: CPU + memory.
            line(76, "CPU", &format!("{:.0}%", cpu), text_col);
            let _ = SetTextColor(mem, dim_col);
            let lab: Vec<u16> = "内存".encode_utf16().collect();
            let _ = TextOutW(mem, 200, 76, &lab);
            let _ = SetTextColor(mem, text_col);
            let txt = format!("{:.0}%", mem_usage);
            let vw: Vec<u16> = txt.encode_utf16().collect();
            let _ = TextOutW(mem, 254, 76, &vw);

            // Row 4: NIC.
            line(98, "网卡", &iface, text_col);

            // Row 5: today's totals.
            let _ = SetTextColor(mem, dim_col);
            let lab: Vec<u16> = "今日".encode_utf16().collect();
            let _ = TextOutW(mem, 10, 120, &lab);
            let _ = SetTextColor(mem, text_col);
            let txt = format!("↓{}  ↑{}", t_down, t_up);
            let vw: Vec<u16> = txt.encode_utf16().collect();
            let _ = TextOutW(mem, 74, 120, &vw);

            SelectObject(mem, old_font);
            let _ = DeleteObject(f);
            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Register + create the hidden detail panel window. Call once from main.
fn create_panel_window() {
    unsafe {
        let wc = WNDCLASSW {
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(panel_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(None).unwrap().into(),
            hIcon: HICON::default(),
            hCursor: HCURSOR::default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: PANEL_CLASS,
        };
        if RegisterClassW(&wc) == 0 {
            let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
            if err != 1410 {
                // 1410 = ERROR_CLASS_ALREADY_EXISTS — fine on re-register.
                return;
            }
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            PANEL_CLASS,
            windows::core::w!("NetSpeed Panel"),
            WS_POPUP,
            0,
            0,
            PANEL_W,
            PANEL_H,
            None,
            None,
            GetModuleHandleW(None).unwrap(),
            None,
        );
        PANEL_HWND = hwnd;
        if hwnd.0 != 0 {
            let _ = ShowWindow(hwnd, SW_HIDE);
            // Win11 look: rounded corners + DWM drop shadow. Corner
            // preference applies to top-level windows regardless of frame;
            // ROUND gives the modern 8px-radius card look and DWM supplies
            // the matching shadow automatically.
            let corner: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
        }
    }
}

// ─── Window positioning ───────────────────────────────────────

/// Watch the widgets-button registry key (TaskbarDa). On change, post
/// WM_APP_REPOS to the main window so it repositions immediately instead of
/// waiting for the next 1s REPOS tick (the "2s delay" the user noticed).
fn watch_taskbar_da(hwnd: HWND) {
    std::thread::spawn(move || unsafe {
        use windows::core::w;
        use windows::Win32::Foundation as f;
        use windows::Win32::System::Registry as reg;
        use windows::Win32::System::Threading as th;
        use windows::Win32::UI::WindowsAndMessaging as wm;
        loop {
            let mut hkey = reg::HKEY::default();
            let r = reg::RegOpenKeyExW(
                reg::HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
                0,
                reg::KEY_NOTIFY,
                &mut hkey,
            );
            if r.is_err() { return; }
            // Manual-reset event: RegNotifyChangeKeyValue signals it on change.
            let ev = match th::CreateEventW(None, true, false, None) {
                Ok(h) => h,
                Err(_) => { let _ = reg::RegCloseKey(hkey); return; }
            };
            if ev.0 == 0 { let _ = reg::RegCloseKey(hkey); return; }
            let filter = reg::REG_NOTIFY_CHANGE_LAST_SET;
            let r = reg::RegNotifyChangeKeyValue(
                hkey,
                false,
                filter,
                ev,
                false,
            );
            if r.0 != 0 { let _ = f::CloseHandle(ev); let _ = reg::RegCloseKey(hkey); return; }
            // Wait for the change (60s timeout as a safety valve).
            let _ = th::WaitForSingleObject(ev, 60000);
            let _ = f::CloseHandle(ev);
            let _ = reg::RegCloseKey(hkey);
            let _ = wm::PostMessageW(hwnd, WM_APP_REPOS, f::WPARAM(0), f::LPARAM(0));
            // Small debounce so Explorer's batch writes don't spam us.
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    });
}

/// Whether the Win11 widgets button (weather entry) is enabled. Mirrors
/// TrafficMonitor's CWindowsSettingHelper::IsTaskbarWidgetsBtnShown():
/// reads HKCU\...\Explorer\Advanced\TaskbarDa (missing/0 = off).
fn widgets_button_shown() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry as reg;
    unsafe {
        let mut hkey = reg::HKEY::default();
        let r = reg::RegOpenKeyExW(
            reg::HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
            0,
            reg::KEY_READ,
            &mut hkey,
        );
        if r.is_ok() {
            let mut data: u32 = 0;
            let mut size: u32 = std::mem::size_of::<u32>() as u32;
            let rr = reg::RegQueryValueExW(
                hkey,
                w!("TaskbarDa"),
                None,
                None,
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut size),
            );
            let _ = reg::RegCloseKey(hkey);
            rr.is_ok() && data != 0
        } else {
            false
        }
    }
}

/// Detect taskbar type and set TASKBAR_WIN11 / WINDOW_H accordingly.
/// Win11 (XAML taskbar): Shell_TrayWnd hosts DesktopWindowContentBridge /
/// CoreWindow (the XAML compositor surface) — the reliable marker. Note the
/// 25H2 taskbar ALSO keeps a ReBarWindow32 compatibility layer, so "has
/// ReBar" is NOT a Win10 signal; only the XAML compositor bridge is.
/// Win10 (classic taskbar): no XAML bridge; ReBarWindow32 + MSTaskSwWClass.
unsafe fn detect_taskbar_type() {
    let tb = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
    if tb.0 == 0 { return; }
    // Win11: XAML compositor bridge present (any depth is fine — direct child
    // in 25H2, nested elsewhere in other builds).
    let xaml = FindWindowExW(tb, None, windows::core::w!("Windows.UI.Composition.DesktopWindowContentBridge"), None);
    let mut win11 = xaml.0 != 0;
    if !win11 {
        let cw = FindWindowExW(tb, None, windows::core::w!("Windows.UI.Core.CoreWindow"), None);
        win11 = cw.0 != 0;
    }
    unsafe {
        TASKBAR_WIN11 = win11;
        // Height: taskbar height minus small margins.
        let mut tbr = RECT::default();
        if GetWindowRect(tb, &mut tbr).is_ok() {
            let h = tbr.bottom - tbr.top;
            // Values and trend graph are side-by-side now, so keep the window
            // compact (Win11 42px / Win10 36px).
            WINDOW_H = if win11 { (h - 6).min(42) } else { (h - 4).min(36) };
            if WINDOW_H < 20 { WINDOW_H = 20; }
        }
        dump_taskbar_diag(tb, &tbr);
    }
}

/// Write a one-shot diagnostic file describing the taskbar layout. Only
/// active while NSPEED_DIAG=1 is set in the environment — avoids shipping
/// debug output in normal runs.
unsafe fn dump_taskbar_diag(tb: HWND, tbr: &RECT) {
    let diag = std::env::var("NSPEED_DIAG").is_ok();
    if !diag { return; }
    use std::io::Write;
    let mut s = String::new();
    s.push_str(&format!("TASKBAR_WIN11={}\n", unsafe { TASKBAR_WIN11 }));
    s.push_str(&format!("taskbar rect=({},{},{},{})\n", tbr.left, tbr.top, tbr.right, tbr.bottom));
    let push_rect = |name: &str, h: HWND| -> String {
        let mut r = RECT::default();
        let ok = GetWindowRect(h, &mut r).is_ok();
        format!("{} hwnd={} ok={} rect=({},{},{},{})\n",
            name, h.0, ok, r.left, r.top, r.right, r.bottom)
    };
    s.push_str(&push_rect("ReBarWindow32", FindWindowExW(tb, None, windows::core::w!("ReBarWindow32"), None)));
    s.push_str(&push_rect("TrayNotifyWnd", FindWindowExW(tb, None, windows::core::w!("TrayNotifyWnd"), None)));
    s.push_str(&push_rect("XamlBridge", FindWindowExW(tb, None, windows::core::w!("Windows.UI.Composition.DesktopWindowContentBridge"), None)));
    s.push_str(&push_rect("CoreWindow", FindWindowExW(tb, None, windows::core::w!("Windows.UI.Core.CoreWindow"), None)));
    let bar = FindWindowExW(tb, None, windows::core::w!("ReBarWindow32"), None);
    s.push_str(&push_rect("MSTaskSwWClass", FindWindowExW(bar, None, windows::core::w!("MSTaskSwWClass"), None)));
    s.push_str(&push_rect("MSTaskListWClass", FindWindowExW(bar, None, windows::core::w!("MSTaskListWClass"), None)));
    // Widgets button (Win11) — the "小组件" button sits right of Start.
    s.push_str(&push_rect("WidgetsBtn", find_taskbar_child(tb, &["Widgets", "WidgetButton", "XamlExplorerHostIslandWindow", "Windows.UI.Composition.DesktopWindowContentBridge"])));
    // Our own window position — must search children (we're embedded in the
    // taskbar, so FindWindowW at top level misses us).
    let mut own = HWND(0);
    unsafe extern "system" fn own_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut cls = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd, &mut cls) };
        if n > 0 {
            let cs = String::from_utf16_lossy(&cls[..n as usize]);
            if cs == "NetSpeedTaskbarWnd" {
                unsafe { *(lparam.0 as *mut HWND) = hwnd; }
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    unsafe { let _ = EnumChildWindows(tb, Some(own_cb), LPARAM(&mut own as *mut HWND as isize)); }
    let mut wr = RECT::default();
    let wok = own.0 != 0 && GetWindowRect(own, &mut wr).is_ok();
    s.push_str(&format!("OurWindow hwnd={} ok={} rect=({},{},{},{}) w={} h={}\n",
        own.0, wok, wr.left, wr.top, wr.right, wr.bottom, wr.right - wr.left, wr.bottom - wr.top));
    if let Ok(mut f) = std::fs::File::create(r"C:\Users\Administrator\netspeed_diag.txt") {
        let _ = f.write_all(s.as_bytes());
    }
}

/// SetParent to Shell_TrayWnd but KEEP the WS_POPUP (top-level) style — do
/// NOT force WS_CHILD. Why this combination works for both problems:
///  • z-order: as a child of the taskbar, our window lives in the taskbar's
///    z-order, so TrackPopupMenu's #32768 menu floats ABOVE us (a standalone
///    HWND_TOPMOST window would cover the menu).
///  • menu dismissal: WS_POPUP windows (even parented) are top-level class
///    windows, so SetForegroundWindow works — TrackPopupMenu needs the owner
///    to be foreground, otherwise outside clicks won't dismiss the menu
///    (MSDN). A WS_CHILD window can never be foreground.
/// TrafficMonitor's taskbar dialog does exactly this (SetParent + popup).
unsafe fn embed_into_taskbar(hwnd: HWND) -> bool {
    if unsafe { EMBEDDED } {
        // Explorer may have restarted: re-parent if needed. Use GetAncestor:
        // GetParent() returns 0 for WS_POPUP windows even when SetParent was
        // used (this window deliberately keeps the popup style).
        let tb = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
        if tb.0 != 0 && GetAncestor(hwnd, GA_PARENT) == tb {
            return true;
        }
        unsafe { EMBEDDED = false; }
    }
    // Parent is always Shell_TrayWnd. (Win10's classic taskbar also accepts
    // this; anchoring differs via compute_target_pos, not the parent.)
    let tb = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
    if tb.0 == 0 { return false; }
    let r = SetParent(hwnd, tb);
    if r.0 == 0 { return false; }
    // Do NOT rewrite the style to WS_CHILD here (see comment above).
    unsafe { EMBEDDED = true; }
    true
}

/// Park the window on the taskbar. Win11: child of Shell_TrayWnd, anchored
/// to the LEFT of TrayNotifyWnd (right side). Win10: child of ReBarWindow32,
/// anchored to the RIGHT of the start button / task list. Both use
/// parent-client coordinates; HWND_TOP, never HWND_TOPMOST (a child can't be
/// topmost and topmost would cover the popup menu).
unsafe fn reposition(hwnd: HWND) {
    // Fast path: already embedded and already in place → do NOTHING. Any
    // system call here (FindWindowW/GetParent/GetWindowRect) on a
    // DComposition child of the taskbar can trigger a taskbar reblend,
    // which the user sees as an occasional flicker.
    if unsafe { EMBEDDED } {
        let mut cr = RECT::default();
        if GetWindowRect(hwnd, &mut cr).is_ok() {
            let tb = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
            if tb.0 != 0 {
                let mut tbr = RECT::default();
                GetWindowRect(tb, &mut tbr).ok();
                if tbr.right - tbr.left > 0 {
                    // Explorer may have restarted (lock screen / crash /
                    // update) — the taskbar HWND changes. If our window is
                    // still parented to the OLD (now destroyed) taskbar we
                    // are invisible forever despite the geometry matching:
                    // the "in place" shortcut below would keep returning
                    // without ever re-embedding. Verify the parent FIRST.
                    // NOTE: GetParent() returns 0 for WS_POPUP windows even
                    // when SetParent was used — must use GetAncestor
                    // (GA_PARENT) which reports the real parent.
                    if GetAncestor(hwnd, GA_PARENT) != tb {
                        unsafe { EMBEDDED = false; }
                    } else {
                        match compute_target_pos(tb, &tbr) {
                            Some((target_x, target_y)) => {
                                if cr.left == target_x && cr.top == target_y
                                    && (cr.right - cr.left) == WINDOW_W
                                    && (cr.bottom - cr.top) == WINDOW_H
                                {
                                    return; // in place — no calls beyond the check above
                                }
                                // anchor ok but window is elsewhere → reposition
                                // below (do NOT hold — a window stuck mid-taskbar
                                // would otherwise never return to the tray side)
                            }
                            None => {
                                // Anchor temporarily unreliable (taskbar re-layout).
                                // Hold ONLY if we're already at the estimated tray
                                // position (right edge - 280, TrafficMonitor's
                                // tray+clock fallback). If we're anywhere else
                                // (e.g. mid-taskbar after a reorder), fall through
                                // and let the rescue move put us back.
                                let est = tbr.right - WINDOW_W - 280;
                                let close = (cr.left - est).abs() <= 40
                                    && (cr.right - cr.left) == WINDOW_W
                                    && (cr.bottom - cr.top) == WINDOW_H;
                                if close { return; } // fine — hold
                                unsafe { EMBEDDED = false; }
                            }
                        }
                    }
                }
            }
        }
    }
    let tb = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
    if tb.0 == 0 { return; }
    let mut tbr = RECT::default();
    GetWindowRect(tb, &mut tbr).ok();
    if tbr.right - tbr.left <= 0 { return; }

    let embedded = embed_into_taskbar(hwnd);
    let (target_x, target_y) = match compute_target_pos(tb, &tbr) {
        Some(v) => v,
        None => {
            // Anchor unreliable (taskbar re-layout). Go back to the last
            // successfully applied position — NOT a guessed spot (guessing
            // landed us mid-taskbar over widgets, or on the tray). If we
            // never had one, fall back to the tray-estimate (right edge
            // minus a generous tray+clock width) to at least stay on the
            // right side.
            let lx = unsafe { LAST_X };
            let ly = unsafe { LAST_Y };
            if lx > 0 && ly > 0 {
                (lx, ly)
            } else {
                let y = tbr.top + (tbr.bottom - tbr.top - WINDOW_H) / 2;
                (tbr.right - WINDOW_W - 280, y)
            }
        }
    };
    if target_x < 0 || target_x + WINDOW_W > tbr.right { return; }
    if target_y < 0 || target_y + WINDOW_H > tbr.bottom { return; }

    if embedded {
        // Child window: SetWindowPos uses parent-client coordinates.
        // Win11 parent = Shell_TrayWnd (no client offset); Win10 parent =
        // ReBarWindow32 — its client origin == taskbar origin at 100% DPI,
        // so screen coords still work (TrafficMonitor does the same).
        let child_x = target_x - tbr.left;
        let child_y = target_y - tbr.top;
        // Skip SetWindowPos when already in place — every SetWindowPos on a
        // DComposition child flashes the surface (the observed flicker).
        let mut cr = RECT::default();
        if GetWindowRect(hwnd, &mut cr).is_ok()
            && cr.left == target_x && cr.top == target_y
            && (cr.right - cr.left) == WINDOW_W
            && (cr.bottom - cr.top) == WINDOW_H
        {
            return;
        }
        SetWindowPos(hwnd, HWND_TOP, child_x, child_y, WINDOW_W, WINDOW_H,
                     SWP_NOACTIVATE | SWP_SHOWWINDOW).ok();
        unsafe { LAST_X = target_x; LAST_Y = target_y; }
    } else {
        // Fallback: absolute screen position + topmost (embedding failed).
        let mut cr = RECT::default();
        if GetWindowRect(hwnd, &mut cr).is_ok()
            && cr.left == target_x && cr.top == target_y
            && (cr.right - cr.left) == WINDOW_W
            && (cr.bottom - cr.top) == WINDOW_H
        {
            return;
        }
        SetWindowPos(hwnd, HWND_TOPMOST, target_x, target_y, WINDOW_W, WINDOW_H,
                     SWP_NOACTIVATE | SWP_SHOWWINDOW).ok();
        unsafe { LAST_X = target_x; LAST_Y = target_y; }
    }
}

/// Compute the desired SCREEN position of our window on the taskbar.
/// Win11: immediately left of TrayNotifyWnd (right-aligned area).
/// Win10: immediately right of the start button / task list (left-aligned).
/// Returns None when the anchor is unreliable (e.g. during a taskbar re-layout
/// the tray window can briefly be missing) — the caller must then NOT move
/// the window, instead of guessing a spot that overlaps the tray/widgets.
unsafe fn compute_target_pos(tb: HWND, tbr: &RECT) -> Option<(i32, i32)> {
    let y = tbr.top + (tbr.bottom - tbr.top - WINDOW_H) / 2;
    if unsafe { TASKBAR_WIN11 } {
        let tn = FindWindowExW(tb, None, windows::core::w!("TrayNotifyWnd"), None);
        let mut tnr = RECT::default();
        let has_tray = tn.0 != 0 && GetWindowRect(tn, &mut tnr).is_ok() && tnr.left > 0;
        if !has_tray { return None; } // anchor gone — hold position
        // The Win11 weather icon (widgets entry) is XAML-rendered just LEFT
        // of the tray and is invisible to GDI enumeration. When the widgets
        // button is enabled it occupies a band (measured ≈240px on 150% DPI)
        // that our window would cover. TrafficMonitor solves this with
        // taskbar_left_space_win11 (default 160); we use a slightly larger
        // reserve so the weather text (wider than the bare icon) also clears.
        let widgets_on = widgets_button_shown();
        let gap = if widgets_on { 250 } else { 6 };
        Some((tnr.left - WINDOW_W - gap, y))
    } else {
        // Win10: anchor = start button / task list (MSTaskSwWClass or
        // MSTaskListWClass). These live inside ReBarWindow32 as DIRECT
        // children on Win7/8, but some Win10 builds nest them deeper — so
        // search the whole subtree, not just direct children.
        let bar = FindWindowExW(tb, None, windows::core::w!("ReBarWindow32"), None);
        let root = if bar.0 != 0 { bar } else { tb };
        let start = find_taskbar_child(root, &["MSTaskSwWClass", "MSTaskListWClass"]);
        // Tray (clock/icons) left edge — we must NEVER overlap it. The task
        // list grows rightward when the "news and interests" flyout button is
        // turned off; anchoring at task-list.right would then push us onto
        // the clock. Clamp: min(task-list right, tray left - width - gap).
        let tn = FindWindowExW(tb, None, windows::core::w!("TrayNotifyWnd"), None);
        let mut tnr = RECT::default();
        let tray_left = tn.0 != 0 && GetWindowRect(tn, &mut tnr).is_ok() && tnr.left > 0;
        if !tray_left { return None; } // anchor gone — hold position
        let clamp_x = tnr.left - WINDOW_W - 6;
        let mut sr = RECT::default();
        if start.0 != 0 && GetWindowRect(start, &mut sr).is_ok() && sr.right > sr.left && sr.right > 0 {
            // Right of the task list. Then avoid any other window that sits
            // between the task list and the tray (e.g. the "news and
            // interests" button): walk all direct children of the taskbar /
            // ReBar and clamp to the left of the nearest one.
            let mut x = (sr.right + 6).min(clamp_x).max(tbr.left + 2);
            let probe = |s: &mut i32, root: HWND| {
                let mut child = GetWindow(root, GW_CHILD);
                while child.0 != 0 {
                    let mut cls = [0u16; 128];
                    let n = GetClassNameW(child, &mut cls);
                    if n > 0 {
                        let cs = String::from_utf16_lossy(&cls[..n as usize]);
                        let ours = cs == "NetSpeedTaskbarWnd";
                        let tasklist = cs == "MSTaskSwWClass" || cs == "MSTaskListWClass";
                        if !ours && !tasklist {
                            let mut cr = RECT::default();
                            if GetWindowRect(child, &mut cr).is_ok() && cr.right > cr.left
                                && cr.right > sr.right && cr.left < clamp_x + WINDOW_W
                            {
                                // This window occupies space between task list
                                // and tray — keep left of it (take the
                                // LEFTMOST such window).
                                let avoid = cr.left - WINDOW_W - 6;
                                if avoid < *s { *s = avoid.max(tbr.left + 2); }
                            }
                        }
                    }
                    child = GetWindow(child, GW_HWNDNEXT);
                }
            };
            probe(&mut x, tb);
            let bar = FindWindowExW(tb, None, windows::core::w!("ReBarWindow32"), None);
            if bar.0 != 0 { probe(&mut x, bar); }
            Some((x, y))
        } else {
            // Task list not found (rare on Win10) — hold position rather than
            // guessing.
            None
        }
    }
}

/// Recursively find the first descendant window whose class name is one of
/// `names`. Uses EnumChildWindows (all depths), unlike FindWindowEx which
/// only searches direct children.
unsafe fn find_taskbar_child(root: HWND, names: &[&str]) -> HWND {
    static mut FOUND: HWND = HWND(0);
    unsafe { FOUND = HWND(0); }
    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let names_ptr = lparam.0 as *const &[&str];
        let names = unsafe { &*names_ptr };
        let mut cls = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd, &mut cls) };
        if n > 0 {
            let s = String::from_utf16_lossy(&cls[..n as usize]);
            if names.iter().any(|want| s == *want) {
                unsafe { FOUND = hwnd; }
                return BOOL(0); // stop
            }
        }
        BOOL(1)
    }
    let names_box: &[&str] = names;
    let _ = unsafe { EnumChildWindows(root, Some(enum_cb), LPARAM(&names_box as *const _ as isize)) };
    unsafe { FOUND }
}

// ─── Autostart (registry Run key) ─────────────────────────────

const RUN_KEY: windows::core::PCWSTR = windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// Read the Run\\NetSpeed value directly via the Win32 registry API.
/// Never spawn a child process from the UI thread: `reg.exe` launches a
/// console host that can stall (esp. over RDP) and breaks TrackPopupMenu
/// (menu never appears / wndproc deadlocks).
fn read_autostart() -> Option<String> {
    unsafe {
        let mut key = HKEY::default();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_READ, &mut key);
        if r != ERROR_SUCCESS { return None; }
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        let r2 = RegQueryValueExW(
            key,
            windows::core::w!("NetSpeed"),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        if r2 != ERROR_SUCCESS { return None; }
        let len = (size as usize) / 2;
        Some(String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string())
    }
}

fn is_autostart() -> bool {
    // Registry is the source of truth; fall back to the ini-remembered value
    // when the registry read fails (restricted environments).
    read_autostart().is_some() || unsafe { AUTOSTART_INI }
}

fn ensure_autostart() {
    let exe = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return,
    };
    unsafe {
        let mut key = HKEY::default();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_WRITE, &mut key);
        if r != ERROR_SUCCESS { return; }
        let mut data: Vec<u16> = exe.encode_utf16().collect();
        data.push(0);
        let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let _ = RegSetValueExW(key, windows::core::w!("NetSpeed"), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
    }
}

fn clear_autostart() {
    unsafe {
        let mut key = HKEY::default();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_WRITE, &mut key);
        if r != ERROR_SUCCESS { return; }
        let _ = RegDeleteValueW(key, windows::core::w!("NetSpeed"));
        let _ = RegCloseKey(key);
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    // Re-entrancy guard: if a menu is already open (WM_RBUTTONUP then
    // WM_CONTEXTMENU for one physical click), don't stack a second one.
    if unsafe { MENU_OPEN } { return; }
    // Pause repositioning while the menu is open: the REPOS timer's
    // SetWindowPos would otherwise flash our window over the menu.
    unsafe { MENU_OPEN = true; }
    let menu = match CreatePopupMenu() {
        Ok(m) => m,
        Err(_) => { unsafe { MENU_OPEN = false; } return; },
    };
    let autostart_on = is_autostart();
    let flags = if autostart_on { MF_STRING | MF_CHECKED } else { MF_STRING | MF_UNCHECKED };
    let _ = AppendMenuW(menu, flags, 1, windows::core::w!("开机自启"));
    let gflags = if SHOW_GRAPHS { MF_STRING | MF_CHECKED } else { MF_STRING | MF_UNCHECKED };
    let _ = AppendMenuW(menu, gflags, 3, windows::core::w!("显示图表"));
    let nflags = if NET_DETECT { MF_STRING | MF_CHECKED } else { MF_STRING | MF_UNCHECKED };
    let _ = AppendMenuW(menu, nflags, 4, windows::core::w!("网络延迟检测"));
    let gflags2 = if GRADIENT_COLOR { MF_STRING | MF_CHECKED } else { MF_STRING | MF_UNCHECKED };
    let _ = AppendMenuW(menu, gflags2, 5, windows::core::w!("渐变配色"));
    let pflags = if SHOW_PANEL { MF_STRING | MF_CHECKED } else { MF_STRING | MF_UNCHECKED };
    let _ = AppendMenuW(menu, pflags, 6, windows::core::w!("悬停详情面板"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, 2, windows::core::w!("退出"));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // MSDN (TrackPopupMenu): the owning window must be the foreground window
    // when the menu is shown, otherwise outside clicks won't dismiss it.
    // Our window is WS_POPUP (top-level style) even while parented to
    // Shell_TrayWnd, so SetForegroundWindow works.
    let _ = SetForegroundWindow(hwnd);
    // Auto-close after 6s (user request): a background thread sends ESC to
    // the popup menu's window, which makes TrackPopupMenu's modal loop exit
    // (same as the user pressing Esc). Guarded by MENU_OPEN so a menu that
    // was already closed by a click isn't touched.
    std::thread::spawn(|| unsafe {
        std::thread::sleep(Duration::from_secs(6));
        if MENU_OPEN {
            let m = FindWindowW(windows::core::w!("#32768"), None);
            if m.0 != 0 {
                let _ = PostMessageW(m, WM_KEYDOWN, WPARAM(0x1B), LPARAM(0)); // VK_ESCAPE
            }
        }
    });
    // TrafficMonitor 原样：TPM_LEFTALIGN | TPM_RIGHTBUTTON，无 TPM_RETURNCMD
    //（命令经 WM_COMMAND 回传）。
    let cmd = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_RIGHTBUTTON,
        pt.x, pt.y,
        0, hwnd, None,
    );
    let _ = DestroyMenu(menu);
    let _ = cmd; // command handled via WM_COMMAND
    unsafe { MENU_OPEN = false; }
}

// ─── Persisted toggles (netspeed.ini next to the exe) ─────────
// SHOW_GRAPHS and NET_DETECT survive restarts. The file is tiny and only
// written when a toggle changes, so IO cost is negligible.

fn config_path() -> Option<std::path::PathBuf> {
    unsafe {
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(GetModuleHandleW(None).unwrap(), &mut buf);
        if n == 0 || n >= buf.len() as u32 { return None; }
        let mut p = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]));
        p.set_extension("ini");
        Some(p)
    }
}

fn load_config() {
    let Some(p) = config_path() else { return };
    let Ok(s) = std::fs::read_to_string(&p) else { return };
    for line in s.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("show_graphs=") {
            unsafe { SHOW_GRAPHS = v == "1"; }
        } else if let Some(v) = line.strip_prefix("net_detect=") {
            unsafe { NET_DETECT = v == "1"; }
        } else if let Some(v) = line.strip_prefix("autostart=") {
            unsafe { AUTOSTART_INI = v == "1"; }
        } else if let Some(v) = line.strip_prefix("gradient=") {
            unsafe { GRADIENT_COLOR = v == "1"; }
        } else if let Some(v) = line.strip_prefix("panel=") {
            unsafe { SHOW_PANEL = v == "1"; }
        }
    }
}

fn save_config() {
    let Some(p) = config_path() else { return };
    let s = format!(
        "show_graphs={}\nnet_detect={}\nautostart={}\ngradient={}\npanel={}\n",
        unsafe { SHOW_GRAPHS as u8 },
        unsafe { NET_DETECT as u8 },
        is_autostart() as u8,
        unsafe { GRADIENT_COLOR as u8 },
        unsafe { SHOW_PANEL as u8 },
    );
    let _ = std::fs::write(p, s);
}

// ─── Network polling thread ────────────────────────────────────

/// Ping the target (Ali DNS 223.5.5.5) every 2s via IcmpSendEcho and update
/// LATENCY_MS (-1 = unreachable). Runs only while NET_DETECT is enabled.
fn latency_thread() {
    use windows::Win32::NetworkManagement::IpHelper::*;
    std::thread::spawn(|| unsafe {
        let h = match IcmpCreateFile() {
            Ok(h) => h,
            Err(_) => return,
        };
        // 223.5.5.5 in network byte order (u32 little-endian memory layout).
        let target: u32 = u32::from_le_bytes([223, 5, 5, 5]);
        loop {
            if NET_DETECT {
                let mut reply = [0u8; 64]; // ICMP_ECHO_REPLY sized buffer
                let sent = IcmpSendEcho(
                    h,
                    target,
                    std::ptr::null(),
                    0,
                    None,
                    reply.as_mut_ptr() as *mut core::ffi::c_void,
                    reply.len() as u32,
                    2000, // timeout ms
                );
                if sent > 0 {
                    // ICMP_ECHO_REPLY (x64 layout):
                    //   [0-3] Address, [4-7] Status (0=IP_SUCCESS),
                    //   [8-11] RoundTripTime (ms)
                    let status = u32::from_ne_bytes([reply[4], reply[5], reply[6], reply[7]]);
                    if status == 0 {
                        let rtt = u32::from_ne_bytes([reply[8], reply[9], reply[10], reply[11]]);
                        LATENCY_MS = rtt as i32;
                    } else {
                        LATENCY_MS = -1;
                    }
                } else {
                    LATENCY_MS = -1;
                }
                DIRTY = true;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        // NOTE: handle intentionally leaked — this thread never exits; the
        // OS reclaims the handle on process exit.
    });
}

fn net_thread() {
    use sysinfo::{Networks, System};
    let mut net = Networks::new_with_refreshed_list();
    let mut system = System::new();
    system.refresh_memory();
    let mut cpu_previous = None;
    read_cpu_usage(&mut cpu_previous);
    std::thread::sleep(Duration::from_millis(500));
    net.refresh(false);
    let mut iface = String::new();
    let mut pt = Instant::now();
    let mut last_r: u64 = 0;
    let mut last_s: u64 = 0;

    // Initial selection: pick the interface with the largest TOTAL traffic,
    // then LOCK it. Switching between interfaces (e.g. a VPN/proxy virtual
    // NIC vs the physical NIC, both active) resets the counters and shows
    // 0 — the "frequent zero" bug. Only re-pick if the chosen one vanishes.
    {
        let mut best = ""; let mut best_t = 0u64;
        for (n, d) in net.iter() {
            let t = d.total_received() + d.total_transmitted();
            if t > best_t { best_t = t; best = n; }
        }
        if !best.is_empty() {
            iface = best.to_string();
            if let Some(d) = net.get(&iface) {
                last_r = d.total_received();
                last_s = d.total_transmitted();
            }
        }
    }

    loop {
        std::thread::sleep(Duration::from_millis(500));
        net.refresh(false);
        let cpu_usage = read_cpu_usage(&mut cpu_previous);
        system.refresh_memory();
        let memory_usage = if system.total_memory() > 0 {
            system.used_memory() as f32 * 100.0 / system.total_memory() as f32
        } else {
            0.0
        };
        let now = Instant::now();
        let el = (now - pt).as_secs_f64();
        if el <= 0.0 { continue; }
        let mut changed = false;

        if !iface.is_empty() {
            if let Some(d) = net.get(&iface) {
                // Use TOTAL counters (monotonic) and compute the delta
                // ourselves. sysinfo's received()/transmitted() are deltas
                // maintained internally; on VMs the refresh can occasionally
                // skip an interface (GetIfTable2 flakiness), which makes
                // those internal deltas 0 → the "frequent zero" symptom.
                // Total counters only jump forward, never silently reset.
                let tr = d.total_received();
                let ts = d.total_transmitted();
                let nd = if tr >= last_r { (tr - last_r) as f64 / el } else { 0.0 };
                let nu = if ts >= last_s { (ts - last_s) as f64 / el } else { 0.0 };
                last_r = tr;
                last_s = ts;
                unsafe {
                    changed |= (nd - DOWN).abs() >= 0.15 || (nu - UP).abs() >= 0.15;
                    DOWN = nd; UP = nu;
                    // Today's traffic totals (bytes), reset at local midnight.
                    // Used by the hover detail panel.
                    IFACE_NAME = iface.clone();
                    let st = GetLocalTime();
                    let ymd = (st.wYear as i32) * 10000 + (st.wMonth as i32) * 100 + st.wDay as i32;
                    if TODAY_DATE != ymd {
                        TODAY_DATE = ymd;
                        TODAY_DOWN = 0.0;
                        TODAY_UP = 0.0;
                    }
                    TODAY_DOWN += nd * el;
                    TODAY_UP += nu * el;
                    // Adaptive bar peak: EMA up fast, decay slowly down.
                    // A hard jump (max) makes the bar hug zero right after a
                    // burst; EMA keeps the scale responsive.
                    let up_f = 0.3f64;   // fast attack
                    let decay = 0.995f64;
                    MAX_DOWN = if nd > MAX_DOWN { MAX_DOWN * (1.0 - up_f) + nd * up_f } else { MAX_DOWN * decay };
                    MAX_UP = if nu > MAX_UP { MAX_UP * (1.0 - up_f) + nu * up_f } else { MAX_UP * decay };
                    if MAX_DOWN < 1024.0 { MAX_DOWN = 1024.0; }
                    if MAX_UP < 1024.0 { MAX_UP = 1024.0; }
                }
            } else {
                // Chosen interface vanished (NIC unplugged, VPN dropped):
                // re-pick the most active remaining one.
                let mut best = ""; let mut best_t = 0u64;
                for (n, d) in net.iter() {
                    let t = d.total_received() + d.total_transmitted();
                    if t > best_t { best_t = t; best = n; }
                }
                if !best.is_empty() {
                    iface = best.to_string();
                    if let Some(d) = net.get(&iface) {
                        last_r = d.total_received();
                        last_s = d.total_transmitted();
                    }
                }
            }
            pt = now;
        }
        // CPU/memory: only repaint when the shown integer % changes.
        unsafe {
            changed |= (cpu_usage - CPU_USAGE).abs() >= 1.5 || (memory_usage - MEMORY_USAGE).abs() >= 1.5;
            CPU_USAGE = cpu_usage;
            MEMORY_USAGE = memory_usage;
            if changed { DIRTY = true; }
        }

        // Trend graph accumulation with noise reduction (TrafficMonitor
        // TASKBAR_GRAPH_STEP): average GRAPH_STEP polls into one column so
        // the curve ignores single-poll spikes. Percent values: network
        // normalized against the adaptive peak, CPU/mem against 100.
        unsafe {
            let down_pct = (DOWN / MAX_DOWN.max(1.0) * 100.0).clamp(0.0, 100.0);
            let up_pct = (UP / MAX_UP.max(1.0) * 100.0).clamp(0.0, 100.0);
            HIST_ACC[0] += down_pct;
            HIST_ACC[1] += up_pct;
            HIST_ACC[2] += cpu_usage as f64;
            HIST_ACC[3] += memory_usage as f64;
            HIST_COUNT += 1;
            if HIST_COUNT >= GRAPH_STEP {
                let idx = HIST_HEAD % GRAPH_LEN;
                HIST_DOWN[idx] = (HIST_ACC[0] / GRAPH_STEP as f64) as u8;
                HIST_UP[idx] = (HIST_ACC[1] / GRAPH_STEP as f64) as u8;
                HIST_CPU[idx] = (HIST_ACC[2] / GRAPH_STEP as f64) as u8;
                HIST_MEM[idx] = (HIST_ACC[3] / GRAPH_STEP as f64) as u8;
                HIST_HEAD += 1;
                HIST_ACC = [0.0; 4];
                HIST_COUNT = 0;
            }
        }
    }
}

// ─── Main ──────────────────────────────────────────────────────

/// Spawn our own watchdog process (netspeed.exe --watchdog) that relaunches
/// this instance if it dies — e.g. after an Explorer crash on the lock
/// screen, which previously left the taskbar widget gone until the user
/// manually started it again. The watchdog is a child process in the same
/// exe (single-file stays). It is NOT started from --watchdog itself.
unsafe fn ensure_watchdog() {
    use windows::Win32::System::Threading::*;
    // Only one watchdog at a time. The WATCHDOG process owns
    // "Local\NetSpeed_Watchdog" (it creates it, not us), so the probe here
    // must be the inverse of the original design: if the mutex EXISTS we
    // already have a watchdog — bail out. If it does NOT exist, the watchdog
    // is not running — spawn one.
    let h = OpenMutexW(MUTEX_ALL_ACCESS, false, windows::core::w!("Local\\NetSpeed_Watchdog"));
    if h.is_ok() {
        let _ = CloseHandle(h.unwrap());
        return;
    }
    // Launch: netspeed.exe --watchdog, hidden window, same dir as exe.
    // Use the STANDARD command-line format (space-separated, exe in quotes)
    // — earlier NUL-joined args were parsed by CreateProcessW as a single
    // token (NUL is not a separator), so the child never saw "--watchdog".
    let mut exe = [0u16; 1024];
    let _ = GetModuleFileNameW(None, &mut exe);
    let path_len = exe.iter().take_while(|&&c| c != 0).count();
    let mut cmd: Vec<u16> = Vec::with_capacity(path_len + 24);
    cmd.push(b'"' as u16);
    cmd.extend_from_slice(&exe[..path_len]);
    cmd.push(b'"' as u16);
    cmd.push(b' ' as u16);
    cmd.extend_from_slice(&"--watchdog".encode_utf16().collect::<Vec<_>>());
    cmd.push(0);
    let mut si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let r = CreateProcessW(
        None,
        windows::core::PWSTR(cmd.as_mut_ptr()),
        None,
        None,
        false,
        CREATE_NO_WINDOW,
        None,
        None,
        &mut si,
        &mut pi,
    );
    if r.is_err() {
        eprintln!("netspeed watchdog spawn failed: {:?}", r.err());
    }
}

/// Signal the watchdog to exit (user chose 退出 in the menu). Without this
/// the watchdog would relaunch us moments after we quit.
unsafe fn stop_watchdog() {
    use windows::Win32::System::Threading::*;
    let ev = OpenEventW(
        EVENT_MODIFY_STATE,
        false,
        windows::core::w!("Local\\NetSpeed_WatchdogStop"),
    );
    if let Ok(e) = ev {
        let _ = SetEvent(e);
        let _ = CloseHandle(e);
    }
}

/// Watchdog entry: loop forever, checking every 2s whether the main
/// netspeed instance is alive (its single-instance mutex is held). If the
/// mutex becomes free the main process died — relaunch it (without the
/// --watchdog flag) and keep watching.
fn watchdog_main() {
    unsafe {
        use windows::Win32::System::Threading::*;
        // This watchdog owns the "Local\NetSpeed_Watchdog" mutex — it is the
        // single-watchdog marker ensure_watchdog() probes for. CreateMutexW
        // with bInitialOwner=true: if another watchdog exists we get
        // ERROR_ALREADY_EXISTS and exit (keep the handle alive otherwise).
        let wd = CreateMutexW(None, true, windows::core::w!("Local\\NetSpeed_Watchdog"));
        if wd.is_err() {
            return;
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ALREADY_EXISTS.0 as i32) {
            return;
        }
        let _wd = wd; // hold for process lifetime
        // Stop event: the main instance sets this when the user explicitly
        // exits, so we don't relaunch them.
        let stop_ev = CreateEventW(
            None,
            true,
            false,
            windows::core::w!("Local\\NetSpeed_WatchdogStop"),
        )
        .unwrap_or_default();
        // Manual-reset event: clear any stale "stop" state from a previous
        // run, otherwise a prior 退出 would make us exit immediately.
        if stop_ev.0 != 0 {
            let _ = ResetEvent(stop_ev);
        }
        loop {
            // User asked to stop?
            if stop_ev.0 != 0
                && WaitForSingleObject(stop_ev, 0) == WAIT_OBJECT_0
            {
                let _ = CloseHandle(stop_ev);
                return;
            }
            // The main instance holds "Global\NetSpeed_SingleInstance" (same
            // name the main() creates). If we can open it, the main process
            // is alive. BUT: an Explorer crash destroys the taskbar (our
            // parent), which destroys the main window while the PROCESS stays
            // alive — the mutex remains held and the process looks healthy
            // yet is a windowless zombie. So also verify the main window
            // exists; if not, treat the main instance as dead and relaunch.
            let mut main_alive = false;
            if let Ok(h) = OpenMutexW(
                MUTEX_ALL_ACCESS,
                false,
                windows::core::w!("Global\\NetSpeed_SingleInstance"),
            ) {
                let _ = CloseHandle(h);
                // Window present? FindWindowW scans top-level windows; our
                // main window is a child of the taskbar. Use FindWindowExW
                // from the desktop to find it regardless of parenting.
                let tb = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
                let mut w: HWND = HWND::default();
                if tb.0 != 0 {
                    w = FindWindowExW(
                        tb,
                        None,
                        windows::core::w!("NetSpeedTaskbarWnd"),
                        None,
                    );
                }
                if w.0 != 0 {
                    main_alive = true;
                }
            }
            if !main_alive {
                // Main instance is dead or a windowless zombie (Explorer
                // crash destroyed our taskbar-child window but the process
                // lingered, still holding the single-instance mutex). Kill
                // any netspeed.exe process OTHER than ourselves so the mutex
                // is released, then relaunch fresh.
                use windows::Win32::System::Threading::*;
                use windows::Win32::System::Diagnostics::ToolHelp::*;
                let self_pid = GetCurrentProcessId();
                if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
                    let mut pe = PROCESSENTRY32W {
                        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                        ..Default::default()
                    };
                    if Process32FirstW(snap, &mut pe).is_ok() {
                        loop {
                            let name = String::from_utf16_lossy(&pe.szExeFile);
                            if name.eq_ignore_ascii_case("netspeed.exe")
                                && pe.th32ProcessID != self_pid
                            {
                                let h = OpenProcess(
                                    PROCESS_TERMINATE,
                                    false,
                                    pe.th32ProcessID,
                                );
                                if let Ok(h2) = h {
                                    let _ = TerminateProcess(h2, 0);
                                    let _ = CloseHandle(h2);
                                }
                            }
                            if !Process32NextW(snap, &mut pe).is_ok() {
                                break;
                            }
                        }
                    }
                    let _ = CloseHandle(snap);
                }
                // Give the old process a moment to release the mutex.
                std::thread::sleep(std::time::Duration::from_millis(300));
                // Launch a fresh main instance.
                let mut exe = [0u16; 1024];
                let _ = GetModuleFileNameW(None, &mut exe);
                let path_len = exe.iter().take_while(|&&c| c != 0).count();
                let mut cmd: Vec<u16> = Vec::with_capacity(path_len + 4);
                cmd.push(b'"' as u16);
                cmd.extend_from_slice(&exe[..path_len]);
                cmd.push(b'"' as u16);
                cmd.push(0);
                let mut si = STARTUPINFOW {
                    cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                    ..Default::default()
                };
                let mut pi = PROCESS_INFORMATION::default();
                let _ = CreateProcessW(
                    None,
                    windows::core::PWSTR(cmd.as_mut_ptr()),
                    None,
                    None,
                    false,
                    CREATE_NO_WINDOW,
                    None,
                    None,
                    &mut si,
                    &mut pi,
                );
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
}

fn main() {
    // ── Watchdog mode ────────────────────────────────────────────────
    // netspeed.exe --watchdog: monitors the main process and relaunches it
    // if it dies (Explorer crash on lock-screen, crash, manual kill of the
    // window process). The main instance spawns one of these on startup.
    // Runs WITHOUT the single-instance mutex and without creating windows.
    if std::env::args().any(|a| a == "--watchdog") {
        watchdog_main();
        return;
    }
    // Per-monitor DPI awareness — prevents bitmap stretching (blurry "mosaic" text)
    // when the display scale is 125%/150%. Call before creating any window.
    unsafe { let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }

    // Detect theme at startup (before first paint)
    unsafe { LIGHT_THEME = system_light_theme(); }

    // Detect taskbar type (Win11 XAML vs Win10 classic) → sets TASKBAR_WIN11
    // and WINDOW_H before the window is created.
    unsafe { detect_taskbar_type(); }

    // Restore persisted toggles (显示图表 / 网络延迟检测) before the window
    // is created, so WINDOW_W matches SHOW_GRAPHS from the start.
    load_config();

    // Single instance: bail out if another instance already holds the mutex.
    unsafe {
        let h = CreateMutexW(None, true, windows::core::w!("Global\\NetSpeed_SingleInstance"));
        if h.is_err() { return; }
        // CreateMutexW succeeds with ERROR_ALREADY_EXISTS when the mutex
        // already exists — that means another instance is running.
        if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ALREADY_EXISTS.0 as i32) {
            return;
        }
        let _ = h; // keep the handle alive for the process lifetime
    }
    // Spawn the self-restart watchdog (one per main instance).
    unsafe { ensure_watchdog(); }
    // Net thread
    std::thread::spawn(net_thread);
    // Latency detection thread (pings; only updates when NET_DETECT on)
    latency_thread();

    // Autostart by default
    ensure_autostart();

    // Register the taskbar class.
    let class_name: Vec<u16> = CLASS_NAME.encode_utf16().collect();
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        RegisterClassW(&wc);
    }

    // Create window — plain tool window. WS_EX_LAYERED is NOT used: the
    // XAML taskbar composites child content through DirectComposition, so
    // content is drawn by D2D into a swapchain attached to a DComposition
    // visual (see D2DRenderer). An opaque background is painted in D2D.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::w!("NetSpeed"),
            WS_POPUP,
            0, 0, WINDOW_W, WINDOW_H,
            None, None, GetModuleHandleW(None).unwrap(), None,
        )
    };

    if hwnd.0 == 0 { return; }
    unsafe { MAIN_HWND = hwnd; }

    // Background health monitor: if the main window is ever destroyed (an
    // Explorer crash on lock-screen destroys the taskbar and our child
    // window), force this process to exit so the watchdog (keyed on the
    // mutex) notices and respawns a fresh instance. MUST be a dedicated
    // thread, NOT a WM_TIMER — timers are auto-killed when their window is
    // destroyed, so an IsWindow check inside REPOS_TIMER would never fire.
    std::thread::spawn(|| unsafe {
        loop {
            if !IsWindow(MAIN_HWND).as_bool() {
                // Our window died but the process lingers; exit so the
                // watchdog rebuilds us. Do NOT stop the watchdog.
                let _ = PostQuitMessage(0);
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });

    // Hover detail panel (hidden until the mouse enters the taskbar window).
    create_panel_window();

    unsafe {
        reposition(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Build the D2D + DirectComposition pipeline after the window exists
        // and is embedded, then draw the first frame.
        rebuild_renderer(hwnd);
    }

    // Message loop
    unsafe {
        let mut msg = MSG::default();
        loop {
            if GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else { break; }
        }
    }
}

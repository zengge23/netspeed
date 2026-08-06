#![windows_subsystem = "windows"]

use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

const WINDOW_W: i32 = 160;
const WINDOW_H: i32 = 42;

// Compact two-row layout tokens
const ROW_LEFT: i32 = 3;
const ARROW_RIGHT: i32 = 26;
const SPEED_LEFT: i32 = 26;
const SPEED_RIGHT: i32 = 81;
const DIVIDER_X: i32 = 84;
const STATUS_LEFT: i32 = 92;
const STATUS_LABEL_RIGHT: i32 = 123;
const STATUS_RIGHT: i32 = 154;
const CONTENT_TOP: i32 = 4;
const ROW_HEIGHT: i32 = 17;
// Color tokens — Dark theme (system dark taskbar)
const DARK_BG: COLORREF = COLORREF(0x00302C2C);
const DARK_DIVIDER: COLORREF = COLORREF(0x00606060);
const DARK_DOWN: COLORREF = COLORREF(0x009EDB6C); // #6cdb9e
const DARK_UP: COLORREF = COLORREF(0x006BB3FF);   // #ffb36b
const DARK_IDLE: COLORREF = COLORREF(0x007A7A7A);
const DARK_UNIT: COLORREF = COLORREF(0x00B0B0B0);
const DARK_WARNING: COLORREF = COLORREF(0x006060E8);
// Color tokens — Light theme (system light taskbar)
const LIGHT_BG: COLORREF = COLORREF(0x00F2F2F4);
const LIGHT_DIVIDER: COLORREF = COLORREF(0x00A8A8AC);
const LIGHT_DOWN: COLORREF = COLORREF(0x003A8A3A); // softer green on light taskbar
const LIGHT_UP: COLORREF = COLORREF(0x00385CB8);   // softer orange-red on light taskbar
const LIGHT_IDLE: COLORREF = COLORREF(0x00909090);
const LIGHT_UNIT: COLORREF = COLORREF(0x00787878);
const LIGHT_WARNING: COLORREF = COLORREF(0x003838C8);
const CLASS_NAME: &str = "NetSpeedTaskbarWnd\0";
const MENU_CLASS_NAME: &str = "NetSpeedPopupMenuWnd\0";
const MENU_W: i32 = 148;
const MENU_H: i32 = 76;
const MENU_AUTOSTART_BOTTOM: i32 = 36;
const MENU_EXIT_TOP: i32 = 40;
const MENU_DISMISS_TIMER: usize = 20;
const REFRESH_TIMER: usize = 1;
const REPOS_TIMER: usize = 2;
const PAINT_TIMER: usize = 3;
const REFRESH_INTERVAL_MS: u32 = 100;
const RUN_VALUE: &str = "NetSpeed";

static mut DOWN: f64 = 0.0;
static mut UP: f64 = 0.0;
static mut CPU_USAGE: f32 = 0.0;
static mut MEMORY_USAGE: f32 = 0.0;
static mut LIGHT_THEME: bool = false;
static mut TASKBAR_COLOR: COLORREF = COLORREF(0);

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

fn fmt_speed(s: f64) -> (String, String) {
    // Unified format: always 1 decimal so digits look consistent across units
    if s < 1024.0 { (format!("{:.1}", s), "B/s".to_string()) }
    else if s < 1048576.0 { (format!("{:.1}", s / 1024.0), "K/s".to_string()) }
    else if s < 1073741824.0 { (format!("{:.1}", s / 1048576.0), "M/s".to_string()) }
    else { (format!("{:.1}", s / 1073741824.0), "G/s".to_string()) }
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

fn sample_taskbar_color(fallback: COLORREF) -> COLORREF {
    unsafe {
        let cached = TASKBAR_COLOR;
        if cached.0 != 0 { cached } else { fallback }
    }
}

unsafe fn refresh_taskbar_color() {
    let taskbar = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
    if taskbar.0 == 0 { return; }
    let mut rect = RECT::default();
    if GetWindowRect(taskbar, &mut rect).is_err() { return; }

    // Sample composited taskbar pixels away from our window and the tray icons.
    // Multiple points reduce noise from a single icon/highlight pixel.
    let hdc = GetDC(None);
    if hdc.0 == 0 { return; }
    let y = rect.top + (rect.bottom - rect.top) / 2;
    let xs = [
        rect.left + (rect.right - rect.left) / 2,
        rect.left + (rect.right - rect.left) * 3 / 5,
        rect.left + (rect.right - rect.left) * 7 / 10,
        rect.left + (rect.right - rect.left) * 3 / 4,
        rect.left + (rect.right - rect.left) * 4 / 5,
    ];
    let mut red = 0u32;
    let mut green = 0u32;
    let mut blue = 0u32;
    let mut count = 0u32;
    for x in xs {
        let color = GetPixel(hdc, x, y);
        if color != COLORREF(CLR_INVALID) {
            red += color.0 & 0xff;
            green += (color.0 >> 8) & 0xff;
            blue += (color.0 >> 16) & 0xff;
            count += 1;
        }
    }
    let _ = ReleaseDC(None, hdc);
    if count > 0 {
        TASKBAR_COLOR = COLORREF(
            (red / count) | ((green / count) << 8) | ((blue / count) << 16),
        );
    }
}

// ─── Window procedure ─────────────────────────────────────────

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            refresh_taskbar_color();
            SetTimer(hwnd, REFRESH_TIMER, REFRESH_INTERVAL_MS, None);
            SetTimer(hwnd, PAINT_TIMER, 500, None);
            SetTimer(hwnd, REPOS_TIMER, 3000, None);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == REFRESH_TIMER => {
            // Shell flyouts temporarily reorder topmost windows. Reassert only
            // position/z-order here; painting stays on the slower paint timer.
            reposition(hwnd);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == PAINT_TIMER => {
            let _ = InvalidateRect(hwnd, None, true);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == REPOS_TIMER => {
            reposition(hwnd);
            refresh_taskbar_color();
            let _ = InvalidateRect(hwnd, None, true);
            // Theme may have changed — re-check and repaint if needed
            let light = system_light_theme();
            unsafe {
                if light != LIGHT_THEME {
                    LIGHT_THEME = light;
                    let _ = InvalidateRect(hwnd, None, true);
                }
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
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);

            // Background — follow system theme (light/dark taskbar)
            let (bg_c, div_c, down_active, up_active, idle_c, unit_c, warning_c) = if unsafe { LIGHT_THEME } {
                (LIGHT_BG, LIGHT_DIVIDER, LIGHT_DOWN, LIGHT_UP, LIGHT_IDLE, LIGHT_UNIT, LIGHT_WARNING)
            } else {
                (DARK_BG, DARK_DIVIDER, DARK_DOWN, DARK_UP, DARK_IDLE, DARK_UNIT, DARK_WARNING)
            };
            // Opaque sampled background keeps ClearType crisp while matching
            // the current taskbar surface more closely than a fixed theme color.
            let bg_color = sample_taskbar_color(bg_c);
            let bg = CreateSolidBrush(bg_color);
            FillRect(hdc, &r, bg);
            let _ = DeleteObject(bg);

            // No outer frame: only text and the central divider remain visible.

            // Main text and larger arrow fonts. Both are selected/restored explicitly
            // so GDI cannot fall back to the bitmap font.
            let hfont = CreateFontW(
                15, 0, 0, 0, 400, 0, 0, 0,
                DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32,
                FF_DONTCARE.0 as u32, windows::core::w!("Segoe UI"),
            );
            let arrow_font = CreateFontW(
                20, 0, 0, 0, 400, 0, 0, 0,
                DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32,
                FF_DONTCARE.0 as u32, windows::core::w!("Segoe UI Symbol"),
            );
            let old_font = SelectObject(hdc, hfont);
            SetBkColor(hdc, bg_color);
            SetBkMode(hdc, OPAQUE);

            let (down, up, cpu_usage, memory_usage) = (DOWN, UP, CPU_USAGE, MEMORY_USAGE);
            let (down_val, down_unit) = fmt_speed(down);
            let (up_val, up_unit) = fmt_speed(up);
            let down_color = if down > 0.0 { down_active } else { idle_c };
            let up_color = if up > 0.0 { up_active } else { idle_c };

            let up_top = CONTENT_TOP;
            let down_top = CONTENT_TOP + ROW_HEIGHT;

            // Larger standalone arrows.
            SelectObject(hdc, arrow_font);
            SetTextColor(hdc, up_color);
            let mut up_arrow_rect = RECT { left: ROW_LEFT, top: up_top - 2, right: ARROW_RIGHT, bottom: up_top + ROW_HEIGHT - 2 };
            DrawTextW(hdc, &mut "↑".encode_utf16().collect::<Vec<_>>(), &mut up_arrow_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SetTextColor(hdc, down_color);
            let mut down_arrow_rect = RECT { left: ROW_LEFT, top: down_top - 2, right: ARROW_RIGHT, bottom: down_top + ROW_HEIGHT - 2 };
            DrawTextW(hdc, &mut "↓".encode_utf16().collect::<Vec<_>>(), &mut down_arrow_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

            // Speed values remain at 15px.
            SelectObject(hdc, hfont);
            SetTextColor(hdc, up_color);
            let mut up_rect = RECT { left: SPEED_LEFT, top: up_top, right: SPEED_RIGHT, bottom: up_top + ROW_HEIGHT };
            let up_text = format!("{} {}", up_val.trim(), up_unit);
            DrawTextW(hdc, &mut up_text.encode_utf16().collect::<Vec<_>>(), &mut up_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            SetTextColor(hdc, down_color);
            let mut down_rect = RECT { left: SPEED_LEFT, top: down_top, right: SPEED_RIGHT, bottom: down_top + ROW_HEIGHT };
            let down_text = format!("{} {}", down_val.trim(), down_unit);
            DrawTextW(hdc, &mut down_text.encode_utf16().collect::<Vec<_>>(), &mut down_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

            // Subtle divider and right-side system status.
            let div_brush = CreateSolidBrush(div_c);
            let div_rect = RECT { left: DIVIDER_X, top: 7, right: DIVIDER_X + 1, bottom: WINDOW_H - 7 };
            FillRect(hdc, &div_rect, div_brush);
            let _ = DeleteObject(div_brush);

            let cpu_color = if cpu_usage >= 85.0 { warning_c } else { unit_c };
            let memory_color = if memory_usage >= 85.0 { warning_c } else { unit_c };
            let mut cpu_label_rect = RECT { left: STATUS_LEFT, top: up_top, right: STATUS_LABEL_RIGHT, bottom: up_top + ROW_HEIGHT };
            let mut cpu_value_rect = RECT { left: STATUS_LABEL_RIGHT, top: up_top, right: STATUS_RIGHT, bottom: up_top + ROW_HEIGHT };
            let mut memory_label_rect = RECT { left: STATUS_LEFT, top: down_top, right: STATUS_LABEL_RIGHT, bottom: down_top + ROW_HEIGHT };
            let mut memory_value_rect = RECT { left: STATUS_LABEL_RIGHT, top: down_top, right: STATUS_RIGHT, bottom: down_top + ROW_HEIGHT };

            SetTextColor(hdc, unit_c);
            DrawTextW(hdc, &mut "CPU".encode_utf16().collect::<Vec<_>>(), &mut cpu_label_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            DrawTextW(hdc, &mut "内存".encode_utf16().collect::<Vec<_>>(), &mut memory_label_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            SetTextColor(hdc, cpu_color);
            let cpu_text = format!("{:>2.0}%", cpu_usage);
            DrawTextW(hdc, &mut cpu_text.encode_utf16().collect::<Vec<_>>(), &mut cpu_value_rect, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
            SetTextColor(hdc, memory_color);
            let memory_text = format!("{:>2.0}%", memory_usage);
            DrawTextW(hdc, &mut memory_text.encode_utf16().collect::<Vec<_>>(), &mut memory_value_rect, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);

            SelectObject(hdc, old_font);
            let _ = DeleteObject(arrow_font);
            let _ = DeleteObject(hfont);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, REFRESH_TIMER);
            let _ = KillTimer(hwnd, PAINT_TIMER);
            let _ = KillTimer(hwnd, REPOS_TIMER);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ─── Window positioning ───────────────────────────────────────

unsafe fn reposition(hwnd: HWND) {
    let tb = FindWindowW(windows::core::w!("Shell_TrayWnd"), None);
    if tb.0 == 0 { return; }
    let mut tbr = RECT::default();
    GetWindowRect(tb, &mut tbr).ok();
    if tbr.right - tbr.left <= 0 { return; }

    let tn = FindWindowExW(tb, None, windows::core::w!("TrayNotifyWnd"), None);
    let mut tnr = RECT::default();
    let tray_x = if tn.0 != 0 && GetWindowRect(tn, &mut tnr).is_ok() && tnr.left > 0 { tnr.left - WINDOW_W - 6 }
    else { tbr.right - WINDOW_W - 80 };

    // Guard: if the computed position is off-screen (tray animating / expanding),
    // skip the move and keep the current spot — next REPOS tick will correct it.
    if tray_x < 0 || tray_x + WINDOW_W > tbr.right { return; }

    let y = tbr.top + (tbr.bottom - tbr.top - WINDOW_H) / 2;
    if y < 0 || y + WINDOW_H > tbr.bottom { return; }
    SetWindowPos(hwnd, HWND_TOPMOST, tray_x, y, WINDOW_W, WINDOW_H, SWP_NOACTIVATE | SWP_SHOWWINDOW).ok();
}

// ─── Autostart (registry Run key) ─────────────────────────────

fn is_autostart() -> bool {
    use std::process::Command;
    let out = Command::new("reg")
        .args(["query", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run", "/v", "NetSpeed"])
        .output();
    match out {
        Ok(o) => o.status.success() && String::from_utf8_lossy(&o.stdout).contains("NetSpeed"),
        Err(_) => false,
    }
}

fn ensure_autostart() {
    use std::process::Command;
    let exe = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return,
    };
    let _ = Command::new("reg")
        .args(["add", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run", "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &exe, "/f"])
        .output();
}

fn clear_autostart() {
    use std::process::Command;
    let _ = Command::new("reg")
        .args(["delete", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run", "/v", "NetSpeed", "/f"])
        .output();
}

unsafe extern "system" fn menu_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            let _ = SetTimer(hwnd, MENU_DISMISS_TIMER, 30, None);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == MENU_DISMISS_TIMER => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            let inside = pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom;
            let left_down = (GetAsyncKeyState(0x01) as u16 & 0x8000) != 0;
            let right_down = (GetAsyncKeyState(0x02) as u16 & 0x8000) != 0;
            if (!inside && (left_down || right_down)) || (GetAsyncKeyState(0x1B) as u16 & 0x8000) != 0 {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let _ = InvalidateRect(hwnd, None, true);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lp.0 & 0xffff) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
            let parent = HWND(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
            let inside = x >= 0 && x < MENU_W && y >= 0 && y < MENU_H;
            if inside && y < MENU_AUTOSTART_BOTTOM {
                if is_autostart() { clear_autostart(); } else { ensure_autostart(); }
                let _ = InvalidateRect(parent, None, true);
            } else if inside && y >= MENU_EXIT_TOP {
                let _ = PostMessageW(parent, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let _ = KillTimer(hwnd, MENU_DISMISS_TIMER);
            LRESULT(0)
        },
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let fallback = if LIGHT_THEME { LIGHT_BG } else { DARK_BG };
            let bg_color = sample_taskbar_color(fallback);
            let bg = CreateSolidBrush(bg_color);
            FillRect(hdc, &rect, bg);
            let _ = DeleteObject(bg);

            let border_color = if LIGHT_THEME { LIGHT_DIVIDER } else { DARK_DIVIDER };
            let border = CreateSolidBrush(border_color);
            FrameRect(hdc, &rect, border);
            let _ = DeleteObject(border);

            let font = CreateFontW(
                14, 0, 0, 0, 400, 0, 0, 0,
                DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32,
                FF_DONTCARE.0 as u32, windows::core::w!("Segoe UI"),
            );
            let old_font = SelectObject(hdc, font);
            SetBkColor(hdc, bg_color);
            SetBkMode(hdc, OPAQUE);
            let text_color = if LIGHT_THEME { COLORREF(0x00333333) } else { COLORREF(0x00EEEEEE) };
            let hover_color = if LIGHT_THEME { COLORREF(0x00E4E4E4) } else { COLORREF(0x004A4A4A) };
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let mut menu_rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut menu_rect);
            let local_x = cursor.x - menu_rect.left;
            let local_y = cursor.y - menu_rect.top;
            let hover_auto = local_x >= 0 && local_x < MENU_W && local_y >= 3 && local_y < MENU_AUTOSTART_BOTTOM;
            let hover_exit = local_x >= 0 && local_x < MENU_W && local_y >= MENU_EXIT_TOP && local_y < MENU_H - 3;
            let hover_brush = CreateSolidBrush(hover_color);
            if hover_auto {
                let hover_rect = RECT { left: 3, top: 3, right: MENU_W - 3, bottom: MENU_AUTOSTART_BOTTOM };
                FillRect(hdc, &hover_rect, hover_brush);
            }
            if hover_exit {
                let hover_rect = RECT { left: 3, top: MENU_EXIT_TOP, right: MENU_W - 3, bottom: MENU_H - 3 };
                FillRect(hdc, &hover_rect, hover_brush);
            }
            let _ = DeleteObject(hover_brush);
            SetTextColor(hdc, text_color);
            let mut auto_rect = RECT { left: 14, top: 5, right: MENU_W - 8, bottom: 32 };
            let mut auto_text = "开机自启".encode_utf16().collect::<Vec<_>>();
            DrawTextW(hdc, &mut auto_text, &mut auto_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            let mut exit_rect = RECT { left: 14, top: 41, right: MENU_W - 8, bottom: 68 };
            let mut exit_text = "退出".encode_utf16().collect::<Vec<_>>();
            DrawTextW(hdc, &mut exit_text, &mut exit_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            if is_autostart() {
                SetTextColor(hdc, if LIGHT_THEME { COLORREF(0x003060A0) } else { COLORREF(0x0088CCFF) });
                let mut check = "✓".encode_utf16().collect::<Vec<_>>();
                let mut check_rect = RECT { left: MENU_W - 27, top: 5, right: MENU_W - 8, bottom: 32 };
                DrawTextW(hdc, &mut check, &mut check_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            }
            let divider = CreateSolidBrush(border_color);
            let line = RECT { left: 10, top: 37, right: MENU_W - 10, bottom: 38 };
            FillRect(hdc, &line, divider);
            let _ = DeleteObject(divider);
            SelectObject(hdc, old_font);
            let _ = DeleteObject(font);
            let _ = EndPaint(hwnd, &ps);
            let _ = hover_color;
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let menu_class: Vec<u16> = MENU_CLASS_NAME.encode_utf16().collect();
    let hinst = GetModuleHandleW(None).unwrap();
    let menu = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        windows::core::PCWSTR(menu_class.as_ptr()),
        windows::core::w!("NetSpeedMenu"),
        WS_POPUP,
        pt.x - MENU_W + 8, pt.y - MENU_H - 4, MENU_W, MENU_H,
        None, None, hinst, Some(hwnd.0 as *const std::ffi::c_void),
    );
    if menu.0 != 0 {
        let _ = SetWindowPos(menu, HWND_TOPMOST, pt.x - MENU_W + 8, pt.y - MENU_H - 4, MENU_W, MENU_H, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

// ─── Network polling thread ────────────────────────────────────

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
    let mut pr: u64 = 0; let mut ps: u64 = 0;
    let mut pt = Instant::now();

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
        unsafe {
            CPU_USAGE = cpu_usage;
            MEMORY_USAGE = memory_usage;
        }
        let now = Instant::now();
        let el = (now - pt).as_secs_f64();
        if el < 0.2 { continue; }

        let mut best = ""; let mut best_t = 0u64;
        for (n, d) in net.iter() { let t = d.received() + d.transmitted(); if t > best_t { best_t = t; best = n; } }
        if best.is_empty() { continue; }

        if iface.is_empty() || best != iface {
            iface = best.to_string();
            if let Some(d) = net.get(&iface) { pr = d.received(); ps = d.transmitted(); }
            pt = now; unsafe { DOWN = 0.0; UP = 0.0; }
            continue;
        }

        if let Some(d) = net.get(&iface) {
            let cr = d.received(); let cs = d.transmitted();
            unsafe {
                if cr >= pr { DOWN = (cr - pr) as f64 / el; }
                if cs >= ps { UP = (cs - ps) as f64 / el; }
            }
            pr = cr; ps = cs;
        }
        pt = now;
    }
}

// ─── Main ──────────────────────────────────────────────────────

fn main() {
    // Per-monitor DPI awareness — prevents bitmap stretching (blurry "mosaic" text)
    // when the display scale is 125%/150%. Call before creating any window.
    unsafe { let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }

    // Detect theme at startup (before first paint)
    unsafe { LIGHT_THEME = system_light_theme(); }

    // Single instance
    unsafe {
        let h = match CreateMutexW(None, true, windows::core::w!("Global\\NetSpeed_SingleInstance")) {
            Ok(handle) => handle,
            Err(_) => return,
        };
        let _ = h;
    }
    // Net thread
    std::thread::spawn(net_thread);

    // Autostart by default
    ensure_autostart();

    // Register the taskbar class and the non-activating custom popup class.
    let class_name: Vec<u16> = CLASS_NAME.encode_utf16().collect();
    let menu_class_name: Vec<u16> = MENU_CLASS_NAME.encode_utf16().collect();
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
        let menu_wc = WNDCLASSW {
            lpfnWndProc: Some(menu_wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: windows::core::PCWSTR(menu_class_name.as_ptr()),
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        RegisterClassW(&menu_wc);
    }

    // Opaque sampled taskbar-color window; keeping it non-layered preserves
    // ClearType sharpness.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::w!("NetSpeed"),
            WS_POPUP,
            0, 0, WINDOW_W, WINDOW_H,
            None, None, GetModuleHandleW(None).unwrap(), None,
        )
    };

    if hwnd.0 == 0 { return; }

    unsafe {
        reposition(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
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

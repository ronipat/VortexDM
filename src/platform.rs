use std::path::Path;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, POINT, RECT};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CreateMutexW;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetWindowRect, IsIconic, IsWindowVisible, LoadImageW, PostMessageW,
    SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindowAsync, HWND_TOPMOST, ICON_BIG,
    ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE,
    SW_SHOW, WM_CLOSE, WM_SETICON,
};

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// Holds the Windows named mutex for the lifetime of the process so only one VortexDM runs.
pub struct SingleInstanceGuard {
    #[cfg(windows)]
    _handle: HANDLE,
}

impl SingleInstanceGuard {
    /// Attempt to acquire single-instance lock.
    /// Returns `None` if another instance is already running (and attempts to restore it).
    pub fn acquire(window_title: &str) -> Option<Self> {
        #[cfg(windows)]
        unsafe {
            let name = to_wide_null("Local\\VortexDM_SingleInstance_Mutex_v1");
            let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
            if handle.is_null() {
                return Some(Self { _handle: handle });
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let title_w = to_wide_null(window_title);
                let hwnd = FindWindowW(std::ptr::null(), title_w.as_ptr());
                if !hwnd.is_null() {
                    ShowWindowAsync(hwnd, SW_SHOW);
                    ShowWindowAsync(hwnd, SW_RESTORE);
                    SetForegroundWindow(hwnd);
                }
                return None;
            }
            Some(Self { _handle: handle })
        }
        #[cfg(not(windows))]
        {
            let _ = window_title;
            Some(Self {})
        }
    }
}

/// Resolve Win32 HWND from cached atomic value, falling back to `FindWindowW` by title.
pub fn resolve_hwnd(raw_hwnd: isize, window_title: &str) -> isize {
    if raw_hwnd != 0 {
        return raw_hwnd;
    }
    #[cfg(windows)]
    unsafe {
        let title_w = to_wide_null(window_title);
        FindWindowW(std::ptr::null(), title_w.as_ptr()) as isize
    }
    #[cfg(not(windows))]
    {
        let _ = window_title;
        0
    }
}

/// Explicitly assign BOTH `ICON_BIG` (Windows Taskbar / Alt-Tab) and `ICON_SMALL` (Titlebar)
/// from our embedded PE resource #1 so Windows 11 Taskbar never displays a stale cached icon.
pub fn apply_window_taskbar_icon(raw_hwnd: isize) {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;
            let hinst = GetModuleHandleW(std::ptr::null());
            let res_id = 1usize as *const u16;
            let hicon_big = LoadImageW(hinst, res_id, IMAGE_ICON, 48, 48, LR_DEFAULTCOLOR);
            if !hicon_big.is_null() {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, hicon_big as isize);
            }
            let hicon_small = LoadImageW(hinst, res_id, IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR);
            if !hicon_small.is_null() {
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, hicon_small as isize);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = raw_hwnd;
}

/// Synchronize the native Windows 11 DWM title bar color & dark/light chrome with the active
/// VortexDM panel theme (`p.bg_alt`: `#1F1F1E` in Dark Mode, `#F8FAFC` in Light Mode).
pub fn apply_window_caption_theme(raw_hwnd: isize, dark: bool) {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;
            let use_dark: i32 = if dark { 1 } else { 0 };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&use_dark as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
            // COLORREF is 0x00BBGGRR: Dark #1F1F1E -> 0x001E1F1F, Light #F8FAFC -> 0x00FCFAF8
            let caption_color: u32 = if dark { 0x001E_1F1F } else { 0x00FC_FAF8 };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR as u32,
                (&caption_color as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            let text_color: u32 = if dark { 0x00ED_EDED } else { 0x000A_0A0A };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TEXT_COLOR as u32,
                (&text_color as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = (raw_hwnd, dark);
}

/// Apply native Windows 11 DWM rounded corners, drop shadow, and theme-matched border color to the
/// frameless tray popup window so its 4 corners & edges are cleanly clipped with zero black artifacts.
pub fn apply_popup_window_chrome(raw_hwnd: isize, dark: bool) {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;
            let use_dark: i32 = if dark { 1 } else { 0 };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&use_dark as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
            let corner_pref: i32 = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&corner_pref as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
            // Match Palette::border (COLORREF 0x00BBGGRR): Dark #383838, Light #E5E7EB
            let border_color: u32 = if dark { 0x0038_3838 } else { 0x00EB_E7E5 };
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR as u32,
                (&border_color as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = (raw_hwnd, dark);
}

/// Thread-safe Win32 HWND wrapper for showing/hiding the app from tray threads even while hidden.
pub fn show_window_by_hwnd(raw_hwnd: isize) {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;
            ShowWindowAsync(hwnd, SW_SHOW);
            if IsIconic(hwnd) != 0 {
                ShowWindowAsync(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
    }
    #[cfg(not(windows))]
    let _ = raw_hwnd;
}

pub fn hide_window_by_hwnd(raw_hwnd: isize) {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;
            ShowWindowAsync(hwnd, SW_HIDE);
        }
    }
    #[cfg(not(windows))]
    let _ = raw_hwnd;
}

pub fn toggle_window_by_hwnd(raw_hwnd: isize) -> bool {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;
            let visible = IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0;
            if visible {
                ShowWindowAsync(hwnd, SW_HIDE);
                return false;
            } else {
                ShowWindowAsync(hwnd, SW_SHOW);
                ShowWindowAsync(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                return true;
            }
        }
    }
    #[cfg(not(windows))]
    let _ = raw_hwnd;
    true
}

pub fn post_close_by_hwnd(raw_hwnd: isize) {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;
            ShowWindowAsync(hwnd, SW_SHOW);
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
    #[cfg(not(windows))]
    let _ = raw_hwnd;
}

pub fn is_window_visible(raw_hwnd: isize) -> bool {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            return IsWindowVisible(raw_hwnd as HWND) != 0;
        }
    }
    #[cfg(not(windows))]
    let _ = raw_hwnd;
    false
}

/// Position the frameless tray popup directly above the Windows taskbar near `(anchor_x, anchor_y)`
/// (clamped safely within the monitor's work area across multi-monitor & high-DPI displays) and focus it.
pub fn show_popup_near_tray(raw_hwnd: isize, anchor_x: Option<i32>, anchor_y: Option<i32>) {
    #[cfg(windows)]
    if raw_hwnd != 0 {
        unsafe {
            let hwnd = raw_hwnd as HWND;

            let mut pt = POINT { x: 0, y: 0 };
            if let (Some(ax), Some(ay)) = (anchor_x, anchor_y) {
                pt.x = ax;
                pt.y = ay;
            } else {
                GetCursorPos(&mut pt);
            }

            // Query monitor work area (automatically excludes Windows taskbar on any edge)
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            let has_mi = GetMonitorInfoW(hmon, &mut mi) != 0;

            // Measure current physical window dimensions
            let mut wr: RECT = std::mem::zeroed();
            let (win_w, win_h) = if GetWindowRect(hwnd, &mut wr) != 0 && (wr.right > wr.left) {
                (wr.right - wr.left, wr.bottom - wr.top)
            } else {
                (276, 354)
            };

            let margin = 10;
            let (x, y) = if has_mi {
                let work = mi.rcWork;
                let mon = mi.rcMonitor;

                // Center horizontally above tray click, clamped within work area with clean margin
                let desired_x = pt.x - win_w / 2;
                let clamped_x = desired_x
                    .min(work.right - win_w - margin)
                    .max(work.left + margin);

                // Place right above bottom taskbar (or below if taskbar is docked at top)
                let desired_y = if work.top > mon.top && pt.y <= work.top + 20 {
                    work.top + margin
                } else {
                    work.bottom - win_h - margin
                };
                let clamped_y = desired_y
                    .min(work.bottom - win_h - margin)
                    .max(work.top + margin);

                (clamped_x, clamped_y)
            } else {
                ((pt.x - win_w).max(12), (pt.y - win_h - 12).max(12))
            };

            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            SetForegroundWindow(hwnd);
        }
    }
    #[cfg(not(windows))]
    let _ = (raw_hwnd, anchor_x, anchor_y);
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Query free disk bytes available to the current user at `dir`.
pub fn available_disk_space(dir: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        let probe_path = if dir.exists() {
            dir.to_path_buf()
        } else {
            dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dir.to_path_buf())
        };
        let wide = to_wide_null(&probe_path.to_string_lossy());
        let mut free_available: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_available,
                &mut total_bytes,
                &mut total_free,
            )
        };
        if ok != 0 {
            Some(free_available)
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        None
    }
}

/// Configure Windows HKCU Run registry key so VortexDM launches minimized in system tray at logon.
pub fn set_windows_autostart(enabled: bool, start_minimized: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = hkcu
            .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .map_err(|e| e.to_string())?;

        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let cmd = if start_minimized {
                format!("\"{}\" --minimized", exe.display())
            } else {
                format!("\"{}\"", exe.display())
            };
            run_key
                .set_value("VortexDM", &cmd)
                .map_err(|e| e.to_string())?;
        } else {
            let _ = run_key.delete_value("VortexDM");
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, start_minimized);
        Ok(())
    }
}

pub fn is_windows_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .ok()
            .and_then(|k| k.get_value::<String, _>("VortexDM").ok())
            .is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Open a downloaded file with its default Windows application.
pub fn open_file(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Open Windows Explorer and select the downloaded file.
pub fn reveal_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        if path.exists() {
            let arg = format!("/select,\"{}\"", path.display());
            let _ = std::process::Command::new("explorer.exe")
                .raw_arg(&arg)
                .spawn();
        } else if let Some(parent) = path.parent() {
            let _ = std::process::Command::new("explorer.exe")
                .arg(parent)
                .spawn();
        }
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Open a folder directly in Windows Explorer.
pub fn open_folder(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::fs::create_dir_all(path);
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(windows))]
    let _ = path;
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

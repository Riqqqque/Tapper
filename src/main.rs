#![cfg(windows)]
#![windows_subsystem = "windows"]

use serde::Deserialize;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString, c_void};
use std::fs;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HINSTANCE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT,
    WAIT_OBJECT_0, WPARAM,
};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    MAPVK_VK_TO_VSC, MapVirtualKeyW, RegisterHotKey, SendInput, UnregisterHotKey,
};
use windows_sys::Win32::UI::Shell::{
    ExtractIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HHOOK, HICON, IDC_ARROW,
    IDI_APPLICATION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LoadCursorW, LoadIconW, MB_ICONERROR, MB_OK,
    MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, MSLLHOOKSTRUCT, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, SetWindowsHookExW,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    UnhookWindowsHookEx, UnregisterClassW, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP, WM_MOUSEWHEEL,
    WM_NULL, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WS_OVERLAPPED,
};

const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
const TOGGLE_HOTKEY_ID: i32 = 1;
const EXIT_HOTKEY_ID: i32 = 2;
const MENU_TOGGLE_ID: u32 = 1001;
const MENU_EXIT_ID: u32 = 1002;
const TRAY_ICON_ID: u32 = 1;
const MOD_CONTROL: u32 = 0x0002;
const MOD_NOREPEAT: u32 = 0x4000;
const VK_A: u32 = 0x41;
const VK_D: u32 = 0x44;
const VK_F8: u32 = 0x77;
const VK_W: u32 = 0x57;
const CREATE_NO_WINDOW: u32 = 0x08000000;

static APP_STATE: OnceLock<Arc<AppState>> = OnceLock::new();
static START_TIME: OnceLock<Instant> = OnceLock::new();
static WINDOW_CLASS_NAME: OnceLock<Vec<u16>> = OnceLock::new();

#[repr(C)]
struct VsFixedFileInfo {
    dw_signature: u32,
    dw_struc_version: u32,
    dw_file_version_ms: u32,
    dw_file_version_ls: u32,
    dw_product_version_ms: u32,
    dw_product_version_ls: u32,
    dw_file_flags_mask: u32,
    dw_file_flags: u32,
    dw_file_os: u32,
    dw_file_type: u32,
    dw_file_subtype: u32,
    dw_file_date_ms: u32,
    dw_file_date_ls: u32,
}

#[link(name = "version")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(filename: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(filename: *const u16, handle: u32, len: u32, data: *mut c_void) -> i32;
    fn VerQueryValueW(
        block: *const c_void,
        sub_block: *const u16,
        buffer: *mut *mut c_void,
        len: *mut u32,
    ) -> i32;
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct Version {
    major: u16,
    minor: u16,
    patch: u16,
    build: u16,
}

impl Version {
    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('.').map(|part| part.parse::<u16>().ok());
        let major = parts.next().flatten()?;
        let minor = parts.next().flatten().unwrap_or(0);
        let patch = parts.next().flatten().unwrap_or(0);
        let build = parts.next().flatten().unwrap_or(0);
        Some(Self {
            major,
            minor,
            patch,
            build,
        })
    }
}

#[derive(Debug)]
struct AppState {
    settings: Settings,
    enabled: AtomicBool,
    a_down: AtomicBool,
    d_down: AtomicBool,
    w_down: AtomicBool,
    last_tap_at_ms: AtomicI64,
    queued_forward_taps: AtomicI32,
    synthetic_forward_held: AtomicBool,
    shutting_down: AtomicBool,
    cleanup_started: AtomicBool,
    window_handle: AtomicIsize,
    keyboard_hook: AtomicIsize,
    mouse_hook: AtomicIsize,
    forward_event: isize,
    worker_handle: Mutex<Option<JoinHandle<()>>>,
    target_cache: Mutex<TargetWindowCache>,
    forward_key_lock: Mutex<()>,
    send_input_lock: Mutex<()>,
    tray_state: Mutex<TrayState>,
}

#[derive(Debug, Default)]
struct TargetWindowCache {
    hwnd: isize,
    is_match: bool,
}

#[derive(Debug, Default)]
struct TrayState {
    icon: isize,
    icon_owned: bool,
    added: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    enabled_on_start: bool,
    forward_tap_hold_ms: i32,
    forward_tap_cooldown_ms: i32,
    forward_tap_burst_count: i32,
    forward_tap_pulse_gap_ms: i32,
    held_forward_retap_release_ms: i32,
    max_queued_forward_taps: i32,
    trigger_on_wheel_down: bool,
    trigger_on_wheel_up: bool,
    require_strafe_key: bool,
    block_when_forward_held: bool,
    process_names: Vec<String>,
    window_title_contains: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled_on_start: true,
            forward_tap_hold_ms: 6,
            forward_tap_cooldown_ms: 0,
            forward_tap_burst_count: 3,
            forward_tap_pulse_gap_ms: 0,
            held_forward_retap_release_ms: 2,
            max_queued_forward_taps: 24,
            trigger_on_wheel_down: true,
            trigger_on_wheel_up: true,
            require_strafe_key: true,
            block_when_forward_held: false,
            process_names: vec!["r5apex.exe".to_string()],
            window_title_contains: vec!["Apex Legends".to_string()],
        }
    }
}

impl Settings {
    fn load(base_dir: &Path) -> Self {
        let path = base_dir.join("tapper.settings.json");
        let mut settings = match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str::<Settings>(&contents).unwrap_or_default(),
            Err(_) => Settings::default(),
        };
        settings.normalize();
        settings
    }

    fn normalize(&mut self) {
        self.forward_tap_hold_ms = self.forward_tap_hold_ms.clamp(1, 25);
        self.forward_tap_cooldown_ms = self.forward_tap_cooldown_ms.clamp(0, 25);
        self.forward_tap_burst_count = self.forward_tap_burst_count.clamp(1, 6);
        self.forward_tap_pulse_gap_ms = self.forward_tap_pulse_gap_ms.clamp(0, 10);
        self.held_forward_retap_release_ms = self.held_forward_retap_release_ms.clamp(1, 10);
        self.max_queued_forward_taps = self.max_queued_forward_taps.clamp(1, 64);
        if !self.trigger_on_wheel_down && !self.trigger_on_wheel_up {
            self.trigger_on_wheel_down = true;
        }

        self.process_names = normalize_entries(&self.process_names, &["r5apex.exe"]);
        self.window_title_contains =
            normalize_entries(&self.window_title_contains, &["Apex Legends"]);
    }
}

fn main() {
    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            show_error_message(&format!("Unable to resolve the executable path: {error}"));
            process::exit(1);
        }
    };

    if try_hand_off_to_installed_copy(&current_exe) {
        return;
    }

    let base_dir = current_exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let settings = Settings::load(&base_dir);
    let state = match AppState::new(settings) {
        Ok(state) => Arc::new(state),
        Err(error) => {
            show_error_message(&error);
            process::exit(1);
        }
    };

    let _ = APP_STATE.set(state.clone());

    if let Err(error) = initialize_app(&current_exe, &state) {
        cleanup(&state);
        show_error_message(&error);
        process::exit(1);
    }

    let exit_code = run_message_loop();
    cleanup(&state);
    if exit_code != 0 {
        process::exit(exit_code);
    }
}

impl AppState {
    fn new(settings: Settings) -> Result<Self, String> {
        let forward_event = unsafe { CreateEventW(null(), 0, 0, null()) };
        if forward_event.is_null() {
            return Err(last_error_message(
                "Unable to create the forward-tap event.",
            ));
        }

        Ok(Self {
            enabled: AtomicBool::new(settings.enabled_on_start),
            settings,
            a_down: AtomicBool::new(false),
            d_down: AtomicBool::new(false),
            w_down: AtomicBool::new(false),
            last_tap_at_ms: AtomicI64::new(0),
            queued_forward_taps: AtomicI32::new(0),
            synthetic_forward_held: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            cleanup_started: AtomicBool::new(false),
            window_handle: AtomicIsize::new(0),
            keyboard_hook: AtomicIsize::new(0),
            mouse_hook: AtomicIsize::new(0),
            forward_event: forward_event as isize,
            worker_handle: Mutex::new(None),
            target_cache: Mutex::new(TargetWindowCache::default()),
            forward_key_lock: Mutex::new(()),
            send_input_lock: Mutex::new(()),
            tray_state: Mutex::new(TrayState::default()),
        })
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn window(&self) -> HWND {
        self.window_handle.load(Ordering::Relaxed) as HWND
    }
}

fn initialize_app(current_exe: &Path, state: &Arc<AppState>) -> Result<(), String> {
    let class_name = WINDOW_CLASS_NAME.get_or_init(|| wide("TapperHiddenWindow"));
    let hinstance = unsafe { GetModuleHandleW(null()) } as HINSTANCE;

    let wnd_class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        hInstance: hinstance,
        lpszClassName: class_name.as_ptr(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        ..unsafe { zeroed() }
    };

    let atom = unsafe { RegisterClassW(&wnd_class) };
    if atom == 0 {
        let last_error = unsafe { GetLastError() };
        if last_error != 1410 {
            return Err(last_error_message(
                "Unable to register the Tapper window class.",
            ));
        }
    }

    let title = wide("Tapper");
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null(),
        )
    };
    if hwnd.is_null() {
        return Err(last_error_message(
            "Unable to create the Tapper message window.",
        ));
    }

    state.window_handle.store(hwnd as isize, Ordering::Relaxed);
    add_tray_icon(current_exe, state)?;
    register_hotkeys(hwnd)?;
    install_hooks(state)?;
    start_forward_tap_worker(state)?;
    update_tray_state(state)?;
    Ok(())
}

fn run_message_loop() -> i32 {
    let mut msg = unsafe { zeroed::<MSG>() };

    loop {
        let result = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if result == -1 {
            return 1;
        }
        if result == 0 {
            return 0;
        }

        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_HOTKEY => {
            handle_hotkey(wparam as i32);
            0
        }
        TRAY_CALLBACK_MESSAGE => {
            handle_tray_callback(hwnd, lparam as u32);
            0
        }
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn handle_hotkey(hotkey_id: i32) {
    match hotkey_id {
        TOGGLE_HOTKEY_ID => toggle_assist(),
        EXIT_HOTKEY_ID => exit_application(),
        _ => {}
    }
}

fn handle_tray_callback(hwnd: HWND, event: u32) {
    if matches!(event, WM_RBUTTONUP | WM_CONTEXTMENU | WM_LBUTTONUP) {
        show_tray_menu(hwnd);
    }
}

fn toggle_assist() {
    let state = state();
    let enabled = !state.enabled.load(Ordering::Relaxed);
    state.enabled.store(enabled, Ordering::Relaxed);
    if !enabled {
        state.queued_forward_taps.store(0, Ordering::Relaxed);
        release_synthetic_forward_hold_if_needed();
    }

    write_status(if enabled {
        "assist enabled"
    } else {
        "assist disabled"
    });
    let _ = update_tray_state(state);
}

fn exit_application() {
    write_status("shutting down");
    let hwnd = state().window();
    if !hwnd.is_null() {
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}

fn add_tray_icon(current_exe: &Path, state: &AppState) -> Result<(), String> {
    let (icon, icon_owned) = load_application_icon(current_exe);
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = state.window();
    data.uID = TRAY_ICON_ID;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
    data.hIcon = icon as HICON;
    copy_utf16_buffer(&build_tray_text(state.is_enabled()), &mut data.szTip);

    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
    if added == 0 {
        if icon_owned && icon != 0 {
            unsafe {
                DestroyIcon(icon as HICON);
            }
        }
        return Err(last_error_message("Unable to add the Tapper tray icon."));
    }

    let mut tray_state = state.tray_state.lock().unwrap();
    tray_state.icon = icon;
    tray_state.icon_owned = icon_owned;
    tray_state.added = true;
    Ok(())
}

fn update_tray_state(state: &AppState) -> Result<(), String> {
    let tray_state = state.tray_state.lock().unwrap();
    if !tray_state.added {
        return Ok(());
    }

    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = state.window();
    data.uID = TRAY_ICON_ID;
    data.uFlags = NIF_TIP;
    copy_utf16_buffer(&build_tray_text(state.is_enabled()), &mut data.szTip);

    let updated = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    if updated == 0 {
        return Err(last_error_message("Unable to update the Tapper tray icon."));
    }

    Ok(())
}

fn remove_tray_icon(state: &AppState) {
    let mut tray_state = state.tray_state.lock().unwrap();
    if tray_state.added {
        let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = state.window();
        data.uID = TRAY_ICON_ID;
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }

    if tray_state.icon_owned && tray_state.icon != 0 {
        unsafe {
            DestroyIcon(tray_state.icon as HICON);
        }
    }

    tray_state.added = false;
    tray_state.icon = 0;
    tray_state.icon_owned = false;
}

fn show_tray_menu(hwnd: HWND) {
    let state = state();
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }

    let status_text = wide(if state.is_enabled() {
        "Assist: enabled"
    } else {
        "Assist: disabled"
    });
    let toggle_text = wide(if state.is_enabled() {
        "Disable Assist (F8)"
    } else {
        "Enable Assist (F8)"
    });
    let exit_text = wide("Exit (Ctrl+F8)");

    unsafe {
        AppendMenuW(
            menu,
            MF_STRING | MF_DISABLED | MF_GRAYED,
            0,
            status_text.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(
            menu,
            MF_STRING,
            MENU_TOGGLE_ID as usize,
            toggle_text.as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, MENU_EXIT_ID as usize, exit_text.as_ptr());
    }

    let mut cursor = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut cursor);
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            cursor.x,
            cursor.y,
            0,
            hwnd,
            null(),
        );

        if command as u32 == MENU_TOGGLE_ID {
            toggle_assist();
        } else if command as u32 == MENU_EXIT_ID {
            exit_application();
        }

        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}

fn register_hotkeys(hwnd: HWND) -> Result<(), String> {
    let toggle = unsafe { RegisterHotKey(hwnd, TOGGLE_HOTKEY_ID, MOD_NOREPEAT, VK_F8) };
    if toggle == 0 {
        return Err(last_error_message(
            "Unable to register the F8 toggle hotkey.",
        ));
    }

    let exit = unsafe { RegisterHotKey(hwnd, EXIT_HOTKEY_ID, MOD_CONTROL | MOD_NOREPEAT, VK_F8) };
    if exit == 0 {
        unsafe {
            UnregisterHotKey(hwnd, TOGGLE_HOTKEY_ID);
        }
        return Err(last_error_message(
            "Unable to register the Ctrl+F8 exit hotkey.",
        ));
    }

    Ok(())
}

fn install_hooks(state: &AppState) -> Result<(), String> {
    let module = unsafe { GetModuleHandleW(null()) };

    let keyboard_hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), module, 0) };
    if keyboard_hook.is_null() {
        return Err(last_error_message("Unable to install the keyboard hook."));
    }

    let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), module, 0) };
    if mouse_hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(keyboard_hook);
        }
        return Err(last_error_message("Unable to install the mouse hook."));
    }

    state
        .keyboard_hook
        .store(keyboard_hook as isize, Ordering::Relaxed);
    state
        .mouse_hook
        .store(mouse_hook as isize, Ordering::Relaxed);
    Ok(())
}

unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        let data = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if (data.flags & LLKHF_INJECTED) == 0 {
            let message = wparam as u32;
            let is_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let is_up = message == WM_KEYUP || message == WM_SYSKEYUP;

            if is_down || is_up {
                let new_state = is_down;
                let state = state();
                match data.vkCode {
                    VK_A => state.a_down.store(new_state, Ordering::Relaxed),
                    VK_D => state.d_down.store(new_state, Ordering::Relaxed),
                    VK_W => {
                        state.w_down.store(new_state, Ordering::Relaxed);
                        if !new_state {
                            release_synthetic_forward_hold_if_needed();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    unsafe { CallNextHookEx(null_mut(), n_code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook_proc(n_code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if n_code >= 0 && (wparam as u32) == WM_MOUSEWHEEL {
        let data = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        let wheel_delta = get_wheel_delta(data.mouseData);
        if should_trigger_for_wheel_delta(wheel_delta) && should_send_forward_tap() {
            queue_forward_tap_burst();
        }
    }

    unsafe { CallNextHookEx(null_mut(), n_code, wparam, lparam) }
}

fn should_trigger_for_wheel_delta(wheel_delta: i16) -> bool {
    let settings = &state().settings;
    if wheel_delta == 0 {
        return false;
    }

    if wheel_delta < 0 {
        settings.trigger_on_wheel_down
    } else {
        settings.trigger_on_wheel_up
    }
}

fn should_send_forward_tap() -> bool {
    let state = state();
    if !state.is_enabled() {
        return false;
    }

    if state.settings.require_strafe_key
        && !(state.a_down.load(Ordering::Relaxed) || state.d_down.load(Ordering::Relaxed))
    {
        return false;
    }

    if state.settings.block_when_forward_held && state.w_down.load(Ordering::Relaxed) {
        return false;
    }

    if !is_target_window_active(state) {
        return false;
    }

    let now = monotonic_millis();
    let previous = state.last_tap_at_ms.load(Ordering::Relaxed);
    if now - previous < i64::from(state.settings.forward_tap_cooldown_ms) {
        return false;
    }

    state.last_tap_at_ms.store(now, Ordering::Relaxed);
    true
}

fn start_forward_tap_worker(state: &Arc<AppState>) -> Result<(), String> {
    let worker_state = state.clone();
    let handle = thread::Builder::new()
        .name("TapperForwardTapWorker".to_string())
        .spawn(move || forward_tap_worker_loop(&worker_state))
        .map_err(|error| format!("Unable to start the forward-tap worker: {error}"))?;

    *state.worker_handle.lock().unwrap() = Some(handle);
    Ok(())
}

fn forward_tap_worker_loop(state: &AppState) {
    loop {
        let wait_result = unsafe { WaitForSingleObject(state.forward_event as _, INFINITE) };
        if wait_result != WAIT_OBJECT_0 {
            return;
        }

        if state.shutting_down.load(Ordering::Relaxed) {
            return;
        }

        while try_take_queued_forward_tap(state) {
            if state.shutting_down.load(Ordering::Relaxed) {
                return;
            }

            if !can_process_queued_forward_tap(state) {
                continue;
            }

            send_forward_tap(state);
            delay_milliseconds_precise(state.settings.forward_tap_pulse_gap_ms);
        }
    }
}

fn queue_forward_tap_burst() {
    let state = state();
    let queue_single_held_forward_tap =
        !state.settings.block_when_forward_held && state.w_down.load(Ordering::Relaxed);
    let tap_count = if queue_single_held_forward_tap {
        1
    } else {
        state.settings.forward_tap_burst_count
    };
    let max_queued_taps = if queue_single_held_forward_tap {
        1
    } else {
        state.settings.max_queued_forward_taps
    };

    loop {
        let current = state.queued_forward_taps.load(Ordering::Relaxed);
        if current >= max_queued_taps {
            return;
        }

        let target = std::cmp::min(max_queued_taps, current + tap_count);
        if state
            .queued_forward_taps
            .compare_exchange(current, target, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            unsafe {
                SetEvent(state.forward_event as _);
            }
            return;
        }
    }
}

fn try_take_queued_forward_tap(state: &AppState) -> bool {
    loop {
        let current = state.queued_forward_taps.load(Ordering::Relaxed);
        if current == 0 {
            return false;
        }

        if state
            .queued_forward_taps
            .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return true;
        }
    }
}

fn can_process_queued_forward_tap(state: &AppState) -> bool {
    if !state.is_enabled() {
        return false;
    }

    if state.settings.require_strafe_key
        && !(state.a_down.load(Ordering::Relaxed) || state.d_down.load(Ordering::Relaxed))
    {
        return false;
    }

    if state.settings.block_when_forward_held && state.w_down.load(Ordering::Relaxed) {
        return false;
    }

    is_target_window_active(state)
}

fn is_target_window_active(state: &AppState) -> bool {
    let window_handle = unsafe { GetForegroundWindow() };
    if window_handle.is_null() {
        return false;
    }

    {
        let cache = state.target_cache.lock().unwrap();
        if cache.hwnd == window_handle as isize {
            return cache.is_match;
        }
    }

    let matches_target = matches_target_window(state, window_handle);
    let mut cache = state.target_cache.lock().unwrap();
    cache.hwnd = window_handle as isize;
    cache.is_match = matches_target;
    matches_target
}

fn matches_target_window(state: &AppState, window_handle: HWND) -> bool {
    let process_name = try_get_foreground_process_name(window_handle);
    if matches_configured_process(&state.settings, &process_name) {
        return true;
    }

    let title = try_get_window_title(window_handle);
    matches_configured_title(&state.settings, &title)
}

fn try_get_foreground_process_name(window_handle: HWND) -> String {
    let mut process_id = 0_u32;
    unsafe {
        GetWindowThreadProcessId(window_handle, &mut process_id);
    }
    if process_id == 0 {
        return String::new();
    }

    query_process_image_path(process_id)
        .and_then(|path| {
            Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_default()
}

fn try_get_window_title(window_handle: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(window_handle) };
    if length == 0 {
        return String::new();
    }

    let mut buffer = vec![0_u16; length as usize + 1];
    let written =
        unsafe { GetWindowTextW(window_handle, buffer.as_mut_ptr(), buffer.len() as i32) };
    if written <= 0 {
        return String::new();
    }

    utf16_to_string(&buffer[..written as usize])
}

fn matches_configured_process(settings: &Settings, candidate: &str) -> bool {
    if candidate.trim().is_empty() {
        return false;
    }

    let normalized_candidate = normalize_process_name(candidate);
    settings
        .process_names
        .iter()
        .any(|configured_name| normalize_process_name(configured_name) == normalized_candidate)
}

fn matches_configured_title(settings: &Settings, candidate: &str) -> bool {
    if candidate.trim().is_empty() {
        return false;
    }

    let lower_candidate = candidate.to_ascii_lowercase();
    settings
        .window_title_contains
        .iter()
        .any(|configured_title| lower_candidate.contains(&configured_title.to_ascii_lowercase()))
}

fn normalize_process_name(name: &str) -> String {
    name.trim_end_matches(".exe").to_ascii_lowercase()
}

fn send_forward_tap(state: &AppState) {
    let _forward_key_guard = state.forward_key_lock.lock().unwrap();
    if !state.settings.block_when_forward_held && state.w_down.load(Ordering::Relaxed) {
        send_keyboard_input(VK_W as u16, true);
        delay_milliseconds_precise(state.settings.held_forward_retap_release_ms);
        send_keyboard_input(VK_W as u16, false);
        state.synthetic_forward_held.store(true, Ordering::Relaxed);
        return;
    }

    release_synthetic_forward_hold_if_needed_no_lock(state);
    send_keyboard_input(VK_W as u16, false);
    delay_milliseconds_precise(state.settings.forward_tap_hold_ms);
    send_keyboard_input(VK_W as u16, true);
}

fn send_keyboard_input(virtual_key: u16, key_up: bool) {
    let state = state();
    let _send_input_guard = state.send_input_lock.lock().unwrap();

    let mut virtual_key = virtual_key;
    let scan_code = unsafe { MapVirtualKeyW(virtual_key as u32, MAPVK_VK_TO_VSC) as u16 };
    let mut flags = if key_up { KEYEVENTF_KEYUP } else { 0 };

    if scan_code != 0 {
        flags |= KEYEVENTF_SCANCODE;
        virtual_key = 0;
    }

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
    if sent != 1 {
        write_status(&format!("SendInput failed with {}", unsafe {
            GetLastError()
        }));
    }
}

fn delay_milliseconds_precise(milliseconds: i32) {
    if milliseconds <= 0 {
        return;
    }

    let target = Duration::from_millis(milliseconds as u64);
    let start = Instant::now();
    if milliseconds > 2 {
        thread::sleep(Duration::from_millis((milliseconds - 1) as u64));
    }

    while start.elapsed() < target {
        std::hint::spin_loop();
    }
}

fn release_synthetic_forward_hold_if_needed() {
    let state = state();
    let _guard = state.forward_key_lock.lock().unwrap();
    release_synthetic_forward_hold_if_needed_no_lock(state);
}

fn release_synthetic_forward_hold_if_needed_no_lock(state: &AppState) {
    if !state.synthetic_forward_held.load(Ordering::Relaxed) {
        return;
    }

    send_keyboard_input(VK_W as u16, true);
    state.synthetic_forward_held.store(false, Ordering::Relaxed);
}

fn cleanup(state: &AppState) {
    if state
        .cleanup_started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    state.shutting_down.store(true, Ordering::Relaxed);
    state.queued_forward_taps.store(0, Ordering::Relaxed);
    unsafe {
        SetEvent(state.forward_event as _);
    }

    if let Some(worker) = state.worker_handle.lock().unwrap().take() {
        let _ = worker.join();
    }

    release_synthetic_forward_hold_if_needed();

    let hwnd = state.window();
    if !hwnd.is_null() {
        unsafe {
            UnregisterHotKey(hwnd, TOGGLE_HOTKEY_ID);
            UnregisterHotKey(hwnd, EXIT_HOTKEY_ID);
        }
    }

    let keyboard_hook = state.keyboard_hook.swap(0, Ordering::Relaxed) as HHOOK;
    if !keyboard_hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(keyboard_hook);
        }
    }

    let mouse_hook = state.mouse_hook.swap(0, Ordering::Relaxed) as HHOOK;
    if !mouse_hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(mouse_hook);
        }
    }

    remove_tray_icon(state);

    if state.forward_event != 0 {
        unsafe {
            CloseHandle(state.forward_event as _);
        }
    }

    let class_name = WINDOW_CLASS_NAME.get_or_init(|| wide("TapperHiddenWindow"));
    unsafe {
        UnregisterClassW(class_name.as_ptr(), GetModuleHandleW(null()) as HINSTANCE);
    }
}

fn write_status(message: &str) {
    let text = format!("[Tapper] {message}");
    let wide_message = wide(text);
    unsafe {
        OutputDebugStringW(wide_message.as_ptr());
    }
}

fn state() -> &'static Arc<AppState> {
    APP_STATE.get().expect("app state not initialized")
}

fn build_tray_text(enabled: bool) -> String {
    if enabled {
        "Tapper - enabled".to_string()
    } else {
        "Tapper - disabled".to_string()
    }
}

fn load_application_icon(current_exe: &Path) -> (isize, bool) {
    let exe = wide(current_exe.as_os_str());
    let icon = unsafe { ExtractIconW(null_mut(), exe.as_ptr(), 0) as isize };
    if icon > 1 {
        return (icon, true);
    }

    let fallback = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) } as isize;
    (fallback, false)
}

fn monotonic_millis() -> i64 {
    START_TIME.get_or_init(Instant::now).elapsed().as_millis() as i64
}

fn get_wheel_delta(mouse_data: u32) -> i16 {
    ((mouse_data >> 16) & 0xFFFF) as u16 as i16
}

fn query_process_image_path(process_id: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if handle.is_null() {
        return None;
    }

    let mut buffer = vec![0_u16; 32768];
    let mut size = buffer.len() as u32;
    let result = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    unsafe {
        CloseHandle(handle);
    }
    if result == 0 {
        return None;
    }

    Some(utf16_to_string(&buffer[..size as usize]))
}

fn try_hand_off_to_installed_copy(current_exe: &Path) -> bool {
    let installed_exe = get_installed_executable_path();
    let installed_dir = match installed_exe.parent() {
        Some(path) => path,
        None => return false,
    };

    if !installed_dir.exists() || paths_equal(current_exe, &installed_exe) {
        return false;
    }

    let current_version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_default();
    let installed_version = get_executable_version(&installed_exe);
    let installed_copy_running = is_process_running_from_path(&installed_exe);

    if installed_exe.exists() && current_version <= installed_version {
        if installed_copy_running {
            return true;
        }

        return try_start_process(&installed_exe);
    }

    try_start_installed_copy_update(current_exe, &installed_exe)
}

fn get_installed_executable_path() -> PathBuf {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|path| path.join("AppData").join("Local"))
        })
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Local"));

    local_app_data.join("Tapper").join("Tapper.exe")
}

fn get_executable_version(executable_path: &Path) -> Version {
    let filename = wide(executable_path.as_os_str());
    let mut handle = 0_u32;
    let size = unsafe { GetFileVersionInfoSizeW(filename.as_ptr(), &mut handle) };
    if size == 0 {
        return Version::default();
    }

    let mut data = vec![0_u8; size as usize];
    let success = unsafe {
        GetFileVersionInfoW(filename.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void)
    };
    if success == 0 {
        return Version::default();
    }

    let mut value_ptr: *mut c_void = null_mut();
    let mut value_len = 0_u32;
    let root = wide("\\");
    let queried = unsafe {
        VerQueryValueW(
            data.as_ptr() as *const c_void,
            root.as_ptr(),
            &mut value_ptr,
            &mut value_len,
        )
    };
    if queried == 0 || value_ptr.is_null() || value_len < size_of::<VsFixedFileInfo>() as u32 {
        return Version::default();
    }

    let info = unsafe { &*(value_ptr as *const VsFixedFileInfo) };
    Version {
        major: (info.dw_file_version_ms >> 16) as u16,
        minor: info.dw_file_version_ms as u16,
        patch: (info.dw_file_version_ls >> 16) as u16,
        build: info.dw_file_version_ls as u16,
    }
}

fn is_process_running_from_path(executable_path: &Path) -> bool {
    let target_process_name = match executable_path.file_name() {
        Some(name) => name.to_string_lossy().to_ascii_lowercase(),
        None => return false,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot == INVALID_HANDLE_VALUE {
        return false;
    }

    let current_process_id = unsafe { GetCurrentProcessId() };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..unsafe { zeroed() }
    };

    let mut found = false;
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while has_entry {
        if entry.th32ProcessID != current_process_id {
            let executable = utf16_nul_terminated_to_string(&entry.szExeFile);
            if executable.to_ascii_lowercase() == target_process_name
                && let Some(path) = query_process_image_path(entry.th32ProcessID)
                && paths_equal(Path::new(&path), executable_path)
            {
                found = true;
                break;
            }
        }

        has_entry = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }

    unsafe {
        CloseHandle(snapshot);
    }
    found
}

fn try_start_installed_copy_update(source_executable: &Path, target_executable: &Path) -> bool {
    let Some(source_directory) = source_executable.parent() else {
        return false;
    };
    let Some(target_directory) = target_executable.parent() else {
        return false;
    };

    let script_path = std::env::temp_dir().join(format!(
        "TapperSelfUpdate-{:x}{:x}.ps1",
        process::id(),
        monotonic_millis()
    ));

    let script = build_installed_copy_update_script(
        source_directory,
        target_directory,
        target_executable,
        process::id(),
    );

    if fs::write(&script_path, script).is_err() {
        return false;
    }

    let powershell_path = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|path| {
            path.join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"));

    Command::new(powershell_path)
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-File")
        .arg(&script_path)
        .current_dir(source_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

fn build_installed_copy_update_script(
    source_directory: &Path,
    target_directory: &Path,
    target_executable: &Path,
    current_process_id: u32,
) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$sourceDirectoryPath = '{source_directory}'
$targetDirectoryPath = '{target_directory}'
$targetExecutablePath = '{target_executable}'
$currentProcessId = {current_process_id}

function Test-SamePath {{
    param(
        [string]$leftPath,
        [string]$rightPath
    )

    if ([string]::IsNullOrWhiteSpace($leftPath) -or [string]::IsNullOrWhiteSpace($rightPath)) {{
        return $false
    }}

    return [string]::Equals(
        [System.IO.Path]::GetFullPath($leftPath),
        [System.IO.Path]::GetFullPath($rightPath),
        [System.StringComparison]::OrdinalIgnoreCase)
}}

$targetProcessName = [System.IO.Path]::GetFileNameWithoutExtension($targetExecutablePath)
$targetProcesses = @(Get-Process -Name $targetProcessName -ErrorAction SilentlyContinue | Where-Object {{
    try {{
        $_.Id -ne $currentProcessId -and (Test-SamePath $_.Path $targetExecutablePath)
    }}
    catch {{
        $false
    }}
}})

foreach ($targetProcess in $targetProcesses) {{
    Stop-Process -Id $targetProcess.Id -Force -ErrorAction SilentlyContinue
}}

if ($targetProcesses.Count -gt 0) {{
    Start-Sleep -Milliseconds 400
}}

New-Item -ItemType Directory -Path $targetDirectoryPath -Force | Out-Null

Get-ChildItem -LiteralPath $sourceDirectoryPath -Force | ForEach-Object {{
    $destinationPath = Join-Path $targetDirectoryPath $_.Name

    if ([string]::Equals($_.Name, 'tapper.settings.json', [System.StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $destinationPath)) {{
        return
    }}

    Copy-Item -LiteralPath $_.FullName -Destination $destinationPath -Recurse -Force
}}

Start-Process -FilePath $targetExecutablePath | Out-Null
Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
"#,
        source_directory = escape_powershell_literal(source_directory),
        target_directory = escape_powershell_literal(target_directory),
        target_executable = escape_powershell_literal(target_executable),
        current_process_id = current_process_id,
    )
}

fn escape_powershell_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn try_start_process(executable_path: &Path) -> bool {
    let current_dir = executable_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    Command::new(executable_path)
        .current_dir(current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim()
        .to_ascii_lowercase()
}

fn normalize_entries(values: &[String], fallback: &[&str]) -> Vec<String> {
    let mut cleaned = Vec::new();
    let mut seen = HashSet::new();

    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }

        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            cleaned.push(trimmed.to_string());
        }
    }

    if cleaned.is_empty() {
        fallback.iter().map(|value| (*value).to_string()).collect()
    } else {
        cleaned
    }
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn utf16_to_string(buffer: &[u16]) -> String {
    String::from_utf16_lossy(buffer)
}

fn utf16_nul_terminated_to_string(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    let os_string = OsString::from_wide(&buffer[..end]);
    os_string.to_string_lossy().to_string()
}

fn copy_utf16_buffer(value: &str, buffer: &mut [u16]) {
    buffer.fill(0);
    if buffer.is_empty() {
        return;
    }

    let encoded = wide(value);
    let max_copy = buffer.len().saturating_sub(1);
    let length = encoded.len().saturating_sub(1).min(max_copy);
    if length > 0 {
        buffer[..length].copy_from_slice(&encoded[..length]);
    }
    buffer[length] = 0;
}

fn last_error_message(prefix: &str) -> String {
    format!("{prefix} Win32 error: {}", unsafe { GetLastError() })
}

fn show_error_message(message: &str) {
    let title = wide("Tapper");
    let body = wide(message);
    unsafe {
        MessageBoxW(
            null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

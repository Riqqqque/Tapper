#![cfg(windows)]
#![windows_subsystem = "windows"]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString, c_void};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::thread_local;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, INVALID_HANDLE_VALUE, LPARAM,
    LRESULT, POINT, WAIT_OBJECT_0, WPARAM,
};
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateMutexW, CreateWaitableTimerExW,
    GetCurrentProcessId, INFINITE, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, SetEvent, SetWaitableTimerEx, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, RegisterHotKey, SendInput,
    UnregisterHotKey,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
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
    MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW,
    PostQuitMessage, RI_MOUSE_WHEEL, RIM_INPUT, RegisterClassW, SetForegroundWindow,
    SetWindowsHookExW, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, UnhookWindowsHookEx, UnregisterClassW, WH_KEYBOARD_LL, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WS_OVERLAPPED,
};

const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
const TOGGLE_HOTKEY_ID: i32 = 1;
const EXIT_HOTKEY_ID: i32 = 2;
const MENU_TOGGLE_ID: u32 = 1001;
const MENU_EXIT_ID: u32 = 1002;
const MENU_OPEN_FOLDER_ID: u32 = 1003;
const MENU_OPEN_LOG_ID: u32 = 1004;
const MENU_OPEN_SETTINGS_ID: u32 = 1005;
const TRAY_ICON_ID: u32 = 1;
const TARGET_WINDOW_CACHE_TTL_MS: i64 = 250;
const SEND_INPUT_ERROR_LOG_INTERVAL_MS: i64 = 1_000;
const KEY_STATE_RETRY_COUNT: usize = 3;
const MOD_CONTROL: u32 = 0x0002;
const MOD_NOREPEAT: u32 = 0x4000;
const VK_A: u32 = 0x41;
const VK_D: u32 = 0x44;
const VK_F6: u32 = 0x75;
const VK_F7: u32 = 0x76;
const VK_F8: u32 = 0x77;
const VK_F9: u32 = 0x78;
const VK_F10: u32 = 0x79;
const VK_W: u32 = 0x57;
const CREATE_NO_WINDOW: u32 = 0x08000000;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_TARGET_PROCESS_NAMES: [&str; 2] = ["r5apex.exe", "r5apex_dx12.exe"];

static APP_STATE: OnceLock<Arc<AppState>> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static START_TIME: OnceLock<Instant> = OnceLock::new();
static WINDOW_CLASS_NAME: OnceLock<Vec<u16>> = OnceLock::new();
thread_local! {
    static HIGH_RES_WAITABLE_TIMER: WaitableTimer = WaitableTimer::create();
}

const HOTKEY_PAIR_CANDIDATES: [HotkeyPairCandidate; 5] = [
    HotkeyPairCandidate {
        base_vk: VK_F8,
        key_name: "F8",
    },
    HotkeyPairCandidate {
        base_vk: VK_F7,
        key_name: "F7",
    },
    HotkeyPairCandidate {
        base_vk: VK_F9,
        key_name: "F9",
    },
    HotkeyPairCandidate {
        base_vk: VK_F6,
        key_name: "F6",
    },
    HotkeyPairCandidate {
        base_vk: VK_F10,
        key_name: "F10",
    },
];

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
        let parts = value.split('.').collect::<Vec<_>>();
        if parts.is_empty() || parts.len() > 4 || parts.iter().any(|part| part.is_empty()) {
            return None;
        }

        let mut parsed = [0_u16; 4];
        for (index, part) in parts.iter().enumerate() {
            parsed[index] = part.parse().ok()?;
        }

        Some(Self {
            major: parsed[0],
            minor: parsed[1],
            patch: parsed[2],
            build: parsed[3],
        })
    }
}

#[derive(Debug)]
struct AppState {
    base_dir: PathBuf,
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
    last_send_input_error_at_ms: AtomicI64,
    forward_event: isize,
    instance_mutex: isize,
    timer_resolution_enabled: AtomicBool,
    worker_handle: Mutex<Option<JoinHandle<()>>>,
    target_cache: Mutex<TargetWindowCache>,
    forward_key_lock: Mutex<()>,
    send_input_lock: Mutex<()>,
    tray_state: Mutex<TrayState>,
    hotkeys: Mutex<HotkeyBindings>,
    forward_key_spec: ForwardKeySpec,
}

#[derive(Debug, Default)]
struct TargetWindowCache {
    hwnd: isize,
    process_id: u32,
    is_match: bool,
    checked_at_ms: i64,
}

#[derive(Debug, Default)]
struct TrayState {
    icon: isize,
    icon_owned: bool,
    added: bool,
}

#[derive(Clone, Copy, Debug)]
struct HotkeyPairCandidate {
    base_vk: u32,
    key_name: &'static str,
}

#[derive(Clone, Debug)]
struct HotkeyBindings {
    toggle_display: Option<String>,
    exit_display: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct ForwardKeySpec {
    virtual_key: u16,
    scan_code: u16,
    base_flags: u32,
}

struct WaitableTimer {
    handle: isize,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self::bound("F8")
    }
}

impl WaitableTimer {
    fn create() -> Self {
        let handle = unsafe {
            CreateWaitableTimerExW(
                null(),
                null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        };
        Self {
            handle: handle as isize,
        }
    }

    fn wait_milliseconds(&self, milliseconds: i32) -> bool {
        if self.handle == 0 {
            return false;
        }

        let due_time_100ns = -i64::from(milliseconds).saturating_mul(10_000);
        let scheduled = unsafe {
            SetWaitableTimerEx(
                self.handle as _,
                &due_time_100ns,
                0,
                None,
                null(),
                null(),
                0,
            )
        };
        if scheduled == 0 {
            return false;
        }

        unsafe { WaitForSingleObject(self.handle as _, INFINITE) == WAIT_OBJECT_0 }
    }
}

impl Drop for WaitableTimer {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe {
                CloseHandle(self.handle as _);
            }
        }
    }
}

impl HotkeyBindings {
    fn bound(key_name: &str) -> Self {
        Self {
            toggle_display: Some(key_name.to_string()),
            exit_display: Some(format!("Ctrl+{key_name}")),
        }
    }

    fn tray_only() -> Self {
        Self {
            toggle_display: None,
            exit_display: None,
        }
    }

    fn summary_text(&self) -> String {
        match (&self.toggle_display, &self.exit_display) {
            (Some(toggle), Some(exit)) => format!("Hotkeys: {toggle} / {exit}"),
            _ => "Hotkeys: tray menu only".to_string(),
        }
    }

    fn toggle_menu_text(&self, enabled: bool) -> String {
        let action = if enabled {
            "Disable Assist"
        } else {
            "Enable Assist"
        };

        match &self.toggle_display {
            Some(toggle) => format!("{action} ({toggle})"),
            None => action.to_string(),
        }
    }

    fn exit_menu_text(&self) -> String {
        match &self.exit_display {
            Some(exit) => format!("Exit ({exit})"),
            None => "Exit".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

impl Settings {
    fn load(base_dir: &Path) -> Self {
        let path = base_dir.join("tapper.settings.json");
        let mut should_persist = false;
        let mut settings = match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Settings>(&contents) {
                Ok(parsed) => parsed,
                Err(_) => {
                    should_persist = true;
                    back_up_invalid_settings_file(&path);
                    write_status("tapper.settings.json was invalid. A clean config was restored.");
                    Settings::default()
                }
            },
            Err(_) => {
                should_persist = true;
                Settings::default()
            }
        };
        let original = settings.clone();
        settings.normalize();
        if settings != original {
            should_persist = true;
        }
        if should_persist {
            persist_settings(&path, &settings);
        }
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

        self.process_names =
            normalize_entries_with_required(&self.process_names, &DEFAULT_TARGET_PROCESS_NAMES);
        self.window_title_contains =
            normalize_entries(&self.window_title_contains, &["Apex Legends"]);
    }
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
            process_names: DEFAULT_TARGET_PROCESS_NAMES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            window_title_contains: vec!["Apex Legends".to_string()],
        }
    }
}

fn build_forward_key_spec() -> ForwardKeySpec {
    let scan_code = unsafe { MapVirtualKeyW(VK_W, MAPVK_VK_TO_VSC) as u16 };
    if scan_code != 0 {
        ForwardKeySpec {
            virtual_key: 0,
            scan_code,
            base_flags: KEYEVENTF_SCANCODE,
        }
    } else {
        ForwardKeySpec {
            virtual_key: VK_W as u16,
            scan_code: 0,
            base_flags: 0,
        }
    }
}

fn try_acquire_single_instance() -> Result<Option<isize>, String> {
    let mutex_name = wide(r"Local\TapperSingleInstanceMutex");
    let handle = unsafe { CreateMutexW(null(), 0, mutex_name.as_ptr()) };
    if handle.is_null() {
        return Err(last_error_message(
            "Unable to create the Tapper single-instance mutex.",
        ));
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        return Ok(None);
    }

    Ok(Some(handle as isize))
}

fn initialize_logging(base_dir: &Path) {
    let _ = fs::create_dir_all(base_dir);
    let log_path = base_dir.join("Tapper.log");
    if let Ok(metadata) = fs::metadata(&log_path)
        && metadata.len() > 512 * 1024
    {
        let _ = fs::remove_file(&log_path);
    }

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(
            file,
            "[+{}ms] Tapper {APP_VERSION} log started",
            monotonic_millis()
        );
    }

    let _ = LOG_PATH.set(log_path);
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
    initialize_logging(&base_dir);

    let instance_mutex = match try_acquire_single_instance() {
        Ok(Some(handle)) => handle,
        Ok(None) => {
            write_status("Another Tapper instance is already running.");
            return;
        }
        Err(error) => {
            show_error_message(&error);
            process::exit(1);
        }
    };

    let settings = Settings::load(&base_dir);
    let state = match AppState::new(base_dir, settings, instance_mutex) {
        Ok(state) => Arc::new(state),
        Err(error) => {
            show_error_message(&error);
            process::exit(1);
        }
    };

    let _ = APP_STATE.set(state.clone());
    write_status(&format!("Tapper {APP_VERSION} starting"));

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
    fn new(base_dir: PathBuf, settings: Settings, instance_mutex: isize) -> Result<Self, String> {
        let forward_event = unsafe { CreateEventW(null(), 0, 0, null()) };
        if forward_event.is_null() {
            return Err(last_error_message(
                "Unable to create the forward-tap event.",
            ));
        }

        let forward_key_spec = build_forward_key_spec();

        Ok(Self {
            base_dir,
            enabled: AtomicBool::new(settings.enabled_on_start),
            settings,
            a_down: AtomicBool::new(is_key_down(VK_A)),
            d_down: AtomicBool::new(is_key_down(VK_D)),
            w_down: AtomicBool::new(is_key_down(VK_W)),
            last_tap_at_ms: AtomicI64::new(0),
            queued_forward_taps: AtomicI32::new(0),
            synthetic_forward_held: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            cleanup_started: AtomicBool::new(false),
            window_handle: AtomicIsize::new(0),
            keyboard_hook: AtomicIsize::new(0),
            last_send_input_error_at_ms: AtomicI64::new(-SEND_INPUT_ERROR_LOG_INTERVAL_MS),
            forward_event: forward_event as isize,
            instance_mutex,
            timer_resolution_enabled: AtomicBool::new(false),
            worker_handle: Mutex::new(None),
            target_cache: Mutex::new(TargetWindowCache::default()),
            forward_key_lock: Mutex::new(()),
            send_input_lock: Mutex::new(()),
            tray_state: Mutex::new(TrayState::default()),
            hotkeys: Mutex::new(HotkeyBindings::default()),
            forward_key_spec,
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
    optimize_runtime_for_low_latency(state);
    register_raw_mouse_input(hwnd)?;
    register_hotkeys(state, hwnd);
    add_tray_icon(current_exe, state)?;
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
        WM_INPUT => {
            handle_raw_mouse_input(lparam as HRAWINPUT);
            if wparam == RIM_INPUT as usize {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            } else {
                0
            }
        }
        WM_CLOSE => {
            unsafe {
                PostQuitMessage(0);
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

fn open_app_directory() {
    let state = state();
    let _ = Command::new("explorer.exe")
        .arg(&state.base_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

fn open_log_file() {
    if let Some(log_path) = LOG_PATH.get()
        && log_path.exists()
    {
        let _ = Command::new("notepad.exe")
            .arg(log_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    } else {
        open_app_directory();
    }
}

fn open_settings_file() {
    let settings_path = state().base_dir.join("tapper.settings.json");
    let _ = Command::new("notepad.exe")
        .arg(settings_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
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
    copy_utf16_buffer(&build_tray_text(state), &mut data.szTip);

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
    copy_utf16_buffer(&build_tray_text(state), &mut data.szTip);

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
    let hotkeys = state.hotkeys.lock().unwrap().clone();
    let hotkey_text = wide(hotkeys.summary_text());
    let folder_text = wide("Open App Folder");
    let log_text = wide("Open Log");
    let settings_text = wide("Open Settings");
    let toggle_text = wide(hotkeys.toggle_menu_text(state.is_enabled()));
    let exit_text = wide(hotkeys.exit_menu_text());

    unsafe {
        AppendMenuW(
            menu,
            MF_STRING | MF_DISABLED | MF_GRAYED,
            0,
            status_text.as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING | MF_DISABLED | MF_GRAYED,
            0,
            hotkey_text.as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(
            menu,
            MF_STRING,
            MENU_OPEN_FOLDER_ID as usize,
            folder_text.as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING,
            MENU_OPEN_LOG_ID as usize,
            log_text.as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING,
            MENU_OPEN_SETTINGS_ID as usize,
            settings_text.as_ptr(),
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
        } else if command as u32 == MENU_OPEN_FOLDER_ID {
            open_app_directory();
        } else if command as u32 == MENU_OPEN_LOG_ID {
            open_log_file();
        } else if command as u32 == MENU_OPEN_SETTINGS_ID {
            open_settings_file();
        } else if command as u32 == MENU_EXIT_ID {
            exit_application();
        }

        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}

fn register_hotkeys(state: &AppState, hwnd: HWND) {
    for candidate in HOTKEY_PAIR_CANDIDATES {
        let toggle =
            unsafe { RegisterHotKey(hwnd, TOGGLE_HOTKEY_ID, MOD_NOREPEAT, candidate.base_vk) };
        if toggle == 0 {
            continue;
        }

        let exit = unsafe {
            RegisterHotKey(
                hwnd,
                EXIT_HOTKEY_ID,
                MOD_CONTROL | MOD_NOREPEAT,
                candidate.base_vk,
            )
        };
        if exit == 0 {
            unsafe {
                UnregisterHotKey(hwnd, TOGGLE_HOTKEY_ID);
            }
            continue;
        }

        *state.hotkeys.lock().unwrap() = HotkeyBindings::bound(candidate.key_name);
        if candidate.base_vk != VK_F8 {
            write_status(&format!(
                "F8 was unavailable. Using {} / Ctrl+{} instead.",
                candidate.key_name, candidate.key_name
            ));
        }
        return;
    }

    *state.hotkeys.lock().unwrap() = HotkeyBindings::tray_only();
    write_status("Global hotkeys were unavailable. Use the tray menu to control Tapper.");
}

fn optimize_runtime_for_low_latency(state: &AppState) {
    if unsafe { timeBeginPeriod(1) } == 0 {
        state
            .timer_resolution_enabled
            .store(true, Ordering::Relaxed);
    }
}

fn register_raw_mouse_input(hwnd: HWND) -> Result<(), String> {
    let device = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x02,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };

    let registered =
        unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) };
    if registered == 0 {
        return Err(last_error_message("Unable to register raw mouse input."));
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

    state
        .keyboard_hook
        .store(keyboard_hook as isize, Ordering::Relaxed);
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

fn handle_raw_mouse_input(raw_input_handle: HRAWINPUT) {
    let wheel_delta = match try_get_wheel_delta_from_raw_input(raw_input_handle) {
        Some(value) => value,
        None => return,
    };

    if should_trigger_for_wheel_delta(wheel_delta) && should_send_forward_tap() {
        queue_forward_tap_burst();
    }
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

    if !refresh_target_window_state(state) {
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
        .spawn(move || {
            forward_tap_worker_loop(&worker_state);
        })
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
    let max_live_burst_taps = state
        .settings
        .forward_tap_burst_count
        .clamp(1, state.settings.max_queued_forward_taps);
    let max_queued_taps = if queue_single_held_forward_tap {
        1
    } else {
        max_live_burst_taps
    };

    loop {
        let current = state.queued_forward_taps.load(Ordering::Relaxed);
        if current >= max_queued_taps {
            return;
        }

        let target = std::cmp::min(max_queued_taps, current + tap_count);
        if state
            .queued_forward_taps
            .compare_exchange(current, target, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            if current == 0 {
                unsafe {
                    SetEvent(state.forward_event as _);
                }
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
            .compare_exchange(current, current - 1, Ordering::AcqRel, Ordering::Relaxed)
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

    refresh_target_window_state(state)
}

fn refresh_target_window_state(state: &AppState) -> bool {
    is_target_window_active(state)
}

fn is_target_window_active(state: &AppState) -> bool {
    let window_handle = unsafe { GetForegroundWindow() };
    if window_handle.is_null() {
        return false;
    }

    let process_id = get_window_process_id(window_handle);
    if process_id == 0 {
        return false;
    }

    let now = monotonic_millis();
    {
        let cache = state.target_cache.lock().unwrap();
        if cache.hwnd == window_handle as isize
            && cache.process_id == process_id
            && now.saturating_sub(cache.checked_at_ms) < TARGET_WINDOW_CACHE_TTL_MS
        {
            return cache.is_match;
        }
    }

    let matches_target = matches_target_window(state, window_handle, process_id);
    let mut cache = state.target_cache.lock().unwrap();
    cache.hwnd = window_handle as isize;
    cache.process_id = process_id;
    cache.is_match = matches_target;
    cache.checked_at_ms = now;
    matches_target
}

fn matches_target_window(state: &AppState, window_handle: HWND, process_id: u32) -> bool {
    let process_name = try_get_foreground_process_name(process_id);
    let title = try_get_window_title(window_handle);
    matches_target_identity(&state.settings, &process_name, &title)
}

fn matches_target_identity(settings: &Settings, process_name: &str, title: &str) -> bool {
    if !process_name.trim().is_empty() {
        return matches_configured_process(settings, process_name);
    }

    matches_configured_title(settings, title)
}

fn try_get_foreground_process_name(process_id: u32) -> String {
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
    let lower_name = name.trim().to_ascii_lowercase();
    lower_name
        .strip_suffix(".exe")
        .unwrap_or(&lower_name)
        .to_string()
}

fn send_forward_tap(state: &AppState) {
    let _forward_key_guard = state.forward_key_lock.lock().unwrap();
    if !state.settings.block_when_forward_held && state.w_down.load(Ordering::Relaxed) {
        if !send_forward_key_input(state, true) {
            return;
        }

        delay_milliseconds_precise(state.settings.held_forward_retap_release_ms);
        if state.w_down.load(Ordering::Relaxed) {
            let restored = send_forward_key_input_with_retry(state, false);
            state
                .synthetic_forward_held
                .store(restored, Ordering::Relaxed);
        } else {
            state.synthetic_forward_held.store(false, Ordering::Relaxed);
        }
        return;
    }

    release_synthetic_forward_hold_if_needed_no_lock(state);
    if !send_forward_key_input(state, false) {
        return;
    }

    state.synthetic_forward_held.store(true, Ordering::Relaxed);
    delay_milliseconds_precise(state.settings.forward_tap_hold_ms);
    release_synthetic_forward_hold_if_needed_no_lock(state);
}

fn send_forward_key_input_with_retry(state: &AppState, key_up: bool) -> bool {
    for _ in 0..KEY_STATE_RETRY_COUNT {
        if send_forward_key_input(state, key_up) {
            return true;
        }
        thread::yield_now();
    }

    false
}

fn send_forward_key_input(state: &AppState, key_up: bool) -> bool {
    let _send_input_guard = state.send_input_lock.lock().unwrap();
    let mut flags = state.forward_key_spec.base_flags;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: state.forward_key_spec.virtual_key,
                wScan: state.forward_key_spec.scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
    if sent == 1 {
        return true;
    }

    log_send_input_failure(state, unsafe { GetLastError() });
    false
}

fn log_send_input_failure(state: &AppState, error: u32) {
    let now = monotonic_millis();
    let previous = state.last_send_input_error_at_ms.load(Ordering::Relaxed);
    if now.saturating_sub(previous) < SEND_INPUT_ERROR_LOG_INTERVAL_MS {
        return;
    }

    state
        .last_send_input_error_at_ms
        .store(now, Ordering::Relaxed);
    write_status(&format!("SendInput failed with {error}"));
}

fn delay_milliseconds_precise(milliseconds: i32) {
    if milliseconds <= 0 {
        return;
    }

    let waited = HIGH_RES_WAITABLE_TIMER.with(|timer| timer.wait_milliseconds(milliseconds));
    if waited {
        return;
    }

    thread::sleep(Duration::from_millis(milliseconds as u64));
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

    if state.w_down.load(Ordering::Relaxed) {
        state.synthetic_forward_held.store(false, Ordering::Relaxed);
        return;
    }

    if send_forward_key_input_with_retry(state, true) {
        state.synthetic_forward_held.store(false, Ordering::Relaxed);

        if state.w_down.load(Ordering::Relaxed) {
            let restored = send_forward_key_input_with_retry(state, false);
            state
                .synthetic_forward_held
                .store(restored, Ordering::Relaxed);
        }
    }
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

    remove_tray_icon(state);

    if !hwnd.is_null() {
        unsafe {
            DestroyWindow(hwnd);
        }
    }

    if state.forward_event != 0 {
        unsafe {
            CloseHandle(state.forward_event as _);
        }
    }

    if state.instance_mutex != 0 {
        unsafe {
            CloseHandle(state.instance_mutex as _);
        }
    }

    if state
        .timer_resolution_enabled
        .swap(false, Ordering::Relaxed)
    {
        unsafe {
            timeEndPeriod(1);
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

    if let Some(log_path) = LOG_PATH.get()
        && let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path)
    {
        let _ = writeln!(file, "[+{}ms] {}", monotonic_millis(), message);
    }
}

fn back_up_invalid_settings_file(path: &Path) {
    if !path.exists() {
        return;
    }

    let backup_path = path.with_file_name("tapper.settings.invalid.json");
    let _ = fs::remove_file(&backup_path);
    let _ = fs::rename(path, backup_path);
}

fn persist_settings(path: &Path, settings: &Settings) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(serialized) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(path, format!("{serialized}\r\n"));
    }
}

fn state() -> &'static Arc<AppState> {
    APP_STATE.get().expect("app state not initialized")
}

fn build_tray_text(state: &AppState) -> String {
    let enabled = if state.is_enabled() {
        "enabled"
    } else {
        "disabled"
    };
    let hotkeys = state.hotkeys.lock().unwrap().summary_text();
    format!("Tapper {APP_VERSION} - {enabled} - {hotkeys}")
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

fn try_get_wheel_delta_from_raw_input(raw_input_handle: HRAWINPUT) -> Option<i16> {
    let mut raw_input = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let data_result = unsafe {
        GetRawInputData(
            raw_input_handle,
            RID_INPUT,
            (&mut raw_input as *mut RAWINPUT).cast(),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if data_result == u32::MAX || data_result < size_of::<RAWINPUT>() as u32 {
        return None;
    }

    if raw_input.header.dwType != RIM_TYPEMOUSE {
        return None;
    }

    let mouse = unsafe { raw_input.data.mouse };
    let button_state = unsafe { mouse.Anonymous.Anonymous };
    wheel_delta_from_raw_mouse(button_state.usButtonFlags, button_state.usButtonData)
}

fn wheel_delta_from_raw_mouse(button_flags: u16, button_data: u16) -> Option<i16> {
    if (u32::from(button_flags) & RI_MOUSE_WHEEL) == 0 {
        return None;
    }

    Some(button_data as i16)
}

fn is_key_down(virtual_key: u32) -> bool {
    unsafe { (GetAsyncKeyState(virtual_key as i32) & i16::MIN) != 0 }
}

fn get_window_process_id(window_handle: HWND) -> u32 {
    let mut process_id = 0_u32;
    unsafe {
        GetWindowThreadProcessId(window_handle, &mut process_id);
    }
    process_id
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
    let Some(installed_exe) = get_installed_executable_path() else {
        return false;
    };
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

fn get_installed_executable_path() -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|path| path.join("AppData").join("Local"))
        })?;

    Some(local_app_data.join("Tapper").join("Tapper.exe"))
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
        source_executable,
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
    source_executable: &Path,
    source_directory: &Path,
    target_directory: &Path,
    target_executable: &Path,
    current_process_id: u32,
) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$sourceExecutablePath = '{source_executable}'
$sourceDirectoryPath = '{source_directory}'
$targetDirectoryPath = '{target_directory}'
$targetExecutablePath = '{target_executable}'
$currentProcessId = {current_process_id}
$stagingDirectoryPath = Join-Path ([System.IO.Path]::GetTempPath()) ('TapperStage-' + [Guid]::NewGuid().ToString('N'))
$backupDirectoryPath = Join-Path ([System.IO.Path]::GetTempPath()) ('TapperBackup-' + [Guid]::NewGuid().ToString('N'))
$installMutationStarted = $false
$targetProcessesStopped = $false
$preserveBackup = $false
$rollbackSucceeded = $true

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

function Remove-TapperInstallContent {{
    param([string]$path)

    Get-ChildItem -LiteralPath $path -Force -ErrorAction SilentlyContinue | Where-Object {{
        -not $_.Name.StartsWith('unins', [System.StringComparison]::OrdinalIgnoreCase)
    }} | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}}

try {{
    New-Item -ItemType Directory -Path $stagingDirectoryPath -Force | Out-Null

    Copy-Item -LiteralPath $sourceExecutablePath -Destination (Join-Path $stagingDirectoryPath 'Tapper.exe') -Force
    foreach ($optionalName in @('tapper.settings.json', 'README.md')) {{
        $optionalSourcePath = Join-Path $sourceDirectoryPath $optionalName
        if (Test-Path -LiteralPath $optionalSourcePath) {{
            Copy-Item -LiteralPath $optionalSourcePath -Destination (Join-Path $stagingDirectoryPath $optionalName) -Force
        }}
    }}

    $sourceLogoPath = Join-Path $sourceDirectoryPath 'assets\logo.png'
    if (Test-Path -LiteralPath $sourceLogoPath) {{
        $stagedAssetsPath = Join-Path $stagingDirectoryPath 'assets'
        New-Item -ItemType Directory -Path $stagedAssetsPath -Force | Out-Null
        Copy-Item -LiteralPath $sourceLogoPath -Destination (Join-Path $stagedAssetsPath 'logo.png') -Force
    }}

    if (-not (Test-Path -LiteralPath (Join-Path $stagingDirectoryPath 'Tapper.exe'))) {{
        throw 'Tapper.exe was not staged.'
    }}

    New-Item -ItemType Directory -Path $targetDirectoryPath -Force | Out-Null
    New-Item -ItemType Directory -Path $backupDirectoryPath -Force | Out-Null

    foreach ($targetProcess in $targetProcesses) {{
        Stop-Process -Id $targetProcess.Id -Force -ErrorAction SilentlyContinue
    }}

    foreach ($targetProcess in $targetProcesses) {{
        Wait-Process -Id $targetProcess.Id -Timeout 5 -ErrorAction SilentlyContinue
    }}

    $targetProcessesStopped = $targetProcesses.Count -gt 0
    $installMutationStarted = $true

    Get-ChildItem -LiteralPath $targetDirectoryPath -Force -ErrorAction SilentlyContinue | Where-Object {{
        -not $_.Name.StartsWith('unins', [System.StringComparison]::OrdinalIgnoreCase)
    }} | ForEach-Object {{
        Move-Item -LiteralPath $_.FullName -Destination (Join-Path $backupDirectoryPath $_.Name) -Force
    }}

    Get-ChildItem -LiteralPath $stagingDirectoryPath -Force | ForEach-Object {{
        $destinationPath = Join-Path $targetDirectoryPath $_.Name
        Copy-Item -LiteralPath $_.FullName -Destination $destinationPath -Recurse -Force
    }}

    if (-not (Test-Path -LiteralPath $targetExecutablePath)) {{
        throw 'Updated Tapper.exe was not copied into place.'
    }}

    Start-Process -FilePath $targetExecutablePath -ErrorAction Stop | Out-Null
}}
catch {{
    $updateError = $_

    if ($installMutationStarted) {{
        try {{
            Remove-TapperInstallContent -path $targetDirectoryPath

            if (Test-Path -LiteralPath $backupDirectoryPath) {{
                Get-ChildItem -LiteralPath $backupDirectoryPath -Force -ErrorAction SilentlyContinue | ForEach-Object {{
                    Move-Item -LiteralPath $_.FullName -Destination (Join-Path $targetDirectoryPath $_.Name) -Force
                }}
            }}
        }}
        catch {{
            $preserveBackup = $true
            $rollbackSucceeded = $false
        }}
    }}

    if ($targetProcessesStopped -and $rollbackSucceeded -and (Test-Path -LiteralPath $targetExecutablePath)) {{
        Start-Process -FilePath $targetExecutablePath -ErrorAction SilentlyContinue | Out-Null
    }}

    throw $updateError
}}
finally {{
    Remove-Item -LiteralPath $stagingDirectoryPath -Recurse -Force -ErrorAction SilentlyContinue
    if (-not $preserveBackup) {{
        Remove-Item -LiteralPath $backupDirectoryPath -Recurse -Force -ErrorAction SilentlyContinue
    }}
    Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}}

"#,
        source_executable = escape_powershell_literal(source_executable),
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

fn normalize_entries_with_required(values: &[String], required: &[&str]) -> Vec<String> {
    let mut combined = values.to_vec();
    combined.extend(required.iter().map(|value| (*value).to_string()));
    normalize_entries(&combined, required)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_fills_missing_parts_with_zero() {
        assert_eq!(
            Version::parse("1.2"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 0,
                build: 0,
            })
        );
        assert_eq!(Version::parse("1.bad"), None);
        assert_eq!(Version::parse("1.2.3.4.5"), None);
        assert_eq!(Version::parse("1..3"), None);
    }

    #[test]
    fn normalize_entries_deduplicates_and_falls_back() {
        let values = vec![
            " r5apex.exe ".to_string(),
            "R5APEX.EXE".to_string(),
            "".to_string(),
        ];
        assert_eq!(
            normalize_entries(&values, &["fallback.exe"]),
            vec!["r5apex.exe"]
        );
        assert_eq!(
            normalize_entries(&[], &["fallback.exe"]),
            vec!["fallback.exe"]
        );
    }

    #[test]
    fn settings_normalization_adds_supported_apex_executables() {
        let mut settings = Settings {
            process_names: vec!["r5apex.exe".to_string()],
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(
            settings.process_names,
            vec!["r5apex.exe", "r5apex_dx12.exe"]
        );
    }

    #[test]
    fn normalize_process_name_strips_exe_case_insensitively() {
        assert_eq!(normalize_process_name(" R5APEX.EXE "), "r5apex");
        assert_eq!(normalize_process_name("r5apex.exe"), "r5apex");
        assert_eq!(normalize_process_name("r5apex"), "r5apex");
    }

    #[test]
    fn target_identity_only_uses_title_when_process_lookup_fails() {
        let settings = Settings::default();

        assert!(matches_target_identity(
            &settings,
            "r5apex.exe",
            "Apex Legends"
        ));
        assert!(matches_target_identity(
            &settings,
            "r5apex_dx12.exe",
            "Apex Legends"
        ));
        assert!(!matches_target_identity(
            &settings,
            "chrome.exe",
            "Apex Legends - Google Chrome"
        ));
        assert!(matches_target_identity(&settings, "", "Apex Legends"));
    }

    #[test]
    fn normalize_path_is_case_and_separator_insensitive() {
        assert_eq!(
            normalize_path(Path::new(r"C:/Games/Tapper/Tapper.exe")),
            normalize_path(Path::new(r"c:\games\tapper\Tapper.exe"))
        );
    }

    #[test]
    fn escape_powershell_literal_doubles_single_quotes() {
        assert_eq!(
            escape_powershell_literal(Path::new(r"C:\O'Reilly\Tapper")),
            "C:\\O''Reilly\\Tapper"
        );
    }

    #[test]
    fn update_script_uses_staging_and_rollback() {
        let script = build_installed_copy_update_script(
            Path::new(r"C:\Source\RenamedTapper.exe"),
            Path::new(r"C:\Source"),
            Path::new(r"C:\Installed"),
            Path::new(r"C:\Installed\Tapper.exe"),
            42,
        );

        assert!(script.contains("$stagingDirectoryPath"));
        assert!(script.contains("$backupDirectoryPath"));
        assert!(script.contains("$installMutationStarted = $false"));
        assert!(script.contains("$targetProcessesStopped = $false"));
        assert!(script.contains("$preserveBackup = $false"));
        assert!(script.contains("$rollbackSucceeded = $true"));
        assert!(script.contains("if ($installMutationStarted)"));
        assert!(script.contains(
            "if ($targetProcessesStopped -and $rollbackSucceeded -and (Test-Path -LiteralPath $targetExecutablePath))"
        ));
        assert!(script.contains(r"$sourceExecutablePath = 'C:\Source\RenamedTapper.exe'"));
        assert!(!script.contains("Get-ChildItem -LiteralPath $sourceDirectoryPath -Force"));
        assert!(
            script
                .find("Copy-Item -LiteralPath $sourceExecutablePath")
                .unwrap()
                < script.find("Stop-Process -Id $targetProcess.Id").unwrap()
        );
        assert!(script.contains("Move-Item -LiteralPath $_.FullName -Destination (Join-Path $backupDirectoryPath $_.Name) -Force"));
        assert!(script.contains("Remove-TapperInstallContent -path $targetDirectoryPath"));
        assert!(script.contains("if (-not $preserveBackup)"));
        assert!(script.contains("$rollbackSucceeded = $false"));
        assert!(script.contains("throw $updateError"));
        assert!(script.contains("throw 'Tapper.exe was not staged.'"));
    }

    #[test]
    fn wheel_delta_parser_only_accepts_wheel_messages() {
        assert_eq!(wheel_delta_from_raw_mouse(0, 120), None);
        assert_eq!(
            wheel_delta_from_raw_mouse(RI_MOUSE_WHEEL as u16, 120),
            Some(120)
        );
        assert_eq!(
            wheel_delta_from_raw_mouse(RI_MOUSE_WHEEL as u16, (-120i16) as u16),
            Some(-120)
        );
    }
}

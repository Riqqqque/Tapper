using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

internal static class Program
{
    private const int WhKeyboardLl = 13;
    private const int WhMouseLl = 14;
    private const int WmKeyDown = 0x0100;
    private const int WmKeyUp = 0x0101;
    private const int WmSysKeyDown = 0x0104;
    private const int WmSysKeyUp = 0x0105;
    private const int WmMouseWheel = 0x020A;
    private const int WmHotkey = 0x0312;
    private const uint LlkhfInjected = 0x00000010;
    private const uint InputKeyboard = 1;
    private const uint KeyeventfKeyup = 0x0002;
    private const uint KeyeventfScancode = 0x0008;
    private const uint MapvkVkToVsc = 0;
    private const uint ModControl = 0x0002;
    private const uint ModNoRepeat = 0x4000;
    private const uint VkA = 0x41;
    private const uint VkD = 0x44;
    private const uint VkF8 = 0x77;
    private const uint VkW = 0x57;
    private const int ToggleHotkeyId = 1;
    private const int ExitHotkeyId = 2;

    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true,
        ReadCommentHandling = JsonCommentHandling.Skip,
        WriteIndented = true
    };

    private static readonly object CleanupSync = new();
    private static readonly object ConsoleSync = new();
    private static readonly object ForwardKeySync = new();
    private static readonly int InputSize = Marshal.SizeOf<Input>();
    private static readonly AutoResetEvent ForwardTapQueued = new(initialState: false);

    private static HookProc? keyboardHookProc;
    private static HookProc? mouseHookProc;
    private static Thread? forwardTapWorkerThread;
    private static IntPtr keyboardHook;
    private static IntPtr mouseHook;
    private static TapperSettings settings = TapperSettings.Load(JsonOptions);
    private static volatile bool enabled = settings.EnabledOnStart;
    private static volatile bool aDown;
    private static volatile bool dDown;
    private static volatile bool wDown;
    private static long lastTapAtMs;
    private static int queuedForwardTaps;
    private static bool syntheticForwardHeld;
    private static bool cleanupStarted;
    private static volatile bool shuttingDown;

    [STAThread]
    private static int Main()
    {
        if (!OperatingSystem.IsWindows())
        {
            Console.Error.WriteLine("This helper only runs on Windows.");
            return 1;
        }

        Console.Title = "Tapper";
        Console.CancelKeyPress += OnCancelKeyPress;
        AppDomain.CurrentDomain.ProcessExit += (_, _) => Cleanup();

        keyboardHookProc = KeyboardHookCallback;
        mouseHookProc = MouseHookCallback;

        keyboardHook = InstallHook(WhKeyboardLl, keyboardHookProc);
        mouseHook = InstallHook(WhMouseLl, mouseHookProc);
        RegisterHotkeys();
        StartForwardTapWorker();
        WriteBanner();

        try
        {
            RunMessageLoop();
            return 0;
        }
        finally
        {
            Cleanup();
        }
    }

    private static void OnCancelKeyPress(object? sender, ConsoleCancelEventArgs args)
    {
        args.Cancel = true;
        NativeMethods.PostQuitMessage(0);
    }

    private static void RunMessageLoop()
    {
        while (NativeMethods.GetMessage(out var msg, IntPtr.Zero, 0, 0) > 0)
        {
            if (msg.message == WmHotkey)
            {
                HandleHotkey((int)msg.wParam);
            }

            NativeMethods.TranslateMessage(ref msg);
            NativeMethods.DispatchMessage(ref msg);
        }
    }

    private static void HandleHotkey(int hotkeyId)
    {
        switch (hotkeyId)
        {
            case ToggleHotkeyId:
                enabled = !enabled;
                if (!enabled)
                {
                    Interlocked.Exchange(ref queuedForwardTaps, 0);
                    ReleaseSyntheticForwardHoldIfNeeded();
                }

                WriteStatus(enabled ? "assist enabled" : "assist disabled");
                break;
            case ExitHotkeyId:
                WriteStatus("shutting down");
                NativeMethods.PostQuitMessage(0);
                break;
        }
    }

    private static IntPtr KeyboardHookCallback(int nCode, IntPtr wParam, IntPtr lParam)
    {
        if (nCode >= 0)
        {
            var data = Marshal.PtrToStructure<KbdLlHookStruct>(lParam);
            if ((data.flags & LlkhfInjected) == 0)
            {
                var message = unchecked((uint)wParam.ToInt64());
                var isDown = message == WmKeyDown || message == WmSysKeyDown;
                var isUp = message == WmKeyUp || message == WmSysKeyUp;

                if (isDown || isUp)
                {
                    var newState = isDown;
                    switch (data.vkCode)
                    {
                        case VkA:
                            aDown = newState;
                            break;
                        case VkD:
                            dDown = newState;
                            break;
                        case VkW:
                            wDown = newState;
                            if (!newState)
                            {
                                ReleaseSyntheticForwardHoldIfNeeded();
                            }

                            break;
                    }
                }
            }
        }

        return NativeMethods.CallNextHookEx(keyboardHook, nCode, wParam, lParam);
    }

    private static IntPtr MouseHookCallback(int nCode, IntPtr wParam, IntPtr lParam)
    {
        if (nCode >= 0 && unchecked((uint)wParam.ToInt64()) == WmMouseWheel)
        {
            var data = Marshal.PtrToStructure<MsLlHookStruct>(lParam);
            if (GetWheelDelta(data.mouseData) < 0 && ShouldSendForwardTap())
            {
                QueueForwardTapBurst();
            }
        }

        return NativeMethods.CallNextHookEx(mouseHook, nCode, wParam, lParam);
    }

    private static bool ShouldSendForwardTap()
    {
        if (!enabled)
        {
            return false;
        }

        if (settings.RequireStrafeKey && !(aDown || dDown))
        {
            return false;
        }

        if (settings.BlockWhenForwardHeld && wDown)
        {
            return false;
        }

        if (!IsTargetWindowActive())
        {
            return false;
        }

        var now = Environment.TickCount64;
        var previous = Interlocked.Read(ref lastTapAtMs);
        if (now - previous < settings.ForwardTapCooldownMs)
        {
            return false;
        }

        Interlocked.Exchange(ref lastTapAtMs, now);
        return true;
    }

    private static void StartForwardTapWorker()
    {
        forwardTapWorkerThread = new Thread(ForwardTapWorkerLoop)
        {
            IsBackground = true,
            Name = "TapperForwardTapWorker"
        };

        forwardTapWorkerThread.Start();
    }

    private static void ForwardTapWorkerLoop()
    {
        while (true)
        {
            ForwardTapQueued.WaitOne();
            if (shuttingDown)
            {
                return;
            }

            while (TryTakeQueuedForwardTap())
            {
                if (!CanProcessQueuedForwardTap())
                {
                    continue;
                }

                SendForwardTap();
                DelayMillisecondsPrecise(settings.ForwardTapPulseGapMs);
            }
        }
    }

    private static void QueueForwardTapBurst()
    {
        while (true)
        {
            var current = Volatile.Read(ref queuedForwardTaps);
            if (current >= settings.MaxQueuedForwardTaps)
            {
                return;
            }

            var target = Math.Min(settings.MaxQueuedForwardTaps, current + settings.ForwardTapBurstCount);
            if (Interlocked.CompareExchange(ref queuedForwardTaps, target, current) == current)
            {
                ForwardTapQueued.Set();
                return;
            }
        }
    }

    private static bool TryTakeQueuedForwardTap()
    {
        while (true)
        {
            var current = Volatile.Read(ref queuedForwardTaps);
            if (current == 0)
            {
                return false;
            }

            if (Interlocked.CompareExchange(ref queuedForwardTaps, current - 1, current) == current)
            {
                return true;
            }
        }
    }

    private static bool CanProcessQueuedForwardTap()
    {
        if (!enabled)
        {
            return false;
        }

        if (settings.RequireStrafeKey && !(aDown || dDown))
        {
            return false;
        }

        if (settings.BlockWhenForwardHeld && wDown)
        {
            return false;
        }

        return IsTargetWindowActive();
    }

    private static bool IsTargetWindowActive()
    {
        var windowHandle = NativeMethods.GetForegroundWindow();
        if (windowHandle == IntPtr.Zero)
        {
            return false;
        }

        var processName = TryGetForegroundProcessName(windowHandle);
        if (MatchesConfiguredProcess(processName))
        {
            return true;
        }

        var title = TryGetWindowTitle(windowHandle);
        return MatchesConfiguredTitle(title);
    }

    private static string TryGetForegroundProcessName(IntPtr windowHandle)
    {
        NativeMethods.GetWindowThreadProcessId(windowHandle, out var processId);
        if (processId == 0)
        {
            return string.Empty;
        }

        try
        {
            using var process = Process.GetProcessById((int)processId);
            return process.ProcessName;
        }
        catch
        {
            return string.Empty;
        }
    }

    private static string TryGetWindowTitle(IntPtr windowHandle)
    {
        var length = NativeMethods.GetWindowTextLength(windowHandle);
        if (length == 0)
        {
            return string.Empty;
        }

        var builder = new StringBuilder(length + 1);
        _ = NativeMethods.GetWindowText(windowHandle, builder, builder.Capacity);
        return builder.ToString();
    }

    private static bool MatchesConfiguredProcess(string candidate)
    {
        if (string.IsNullOrWhiteSpace(candidate))
        {
            return false;
        }

        foreach (var configuredName in settings.ProcessNames)
        {
            if (NormalizeProcessName(candidate).Equals(NormalizeProcessName(configuredName), StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        return false;
    }

    private static bool MatchesConfiguredTitle(string candidate)
    {
        if (string.IsNullOrWhiteSpace(candidate))
        {
            return false;
        }

        foreach (var configuredTitle in settings.WindowTitleContains)
        {
            if (candidate.Contains(configuredTitle, StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        return false;
    }

    private static string NormalizeProcessName(string name)
    {
        return name.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)
            ? name[..^4]
            : name;
    }

    private static short GetWheelDelta(uint mouseData)
    {
        return unchecked((short)((mouseData >> 16) & 0xFFFF));
    }

    private static void SendForwardTap()
    {
        lock (ForwardKeySync)
        {
            if (!settings.BlockWhenForwardHeld && wDown)
            {
                SendKeyboardInput((ushort)VkW, keyUp: true);
                DelayMillisecondsPrecise(settings.HeldForwardRetapReleaseMs);
                SendKeyboardInput((ushort)VkW, keyUp: false);
                syntheticForwardHeld = true;
                return;
            }

            ReleaseSyntheticForwardHoldIfNeededNoLock();
            SendKeyboardInput((ushort)VkW, keyUp: false);
            DelayMillisecondsPrecise(settings.ForwardTapHoldMs);
            SendKeyboardInput((ushort)VkW, keyUp: true);
        }
    }

    private static void SendKeyboardInput(ushort virtualKey, bool keyUp)
    {
        var scanCode = (ushort)NativeMethods.MapVirtualKey(virtualKey, MapvkVkToVsc);
        var flags = keyUp ? KeyeventfKeyup : 0;

        if (scanCode != 0)
        {
            flags |= KeyeventfScancode;
            virtualKey = 0;
        }

        var input = new Input
        {
            type = InputKeyboard,
            U = new InputUnion
            {
                ki = new KeybdInput
                {
                    wVk = virtualKey,
                    wScan = scanCode,
                    dwFlags = flags,
                    dwExtraInfo = IntPtr.Zero
                }
            }
        };

        var sent = NativeMethods.SendInput(1, new[] { input }, InputSize);
        if (sent != 1)
        {
            WriteStatus($"SendInput failed with {Marshal.GetLastWin32Error()}");
        }
    }

    private static void DelayMillisecondsPrecise(int milliseconds)
    {
        if (milliseconds <= 0)
        {
            return;
        }

        var targetTicks = milliseconds * Stopwatch.Frequency / 1000L;
        var stopwatch = Stopwatch.StartNew();

        if (milliseconds > 2)
        {
            Thread.Sleep(milliseconds - 1);
        }

        while (stopwatch.ElapsedTicks < targetTicks)
        {
            Thread.SpinWait(64);
        }
    }

    private static void ReleaseSyntheticForwardHoldIfNeeded()
    {
        lock (ForwardKeySync)
        {
            ReleaseSyntheticForwardHoldIfNeededNoLock();
        }
    }

    private static void ReleaseSyntheticForwardHoldIfNeededNoLock()
    {
        if (!syntheticForwardHeld)
        {
            return;
        }

        SendKeyboardInput((ushort)VkW, keyUp: true);
        syntheticForwardHeld = false;
    }

    private static IntPtr InstallHook(int hookType, HookProc callback)
    {
        var hookHandle = NativeMethods.SetWindowsHookEx(hookType, callback, NativeMethods.GetModuleHandle(null), 0);
        if (hookHandle == IntPtr.Zero)
        {
            throw new InvalidOperationException($"Unable to install hook {hookType}. Win32 error: {Marshal.GetLastWin32Error()}");
        }

        return hookHandle;
    }

    private static void RegisterHotkeys()
    {
        if (!NativeMethods.RegisterHotKey(IntPtr.Zero, ToggleHotkeyId, ModNoRepeat, VkF8))
        {
            throw new InvalidOperationException($"Unable to register F8 toggle hotkey. Win32 error: {Marshal.GetLastWin32Error()}");
        }

        if (!NativeMethods.RegisterHotKey(IntPtr.Zero, ExitHotkeyId, ModControl | ModNoRepeat, VkF8))
        {
            throw new InvalidOperationException($"Unable to register Ctrl+F8 exit hotkey. Win32 error: {Marshal.GetLastWin32Error()}");
        }
    }

    private static void Cleanup()
    {
        lock (CleanupSync)
        {
            if (cleanupStarted)
            {
                return;
            }

            cleanupStarted = true;
            shuttingDown = true;
        }

        ForwardTapQueued.Set();
        forwardTapWorkerThread?.Join(millisecondsTimeout: 250);
        ReleaseSyntheticForwardHoldIfNeeded();

        NativeMethods.UnregisterHotKey(IntPtr.Zero, ToggleHotkeyId);
        NativeMethods.UnregisterHotKey(IntPtr.Zero, ExitHotkeyId);

        if (keyboardHook != IntPtr.Zero)
        {
            _ = NativeMethods.UnhookWindowsHookEx(keyboardHook);
            keyboardHook = IntPtr.Zero;
        }

        if (mouseHook != IntPtr.Zero)
        {
            _ = NativeMethods.UnhookWindowsHookEx(mouseHook);
            mouseHook = IntPtr.Zero;
        }
    }

    private static void WriteBanner()
    {
        lock (ConsoleSync)
        {
            Console.WriteLine("Tapper is running.");
            Console.WriteLine("F8 toggles the assist on or off.");
            Console.WriteLine("Ctrl+F8 exits the app.");
            Console.WriteLine($"Current state: {(enabled ? "enabled" : "disabled")}.");
            Console.WriteLine($"Target process names: {string.Join(", ", settings.ProcessNames)}");
            Console.WriteLine($"Target title matches: {string.Join(", ", settings.WindowTitleContains)}");
            Console.WriteLine($"Forward pulses per wheel notch: {settings.ForwardTapBurstCount}");
            Console.WriteLine();
        }
    }

    private static void WriteStatus(string message)
    {
        lock (ConsoleSync)
        {
            Console.WriteLine($"[{DateTime.Now:HH:mm:ss}] {message}");
        }
    }

    private sealed class TapperSettings
    {
        public bool EnabledOnStart { get; set; } = true;
        public int ForwardTapHoldMs { get; set; } = 6;
        public int ForwardTapCooldownMs { get; set; } = 0;
        public int ForwardTapBurstCount { get; set; } = 3;
        public int ForwardTapPulseGapMs { get; set; } = 0;
        public int HeldForwardRetapReleaseMs { get; set; } = 2;
        public int MaxQueuedForwardTaps { get; set; } = 24;
        public bool RequireStrafeKey { get; set; } = true;
        public bool BlockWhenForwardHeld { get; set; } = false;
        public string[] ProcessNames { get; set; } = ["r5apex.exe"];
        public string[] WindowTitleContains { get; set; } = ["Apex Legends"];

        public static TapperSettings Load(JsonSerializerOptions options)
        {
            var path = Path.Combine(AppContext.BaseDirectory, "tapper.settings.json");
            if (!File.Exists(path))
            {
                return new TapperSettings();
            }

            try
            {
                var loaded = JsonSerializer.Deserialize<TapperSettings>(File.ReadAllText(path), options) ?? new TapperSettings();
                loaded.Normalize();
                return loaded;
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"Failed to load settings from '{path}': {ex.Message}. Using defaults.");
                return new TapperSettings();
            }
        }

        private void Normalize()
        {
            ForwardTapHoldMs = Math.Clamp(ForwardTapHoldMs, 1, 25);
            ForwardTapCooldownMs = Math.Clamp(ForwardTapCooldownMs, 0, 25);
            ForwardTapBurstCount = Math.Clamp(ForwardTapBurstCount, 1, 6);
            ForwardTapPulseGapMs = Math.Clamp(ForwardTapPulseGapMs, 0, 10);
            HeldForwardRetapReleaseMs = Math.Clamp(HeldForwardRetapReleaseMs, 1, 10);
            MaxQueuedForwardTaps = Math.Clamp(MaxQueuedForwardTaps, 1, 64);
            ProcessNames = NormalizeEntries(ProcessNames, ["r5apex.exe"]);
            WindowTitleContains = NormalizeEntries(WindowTitleContains, ["Apex Legends"]);
        }

        private static string[] NormalizeEntries(string[]? values, string[] fallback)
        {
            var cleaned = values?
                .Where(value => !string.IsNullOrWhiteSpace(value))
                .Select(value => value.Trim())
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .ToArray();

            return cleaned is { Length: > 0 } ? cleaned : fallback;
        }
    }

    private delegate IntPtr HookProc(int code, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    private struct Point
    {
        public int x;
        public int y;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Msg
    {
        public IntPtr hwnd;
        public uint message;
        public UIntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public Point pt;
        public uint lPrivate;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct KbdLlHookStruct
    {
        public uint vkCode;
        public uint scanCode;
        public uint flags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MsLlHookStruct
    {
        public Point pt;
        public uint mouseData;
        public uint flags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Input
    {
        public uint type;
        public InputUnion U;
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct InputUnion
    {
        [FieldOffset(0)]
        public MouseInput mi;

        [FieldOffset(0)]
        public KeybdInput ki;

        [FieldOffset(0)]
        public HardwareInput hi;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MouseInput
    {
        public int dx;
        public int dy;
        public uint mouseData;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct KeybdInput
    {
        public ushort wVk;
        public ushort wScan;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HardwareInput
    {
        public uint uMsg;
        public ushort wParamL;
        public ushort wParamH;
    }

    private static class NativeMethods
    {
        [DllImport("user32.dll", SetLastError = true)]
        public static extern IntPtr SetWindowsHookEx(int idHook, HookProc lpfn, IntPtr hMod, uint dwThreadId);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool UnhookWindowsHookEx(IntPtr hhk);

        [DllImport("user32.dll")]
        public static extern IntPtr CallNextHookEx(IntPtr hhk, int nCode, IntPtr wParam, IntPtr lParam);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern IntPtr GetModuleHandle(string? lpModuleName);

        [DllImport("user32.dll")]
        public static extern int GetMessage(out Msg lpMsg, IntPtr hWnd, uint wMsgFilterMin, uint wMsgFilterMax);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool TranslateMessage([In] ref Msg lpMsg);

        [DllImport("user32.dll")]
        public static extern IntPtr DispatchMessage([In] ref Msg lpMsg);

        [DllImport("user32.dll")]
        public static extern void PostQuitMessage(int nExitCode);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool RegisterHotKey(IntPtr hWnd, int id, uint fsModifiers, uint vk);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool UnregisterHotKey(IntPtr hWnd, int id);

        [DllImport("user32.dll")]
        public static extern IntPtr GetForegroundWindow();

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetWindowTextLength(IntPtr hWnd);

        [DllImport("user32.dll")]
        public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

        [DllImport("user32.dll", SetLastError = true)]
        public static extern uint SendInput(uint nInputs, Input[] pInputs, int cbSize);

        [DllImport("user32.dll")]
        public static extern uint MapVirtualKey(uint uCode, uint uMapType);
    }
}

# Tapper

![Tapper logo](assets/logo.png)

Small Windows helper for Apex tap-strafing with `Mouse Wheel`.

The app is now built as a native Rust executable and does not need the .NET runtime on the target machine.

## What it does

- Leaves your wheel input alone so Apex still sees your normal scroll-wheel input.
- Sends a short burst of `W` taps alongside each wheel-up or wheel-down notch.
- Only triggers when the target Apex window is active.
- Only triggers while `A` or `D` is held.

## Controls

- `F8`: toggle the assist on or off
- `Ctrl+F8`: close the helper
- If another app already owns `F8`, Tapper automatically falls back to a nearby function-key pair or keeps working from the tray menu.

## Use

1. Bind jump to the wheel directions you actually use in Apex.
2. Install Tapper with `TapperSetup-<version>.exe`, or run the built `Tapper.exe` directly if you are testing locally.
3. Tapper starts in the system tray on launch so it stays out of the way.
4. When you want the assist, leave it enabled and use `A` or `D` with your mouse turn as usual.
5. Launching Tapper again does not stack another copy on top of the running one.

## Installer

- Run `scripts\build-installer.ps1` to build the Rust release, refresh `dist`, and compile the setup exe.
- The shareable installer is written to `installer-dist\TapperSetup-<version>.exe`.
- The installer defaults to `%LocalAppData%\Tapper`, creates a Start Menu shortcut, and can optionally add a desktop shortcut or startup shortcut.
- Upgrades fully refresh the installed Tapper files and bundled config so stale runtime files do not linger between versions.

## Config

Edit `tapper.settings.json` next to the executable if you want to tune it.

Tapper writes a simple `Tapper.log` file next to the executable, and the tray menu can open the app folder, log, or settings file directly.
If the config file is missing or invalid, Tapper restores a clean one automatically and keeps the bad file as `tapper.settings.invalid.json`.

- `forwardTapHoldMs`: how long the injected `W` press stays down
- `forwardTapCooldownMs`: minimum gap between wheel events before another burst can queue
- `forwardTapBurstCount`: how many `W` taps each wheel notch creates
- `forwardTapPulseGapMs`: extra gap between taps inside the same burst
- `heldForwardRetapReleaseMs`: how long `W` is released before it is reasserted when you are already holding it
- `maxQueuedForwardTaps`: cap for queued taps so scroll spam does not backlog forever
- `triggerOnWheelDown`: allow wheel-down notches to trigger the assist
- `triggerOnWheelUp`: allow wheel-up notches to trigger the assist
- `requireStrafeKey`: only fire when `A` or `D` is held
- `blockWhenForwardHeld`: skip the assist if you want it disabled while `W` is already held
- `processNames` / `windowTitleContains`: change these if your Apex window is not detected

# Tapper

Small Windows helper for Apex tap-strafing with `Mouse Wheel`.

## What it does

- Leaves your wheel input alone so Apex still sees your normal scroll-wheel input.
- Sends a short burst of `W` taps alongside each wheel-up or wheel-down notch.
- Only triggers when the target Apex window is active.
- Only triggers while `A` or `D` is held.

## Controls

- `F8`: toggle the assist on or off
- `Ctrl+F8`: close the helper

## Use

1. Bind jump to the wheel directions you actually use in Apex.
2. Run the built `Tapper.exe`.
3. When you want the assist, leave it enabled and use `A` or `D` with your mouse turn as usual.

## Config

Edit `tapper.settings.json` next to the executable if you want to tune it.

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

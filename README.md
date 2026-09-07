# codex-bar-win

A small Windows tray utility for quickly viewing Codex / ChatGPT Work usage.

> Unofficial community utility. Not affiliated with or endorsed by OpenAI. It relies on ChatGPT/Codex usage endpoints that may change over time.

## Current behavior

- Runs from a single portable executable and does not create app-owned config, cache, or log files.
- Enforces a single running instance. Launching the EXE again activates the existing instance instead of creating another tray process.
- Left-click the tray icon to toggle the usage popup.
- Shows the 5-hour and weekly usage windows, reset times, account email / subscription tier, and available reset credits when present.
- Defaults to remaining quota; the tray menu can switch between remaining and used quota for the current run.
- Refreshes immediately when the popup opens, with a 10-second duplicate-request guard.
- Background refresh can be switched between 5 minutes, 30 minutes, and 1 hour for the current run. The default is 5 minutes.
- Reset-credit details refresh at most once per hour and failures do not affect the main usage display.
- Hides the popup after 8 seconds or when it loses focus.
- Uses a Direct2D / DirectWrite UI styled to match WinUI 3 / Fluent visual conventions, including light/dark theme and Windows accent color.
- Right-click the tray icon for Refresh, display-mode selection, refresh-interval selection, and Exit.
- Reads the existing Codex login from `CODEX_HOME\auth.json` or `%USERPROFILE%\.codex\auth.json`.
- Uses the current Windows system proxy automatically through reqwest system-proxy support.

All tray-menu preferences are intentionally in-memory only for now, so restarting restores the defaults (remaining quota + 5-minute refresh).

## Build

```powershell
cargo build --release
```

The executable is created at:

```text
target\release\codex-bar-win.exe
```

GitHub Actions runs tests, builds the portable executable, and uploads it as the `codex-bar-win-x64-portable` artifact. A successful push to `main` publishes the Cargo package version as a GitHub Release when that version has not already been released.

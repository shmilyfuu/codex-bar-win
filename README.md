# codex-bar-win

A small Windows tray utility for quickly viewing Codex usage.

## Current behavior

- Runs from a single portable executable.
- Left-click the tray icon to open a compact usage popup.
- Shows the 5-hour window, weekly window, and reset times.
- Refreshes immediately when the popup opens.
- Refreshes in the background every 5 minutes.
- Hides the popup after 8 seconds or when it loses focus.
- Right-click the tray icon for Refresh and Exit.
- Reads the existing Codex login from `CODEX_HOME\auth.json` or `%USERPROFILE%\.codex\auth.json`.
- Uses the current Windows system proxy automatically through reqwest system-proxy support.

## Build

```powershell
cargo build --release
```

The executable is created at:

```text
target\release\codex-bar-win.exe
```

GitHub Actions also uploads the release build as the `codex-bar-win-x64-portable` artifact.

# GrintaHub Clicker - Windows Setup & Troubleshooting

---

## Requirements

| Requirement | Details |
|---|---|
| **Google Chrome** | Must be installed (standard location) |
| **Internet access** | Port 60000 outbound for Oxylabs HTTP proxy |
| **No admin needed** | App runs as normal user |

---

## Installation

1. Download the `.exe` installer from GitHub Releases
2. Run it - Windows SmartScreen will warn (unsigned app)
3. Click **"More info"** then **"Run anyway"**
4. App installs to `C:\Users\<user>\AppData\Local\grintahub-clicker\`

---

## File Paths on Windows

All data is stored in user-writable directories. Nothing is written to the install folder.

| What | Path | Notes |
|---|---|---|
| **Config** | `%APPDATA%\grintahub-clicker\config.json` | Proxy creds, settings |
| **Logs** | `%APPDATA%\grintahub-clicker\logs\` | Daily rolling log files |
| **Browser data** | `%TEMP%\grintahub-clicker\browser_data\{session-id}` | Per-session Chrome profiles, auto-cleaned |
| **App install** | `%LOCALAPPDATA%\grintahub-clicker\` | NSIS installer location |

### How to find these folders

- Press `Win + R`, type `%APPDATA%\grintahub-clicker` and hit Enter (config + logs)
- Press `Win + R`, type `%TEMP%\grintahub-clicker` and hit Enter (browser data)

---

## Log Files

Log files are saved automatically with daily rotation:

```
%APPDATA%\grintahub-clicker\logs\grintahub-clicker.log.2026-01-26
```

- New log file created each day
- Contains all INFO/WARN/ERROR messages with timestamps
- Useful for debugging proxy, browser, or session issues
- Use `get_log_dir` API command from frontend to get the exact path

---

## Chrome Detection

The app looks for Chrome in these locations (in order):

1. `C:\Program Files\Google\Chrome\Application\chrome.exe`
2. `C:\Program Files (x86)\Google\Chrome\Application\chrome.exe`
3. `%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe`

If Chrome is not found, you get:
> **"Google Chrome not found. Please install Chrome from https://www.google.com/chrome/ and restart the app."**

---

## Proxy Test

The proxy test uses explicit HTTP basic auth (not URL-embedded credentials) for Windows compatibility:

- Connects to `pr.oxylabs.io:60000` (HTTP proxy)
- Uses `Proxy::basic_auth()` for reliable authentication
- System proxy settings are bypassed (`.no_proxy()`)
- TLS cert issues through proxy are handled (`danger_accept_invalid_certs`)

### If proxy test fails

1. Check proxy credentials (use Show button to verify password)
2. Check internet connection
3. Check if port 60000 outbound is blocked (firewall/corporate network)
4. Check logs at `%APPDATA%\grintahub-clicker\logs\`

---

## Browser Sessions

Each browser session:

- Gets a unique Chrome profile in `%TEMP%\grintahub-clicker\browser_data\{id}`
- Gets a unique Saudi IP via Oxylabs session ID
- Uses a local proxy forwarder on `127.0.0.1:{port}` (starting from port 18080)
- Runs with anti-detection evasions (webdriver removal, plugin mocking, etc.)

### If session creation fails

1. **Chrome not installed** - Install Google Chrome
2. **Port in use** - Another app using ports 18080+ (rare)
3. **Antivirus blocking** - Add exception for the app
4. **Check logs** for detailed error message

---

## Windows Firewall

The app needs outbound access to:

| Destination | Port | Protocol | Purpose |
|---|---|---|---|
| `pr.oxylabs.io` | 60000 | HTTP | Proxy connection |
| `api.ipify.org` | 443 | HTTPS | IP detection |
| `grintahub.com` | 443 | HTTPS | Target site |
| `127.0.0.1` | 18080+ | TCP | Local proxy forwarder (internal) |

Localhost connections (127.0.0.1) should never be blocked by firewall.

---

## Windows Defender / Antivirus

The app automates Chrome which may trigger antivirus alerts:

- **Chrome launched with flags** - Some AV flags `--disable-blink-features` etc.
- **Multiple Chrome instances** - Unusual pattern may trigger alerts
- **Solution**: Add the app folder to antivirus exclusions

---

## Troubleshooting Checklist

| Issue | Check |
|---|---|
| App won't start | SmartScreen blocked it - click "More info" > "Run anyway" |
| Session creation fails | Is Google Chrome installed? |
| Proxy test fails | Check credentials, check port 60000 not blocked |
| No logs found | Check `%APPDATA%\grintahub-clicker\logs\` |
| Browser closes immediately | Check logs for error, might be antivirus |
| Slow performance | Reduce concurrent sessions, increase delays |
| CAPTCHA detected | Google detected automation, IP will rotate |

---

## Changes Log

### Proxy Test Fix
- Changed from URL-embedded credentials to explicit `Proxy::basic_auth()`
- Added `.no_proxy()` to prevent Windows system proxy interference
- Added `danger_accept_invalid_certs(true)` for proxy tunnel TLS issues

### Chrome Detection
- Added `find_chrome()` checking standard Windows install paths
- Clear error message if Chrome not found
- Explicitly passes detected Chrome path to chromiumoxide

### File Logging
- Added `tracing-appender` for daily rolling log files
- Logs saved to `%APPDATA%\grintahub-clicker\logs\`
- Both console and file logging active simultaneously

### Password Visibility
- Proxy password field has Show/Hide toggle button

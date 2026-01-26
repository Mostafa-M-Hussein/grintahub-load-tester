//! Zombie Chrome process cleanup
//!
//! Detects and kills orphaned Chrome/Chromium processes that belong to
//! GrintaHub sessions but are no longer tracked by the BrowserPool.

use std::collections::HashSet;
use std::sync::Arc;
use tracing::info;

use crate::browser::BrowserPool;

/// Cleanup orphaned Chrome processes.
///
/// Finds Chrome processes launched by GrintaHub (identified by `grintahub-clicker`
/// in their command line) whose session ID is not in the active BrowserPool.
/// Returns the number of killed processes.
pub async fn cleanup_zombie_chromes(pool: &Arc<BrowserPool>) -> u32 {
    #[cfg(target_os = "windows")]
    {
        cleanup_zombie_chromes_windows(pool).await
    }

    #[cfg(not(target_os = "windows"))]
    {
        cleanup_zombie_chromes_unix(pool).await
    }
}

/// Extract session ID from a Chrome command line containing --user-data-dir.
///
/// Looks for the pattern: `browser_data\{session_id}` or `browser_data/{session_id}`
fn extract_session_id_from_cmdline(cmdline: &str) -> Option<String> {
    let marker = "browser_data";
    if let Some(pos) = cmdline.find(marker) {
        let after = &cmdline[pos + marker.len()..];
        // Skip path separator (\ or /)
        let after = after.trim_start_matches(|c: char| c == '\\' || c == '/');
        // Take until next space, quote, or separator
        let session_id: String = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != '\\' && *c != '/')
            .collect();
        if !session_id.is_empty() {
            return Some(session_id);
        }
    }
    None
}

/// Get active session IDs from the pool
async fn get_active_session_ids(pool: &Arc<BrowserPool>) -> HashSet<String> {
    pool.get_all_session_info()
        .await
        .into_iter()
        .map(|info| info.id)
        .collect()
}

#[cfg(target_os = "windows")]
async fn cleanup_zombie_chromes_windows(pool: &Arc<BrowserPool>) -> u32 {
    use std::process::Command;
    use tracing::debug;

    // Try WMIC first (available on Windows 10)
    let output = match Command::new("wmic")
        .args(["process", "where", "Name='chrome.exe'", "get", "ProcessId,CommandLine", "/FORMAT:CSV"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            debug!("[Zombie] WMIC not available ({}), trying PowerShell", e);
            // Fallback: PowerShell (Windows 11+)
            match Command::new("powershell")
                .args(["-NoProfile", "-Command",
                    "Get-Process chrome -ErrorAction SilentlyContinue | ForEach-Object { $id=$_.Id; $cmd=(Get-CimInstance Win32_Process -Filter \"ProcessId=$id\" -ErrorAction SilentlyContinue).CommandLine; \"$id|$cmd\" }"])
                .output()
            {
                Ok(o) => o,
                Err(e2) => {
                    debug!("[Zombie] Cannot enumerate Chrome processes: {}", e2);
                    return 0;
                }
            }
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let active_sessions = get_active_session_ids(pool).await;
    let grintahub_marker = "grintahub-clicker";

    let mut killed = 0u32;

    for line in stdout.lines() {
        if !line.contains(grintahub_marker) {
            continue;
        }

        if let Some(session_id) = extract_session_id_from_cmdline(line) {
            if !active_sessions.contains(&session_id) {
                // Extract PID from the line
                if let Some(pid) = extract_pid_from_line(line) {
                    info!("[Zombie] Killing orphaned Chrome PID {} (session: {})", pid, session_id);
                    let _ = Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .output();
                    killed += 1;
                }
            }
        }
    }

    if killed > 0 {
        info!("[Zombie] Cleaned up {} orphaned Chrome processes", killed);
    }

    killed
}

#[cfg(not(target_os = "windows"))]
async fn cleanup_zombie_chromes_unix(pool: &Arc<BrowserPool>) -> u32 {
    use std::process::Command;

    let output = match Command::new("ps")
        .args(["aux"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return 0,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let active_sessions = get_active_session_ids(pool).await;

    let mut killed = 0u32;

    for line in stdout.lines() {
        if !line.contains("grintahub-clicker") || !line.contains("chrome") {
            continue;
        }

        if let Some(session_id) = extract_session_id_from_cmdline(line) {
            if !active_sessions.contains(&session_id) {
                // Extract PID (second field in ps aux output)
                if let Some(pid) = line.split_whitespace().nth(1).and_then(|s| s.parse::<u32>().ok()) {
                    info!("[Zombie] Killing orphaned Chrome PID {} (session: {})", pid, session_id);
                    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
                    killed += 1;
                }
            }
        }
    }

    if killed > 0 {
        info!("[Zombie] Cleaned up {} orphaned Chrome processes", killed);
    }

    killed
}

/// Extract PID from WMIC CSV or PowerShell output line.
///
/// WMIC CSV format: `Node,CommandLine,ProcessId`
/// PowerShell format: `PID|CommandLine`
#[allow(dead_code)]
fn extract_pid_from_line(line: &str) -> Option<u32> {
    // Try pipe-separated format first (PowerShell)
    if line.contains('|') {
        return line.split('|').next().and_then(|s| s.trim().parse::<u32>().ok());
    }

    // WMIC CSV: last numeric field is the PID
    line.split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .last()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_session_id_windows_path() {
        let cmdline = r#"chrome.exe --user-data-dir=C:\Users\user\AppData\Local\Temp\grintahub-clicker\browser_data\abc12345_0 --disable-blink-features"#;
        assert_eq!(extract_session_id_from_cmdline(cmdline), Some("abc12345_0".to_string()));
    }

    #[test]
    fn test_extract_session_id_unix_path() {
        let cmdline = "chrome --user-data-dir=/tmp/grintahub-clicker/browser_data/def67890_1 --headless";
        assert_eq!(extract_session_id_from_cmdline(cmdline), Some("def67890_1".to_string()));
    }

    #[test]
    fn test_extract_session_id_no_match() {
        let cmdline = "chrome.exe --user-data-dir=C:\\Users\\user\\Default";
        assert_eq!(extract_session_id_from_cmdline(cmdline), None);
    }

    #[test]
    fn test_extract_pid_from_wmic_csv() {
        let line = "NODE,\"chrome.exe --user-data-dir=...\",12345";
        assert_eq!(extract_pid_from_line(line), Some(12345));
    }

    #[test]
    fn test_extract_pid_from_powershell() {
        let line = "12345|chrome.exe --user-data-dir=...";
        assert_eq!(extract_pid_from_line(line), Some(12345));
    }
}

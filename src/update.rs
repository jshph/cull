use std::sync::mpsc;
use std::thread;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const VERSION_URL: &str = "https://getcull.fyi/version.json";

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
}

/// Check for updates in a background thread. Returns a receiver that will
/// eventually contain `Some(UpdateInfo)` if a newer version is available,
/// or `None` if we're up to date (or the check failed silently).
pub fn check_for_updates() -> mpsc::Receiver<Option<UpdateInfo>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = check_version();
        let _ = tx.send(result);
    });
    rx
}

fn check_version() -> Option<UpdateInfo> {
    let resp = ureq::get(VERSION_URL).call().ok()?;
    let body: serde_json::Value = resp.into_json().ok()?;
    let latest = body.get("version")?.as_str()?;
    let download_url = body.get("download_url")?.as_str()?.to_string();

    if is_newer(latest, CURRENT_VERSION) {
        Some(UpdateInfo {
            version: latest.to_string(),
            download_url,
        })
    } else {
        None
    }
}

/// Simple semver comparison: "0.2.0" > "0.1.0"
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|p| p.parse().ok()).collect()
    };
    let l = parse(latest);
    let c = parse(current);
    l > c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }
}

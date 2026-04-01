use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

// Public key for license verification (pair lives in keygen/private.key).
// Generated at development time — replace when rotating keys.
const PUBLIC_KEY_BYTES: [u8; 32] = [
    0x2a, 0xa4, 0x03, 0xc5, 0xc7, 0x97, 0x48, 0x84,
    0xe4, 0x54, 0xf7, 0x27, 0x1d, 0x23, 0xe7, 0x55,
    0x98, 0xa9, 0x43, 0x41, 0x3e, 0xe1, 0xb6, 0x3a,
    0x7d, 0x0d, 0xe9, 0x55, 0x4e, 0x34, 0xe9, 0xfe,
];

// ── types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LicenseType {
    Trial,
    Personal,
    Pro,
}

impl std::fmt::Display for LicenseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trial => write!(f, "Trial"),
            Self::Personal => write!(f, "Personal"),
            Self::Pro => write!(f, "Pro"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicensePayload {
    pub id: String,
    pub email: String,
    pub license_type: LicenseType,
    pub issued_at: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone)]
pub enum LicenseStatus {
    Licensed { license_type: LicenseType, email: String },
    Trial { days_remaining: u32 },
    Expired,
    Unlicensed,
}

// ── validation ─────────────────────────────────────────────────────────────

/// Parse and verify a license key string: `cull-<base64_payload>.<base64_signature>`
pub fn validate_license_key(key: &str) -> LicenseStatus {
    let key = key.trim();

    let rest = match key.strip_prefix("cull-") {
        Some(r) => r,
        None => return LicenseStatus::Unlicensed,
    };

    let (payload_b64, sig_b64) = match rest.rsplit_once('.') {
        Some(pair) => pair,
        None => return LicenseStatus::Unlicensed,
    };

    let payload_bytes = match URL_SAFE_NO_PAD.decode(payload_b64) {
        Ok(b) => b,
        Err(_) => return LicenseStatus::Unlicensed,
    };

    let sig_bytes = match URL_SAFE_NO_PAD.decode(sig_b64) {
        Ok(b) => b,
        Err(_) => return LicenseStatus::Unlicensed,
    };

    // Verify signature
    let verifying_key = match VerifyingKey::from_bytes(&PUBLIC_KEY_BYTES) {
        Ok(k) => k,
        Err(_) => return LicenseStatus::Unlicensed,
    };

    let signature = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(_) => return LicenseStatus::Unlicensed,
    };

    if verifying_key.verify(&payload_bytes, &signature).is_err() {
        return LicenseStatus::Unlicensed;
    }

    // Deserialize payload
    let payload: LicensePayload = match serde_json::from_slice(&payload_bytes) {
        Ok(p) => p,
        Err(_) => return LicenseStatus::Unlicensed,
    };

    // Check expiry
    if let Some(ref expires) = payload.expires_at {
        if let Ok(exp) = chrono::NaiveDateTime::parse_from_str(expires, "%Y-%m-%dT%H:%M:%S") {
            let now = chrono::Utc::now().naive_utc();
            if now > exp {
                if payload.license_type == LicenseType::Trial {
                    return LicenseStatus::Trial { days_remaining: 0 };
                }
                return LicenseStatus::Expired;
            }
            if payload.license_type == LicenseType::Trial {
                let remaining = (exp - now).num_days().max(0) as u32;
                return LicenseStatus::Trial { days_remaining: remaining };
            }
        }
    }

    LicenseStatus::Licensed {
        license_type: payload.license_type,
        email: payload.email,
    }
}

// ── persistence ────────────────────────────────────────────────────────────

fn license_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("cull")
        .join("license.key")
}

/// Load and validate the license key from `~/.config/cull/license.key`.
pub fn load_license() -> LicenseStatus {
    let path = license_path();
    match std::fs::read_to_string(&path) {
        Ok(key) => validate_license_key(&key),
        Err(_) => LicenseStatus::Unlicensed,
    }
}

/// Save a license key string to `~/.config/cull/license.key`.
pub fn save_license(key: &str) -> std::io::Result<()> {
    let path = license_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, key.trim())
}

/// Human-readable summary of the license status.
pub fn license_display_text(status: &LicenseStatus) -> &str {
    match status {
        LicenseStatus::Licensed { license_type: LicenseType::Personal, .. } => "Licensed (Personal)",
        LicenseStatus::Licensed { license_type: LicenseType::Pro, .. } => "Licensed (Pro)",
        LicenseStatus::Licensed { license_type: LicenseType::Trial, .. } => "Trial",
        LicenseStatus::Trial { days_remaining: 0 } => "Trial expired",
        LicenseStatus::Trial { .. } => "Trial",
        LicenseStatus::Expired => "License expired",
        LicenseStatus::Unlicensed => "Unlicensed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_key_roundtrip() {
        // Key generated by cull-keygen with the current private key
        let key = "cull-eyJpZCI6IjQyZDJlYTJhLWM1NzMtNGY3MS1hYjk5LWY2OWI0ZjIyMzRiMSIsImVtYWlsIjoiam9zaEBleGFtcGxlLmNvbSIsImxpY2Vuc2VfdHlwZSI6InBlcnNvbmFsIiwiaXNzdWVkX2F0IjoiMjAyNi0wMy0zMVQyMTo0ODoyMCIsImV4cGlyZXNfYXQiOm51bGx9.aGKieFTq2VuycxNzQYAWXccg4ezBlEWIcaT8lTPbrl3p45-cfCjmLW53gGwTi5levaavFDIjg7_b_7z_2-9uDg";
        let status = validate_license_key(key);
        match status {
            LicenseStatus::Licensed { ref license_type, ref email } => {
                assert_eq!(*license_type, LicenseType::Personal);
                assert_eq!(email, "josh@example.com");
            }
            other => panic!("Expected Licensed, got {:?}", other),
        }
    }

    #[test]
    fn test_invalid_key() {
        assert!(matches!(validate_license_key("garbage"), LicenseStatus::Unlicensed));
        assert!(matches!(validate_license_key("cull-bad.sig"), LicenseStatus::Unlicensed));
    }

    #[test]
    fn test_tampered_key() {
        // Valid format but tampered payload (changed one char)
        let key = "cull-XyJpZCI6IjQyZDJlYTJhLWM1NzMtNGY3MS1hYjk5LWY2OWI0ZjIyMzRiMSIsImVtYWlsIjoiam9zaEBleGFtcGxlLmNvbSIsImxpY2Vuc2VfdHlwZSI6InBlcnNvbmFsIiwiaXNzdWVkX2F0IjoiMjAyNi0wMy0zMVQyMTo0ODoyMCIsImV4cGlyZXNfYXQiOm51bGx9.aGKieFTq2VuycxNzQYAWXccg4ezBlEWIcaT8lTPbrl3p45-cfCjmLW53gGwTi5levaavFDIjg7_b_7z_2-9uDg";
        assert!(matches!(validate_license_key(key), LicenseStatus::Unlicensed));
    }
}

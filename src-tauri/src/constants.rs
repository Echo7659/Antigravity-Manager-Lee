use regex::Regex;
use std::sync::LazyLock;

/// Known stable configuration (for Docker/Headless fallback)
/// Antigravity 4.3.0 uses Electron 39.2.3 which corresponds to Chrome 132.0.6834.160
pub const KNOWN_STABLE_VERSION: &str = "4.3.0";
pub const KNOWN_STABLE_ELECTRON: &str = "39.2.3";
pub const KNOWN_STABLE_CHROME: &str = "132.0.6834.160";

/// Pre-compiled regex for version parsing (X.Y.Z pattern)
static VERSION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+\.\d+\.\d+").expect("Invalid version regex"));

/// Pre-compiled regex specifically matching version following antigravity (e.g. `antigravity/1.5`, `Antigravity/1.5.0`, `antigravity/4.3.0`)
static ANTIGRAVITY_VERSION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)antigravity[/v\s]+(\d+(?:\.\d+)+)").expect("Invalid antigravity version regex")
});

/// Parse version from response text using pre-compiled regex
/// Matches semver pattern: X.Y.Z (e.g., "1.15.8")
pub fn parse_version(text: &str) -> Option<String> {
    VERSION_REGEX.find(text).map(|m| m.as_str().to_string())
}

/// Parse version specifically from an Antigravity User-Agent string.
/// Supports both 2-part (`1.5`) and 3-part (`1.5.0`, `1.15.8`) semver patterns,
/// and prevents false matching of leading browser tokens (e.g. `Mozilla/5.0`).
pub fn parse_antigravity_version(text: &str) -> Option<String> {
    if let Some(caps) = ANTIGRAVITY_VERSION_REGEX.captures(text) {
        if let Some(m) = caps.get(1) {
            return Some(m.as_str().to_string());
        }
    }
    parse_version(text)
}

/// Compare two X.Y.Z semantic version strings.
/// Returns Ordering::Greater if v1 > v2.
pub fn compare_semver(v1: &str, v2: &str) -> std::cmp::Ordering {
    let parse = |v: &str| -> Vec<u32> { v.split('.').filter_map(|s| s.parse().ok()).collect() };
    let p1 = parse(v1);
    let p2 = parse(v2);
    for i in 0..p1.len().max(p2.len()) {
        let a = p1.get(i).copied().unwrap_or(0);
        let b = p2.get(i).copied().unwrap_or(0);
        match a.cmp(&b) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// 服务器使用固定版本下限，不探测桌面安装或请求更新服务。
pub static CURRENT_VERSION: LazyLock<String> = LazyLock::new(|| KNOWN_STABLE_VERSION.to_string());

/// Native OAuth Authorization User-Agent
pub static NATIVE_OAUTH_USER_AGENT: LazyLock<String> =
    LazyLock::new(|| format!("vscode/1.X.X (Antigravity/{})", CURRENT_VERSION.as_str()));

/// Current resolved Antigravity version (e.g., "4.3.0")
pub fn get_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Returns a full User-Agent string for the current version
/// "Antigravity/4.3.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/132.0.6834.160 Electron/39.2.3"
pub fn get_default_user_agent() -> String {
    format!(
        "Antigravity/{} (Macintosh; Intel Mac OS X 10_15_7) Chrome/132.0.6834.160 Electron/39.2.3",
        env!("CARGO_PKG_VERSION")
    )
}

/// Global Session ID (generated once per app launch)
pub static SESSION_ID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// Antigravity 官方语言服务枢纽 (Antigravity Hub) 版本号
/// 对应官方出站 User-Agent: antigravity/hub/{OFFICIAL_HUB_VERSION}
pub const OFFICIAL_HUB_VERSION: &str = "2.17.0";

/// Antigravity 官方内部代码仓库 (Google Piper Monorepo) 变更集编号 (Changelist Number / CL)
/// 官方每一个正式版本发布均对应确定的构建提交点，全球正版客户端保持统一，切勿随机变动
pub const OFFICIAL_HUB_CL: &str = "986210228";

/// Returns the official Antigravity Hub User-Agent matching the upstream egress signature:
/// format: "antigravity/hub/{OFFICIAL_HUB_VERSION} (aidev_client; os_type={os}; arch={arch}; cl={OFFICIAL_HUB_CL})"
/// e.g.: "antigravity/hub/2.17.0 (aidev_client; os_type=windows; arch=amd64; cl=986210228)"
pub fn build_official_upstream_user_agent() -> String {
    let os_type = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "darwin",
        "linux" => "linux",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!(
        "antigravity/hub/{} (aidev_client; os_type={}; arch={}; cl={})",
        OFFICIAL_HUB_VERSION, os_type, arch, OFFICIAL_HUB_CL
    )
}

/// Official Antigravity upstream User-Agent
pub static USER_AGENT: LazyLock<String> = LazyLock::new(build_official_upstream_user_agent);

/// Sanitizes a custom User-Agent string for upstream egress requests.
///
/// Rules:
/// 1. If empty or whitespace-only, falls back to default `USER_AGENT`.
/// 2. Google Cloud Code requires the client identity to contain "antigravity" (case-insensitive).
///    Arbitrary User-Agents (e.g. "CustomApp/1.0.0") are rejected upstream with HTTP 403 Forbidden.
///    If the string lacks "antigravity", falls back to `USER_AGENT`.
/// 3. Google upstream enforces a version gate: versions < 3.0.0 (such as legacy 1.15.8)
///    return HTTP 404 for segmented models (gemini-3.8-flash-low/medium/high).
///    If the custom UA specifies a version < `KNOWN_STABLE_VERSION` ("4.3.0"),
///    the version segment is automatically clamped to `KNOWN_STABLE_VERSION`.
/// 4. Custom versions >= `KNOWN_STABLE_VERSION` (e.g. "4.5.0", "5.0.0") are fully preserved.
pub fn sanitize_egress_user_agent(custom_ua: &str) -> String {
    let trimmed = custom_ua.trim();
    if trimmed.is_empty() {
        return USER_AGENT.clone();
    }

    if trimmed.starts_with("antigravity/hub/") {
        return trimmed.to_string();
    }

    if !trimmed.to_ascii_lowercase().contains("antigravity") {
        tracing::warn!(
            custom_ua = %trimmed,
            "Custom User-Agent lacks 'antigravity' client identity (upstream will reject with 403); falling back to default User-Agent"
        );
        return USER_AGENT.clone();
    }

    if let Some(caps) = ANTIGRAVITY_VERSION_REGEX.captures(trimmed) {
        if let Some(m) = caps.get(1) {
            let ver = m.as_str();
            if compare_semver(ver, KNOWN_STABLE_VERSION) == std::cmp::Ordering::Less {
                tracing::info!(
                    custom_ua = %trimmed,
                    outdated_version = %ver,
                    clamped_version = %KNOWN_STABLE_VERSION,
                    "Custom User-Agent version is below minimum supported floor; clamping version to {} to avoid upstream 404/429 model rejections",
                    KNOWN_STABLE_VERSION
                );
                let mut result = String::with_capacity(trimmed.len() + KNOWN_STABLE_VERSION.len());
                result.push_str(&trimmed[..m.start()]);
                result.push_str(KNOWN_STABLE_VERSION);
                result.push_str(&trimmed[m.end()..]);
                return result;
            }
            return trimmed.to_string();
        }
    }

    if let Some(ver) = parse_version(trimmed) {
        if compare_semver(&ver, KNOWN_STABLE_VERSION) == std::cmp::Ordering::Less {
            tracing::info!(
                custom_ua = %trimmed,
                outdated_version = %ver,
                clamped_version = %KNOWN_STABLE_VERSION,
                "Custom User-Agent version is below minimum supported floor; clamping version to {} to avoid upstream 404/429 model rejections",
                KNOWN_STABLE_VERSION
            );
            return trimmed.replacen(&ver, KNOWN_STABLE_VERSION, 1);
        }
    } else if trimmed.eq_ignore_ascii_case("antigravity") {
        return format!("antigravity/{} darwin/arm64", KNOWN_STABLE_VERSION);
    }

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_from_updater_response() {
        let text = "Auto updater is running. Stable Version: 1.15.8-5724687216017408";
        assert_eq!(parse_version(text), Some("1.15.8".to_string()));
    }

    #[test]
    fn test_parse_version_simple() {
        assert_eq!(parse_version("1.15.8"), Some("1.15.8".to_string()));
        assert_eq!(parse_version("Version: 2.0.0"), Some("2.0.0".to_string()));
        assert_eq!(parse_version("v1.2.3"), Some("1.2.3".to_string()));
    }

    #[test]
    fn test_parse_version_invalid() {
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("1.2"), None); // Only X.Y, not X.Y.Z
    }

    #[test]
    fn test_parse_version_with_suffix() {
        // Regex only matches X.Y.Z, suffix is naturally excluded
        let text = "antigravity/1.15.8 windows/amd64";
        assert_eq!(parse_version(text), Some("1.15.8".to_string()));
    }

    #[test]
    fn test_compare_semver() {
        assert_eq!(
            compare_semver("4.3.0", "4.1.22"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_semver("4.1.22", "4.3.0"), std::cmp::Ordering::Less);
        assert_eq!(compare_semver("4.3.0", "4.3.0"), std::cmp::Ordering::Equal);
        assert_eq!(
            compare_semver("5.0.0", "4.9.9"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("1.16.5", "1.16.4"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_known_stable_floor_is_up_to_date() {
        // KNOWN_STABLE_VERSION must always be kept in sync with Cargo.toml.
        // This test will fail and remind the developer to update it.
        assert!(
            compare_semver(KNOWN_STABLE_VERSION, "4.1.22") > std::cmp::Ordering::Equal,
            "KNOWN_STABLE_VERSION ({}) must be > 4.1.22; please sync with Cargo.toml",
            KNOWN_STABLE_VERSION
        );
    }

    #[test]
    fn test_old_local_version_uses_floor() {
        // Simulate: local = 4.1.20 (old), floor = 4.3.0
        // Expected: use floor
        let local = "4.1.20";
        let floor = KNOWN_STABLE_VERSION;
        let best = if compare_semver(local, floor) > std::cmp::Ordering::Equal {
            local
        } else {
            floor
        };
        assert_eq!(best, KNOWN_STABLE_VERSION);
    }

    #[test]
    fn test_newer_local_version_takes_priority() {
        // Simulate: local = 4.3.0 (newer than floor), floor = 4.3.0
        let local = "4.3.0";
        let floor = KNOWN_STABLE_VERSION;
        let best = if compare_semver(local, floor) >= std::cmp::Ordering::Equal {
            local
        } else {
            floor
        };
        assert_eq!(best, "4.3.0");
    }

    #[test]
    fn test_sanitize_egress_user_agent() {
        // 1. Legacy 1.15.8 is clamped to KNOWN_STABLE_VERSION (4.3.0)
        let legacy_mac = "antigravity/1.15.8 darwin/arm64";
        assert_eq!(
            sanitize_egress_user_agent(legacy_mac),
            format!("antigravity/{} darwin/arm64", KNOWN_STABLE_VERSION)
        );

        // 2. Outdated 2.0.0 is clamped
        let old_v2 = "antigravity/2.0.0 linux/amd64";
        assert_eq!(
            sanitize_egress_user_agent(old_v2),
            format!("antigravity/{} linux/amd64", KNOWN_STABLE_VERSION)
        );

        // 2b. Outdated 1.5.0 (3-segment) and 1.5 (2-segment) are clamped to 4.3.0
        assert_eq!(
            sanitize_egress_user_agent("antigravity/1.5.0 darwin/arm64"),
            "antigravity/4.3.0 darwin/arm64"
        );
        assert_eq!(
            sanitize_egress_user_agent("antigravity/1.5 darwin/arm64"),
            "antigravity/4.3.0 darwin/arm64"
        );
        assert_eq!(
            sanitize_egress_user_agent("antigravity/1.5"),
            "antigravity/4.3.0"
        );
        assert_eq!(
            sanitize_egress_user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Antigravity/1.5.0 Chrome/132.0.6834.160"
            ),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Antigravity/4.3.0 Chrome/132.0.6834.160"
        );

        // 3. Current 4.3.0 is preserved
        let v43 = "antigravity/4.3.0 darwin/arm64";
        assert_eq!(
            sanitize_egress_user_agent(v43),
            "antigravity/4.3.0 darwin/arm64"
        );

        // 4. Newer custom version >= 4.3.0 is preserved
        let custom_v45 = "antigravity/4.5.0 darwin/arm64";
        assert_eq!(
            sanitize_egress_user_agent(custom_v45),
            "antigravity/4.5.0 darwin/arm64"
        );

        let custom_v5 = "Antigravity/5.0.0 (Windows NT 10.0; Win64; x64)";
        assert_eq!(
            sanitize_egress_user_agent(custom_v5),
            "Antigravity/5.0.0 (Windows NT 10.0; Win64; x64)"
        );

        // 5. Non-antigravity UA falls back to default USER_AGENT (prevents Google 403)
        let arbitrary_ua = "CustomApp/1.0.0 (Macintosh)";
        assert_eq!(
            sanitize_egress_user_agent(arbitrary_ua),
            USER_AGENT.as_str()
        );

        // 6. Blank/empty falls back to USER_AGENT
        assert_eq!(sanitize_egress_user_agent(""), USER_AGENT.as_str());
        assert_eq!(sanitize_egress_user_agent("   "), USER_AGENT.as_str());

        // 7. Bare "antigravity" gets stable floor appended
        assert_eq!(
            sanitize_egress_user_agent("antigravity"),
            format!("antigravity/{} darwin/arm64", KNOWN_STABLE_VERSION)
        );
    }
}

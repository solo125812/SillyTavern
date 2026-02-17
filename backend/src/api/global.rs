//! Global route handlers — Phase 1.
//!
//! Implements `GET /version` and `POST /api/ping`, mirroring the Node
//! handlers in `src/server-main.js` and `src/util.js#getVersion()`.
//!
//! Both routes require auth — Rust validates the [`UserContext`] forwarded
//! by the Node proxy.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::Serialize;

use crate::api::router::AppState;
use crate::http::middleware::UserContext;

const BACKEND_HEADER: &str = "x-st-backend";

fn maybe_add_backend_header(response: &mut Response, enabled: bool) {
    if enabled {
        response
            .headers_mut()
            .insert(BACKEND_HEADER, HeaderValue::from_static("rust"));
    }
}

// ---------------------------------------------------------------------------
// GET /version
// ---------------------------------------------------------------------------

/// Response shape for `GET /version` — mirrors Node's `getVersion()` in
/// [`src/util.js`](../../src/util.js:136).
///
/// Field names are camelCase to match the Node JSON contract.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// Agent string: `SillyTavern:<version>:Cohee#1207`
    pub agent: String,
    /// Package version from `package.json`
    pub pkg_version: String,
    /// Short git revision hash (e.g. `"abc1234"`)
    pub git_revision: Option<String>,
    /// Current git branch name
    pub git_branch: Option<String>,
    /// Commit date (ISO-ish format from `git show -s --format=%ci`)
    pub commit_date: Option<String>,
    /// Whether the local HEAD matches the remote tracking branch
    pub is_latest: bool,
}

/// `GET /version` — mirrors the Node version payload shape.
///
/// Supports env overrides for deterministic test output:
/// - `ST_VERSION_OVERRIDE`
/// - `ST_GIT_REVISION_OVERRIDE`
/// - `ST_GIT_BRANCH_OVERRIDE`
pub async fn version_handler(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let info = compute_version_info(&state.config.server_directory);
    let mut response = Json(info).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    maybe_add_backend_header(&mut response, state.config.backend_header);
    response
}

/// Compute version info from `package.json` and git, with env var overrides.
fn compute_version_info(server_dir: &Path) -> VersionInfo {
    // Allow env var overrides for deterministic test output
    let pkg_version = std::env::var("ST_VERSION_OVERRIDE")
        .ok()
        .or_else(|| read_pkg_version(server_dir))
        .unwrap_or_else(|| "UNKNOWN".to_string());

    let git_revision = std::env::var("ST_GIT_REVISION_OVERRIDE")
        .ok()
        .or_else(|| run_git(server_dir, &["rev-parse", "--short", "HEAD"]));
    let git_branch = std::env::var("ST_GIT_BRANCH_OVERRIDE")
        .ok()
        .or_else(|| run_git(server_dir, &["rev-parse", "--abbrev-ref", "HEAD"]));
    let commit_date = git_revision
        .as_deref()
        .and_then(|rev| run_git(server_dir, &["show", "-s", "--format=%ci", rev]))
        .map(|value| value.trim().to_string());

    let tracking_branch = run_git(server_dir, &["rev-parse", "--abbrev-ref", "@{u}"]);
    let local_latest = run_git(server_dir, &["rev-parse", "HEAD"]);
    let remote_latest = tracking_branch
        .as_deref()
        .and_then(|branch| run_git(server_dir, &["rev-parse", branch]));

    let is_latest = match (local_latest, remote_latest) {
        (Some(local), Some(remote)) => local == remote,
        _ => true,
    };

    let agent = format!("SillyTavern:{}:Cohee#1207", pkg_version);

    VersionInfo {
        agent,
        pkg_version,
        git_revision,
        git_branch,
        commit_date,
        is_latest,
    }
}

/// Read `version` field from `package.json` in the server directory.
fn read_pkg_version(server_dir: &Path) -> Option<String> {
    let pkg_path = server_dir.join("package.json");
    let contents = fs::read_to_string(pkg_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&contents).ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

/// Run a git command in the server directory and return trimmed stdout.
fn run_git(server_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(server_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

// ---------------------------------------------------------------------------
// POST /api/ping
// ---------------------------------------------------------------------------

/// `POST /api/ping` — mirrors Node's 204 No Content behavior.
///
/// In Node, this also touches `session.touch = Date.now()` when the
/// `?extend` query param is set. Since session management stays on Node,
/// the Rust sidecar simply returns 204.
pub async fn ping_handler(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<UserContext>,
) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    maybe_add_backend_header(&mut response, state.config.backend_header);
    response
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_info_serializes_to_camel_case() {
        let info = VersionInfo {
            agent: "SillyTavern:1.0.0:Cohee#1207".to_string(),
            pkg_version: "1.0.0".to_string(),
            git_revision: Some("abc1234".to_string()),
            git_branch: Some("main".to_string()),
            commit_date: Some("2026-01-01 00:00:00 +0000".to_string()),
            is_latest: true,
        };

        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("pkgVersion").is_some());
        assert!(json.get("gitRevision").is_some());
        assert!(json.get("gitBranch").is_some());
        assert!(json.get("commitDate").is_some());
        assert!(json.get("isLatest").is_some());
        assert!(json.get("agent").is_some());

        // Ensure snake_case keys are NOT present
        assert!(json.get("pkg_version").is_none());
        assert!(json.get("git_revision").is_none());
    }

    #[test]
    fn version_info_handles_none_fields() {
        let info = VersionInfo {
            agent: "SillyTavern:UNKNOWN:Cohee#1207".to_string(),
            pkg_version: "UNKNOWN".to_string(),
            git_revision: None,
            git_branch: None,
            commit_date: None,
            is_latest: true,
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["gitRevision"], serde_json::Value::Null);
        assert_eq!(json["gitBranch"], serde_json::Value::Null);
        assert_eq!(json["commitDate"], serde_json::Value::Null);
    }
}

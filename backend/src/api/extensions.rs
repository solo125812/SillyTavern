//! Extension management endpoints — Phase 8.
//!
//! Mirrors Node's [`src/endpoints/extensions.js`](../../../src/endpoints/extensions.js).
//!
//! ## Endpoints
//! - `POST /api/extensions/install`   — Clone a git extension repo
//! - `POST /api/extensions/update`    — Pull latest from a git extension repo
//! - `POST /api/extensions/branches`  — List branches of an extension repo
//! - `POST /api/extensions/switch`    — Switch branch of an extension repo
//! - `POST /api/extensions/move`      — Move extension between user/global scope
//! - `POST /api/extensions/version`   — Get commit hash / branch / up-to-date status
//! - `POST /api/extensions/delete`    — Delete an extension
//! - `GET  /api/extensions/discover`  — Discover available extensions
//!
//! ## Implementation Note
//! Git operations (`clone`, `pull`, `fetch`, `checkout`) shell out to the `git`
//! command-line tool. This avoids pulling in a native Rust git library and
//! matches the Node implementation's reliance on `simple-git` (which itself
//! shells out).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::router::AppState;
use crate::http::middleware::UserContext;
use crate::storage::paths::{PublicDirectories, UserDirectories};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    url: Option<String>,
    global: Option<bool>,
    branch: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionNameRequest {
    extension_name: Option<String>,
    global: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRequest {
    extension_name: Option<String>,
    branch: Option<String>,
    global: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveRequest {
    extension_name: Option<String>,
    source: Option<String>,
    destination: Option<String>,
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

/// Run a git command in the given directory. Returns stdout on success.
fn git_in_dir(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {}: {}", args.join(" "), stderr))
    }
}

/// Run a git clone.
fn git_clone(url: &str, dest: &Path, branch: Option<&str>) -> Result<(), String> {
    let mut args = vec!["clone", "--depth", "1"];
    if let Some(b) = branch {
        args.push("--branch");
        args.push(b);
    }
    args.push(url);
    let dest_str = dest.to_string_lossy();
    args.push(&dest_str);

    let output = Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run git clone: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git clone failed: {stderr}"))
    }
}

/// Get manifest.json from extension path.
fn get_manifest(extension_path: &Path) -> Result<Value, String> {
    let manifest_path = extension_path.join("manifest.json");
    if !manifest_path.exists() {
        return Err(format!(
            "Manifest file not found at {}",
            manifest_path.display()
        ));
    }
    let content = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read manifest: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse manifest: {e}"))
}

/// Check if a repo is up-to-date with its remote.
fn check_if_repo_is_up_to_date(
    extension_path: &Path,
) -> Result<(bool, String), String> {
    // Fetch origin
    git_in_dir(extension_path, &["fetch", "origin"])?;

    // Get current branch
    let current_branch =
        git_in_dir(extension_path, &["rev-parse", "--abbrev-ref", "HEAD"])?;

    // Get current commit hash
    let current_commit = git_in_dir(extension_path, &["rev-parse", "HEAD"])?;

    // Get remotes
    let remotes_output =
        git_in_dir(extension_path, &["remote", "-v"]).unwrap_or_default();
    if remotes_output.is_empty() {
        return Ok((true, String::new()));
    }

    // Extract remote URL (first fetch line)
    let remote_url = remotes_output
        .lines()
        .find(|line| line.contains("(fetch)"))
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();

    // Check if there are commits between HEAD and origin/branch
    let log_count = git_in_dir(
        extension_path,
        &[
            "rev-list",
            "--count",
            &format!("{current_commit}..origin/{current_branch}"),
        ],
    )
    .unwrap_or_else(|_| "0".to_string());

    let is_up_to_date = log_count.trim().parse::<u64>().unwrap_or(0) == 0;

    Ok((is_up_to_date, remote_url))
}

/// Resolve the base path for an extension (user or global).
fn resolve_base_path(
    user: &UserContext,
    state: &AppState,
    global: bool,
) -> PathBuf {
    if global {
        let pub_dirs = PublicDirectories::new(&state.config.server_directory);
        pub_dirs.global_extensions
    } else {
        let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
        dirs.extensions
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/extensions/install` — Clone a git extension repo.
pub async fn install_extension(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<InstallRequest>,
) -> Response {
    let url = match &body.url {
        Some(u) if !u.is_empty() => u.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request: URL is required in the request body.",
            )
                .into_response()
        }
    };

    let global = body.global.unwrap_or(false);

    if global && !user.is_admin {
        tracing::error!(
            "User {} does not have permission to install global extensions.",
            user.handle
        );
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: No permission to install global extensions.",
        )
            .into_response();
    }

    let base_path = resolve_base_path(&user, &state, global);

    // Ensure base directory exists
    let _ = fs::create_dir_all(&base_path);

    // Also ensure global extensions dir exists
    let pub_dirs = PublicDirectories::new(&state.config.server_directory);
    let _ = fs::create_dir_all(&pub_dirs.global_extensions);

    // Derive extension directory name from URL
    let repo_name = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("extension")
        .trim_end_matches(".git");
    let safe_name = sanitize_filename::sanitize(repo_name);
    let extension_path = base_path.join(&safe_name);

    if extension_path.exists() {
        return (
            StatusCode::CONFLICT,
            format!("Directory already exists at {}", extension_path.display()),
        )
            .into_response();
    }

    match git_clone(&url, &extension_path, body.branch.as_deref()) {
        Ok(()) => {
            tracing::info!(
                "Extension has been cloned to {} from {} at {} branch",
                extension_path.display(),
                url,
                body.branch.as_deref().unwrap_or("(default)")
            );

            match get_manifest(&extension_path) {
                Ok(manifest) => {
                    let result = json!({
                        "version": manifest.get("version"),
                        "author": manifest.get("author"),
                        "display_name": manifest.get("display_name"),
                        "extensionPath": extension_path.to_string_lossy(),
                    });
                    Json(result).into_response()
                }
                Err(e) => {
                    tracing::error!("Failed to read manifest: {e}");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Server Error: {e}"),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            tracing::error!("Importing custom content failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Server Error: {e}"),
            )
                .into_response()
        }
    }
}

/// `POST /api/extensions/update` — Pull latest updates from a git extension repo.
pub async fn update_extension(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ExtensionNameRequest>,
) -> Response {
    let extension_name = match &body.extension_name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request: extensionName is required in the request body.",
            )
                .into_response()
        }
    };

    let global = body.global.unwrap_or(false);

    if global && !user.is_admin {
        tracing::error!(
            "User {} does not have permission to update global extensions.",
            user.handle
        );
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: No permission to update global extensions.",
        )
            .into_response();
    }

    let base_path = resolve_base_path(&user, &state, global);
    let safe_name = sanitize_filename::sanitize(&extension_name);
    let extension_path = base_path.join(&safe_name);

    if !extension_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            format!("Directory does not exist at {}", extension_path.display()),
        )
            .into_response();
    }

    // Check if it's a git repo
    let is_repo = git_in_dir(&extension_path, &["rev-parse", "--is-inside-work-tree"])
        .map(|out| out.trim() == "true")
        .unwrap_or(false);

    if !is_repo {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "Directory is not a Git repository at {}",
                extension_path.display()
            ),
        )
            .into_response();
    }

    match check_if_repo_is_up_to_date(&extension_path) {
        Ok((is_up_to_date, remote_url)) => {
            let current_branch = git_in_dir(
                &extension_path,
                &["rev-parse", "--abbrev-ref", "HEAD"],
            )
            .unwrap_or_default();

            if !is_up_to_date {
                if let Err(e) =
                    git_in_dir(&extension_path, &["pull", "origin", &current_branch])
                {
                    tracing::error!("Updating extension failed: {e}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Internal Server Error. Check the server logs for more details.",
                    )
                        .into_response();
                }
                tracing::info!(
                    "Extension has been updated at {}",
                    extension_path.display()
                );
            } else {
                tracing::info!(
                    "Extension is up to date at {}",
                    extension_path.display()
                );
            }

            // Fetch origin again to update refs
            let _ = git_in_dir(&extension_path, &["fetch", "origin"]);

            let full_commit_hash =
                git_in_dir(&extension_path, &["rev-parse", "HEAD"]).unwrap_or_default();
            let short_commit_hash = if full_commit_hash.len() >= 7 {
                &full_commit_hash[..7]
            } else {
                &full_commit_hash
            };

            let result = json!({
                "shortCommitHash": short_commit_hash,
                "extensionPath": extension_path.to_string_lossy(),
                "isUpToDate": is_up_to_date,
                "remoteUrl": remote_url,
            });
            Json(result).into_response()
        }
        Err(e) => {
            tracing::error!("Updating extension failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error. Check the server logs for more details.",
            )
                .into_response()
        }
    }
}

/// `POST /api/extensions/branches` — List branches of an extension repo.
pub async fn list_branches(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ExtensionNameRequest>,
) -> Response {
    let extension_name = match &body.extension_name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request: extensionName is required in the request body.",
            )
                .into_response()
        }
    };

    let global = body.global.unwrap_or(false);

    if global && !user.is_admin {
        tracing::error!(
            "User {} does not have permission to list branches of global extensions.",
            user.handle
        );
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: No permission to list branches of global extensions.",
        )
            .into_response();
    }

    let base_path = resolve_base_path(&user, &state, global);
    let safe_name = sanitize_filename::sanitize(&extension_name);
    let extension_path = base_path.join(&safe_name);

    if !extension_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            format!("Directory does not exist at {}", extension_path.display()),
        )
            .into_response();
    }

    // Unshallow if needed
    let is_shallow = git_in_dir(&extension_path, &["rev-parse", "--is-shallow-repository"])
        .map(|o| o.trim() == "true")
        .unwrap_or(false);

    if is_shallow {
        tracing::info!(
            "Unshallowing the repository at {}",
            extension_path.display()
        );
        let _ = git_in_dir(&extension_path, &["fetch", "origin", "--unshallow"]);
    }

    // Fetch all branches
    let _ = git_in_dir(
        &extension_path,
        &["remote", "set-branches", "origin", "*"],
    );
    let _ = git_in_dir(&extension_path, &["fetch", "origin"]);

    // Get the current branch
    let current_branch = git_in_dir(
        &extension_path,
        &["rev-parse", "--abbrev-ref", "HEAD"],
    )
    .unwrap_or_default();

    // Get local branches
    let local_output = git_in_dir(
        &extension_path,
        &["branch", "--format=%(refname:short) %(objectname:short)"],
    )
    .unwrap_or_default();

    // Get remote branches
    let remote_output = git_in_dir(
        &extension_path,
        &[
            "branch",
            "-r",
            "--list",
            "origin/*",
            "--format=%(refname:short) %(objectname:short)",
        ],
    )
    .unwrap_or_default();

    let mut result: Vec<Value> = Vec::new();

    for line in local_output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            result.push(json!({
                "current": parts[0] == current_branch,
                "commit": parts[1],
                "name": parts[0],
                "label": format!("{} {}", parts[0], parts[1]),
            }));
        }
    }

    for line in remote_output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            result.push(json!({
                "current": false,
                "commit": parts[1],
                "name": parts[0],
                "label": format!("{} {}", parts[0], parts[1]),
            }));
        }
    }

    Json(result).into_response()
}

/// `POST /api/extensions/switch` — Switch branch of an extension repo.
pub async fn switch_branch(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<SwitchRequest>,
) -> Response {
    let extension_name = match &body.extension_name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request: extensionName and branch are required in the request body.",
            )
                .into_response()
        }
    };

    let branch = match &body.branch {
        Some(b) if !b.is_empty() => b.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request: extensionName and branch are required in the request body.",
            )
                .into_response()
        }
    };

    let global = body.global.unwrap_or(false);

    if global && !user.is_admin {
        tracing::error!(
            "User {} does not have permission to switch branches of global extensions.",
            user.handle
        );
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: No permission to switch branches of global extensions.",
        )
            .into_response();
    }

    let base_path = resolve_base_path(&user, &state, global);
    let safe_name = sanitize_filename::sanitize(&extension_name);
    let extension_path = base_path.join(&safe_name);

    if !extension_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            format!("Directory does not exist at {}", extension_path.display()),
        )
            .into_response();
    }

    // Handle remote branch (origin/*)
    if branch.starts_with("origin/") {
        let local_branch = branch.strip_prefix("origin/").unwrap();

        // Check if local branch already exists
        let local_branches = git_in_dir(
            &extension_path,
            &["branch", "--format=%(refname:short)"],
        )
        .unwrap_or_default();

        let local_exists = local_branches
            .lines()
            .any(|b| b.trim() == local_branch);

        if local_exists {
            tracing::info!(
                "Branch {local_branch} already exists locally, checking it out"
            );
            if let Err(e) = git_in_dir(&extension_path, &["checkout", local_branch]) {
                tracing::error!("Switching branches failed: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal Server Error. Check the server logs for more details.",
                )
                    .into_response();
            }
            return StatusCode::NO_CONTENT.into_response();
        }

        tracing::info!(
            "Branch {local_branch} does not exist locally, creating it from {branch}"
        );
        if let Err(e) = git_in_dir(
            &extension_path,
            &["checkout", "-b", local_branch, &branch],
        ) {
            tracing::error!("Switching branches failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error. Check the server logs for more details.",
            )
                .into_response();
        }
        return StatusCode::NO_CONTENT.into_response();
    }

    // Check if local branch exists
    let local_branches = git_in_dir(
        &extension_path,
        &["branch", "--format=%(refname:short)"],
    )
    .unwrap_or_default();

    let branch_exists = local_branches.lines().any(|b| b.trim() == branch);

    if !branch_exists {
        tracing::error!("Branch {branch} does not exist locally");
        return (
            StatusCode::NOT_FOUND,
            format!("Branch {branch} does not exist locally"),
        )
            .into_response();
    }

    // Check if already checked out
    let current_branch = git_in_dir(
        &extension_path,
        &["rev-parse", "--abbrev-ref", "HEAD"],
    )
    .unwrap_or_default();

    if current_branch == branch {
        tracing::info!("Branch {branch} is already checked out");
        return StatusCode::NO_CONTENT.into_response();
    }

    // Checkout the branch
    match git_in_dir(&extension_path, &["checkout", &branch]) {
        Ok(_) => {
            tracing::info!(
                "Checked out branch {branch} at {}",
                extension_path.display()
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            tracing::error!("Switching branches failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error. Check the server logs for more details.",
            )
                .into_response()
        }
    }
}

/// `POST /api/extensions/move` — Move extension between user/global scope.
pub async fn move_extension(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<MoveRequest>,
) -> Response {
    let extension_name = match &body.extension_name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request. Not all required parameters are provided.",
            )
                .into_response()
        }
    };

    let source = match &body.source {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request. Not all required parameters are provided.",
            )
                .into_response()
        }
    };

    let destination = match &body.destination {
        Some(d) if !d.is_empty() => d.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request. Not all required parameters are provided.",
            )
                .into_response()
        }
    };

    if !user.is_admin {
        tracing::error!(
            "User {} does not have permission to move extensions.",
            user.handle
        );
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: No permission to move extensions.",
        )
            .into_response();
    }

    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let pub_dirs = PublicDirectories::new(&state.config.server_directory);

    let source_directory = if source == "global" {
        &pub_dirs.global_extensions
    } else {
        &dirs.extensions
    };

    let destination_directory = if destination == "global" {
        &pub_dirs.global_extensions
    } else {
        &dirs.extensions
    };

    let safe_name = sanitize_filename::sanitize(&extension_name);
    let source_path = source_directory.join(&safe_name);
    let destination_path = destination_directory.join(&safe_name);

    if !source_path.exists() || !source_path.is_dir() {
        tracing::error!(
            "Source directory does not exist at {}",
            source_path.display()
        );
        return (StatusCode::NOT_FOUND, "Source directory does not exist.").into_response();
    }

    if destination_path.exists() {
        tracing::error!(
            "Destination directory already exists at {}",
            destination_path.display()
        );
        return (StatusCode::CONFLICT, "Destination directory already exists.").into_response();
    }

    if source == destination {
        tracing::error!("Source and destination directories are the same");
        return (
            StatusCode::CONFLICT,
            "Source and destination directories are the same.",
        )
            .into_response();
    }

    // Copy and remove
    if let Err(e) = copy_dir_recursive(&source_path, &destination_path) {
        tracing::error!("Moving extension failed: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error. Check the server logs for more details.",
        )
            .into_response();
    }

    if let Err(e) = fs::remove_dir_all(&source_path) {
        tracing::error!("Failed to remove source directory: {e}");
        // Don't fail — the copy succeeded
    }

    tracing::info!(
        "Extension has been moved from {} to {}",
        source_path.display(),
        destination_path.display()
    );

    StatusCode::NO_CONTENT.into_response()
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// `POST /api/extensions/version` — Get version info for an extension.
pub async fn get_version(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ExtensionNameRequest>,
) -> Response {
    let extension_name = match &body.extension_name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request: extensionName is required in the request body.",
            )
                .into_response()
        }
    };

    let global = body.global.unwrap_or(false);
    let base_path = resolve_base_path(&user, &state, global);
    let safe_name = sanitize_filename::sanitize(&extension_name);
    let extension_path = base_path.join(&safe_name);

    if !extension_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            format!("Directory does not exist at {}", extension_path.display()),
        )
            .into_response();
    }

    // Check if it's a repo and get HEAD
    let current_commit_hash = match git_in_dir(&extension_path, &["rev-parse", "HEAD"]) {
        Ok(hash) => hash,
        Err(_) => {
            // Not a git repo or has no commits
            return Json(json!({
                "currentBranchName": "",
                "currentCommitHash": "",
                "isUpToDate": true,
                "remoteUrl": "",
            }))
            .into_response();
        }
    };

    let current_branch_name = git_in_dir(
        &extension_path,
        &["rev-parse", "--abbrev-ref", "HEAD"],
    )
    .unwrap_or_default();

    // Fetch and check if up to date
    let _ = git_in_dir(&extension_path, &["fetch", "origin"]);
    tracing::debug!(
        "{} {} {}",
        extension_name,
        current_branch_name,
        current_commit_hash
    );

    match check_if_repo_is_up_to_date(&extension_path) {
        Ok((is_up_to_date, remote_url)) => {
            Json(json!({
                "currentBranchName": current_branch_name,
                "currentCommitHash": current_commit_hash,
                "isUpToDate": is_up_to_date,
                "remoteUrl": remote_url,
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!("Getting extension version failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Server Error: {e}"),
            )
                .into_response()
        }
    }
}

/// `POST /api/extensions/delete` — Delete an extension.
pub async fn delete_extension(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
    Json(body): Json<ExtensionNameRequest>,
) -> Response {
    let extension_name = match &body.extension_name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Bad Request: extensionName is required in the request body.",
            )
                .into_response()
        }
    };

    let global = body.global.unwrap_or(false);

    if global && !user.is_admin {
        tracing::error!(
            "User {} does not have permission to delete global extensions.",
            user.handle
        );
        return (
            StatusCode::FORBIDDEN,
            "Forbidden: No permission to delete global extensions.",
        )
            .into_response();
    }

    let base_path = resolve_base_path(&user, &state, global);
    let safe_name = sanitize_filename::sanitize(&extension_name);
    let extension_path = base_path.join(&safe_name);

    if !extension_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            format!("Directory does not exist at {}", extension_path.display()),
        )
            .into_response();
    }

    match fs::remove_dir_all(&extension_path) {
        Ok(()) => {
            tracing::info!(
                "Extension has been deleted at {}",
                extension_path.display()
            );
            format!(
                "Extension has been deleted at {}",
                extension_path.display()
            )
            .into_response()
        }
        Err(e) => {
            tracing::error!("Deleting custom content failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Server Error: {e}"),
            )
                .into_response()
        }
    }
}

/// `GET /api/extensions/discover` — Discover available extension folders.
pub async fn discover_extensions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<UserContext>,
) -> Response {
    let dirs = UserDirectories::new(&state.config.data_root, &user.handle);
    let pub_dirs = PublicDirectories::new(&state.config.server_directory);

    // Ensure directories exist
    let _ = fs::create_dir_all(&dirs.extensions);
    let _ = fs::create_dir_all(&pub_dirs.global_extensions);

    // Get all folders in system extensions folder, excluding third-party
    let built_in_extensions: Vec<Value> = fs::read_dir(&pub_dirs.extensions)
        .into_iter()
        .flat_map(|entries| entries)
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
                && e.file_name() != "third-party"
        })
        .map(|e| {
            json!({
                "type": "system",
                "name": e.file_name().to_string_lossy(),
            })
        })
        .collect();

    // Get all folders in local (user) extensions folder
    let user_extensions: Vec<Value> = fs::read_dir(&dirs.extensions)
        .into_iter()
        .flat_map(|entries| entries)
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .map(|e| {
            json!({
                "type": "local",
                "name": format!("third-party/{}", e.file_name().to_string_lossy()),
            })
        })
        .collect();

    // Get user extension names for dedup
    let user_ext_names: Vec<String> = user_extensions
        .iter()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .map(|s| s.to_string())
        .collect();

    // Get all folders in global extensions folder, filtered by user overlap
    let global_extensions: Vec<Value> = fs::read_dir(&pub_dirs.global_extensions)
        .into_iter()
        .flat_map(|entries| entries)
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .map(|e| {
            let name = format!("third-party/{}", e.file_name().to_string_lossy());
            (name, e)
        })
        .filter(|(name, _)| !user_ext_names.contains(name))
        .map(|(name, _)| {
            json!({
                "type": "global",
                "name": name,
            })
        })
        .collect();

    let mut all_extensions = built_in_extensions;
    all_extensions.extend(user_extensions);
    all_extensions.extend(global_extensions);

    tracing::debug!(
        "Extensions available for {}: {:?}",
        user.handle,
        all_extensions
    );

    Json(all_extensions).into_response()
}

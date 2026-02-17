//! Path resolution for the SillyTavern data layout.
//!
//! Mirrors the Node directory constants from
//! [`src/constants.js`](../../../src/constants.js) (`USER_DIRECTORY_TEMPLATE`)
//! and the path resolution in
//! [`src/users.js`](../../../src/users.js) (`getUserDirectories()`).

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Server-level directories (relative to serverDirectory / project root)
// ---------------------------------------------------------------------------

/// Public directories served by Express static middleware.
/// Mirrors `PUBLIC_DIRECTORIES` in `src/constants.js`.
pub struct PublicDirectories {
    /// `public/img/`
    pub images: PathBuf,
    /// `backups/`
    pub backups: PathBuf,
    /// `public/sounds`
    pub sounds: PathBuf,
    /// `public/scripts/extensions`
    pub extensions: PathBuf,
    /// `public/scripts/extensions/third-party`
    pub global_extensions: PathBuf,
}

impl PublicDirectories {
    /// Resolve public directories relative to the server directory.
    pub fn new(server_dir: &Path) -> Self {
        Self {
            images: server_dir.join("public/img"),
            backups: server_dir.join("backups"),
            sounds: server_dir.join("public/sounds"),
            extensions: server_dir.join("public/scripts/extensions"),
            global_extensions: server_dir.join("public/scripts/extensions/third-party"),
        }
    }
}

// ---------------------------------------------------------------------------
// Data-root-level directories
// ---------------------------------------------------------------------------

/// Global directories under `DATA_ROOT` (not per-user).
pub struct DataRootDirectories {
    /// `_storage/` — node-persist user storage
    pub storage: PathBuf,
    /// `_cache/` — transformers/tokenizers/character disk cache
    pub cache: PathBuf,
    /// `_uploads/` — multer temp uploads
    pub uploads: PathBuf,
}

impl DataRootDirectories {
    /// Resolve data root directories.
    pub fn new(data_root: &Path) -> Self {
        Self {
            storage: data_root.join("_storage"),
            cache: data_root.join("_cache"),
            uploads: data_root.join("_uploads"),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-user directories
// ---------------------------------------------------------------------------

/// Per-user directory layout — mirrors `USER_DIRECTORY_TEMPLATE` in
/// [`src/constants.js:16-48`](../../../src/constants.js:16).
///
/// Each field is an absolute path: `DATA_ROOT / {handle} / {template_value}`.
#[derive(Debug, Clone)]
pub struct UserDirectories {
    /// User root: `DATA_ROOT/{handle}/`
    pub root: PathBuf,
    /// `thumbnails`
    pub thumbnails: PathBuf,
    /// `thumbnails/bg`
    pub thumbnails_bg: PathBuf,
    /// `thumbnails/avatar`
    pub thumbnails_avatar: PathBuf,
    /// `thumbnails/persona`
    pub thumbnails_persona: PathBuf,
    /// `worlds`
    pub worlds: PathBuf,
    /// `user`
    pub user: PathBuf,
    /// `User Avatars`
    pub avatars: PathBuf,
    /// `user/images`
    pub user_images: PathBuf,
    /// `groups`
    pub groups: PathBuf,
    /// `group chats`
    pub group_chats: PathBuf,
    /// `chats`
    pub chats: PathBuf,
    /// `characters`
    pub characters: PathBuf,
    /// `backgrounds`
    pub backgrounds: PathBuf,
    /// `NovelAI Settings`
    pub novel_ai_settings: PathBuf,
    /// `KoboldAI Settings`
    pub kobold_ai_settings: PathBuf,
    /// `OpenAI Settings`
    pub open_ai_settings: PathBuf,
    /// `TextGen Settings`
    pub text_gen_settings: PathBuf,
    /// `themes`
    pub themes: PathBuf,
    /// `movingUI`
    pub moving_ui: PathBuf,
    /// `extensions`
    pub extensions: PathBuf,
    /// `instruct`
    pub instruct: PathBuf,
    /// `context`
    pub context: PathBuf,
    /// `QuickReplies`
    pub quick_replies: PathBuf,
    /// `assets`
    pub assets: PathBuf,
    /// `user/workflows`
    pub comfy_workflows: PathBuf,
    /// `user/files`
    pub files: PathBuf,
    /// `vectors`
    pub vectors: PathBuf,
    /// `backups`
    pub backups: PathBuf,
    /// `sysprompt`
    pub sysprompt: PathBuf,
    /// `reasoning`
    pub reasoning: PathBuf,
}

/// Template values matching Node's `USER_DIRECTORY_TEMPLATE` in
/// `src/constants.js`. Each entry is a relative path from the user root.
const USER_DIR_TEMPLATES: &[(&str, &str)] = &[
    ("root", ""),
    ("thumbnails", "thumbnails"),
    ("thumbnails_bg", "thumbnails/bg"),
    ("thumbnails_avatar", "thumbnails/avatar"),
    ("thumbnails_persona", "thumbnails/persona"),
    ("worlds", "worlds"),
    ("user", "user"),
    ("avatars", "User Avatars"),
    ("user_images", "user/images"),
    ("groups", "groups"),
    ("group_chats", "group chats"),
    ("chats", "chats"),
    ("characters", "characters"),
    ("backgrounds", "backgrounds"),
    ("novel_ai_settings", "NovelAI Settings"),
    ("kobold_ai_settings", "KoboldAI Settings"),
    ("open_ai_settings", "OpenAI Settings"),
    ("text_gen_settings", "TextGen Settings"),
    ("themes", "themes"),
    ("moving_ui", "movingUI"),
    ("extensions", "extensions"),
    ("instruct", "instruct"),
    ("context", "context"),
    ("quick_replies", "QuickReplies"),
    ("assets", "assets"),
    ("comfy_workflows", "user/workflows"),
    ("files", "user/files"),
    ("vectors", "vectors"),
    ("backups", "backups"),
    ("sysprompt", "sysprompt"),
    ("reasoning", "reasoning"),
];

impl UserDirectories {
    /// Resolve per-user directories for a given handle.
    ///
    /// Mirrors [`getUserDirectories()`](../../../src/users.js:637) in Node:
    /// each path is `DATA_ROOT / handle / template_value`.
    pub fn new(data_root: &Path, handle: &str) -> Self {
        let user_root = data_root.join(handle);

        Self {
            root: user_root.clone(),
            thumbnails: user_root.join("thumbnails"),
            thumbnails_bg: user_root.join("thumbnails/bg"),
            thumbnails_avatar: user_root.join("thumbnails/avatar"),
            thumbnails_persona: user_root.join("thumbnails/persona"),
            worlds: user_root.join("worlds"),
            user: user_root.join("user"),
            avatars: user_root.join("User Avatars"),
            user_images: user_root.join("user/images"),
            groups: user_root.join("groups"),
            group_chats: user_root.join("group chats"),
            chats: user_root.join("chats"),
            characters: user_root.join("characters"),
            backgrounds: user_root.join("backgrounds"),
            novel_ai_settings: user_root.join("NovelAI Settings"),
            kobold_ai_settings: user_root.join("KoboldAI Settings"),
            open_ai_settings: user_root.join("OpenAI Settings"),
            text_gen_settings: user_root.join("TextGen Settings"),
            themes: user_root.join("themes"),
            moving_ui: user_root.join("movingUI"),
            extensions: user_root.join("extensions"),
            instruct: user_root.join("instruct"),
            context: user_root.join("context"),
            quick_replies: user_root.join("QuickReplies"),
            assets: user_root.join("assets"),
            comfy_workflows: user_root.join("user/workflows"),
            files: user_root.join("user/files"),
            vectors: user_root.join("vectors"),
            backups: user_root.join("backups"),
            sysprompt: user_root.join("sysprompt"),
            reasoning: user_root.join("reasoning"),
        }
    }

    /// Returns a list of all directory paths that should exist for a user.
    /// Used for ensuring directory structure on user creation.
    pub fn all_directories(&self) -> Vec<&Path> {
        vec![
            &self.root,
            &self.thumbnails,
            &self.thumbnails_bg,
            &self.thumbnails_avatar,
            &self.thumbnails_persona,
            &self.worlds,
            &self.user,
            &self.avatars,
            &self.user_images,
            &self.groups,
            &self.group_chats,
            &self.chats,
            &self.characters,
            &self.backgrounds,
            &self.novel_ai_settings,
            &self.kobold_ai_settings,
            &self.open_ai_settings,
            &self.text_gen_settings,
            &self.themes,
            &self.moving_ui,
            &self.extensions,
            &self.instruct,
            &self.context,
            &self.quick_replies,
            &self.assets,
            &self.comfy_workflows,
            &self.files,
            &self.vectors,
            &self.backups,
            &self.sysprompt,
            &self.reasoning,
        ]
    }

    /// Ensures all user directories exist, creating them if necessary.
    pub fn ensure_directories(&self) -> std::io::Result<()> {
        for dir in self.all_directories() {
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
            }
        }
        Ok(())
    }
}

/// Returns the list of template entries (field_name, relative_path) for
/// iteration or dynamic lookup.
pub fn user_directory_templates() -> &'static [(&'static str, &'static str)] {
    USER_DIR_TEMPLATES
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_directories_resolve_correctly() {
        let data_root = Path::new("/data");
        let dirs = UserDirectories::new(data_root, "test-user");

        assert_eq!(dirs.root, PathBuf::from("/data/test-user"));
        assert_eq!(dirs.characters, PathBuf::from("/data/test-user/characters"));
        assert_eq!(
            dirs.avatars,
            PathBuf::from("/data/test-user/User Avatars")
        );
        assert_eq!(
            dirs.group_chats,
            PathBuf::from("/data/test-user/group chats")
        );
        assert_eq!(
            dirs.novel_ai_settings,
            PathBuf::from("/data/test-user/NovelAI Settings")
        );
        assert_eq!(
            dirs.user_images,
            PathBuf::from("/data/test-user/user/images")
        );
        assert_eq!(
            dirs.comfy_workflows,
            PathBuf::from("/data/test-user/user/workflows")
        );
        assert_eq!(dirs.files, PathBuf::from("/data/test-user/user/files"));
    }

    #[test]
    fn all_directories_returns_complete_set() {
        let data_root = Path::new("/data");
        let dirs = UserDirectories::new(data_root, "user");
        let all = dirs.all_directories();

        // USER_DIRECTORY_TEMPLATE has 31 entries (including root)
        assert_eq!(all.len(), 31);
    }

    #[test]
    fn public_directories_resolve_correctly() {
        let server_dir = Path::new("/app");
        let pub_dirs = PublicDirectories::new(server_dir);

        assert_eq!(pub_dirs.images, PathBuf::from("/app/public/img"));
        assert_eq!(pub_dirs.backups, PathBuf::from("/app/backups"));
        assert_eq!(
            pub_dirs.global_extensions,
            PathBuf::from("/app/public/scripts/extensions/third-party")
        );
    }

    #[test]
    fn data_root_directories_resolve_correctly() {
        let data_root = Path::new("/data");
        let dr_dirs = DataRootDirectories::new(data_root);

        assert_eq!(dr_dirs.storage, PathBuf::from("/data/_storage"));
        assert_eq!(dr_dirs.cache, PathBuf::from("/data/_cache"));
        assert_eq!(dr_dirs.uploads, PathBuf::from("/data/_uploads"));
    }

    #[test]
    fn template_entries_match_node_constants() {
        let templates = user_directory_templates();

        // Verify key entries match Node's USER_DIRECTORY_TEMPLATE
        let find = |name: &str| -> &str {
            templates
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| *v)
                .unwrap_or("NOT_FOUND")
        };

        assert_eq!(find("root"), "");
        assert_eq!(find("thumbnails"), "thumbnails");
        assert_eq!(find("avatars"), "User Avatars");
        assert_eq!(find("group_chats"), "group chats");
        assert_eq!(find("novel_ai_settings"), "NovelAI Settings");
        assert_eq!(find("kobold_ai_settings"), "KoboldAI Settings");
        assert_eq!(find("open_ai_settings"), "OpenAI Settings");
        assert_eq!(find("text_gen_settings"), "TextGen Settings");
        assert_eq!(find("moving_ui"), "movingUI");
        assert_eq!(find("quick_replies"), "QuickReplies");
        assert_eq!(find("comfy_workflows"), "user/workflows");
        assert_eq!(find("files"), "user/files");
    }
}

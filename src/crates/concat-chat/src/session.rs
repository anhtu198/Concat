// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors
//! The conversation each project carries on: Claude Code's session id,
//! kept in the app's data folder (never inside the project folder, which
//! may be synced or shared).

use std::path::PathBuf;

/// Session ids, one small file per project folder.
#[derive(Debug, Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    /// A store in `dir`, made when first written.
    pub fn new(dir: PathBuf) -> SessionStore {
        SessionStore { dir }
    }

    /// The session the project's conversation carries on, if any.
    pub fn get(&self, project_path: &str) -> Option<String> {
        let id = std::fs::read_to_string(self.file(project_path)).ok()?;
        Some(id.trim().to_owned()).filter(|id| !id.is_empty())
    }

    /// Remembers `id` for the project. A failure to write only costs the
    /// next message its memory of this one.
    pub fn set(&self, project_path: &str, id: &str) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.file(project_path), id);
    }

    /// Forgets the project's conversation ("New conversation").
    pub fn clear(&self, project_path: &str) {
        let _ = std::fs::remove_file(self.file(project_path));
    }

    fn file(&self, project_path: &str) -> PathBuf {
        self.dir
            .join(format!("{:016x}.session", fnv1a(&normalize(project_path))))
    }
}

/// One spelling per folder: no trailing separator, and on Windows, where
/// paths ignore case, lower case.
fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    if cfg!(windows) {
        trimmed.replace('/', "\\").to_lowercase()
    } else {
        trimmed.to_owned()
    }
}

/// FNV-1a, 64 bits: stable across Rust versions, unlike `DefaultHasher`, so
/// a toolchain update does not forget every conversation.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "concat-chat-sessions-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    #[test]
    fn session_roundtrip_and_clear() {
        let store = store("roundtrip");
        let project = r"D:\OneDrive - xistatu\Desktop\Concat\thu-live";
        assert_eq!(store.get(project), None);
        store.set(project, "abc-123");
        assert_eq!(store.get(project).as_deref(), Some("abc-123"));
        assert_eq!(store.get(r"D:\Other"), None);
        store.clear(project);
        assert_eq!(store.get(project), None);
    }

    #[cfg(windows)]
    #[test]
    fn session_path_case_insensitive_on_windows() {
        let store = store("case");
        store.set(r"D:\Concat\Thu-Live\", "s1");
        assert_eq!(store.get(r"d:\concat\thu-live").as_deref(), Some("s1"));
    }
}

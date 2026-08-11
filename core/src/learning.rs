//! User learning module.
//!
//! Responsibilities:
//!   - Record every word the user selects and increase its ranking weight
//!   - Allow user-defined words to be added (pinyin + characters)
//!   - Persist data to a local file in the platform data directory
//!
//! # Persistence format
//!
//! Plain UTF-8 text; one record per line:
//! ```text
//! word<TAB>selection_count
//! ```
//!
//! Example:
//! ```text
//! 你好    42
//! 中国人  17
//! ```
//!
//! # Write strategy
//!
//! A dirty flag tracks unsaved changes. Data is only written to disk at the
//! end of an input session (composition cancelled / committed) or on `Drop`,
//! so normal typing never triggers disk I/O.
//! Writes go to a `.tmp` file first; the file is then renamed atomically to
//! avoid corruption if the process is killed mid-write.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Persistent store for user selection history.
pub struct LearningStore {
    /// word → number of times the user has selected it
    freq_map: HashMap<String, u32>,
    /// Path to the on-disk file (supplied by the platform layer).
    store_path: PathBuf,
    /// Whether there are unsaved changes.
    dirty: bool,
}

impl LearningStore {
    /// Load from a local file. Returns an empty store if the file does not yet
    /// exist (normal on first run).
    pub fn load(path: PathBuf) -> Self {
        let freq_map = Self::read_file(&path).unwrap_or_default();
        LearningStore {
            freq_map,
            store_path: path,
            dirty: false,
        }
    }

    /// Record that the user selected `word`. Marks the store as dirty.
    ///
    /// Callers are responsible for ensuring single-threaded access.
    pub fn record_selection(&mut self, word: &str) {
        *self.freq_map.entry(word.to_string()).or_insert(0) += 1;
        self.dirty = true;
    }

    /// Add a user-defined word with an initial selection count of 1.
    ///
    /// If the word already exists its count is left unchanged.
    pub fn add_user_word(&mut self, word: &str) {
        self.freq_map.entry(word.to_string()).or_insert(1);
        self.dirty = true;
    }

    /// Return the number of times the user has selected `word` (0 if unknown).
    pub fn user_freq(&self, word: &str) -> u32 {
        self.freq_map.get(word).copied().unwrap_or(0)
    }

    /// Write to disk only if there are unsaved changes.
    ///
    /// Call this at the end of each input session and before process exit.
    pub fn flush_if_dirty(&mut self) {
        if self.dirty {
            if let Err(e) = self.save() {
                // Non-fatal: log and continue. Data will be lost for this session
                // but the next session starts cleanly.
                eprintln!("[ztap] failed to save learning store: {e}");
            } else {
                self.dirty = false;
            }
        }
    }

    /// Unconditionally write the current data to disk.
    ///
    /// Entries are sorted alphabetically so the file is stable and diff-friendly.
    /// Uses a write-then-rename pattern to avoid partial writes.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.store_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut pairs: Vec<(&String, &u32)> = self.freq_map.iter().collect();
        pairs.sort_by_key(|(word, _)| word.as_str());

        let mut content = String::new();
        for (word, &count) in &pairs {
            content.push_str(word);
            content.push('\t');
            content.push_str(&count.to_string());
            content.push('\n');
        }

        let tmp_path = self.store_path.with_extension("tmp");
        fs::write(&tmp_path, &content)?;
        fs::rename(&tmp_path, &self.store_path)?;
        Ok(())
    }

    /// Parse the on-disk file into a frequency map.
    fn read_file(path: &PathBuf) -> Option<HashMap<String, u32>> {
        let content = fs::read_to_string(path).ok()?;
        let mut map = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(2, '\t');
            let word  = parts.next()?.to_string();
            let count: u32 = parts.next()?.trim().parse().ok()?;
            map.insert(word, count);
        }
        Some(map)
    }

    /// Number of words tracked in this store.
    pub fn len(&self) -> usize {
        self.freq_map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.freq_map.is_empty()
    }
}

impl Drop for LearningStore {
    /// Automatically flush on drop so normal process exits never lose data.
    fn drop(&mut self) {
        self.flush_if_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ztap_learning_{tag}.txt"))
    }

    #[test]
    fn test_record_and_query() {
        let path = tmp_path("record");
        let _ = fs::remove_file(&path);
        let mut store = LearningStore::load(path);
        assert_eq!(store.user_freq("你好"), 0);
        store.record_selection("你好");
        store.record_selection("你好");
        assert_eq!(store.user_freq("你好"), 2);
    }

    #[test]
    fn test_save_and_reload() {
        let path = tmp_path("reload");
        let _ = fs::remove_file(&path);
        {
            let mut store = LearningStore::load(path.clone());
            store.record_selection("中国");
            store.record_selection("中国");
            store.record_selection("北京");
            store.save().unwrap();
        }
        let store2 = LearningStore::load(path.clone());
        assert_eq!(store2.user_freq("中国"), 2);
        assert_eq!(store2.user_freq("北京"), 1);
        assert_eq!(store2.user_freq("上海"), 0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let store = LearningStore::load(tmp_path("nonexistent_xyz_abc"));
        assert!(store.is_empty());
    }

    #[test]
    fn test_add_user_word() {
        let path = tmp_path("userword");
        let _ = fs::remove_file(&path);
        let mut store = LearningStore::load(path);
        store.add_user_word("自定义词");
        assert_eq!(store.user_freq("自定义词"), 1);
        // Calling again should not overwrite existing count.
        store.record_selection("自定义词");
        store.add_user_word("自定义词");
        assert_eq!(store.user_freq("自定义词"), 2);
    }
}

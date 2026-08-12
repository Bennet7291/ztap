use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub struct LearningStore {

    freq_map: HashMap<String, u32>,

    store_path: PathBuf,

    dirty: bool,
}

impl LearningStore {

    pub fn load(path: PathBuf) -> Self {
        let freq_map = Self::read_file(&path).unwrap_or_default();
        LearningStore {
            freq_map,
            store_path: path,
            dirty: false,
        }
    }

    pub fn record_selection(&mut self, word: &str) {
        *self.freq_map.entry(word.to_string()).or_insert(0) += 1;
        self.dirty = true;
    }

    pub fn add_user_word(&mut self, word: &str) {
        self.freq_map.entry(word.to_string()).or_insert(1);
        self.dirty = true;
    }

    pub fn user_freq(&self, word: &str) -> u32 {
        self.freq_map.get(word).copied().unwrap_or(0)
    }

    pub fn flush_if_dirty(&mut self) {
        if self.dirty {
            if let Err(e) = self.save() {

                eprintln!("[ztap] failed to save learning store: {e}");
            } else {
                self.dirty = false;
            }
        }
    }

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

    pub fn len(&self) -> usize {
        self.freq_map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.freq_map.is_empty()
    }
}

impl Drop for LearningStore {

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

        store.record_selection("自定义词");
        store.add_user_word("自定义词");
        assert_eq!(store.user_freq("自定义词"), 2);
    }
}

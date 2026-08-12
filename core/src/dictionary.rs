use std::collections::HashMap;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Entry {

    pub word: String,

    pub syllables: Vec<String>,

    pub base_freq: u32,
}

static BUILTIN_DICT_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ztap.dict.bin"));

pub struct Dictionary {

    index: HashMap<String, Vec<Entry>>,

    total: usize,
}

impl Dictionary {

    pub fn load_builtin() -> Self {
        let entries: Vec<Entry> = bincode::deserialize(BUILTIN_DICT_BLOB)
            .expect("ztap-core: failed to deserialize embedded dictionary blob (built by build.rs) \
                     — this indicates a build.rs / dictionary.rs Entry-shape mismatch, see the \
                     \"Keep in sync\" note on Entry");

        let mut index: HashMap<String, Vec<Entry>> = HashMap::new();
        let mut total = 0usize;
        for entry in entries {
            if let Some(first) = entry.syllables.first() {
                index.entry(first.clone()).or_default().push(entry);
                total += 1;
            }
        }

        Dictionary { index, total }
    }

    pub fn parse(data: &str) -> Self {
        let mut index: HashMap<String, Vec<Entry>> = HashMap::new();
        let mut total = 0usize;

        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(entry) = Self::parse_line(line) {
                if let Some(first) = entry.syllables.first() {
                    index.entry(first.clone()).or_default().push(entry);
                    total += 1;
                }
            }
        }

        Dictionary { index, total }
    }

    fn parse_line(line: &str) -> Option<Entry> {
        let mut parts = line.splitn(3, '\t');
        let word = parts.next()?.trim().to_string();
        let pinyin_raw = parts.next()?.trim();
        let freq_str = parts.next().unwrap_or("1").trim();

        if word.is_empty() || pinyin_raw.is_empty() {
            return None;
        }

        let syllables: Vec<String> = pinyin_raw
            .split_whitespace()
            .map(strip_tone)
            .collect();

        let base_freq: u32 = freq_str.parse().unwrap_or(1);

        Some(Entry { word, syllables, base_freq })
    }

    pub fn lookup(&self, syllables: &[&str]) -> Vec<Entry> {
        let first = match syllables.first() {
            Some(s) => *s,
            None => return vec![],
        };
        self.index
            .get(first)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| {
                        e.syllables.len() >= syllables.len()
                            && e.syllables
                                .iter()
                                .zip(syllables.iter())
                                .all(|(a, b)| a == b)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn lookup_exact(&self, syllables: &[&str]) -> Vec<Entry> {
        let first = match syllables.first() {
            Some(s) => *s,
            None => return vec![],
        };
        self.index
            .get(first)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| {
                        e.syllables.len() == syllables.len()
                            && e.syllables
                                .iter()
                                .zip(syllables.iter())
                                .all(|(a, b)| a == b)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn merge(&mut self, other: Dictionary) {
        for (key, entries) in other.index {
            self.index.entry(key).or_default().extend(entries);
        }
        self.total += other.total;
    }

    pub fn len(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

fn strip_tone(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'ā' | 'á' | 'ǎ' | 'à' => 'a',
            'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
            'ē' | 'é' | 'ě' | 'è' | 'ê' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' => 'i',
            'ū' | 'ú' | 'ǔ' | 'ù' => 'u',

            'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'v',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dict() -> Dictionary {
        Dictionary::parse(
            "你好\tni hao\t99000\n\
             你们\tni men\t80000\n\
             中国\tzhong guo\t120000\n\
             中国人\tzhong guo ren\t60000\n",
        )
    }

    #[test]
    fn test_lookup_prefix() {
        let d = make_dict();
        let results = d.lookup(&["ni"]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_lookup_exact() {
        let d = make_dict();
        let results = d.lookup_exact(&["ni", "hao"]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].word, "你好");
    }

    #[test]
    fn test_empty_query() {
        let d = make_dict();
        assert!(d.lookup(&[]).is_empty());
    }

    #[test]
    fn test_merge() {
        let mut base = make_dict();
        let extra = Dictionary::parse("上海\tshang hai\t108000\n");
        base.merge(extra);
        assert!(!base.lookup(&["shang"]).is_empty());
    }

    #[test]
    fn test_strip_tone_all_vowels() {
        assert_eq!(strip_tone("zhōng"), "zhong");
        assert_eq!(strip_tone("guó"), "guo");
        assert_eq!(strip_tone("rén"), "ren");
        assert_eq!(strip_tone("nǐ"), "ni");
        assert_eq!(strip_tone("hǎo"), "hao");
        assert_eq!(strip_tone("lǜ"), "lv");
        assert_eq!(strip_tone("nǚ"), "nv");
    }

    #[test]
    fn test_strip_tone_passthrough_for_untoned_input() {

        assert_eq!(strip_tone("zhong"), "zhong");
        assert_eq!(strip_tone("lv"), "lv");
    }

    #[test]
    fn test_parse_line_with_toned_pinyin() {

        let d = Dictionary::parse("中国人\tzhōng guó rén\t60000\n");
        let results = d.lookup_exact(&["zhong", "guo", "ren"]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].word, "中国人");
    }

    #[test]
    fn test_load_builtin_contains_common_words() {
        let dict = Dictionary::load_builtin();
        assert!(dict.len() > 100_000, "expected a full-size dictionary, got {} entries", dict.len());

        let nihao = dict.lookup_exact(&["ni", "hao"]);
        assert!(nihao.iter().any(|e| e.word == "你好"), "\"你好\" (ni hao) missing from built-in dictionary");

        let zhongguo = dict.lookup_exact(&["zhong", "guo"]);
        assert!(zhongguo.iter().any(|e| e.word == "中国"), "\"中国\" (zhong guo) missing from built-in dictionary");
    }
}

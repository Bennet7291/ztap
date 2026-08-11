//! Dictionary module.
//!
//! Responsibilities:
//!   - Load and hold the built-in word list (derived from Rime data, compiled into the binary)
//!   - Query candidate entries by pinyin syllable sequence
//!   - Support prefix matching for live typing
//!
//! # Dictionary format (text source; build.rs compiles it to a binary blob)
//!
//! Tab-separated line format (comments and YAML header stripped):
//! ```text
//! 中国人   zhōng guó rén   123456
//! 你好     nǐ hǎo           98765
//! ```
//! Tone marks are stripped on load; syllables are stored as a `Vec<String>`.
//! See `dict/ztap.dict.txt`'s header for the full provenance of the bundled
//! data (merged from Rime's `luna_pinyin` and `rime-ice`'s `base` word
//! list — the latter fills in common multi-character words the former
//! omits by design, relying instead on a runtime grammar model that
//! `ztap-core`, being a static word-list engine, does not have).

use std::collections::HashMap;

/// A single dictionary entry.
///
/// # Keep in sync with `build.rs`
///
/// `build.rs` maintains its own `BuildEntry` mirror of this struct's shape
/// (word, syllables, base_freq, in that order) because it cannot depend on
/// the not-yet-compiled crate this type lives in. `bincode`'s wire format
/// has no field names, only position — if you add, remove, or reorder a
/// field here, make the matching change in `build.rs`'s `BuildEntry`, or
/// the compiled dictionary blob will silently deserialize into the wrong
/// fields.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Entry {
    /// The Chinese word, e.g. "中国人".
    pub word: String,
    /// Tone-stripped pinyin syllables, e.g. ["zhong", "guo", "ren"].
    pub syllables: Vec<String>,
    /// Base frequency weight from the source corpus (higher = more common).
    pub base_freq: u32,
}

/// Compiled dictionary blob, embedded at compile time.
///
/// `build.rs` parses `dict/ztap.dict.txt` once, at build time, into a
/// `bincode`-serialized `Vec<Entry>` written to `$OUT_DIR/ztap.dict.bin`;
/// this just pulls those bytes into the binary. Deserializing a flat,
/// already-tokenized `Vec<Entry>` at startup is far cheaper than parsing
/// ~590k lines of tab-separated text on every launch — see build.rs's
/// module doc comment for the full rationale.
static BUILTIN_DICT_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ztap.dict.bin"));

/// Runtime dictionary, indexed by first syllable for fast prefix lookup.
pub struct Dictionary {
    /// first syllable → list of entries whose syllable sequence starts with that syllable
    index: HashMap<String, Vec<Entry>>,
    /// Total number of entries (for diagnostics).
    total: usize,
}

impl Dictionary {
    /// Load from the compile-time embedded dictionary blob.
    ///
    /// Panics if the embedded blob fails to deserialize. This should be
    /// impossible in practice — `build.rs` and this module encode the same
    /// format — but a panic here (rather than silently returning an empty
    /// dictionary) is deliberate: an IME that starts up and produces zero
    /// candidates for every keystroke is a far more confusing failure mode
    /// for the person using it than a crash-on-launch that immediately
    /// surfaces the bug to whoever's building the app.
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

    /// Parse a dictionary from an arbitrary text string.
    ///
    /// Also used in tests and for merging user dictionaries.
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

    /// Parse a single dictionary line.
    ///
    /// Expected format: `word\tpinyin (space-separated, with or without tones)\tfreq`
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

    /// Find all entries whose syllable sequence starts with the given prefix.
    ///
    /// Used for live candidate generation while the user is still typing.
    /// E.g. `lookup(&["ni"])` returns "你", "你好", "你们", etc.
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

    /// Find all entries whose syllable sequence exactly matches the given sequence.
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

    /// Merge another dictionary (e.g. the user dictionary) into this one.
    pub fn merge(&mut self, other: Dictionary) {
        for (key, entries) in other.index {
            self.index.entry(key).or_default().extend(entries);
        }
        self.total += other.total;
    }

    /// Total number of entries in this dictionary.
    pub fn len(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Strip tone diacritic marks from a pinyin syllable string.
///
/// E.g. "zhōng" → "zhong", "nǐ" → "ni".
///
/// Maps every toned vowel (ā á ǎ à, ō ó ǒ ò, ē é ě è, ī í ǐ ì, ū ú ǔ ù,
/// ǖ ǘ ǚ ǜ ü, and the standalone tone-5/neutral-tone `ê`) to its bare
/// ASCII vowel, so a dictionary source that ships with tone marks (unlike
/// the bundled Rime `luna_pinyin` data, which already omits them) can
/// still be parsed. `ü` itself maps to `v`, matching the ASCII-umlaut
/// convention `pinyin::VALID_SYLLABLES` expects (see that module's doc
/// comment on `is_valid_syllable`).
///
/// Untoned/already-ASCII input passes straight through with no
/// allocation beyond what `to_string()`/`String` already requires at the
/// call site — this function only rewrites characters it recognizes.
fn strip_tone(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'ā' | 'á' | 'ǎ' | 'à' => 'a',
            'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
            'ē' | 'é' | 'ě' | 'è' | 'ê' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' => 'i',
            'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
            // ü and all four toned ü variants → ASCII "v" substitute.
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
        assert_eq!(strip_tone("lǜ"), "lv"); // 绿 -> ASCII "v" substitute
        assert_eq!(strip_tone("nǚ"), "nv"); // 女
    }

    #[test]
    fn test_strip_tone_passthrough_for_untoned_input() {
        // The bundled Rime data ships without tone marks; this must be a no-op.
        assert_eq!(strip_tone("zhong"), "zhong");
        assert_eq!(strip_tone("lv"), "lv");
    }

    #[test]
    fn test_parse_line_with_toned_pinyin() {
        // A dictionary source with tone marks (unlike the bundled stub)
        // must still parse into tone-stripped, ASCII-only syllables.
        let d = Dictionary::parse("中国人\tzhōng guó rén\t60000\n");
        let results = d.lookup_exact(&["zhong", "guo", "ren"]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].word, "中国人");
    }

    /// End-to-end check that the compiled dictionary blob actually loads
    /// and contains the everyday words the build pipeline exists to
    /// provide (see dict/ztap.dict.txt's header: the bundled bare
    /// `luna_pinyin` corpus alone does not contain "你好" or "中国" — Rime
    /// normally composes those from single characters via a grammar model
    /// this static-word-list engine doesn't have, hence the rime-ice merge).
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

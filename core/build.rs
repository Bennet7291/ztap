//! Build script: compile the plain-text dictionary source into a compact
//! binary blob that `dictionary.rs` embeds via `include_bytes!` +
//! `bincode::deserialize`.
//!
//! # Why a build step at all
//!
//! `dict/ztap.dict.txt` is ~590k lines of tab-separated
//! `word\tsyl1 syl2 ...\tfreq` records (see that file's header comment for
//! provenance). Parsing 590k lines of text on every IME startup — on every
//! keystroke-adjacent cold start after a reboot — is wasted work the user
//! would feel as input lag. Rust's `include_str!` embeds the text as-is with
//! no such cost, but shipping raw un-tokenized text also means `Dictionary`
//! would still have to split every line by tabs and spaces at runtime before
//! it can build its lookup index.
//!
//! Instead this script parses the text once, at compile time, into the same
//! `Vec<Entry>`-shaped data the runtime wants, and serializes that directly
//! with `bincode`. At runtime, `dictionary.rs` just deserializes the blob —
//! no string splitting, no number parsing — and builds the `HashMap` index
//! from the ready-made entries.
//!
//! # Wire format
//!
//! `bincode`-encoded `Vec<BuildEntry>`. `BuildEntry` mirrors
//! `dictionary::Entry` field-for-field (word, syllables, base_freq) — see
//! the "keep in sync" note on that struct below. `bincode` does not carry
//! field names in the wire format, so field *order* is what ties the two
//! struct definitions together; changing one without the other silently
//! breaks deserialization at runtime (it would either error or, worse,
//! misalign fields without erroring). There's no way to `use` the real
//! `Entry` type here because `build.rs` runs *before* the crate it belongs
//! to is compiled, so this mirror struct is the standard `bincode`
//! build-script pattern, not a shortcut.
//!
//! # Regenerating the dictionary
//!
//! Editing `dict/ztap.dict.txt` (or replacing it with a fresher pull of the
//! upstream corpora — see that file's header) and running `cargo build` is
//! sufficient; Cargo re-runs this script automatically because of the
//! `cargo:rerun-if-changed` line below.

use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Mirror of `dictionary::Entry`'s shape for the sole purpose of
/// `bincode` serialization from `build.rs`. Keep field order in sync with
/// `dictionary::Entry` — see module doc comment above.
#[derive(serde::Serialize)]
struct BuildEntry {
    word: String,
    syllables: Vec<String>,
    base_freq: u32,
}

fn main() {
    let src_path = "dict/ztap.dict.txt";
    println!("cargo:rerun-if-changed={src_path}");

    let text = fs::read_to_string(src_path).unwrap_or_else(|e| {
        panic!(
            "ztap-core build.rs: failed to read dictionary source at \
             '{src_path}' (relative to the ztap-core crate root): {e}. \
             This file is required to build the built-in dictionary blob."
        )
    });

    let mut entries = Vec::with_capacity(600_000);
    let mut skipped = 0usize;

    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match parse_line(line) {
            Some(entry) => entries.push(entry),
            None => {
                // Don't fail the build over a handful of malformed lines —
                // a bad line in a 590k-line corpus is a data-quality issue,
                // not a build-breaking one — but do surface it, since silent
                // data loss here would be invisible until someone notices a
                // missing word at runtime.
                skipped += 1;
                if skipped <= 20 {
                    println!(
                        "cargo:warning=ztap dict: skipping malformed line {}: {line:?}",
                        lineno + 1
                    );
                }
            }
        }
    }

    if skipped > 20 {
        println!(
            "cargo:warning=ztap dict: ...and {} more malformed lines (only first 20 shown)",
            skipped - 20
        );
    }

    if entries.is_empty() {
        panic!(
            "ztap-core build.rs: parsed zero valid entries from '{src_path}'. \
             Refusing to build an empty dictionary blob."
        );
    }

    let encoded = bincode::serialize(&entries).unwrap_or_else(|e| {
        panic!("ztap-core build.rs: bincode serialization of {} entries failed: {e}", entries.len())
    });

    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR not set by cargo");
    let out_path = Path::new(&out_dir).join("ztap.dict.bin");
    let mut f = fs::File::create(&out_path).unwrap_or_else(|e| {
        panic!("ztap-core build.rs: failed to create {}: {e}", out_path.display())
    });
    f.write_all(&encoded).unwrap_or_else(|e| {
        panic!("ztap-core build.rs: failed to write {}: {e}", out_path.display())
    });

    println!(
        "cargo:warning=ztap dict: compiled {} entries ({} bytes) into {}",
        entries.len(),
        encoded.len(),
        out_path.display()
    );
}

/// Parse one `word\tsyl1 syl2 ...\tfreq` line.
///
/// Deliberately duplicates `dictionary::Dictionary::parse_line`'s logic
/// rather than sharing it, since `build.rs` cannot depend on its own
/// not-yet-compiled crate. Keep the two in sync if the text format changes —
/// `dictionary.rs`'s `Dictionary::parse` is still the parser of record for
/// runtime-supplied text (e.g. a future user dictionary file), so this
/// duplication is confined to build time and never runs against untrusted
/// input.
fn parse_line(line: &str) -> Option<BuildEntry> {
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

    if syllables.is_empty() {
        return None;
    }

    let base_freq: u32 = freq_str.parse().unwrap_or(1);

    Some(BuildEntry { word, syllables, base_freq })
}

/// Tone-mark stripper, duplicated from `dictionary::strip_tone` for the same
/// reason `parse_line` above is duplicated — see that function's doc
/// comment for the full character mapping rationale. The bundled
/// `dict/ztap.dict.txt` already ships tone-free (see its header), so this
/// is a no-op passthrough in practice today, but keeping it here means a
/// future source with tone marks compiles correctly without a `build.rs`
/// change.
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

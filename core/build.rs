use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

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

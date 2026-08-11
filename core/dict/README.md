# `ztap.dict.txt` provenance

This directory holds the plain-text source dictionary that `build.rs`
compiles into the binary blob embedded in the `ztap-core` library (see
`build.rs`'s module doc comment and `dictionary.rs`'s `BUILTIN_DICT_BLOB`
for the build pipeline itself).

## What's in it

`ztap.dict.txt` is a merge of two upstream corpora:

1. **[`rime/rime-luna-pinyin`](https://github.com/rime/rime-luna-pinyin)**
   (`luna_pinyin.dict.yaml`) — the dictionary this project's README names as
   its source. Provides single-character entries plus a modest set of
   multi-character idioms and proper nouns. Per that repository's `AUTHORS`
   file, its data derives from the Android Open Source Project's Pinyin IME
   (Apache License 2.0) and moedict.tw (CC0 1.0); the `rime-luna-pinyin`
   packaging itself is distributed under LGPL by various downstream
   packagers (Arch Linux, MSYS2).

2. **[`iDvel/rime-ice`](https://github.com/iDvel/rime-ice)**
   (`cn_dicts/base.dict.yaml`) — an actively-maintained, purpose-built
   simplified-Chinese lexicon. This supplies the everyday multi-character
   words (你好, 中国, 输入法, ...) that bare `luna_pinyin` does **not**
   include — Rime's reference implementation composes those from single
   characters at runtime via a statistical grammar model, which
   `ztap-core`'s static-dictionary architecture (see the crate's module
   docs) does not have. Without this supplement, `ztap` would fail to
   produce sensible candidates for extremely common input. `rime-ice` is
   **GPL-3.0-only**.

Both sources were filtered before merging:

- **Simplified-only**: any entry whose word form differs after an
  OpenCC Traditional→Simplified pass was dropped, since the README scopes
  this project to Simplified Chinese and the raw `luna_pinyin` corpus mixes
  both scripts.
- **Valid-syllable-only**: any entry using a syllable outside
  `pinyin::VALID_SYLLABLES` (the project's own definition of "valid
  Mandarin syllable," standard Hanyu Pinyin minus a handful of dialectal
  outliers — see that module's doc comment) was dropped, since it could
  never be reached through normal segmentation.
- **CJK-ideograph-only**: non-ideograph noise in the raw `luna_pinyin` data
  (bopomofo phonetic symbols, a few stray non-word entries) was dropped.

Weights from both sources were rescaled onto a common range so
`ranking.rs`'s scoring (base frequency + user frequency × 12 + per-char
length bonus × 5,000) behaves sensibly across entries pulled from either
source. `rime-ice` entries win on `(word, syllables)` collisions, since
that corpus was purpose-built and more precisely weighted for exactly the
common-word case being filled in.

## License

**Because `rime-ice` data (GPL-3.0-only) is compiled directly into the
`ztap-core` binary via `build.rs` + `include_bytes!`, the entire Ztap
workspace is licensed GPL-3.0-only** — see `../../LICENSE` and each crate's
`Cargo.toml`. This is a stricter license than the individual `luna_pinyin`
component alone would require; the whole-workspace license reflects the
strictest applicable term to avoid ambiguity about whether an
`include_bytes!`-embedded blob constitutes linking/a combined work under
copyleft terms in the applicable jurisdiction. This project takes the
conservative reading rather than assuming otherwise.

If a future contributor wants to keep `ztap` permissively licensed, the fix
is to *not* ship a GPL corpus in the default build — e.g. gate the
`rime-ice` merge behind an opt-in build feature that's off by default and
documented as changing the effective license of the resulting binary, or
replace it with a permissively-licensed common-word supplement and drop
`rime-ice` entirely. Swapping the license back to something permissive
without addressing the underlying data would misrepresent the terms the
bundled dictionary is actually available under.

## Regenerating

The generation pipeline (OpenCC filtering, syllable validation, weight
rescaling, merge) was run as a one-off Python script against the two
upstream `.dict.yaml` files. It is not currently checked into this
repository as a script — reproducing or updating the merge means
re-fetching both upstream files and re-running the equivalent filtering
logic described above.

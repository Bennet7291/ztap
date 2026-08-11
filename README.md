# Ztap

A lightweight, offline, zero-configuration Simplified Chinese pinyin input method.

## Project structure

```
ztap/
├── core/                        # Platform-agnostic input engine
│   ├── build.rs                 # Compiles dict/ztap.dict.txt into a bincode blob
│   ├── dict/
│   │   ├── ztap.dict.txt        # Merged dictionary source (see README.md below)
│   │   └── README.md            # Dictionary provenance & license notes
│   └── src/
│       ├── lib.rs               # InputSession façade + module re-exports
│       ├── pinyin.rs            # Syllable segmentation (DP over valid syllable set)
│       ├── dictionary.rs        # Word list load & query (built-in + user)
│       ├── ranking.rs           # Candidate scoring (corpus freq + user pref + length)
│       ├── learning.rs          # User selection history, local persistence
│       └── punctuation.rs       # ASCII → Chinese punctuation mapping
│
├── platform/
│   ├── windows/                 # cdylib — Windows TSF IME DLL
│   │   └── src/
│   │       ├── lib.rs           # DLL entry points, COM class factory, TSF registration
│   │       ├── tsf.rs           # ITfTextInputProcessor implementation
│   │       └── candidate_window.rs  # Win32 + Direct2D + DirectWrite UI
│   │
│   └── macos/                   # [[bin]] — macOS InputMethodKit bundle executable
│       ├── Info.plist           # Input-method bundle manifest (IMKServer keys)
│       └── src/
│           ├── main.rs          # Thin binary entry point
│           ├── lib.rs           # IMKServer setup, AppKit run loop
│           ├── input_method.rs  # IMKInputController subclass
│           └── candidate_window.rs  # NSPanel + CoreText UI
│
├── LICENSE                      # GPL-3.0-only (see License section below)
└── Cargo.toml                   # Workspace root
```

## Architecture

```
keyboard event
      │
      ▼
platform layer  (TSF on Windows / InputMethodKit on macOS)
      │  handles OS integration, text commit, UI display
      ▼
core layer
      ├── pinyin      →  segment continuous pinyin into syllable paths
      ├── dictionary  →  look up candidates by syllable sequence
      ├── ranking     →  score and sort candidates
      └── learning    →  record selections, persist user data locally
```

## Building

```bash
# Windows (run on Windows or via cross)
cargo build -p ztap-windows --release
```
Produces a `cdylib` (`ztap_windows.dll`) that must be registered as a TSF
text service via `regsvr32` (which calls the `DllRegisterServer` export;
see `platform/windows/src/lib.rs`).

```bash
# macOS
cargo build -p ztap-macos --release --target aarch64-apple-darwin
```
Produces a `Ztap` binary (`[[bin]]` target, not a `cdylib` — InputMethodKit
runs one long-lived process per input method rather than loading a plugin
into each client). That binary, `platform/macos/Info.plist`, and an app
icon need to be assembled by hand into a `Ztap.app` bundle
(`Contents/MacOS/Ztap`, `Contents/Info.plist`, ...) and installed under
`~/Library/Input Methods/` — there is no Xcode project or bundling script
in this repository yet. See `platform/macos/src/lib.rs`'s module doc
comment for the App Sandbox caveat that must be handled during that
packaging step.

**Neither platform target has been compiled or run** — see the
`# WARNING: UNTESTED` module doc comment at the top of every file under
`platform/windows/src/` and `platform/macos/src/` for what's been checked
against each API's published documentation versus what still needs
verification against a real compiler and a live client application.
`ztap-core` (the platform-agnostic engine) has no such caveat and is fully
tested (`cargo test -p ztap-core`).

## Dictionary source

The built-in word list merges two upstream corpora:

- [`rime/rime-luna-pinyin`](https://github.com/rime/rime-luna-pinyin)
  (`luna_pinyin`) — single characters and a modest set of idioms/proper
  nouns.
- [`iDvel/rime-ice`](https://github.com/iDvel/rime-ice) (`cn_dicts/base`) —
  supplies common multi-character words (你好, 中国, ...) that bare
  `luna_pinyin` omits by design, since Rime's reference implementation
  composes those at runtime from a grammar model `ztap-core`'s static
  dictionary doesn't have.

Only the data from each is used; neither Rime nor `rime-ice`'s Lua/schema
tooling is a runtime dependency. Full provenance, filtering, and the
license implication below are documented in `core/dict/README.md`.

## License

**GPL-3.0-only**, workspace-wide — see `LICENSE`. This is driven by the
`rime-ice` dictionary data above, which is GPL-3.0-only and is compiled
directly into the `ztap-core` binary; see `core/dict/README.md` for the
full reasoning and for what changing this later would require.

## Design goals

- **Single binary** — no runtime dependencies, no Electron, no WebView
- **Offline** — no network access, no user account
- **Zero configuration** — works immediately after installation
- **Native UI** — candidate window drawn with platform APIs (Direct2D / CoreText)

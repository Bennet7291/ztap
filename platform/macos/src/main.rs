//! Binary entry point for the `Ztap` executable inside `Ztap.app`.
//!
//! This is a thin wrapper: all real setup lives in `lib.rs::run`, kept
//! separate so `ztap-macos`'s actual logic is unit-testable as a library
//! (`cargo test -p ztap-macos` can exercise `input_method`/
//! `candidate_window` without needing to launch a full `.app` bundle),
//! while `CFBundleExecutable` in Info.plist still has a concrete binary to
//! point at.

fn main() {
    ztap_macos::run();
}

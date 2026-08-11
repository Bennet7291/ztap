//! macOS platform entry point.
//!
//! Ztap on macOS is a standalone executable inside an Input Method bundle
//! (`Ztap.app`, installed under `~/Library/Input Methods/`), not a `cdylib`
//! -- this differs from the Windows side (a TSF DLL loaded into every
//! process) because InputMethodKit's architecture is "one long-running
//! process per input method, talking to clients over Mach IPC via
//! IMKServer/NSConnection," not "load a plugin into each client process."
//!
//! # WARNING: UNTESTED -- see input_method.rs's module doc comment
//!
//! The IMKServer setup sequence below (name/bundleIdentifier from
//! Info.plist, NSApplicationMain-equivalent run loop) follows the pattern
//! used by essentially every open InputMethodKit sample (Apple's own
//! "InputMethodKit sample code," countless open-source Chinese/Japanese
//! IMEs) translated into objc2, but has never been compiled or run. Two
//! things flagged as specifically likely to need adjustment on a real
//! build:
//!
//! 1. **App Sandbox must be disabled** in this bundle's entitlements.
//!    `IMKServer` depends on `NSConnection`, which does not work inside
//!    the sandbox; multiple independent IMKit tutorials report this as the
//!    single most common "why won't my IME connect" bug. There is no
//!    entitlements file in this draft -- one needs to be created
//!    (`com.apple.security.app-sandbox` set to `false`, or simply no
//!    sandbox entitlement at all) as part of the Xcode project / bundle
//!    build step, which is outside what a `cargo build` alone produces.
//! 2. The exact `IMKServer` initializer signature exposed by
//!    `objc2-input-method-kit` (argument order, `Option<&NSString>` vs
//!    `&NSString` for the two string parameters) should be checked against
//!    that crate's real docs for the `IMKServer` struct specifically --
//!    this draft's search access surfaced `IMKInputController`'s and
//!    `NSObjectIMKServerInput`'s exact signatures but not `IMKServer`'s own
//!    initializer in the same level of detail.

pub mod candidate_window;
pub mod input_method;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSBundle, NSString};
use objc2_input_method_kit::IMKServer;

/// Entry point for the `Ztap` executable inside the `.app` bundle.
///
/// Mirrors the classic Objective-C IMKit `main.m`:
/// ```objc
/// int main(int argc, char *argv[]) {
///     @autoreleasepool {
///         NSString *name = [[NSBundle mainBundle] objectForInfoDictionaryKey:@"InputMethodConnectionName"];
///         NSString *identifier = [[NSBundle mainBundle] bundleIdentifier];
///         IMKServer *server = [[IMKServer alloc] initWithName:name bundleIdentifier:identifier];
///         [NSApplication sharedApplication];
///         [NSApp run];
///     }
///     return 0;
/// }
/// ```
/// translated into objc2. `#[no_mangle] pub extern "C" fn main` is how a
/// `cdylib`-free, plain Rust `bin`-shaped crate can still serve as the
/// bundle's actual Unix process entry point (set via `CFBundleExecutable`
/// in Info.plist pointing at this compiled binary's filename).
pub fn run() {
    // SAFETY: this function is the process entry point and runs before any
    // other thread exists, so MainThreadMarker::new() succeeding here is
    // guaranteed -- the very first thread of any process is always
    // "the main thread."
    let mtm = MainThreadMarker::new().expect("run() must be called from the process's initial thread");

    // SAFETY: NSAutoreleasePool-equivalent scoping. objc2's #[autoreleasepool]
    // machinery (or an explicit objc2::rc::autoreleasepool closure) should
    // wrap this whole function body on a real build -- omitted here as an
    // explicit gap rather than guessed at, since the exact autoreleasepool
    // API shape (attribute vs. closure-based) is another detail this draft
    // could not verify against objc2's current version. Without it, this
    // still runs, but autoreleased objects created during setup (e.g. the
    // NSString below) may leak for the process's lifetime rather than
    // being cleaned up promptly -- acceptable for a long-running, single-
    // pool-scope process like an IME's main(), but worth fixing properly.

    let bundle = NSBundle::mainBundle();
    let connection_name: Option<Retained<NSString>> = unsafe {
        let key = NSString::from_str("InputMethodConnectionName");
        bundle
            .objectForInfoDictionaryKey(&key)
            .and_then(|obj| obj.downcast::<NSString>().ok())
    };
    let bundle_identifier = bundle.bundleIdentifier();

    // WARNING: IMKServer's initializer signature (argument order,
    // Option<&NSString> vs &NSString) is transcribed from the
    // Objective-C header (`initWithName:bundleIdentifier:`) rather than
    // objc2-input-method-kit's generated Rust signature specifically --
    // see this module's doc comment, point 2.
    let _server: Retained<IMKServer> = unsafe {
        let alloc = IMKServer::alloc();
        objc2::msg_send![
            alloc,
            initWithName: connection_name.as_deref(),
            bundleIdentifier: bundle_identifier.as_deref(),
        ]
    };
    // `_server` is intentionally kept alive for the whole process lifetime
    // (leaked into this function's stack frame, which never returns until
    // the process exits) -- IMKServer's own docs describe it as something
    // an input method allocates once in `main` and keeps alive for the
    // program's duration; there is no "shutdown" call to make here.

    // SAFETY: NSApplication::sharedApplication() is always safe to call;
    // it's the standard way to bring up the AppKit run loop infrastructure
    // that IMKServer's NSConnection-based client dispatch relies on, even
    // though Ztap draws no visible application window of its own (only the
    // candidate panel, an NSPanel, which does not require a Dock icon or
    // menu bar -- see Info.plist's LSUIElement key, set to true for exactly
    // this reason).
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: run() blocks until the application is asked to terminate;
    // standard AppKit main-loop entry, safe to call once from the main
    // thread after sharedApplication() above.
    unsafe { app.run() };
}

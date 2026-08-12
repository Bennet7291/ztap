pub mod candidate_window;
pub mod input_method;

use objc2::rc::Retained;
use objc2::{MainThreadMarker};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSBundle, NSString};
use objc2_input_method_kit::IMKServer;

pub fn run() {
    let mtm = MainThreadMarker::new().expect("run() must be called from the process's initial thread");

    let bundle = NSBundle::mainBundle();
    let key = NSString::from_str("InputMethodConnectionName");
    let connection_name: Option<Retained<NSString>> = bundle
        .objectForInfoDictionaryKey(&key)
        .and_then(|obj| obj.downcast::<NSString>().ok());
    let bundle_identifier = bundle.bundleIdentifier();

    let _server: Retained<IMKServer> = unsafe {
        let alloc = IMKServer::alloc();
        objc2::msg_send![
            alloc,
            initWithName: connection_name.as_deref(),
            bundleIdentifier: bundle_identifier.as_deref(),
        ]
    };

    let app = NSApplication::sharedApplication(mtm);
    app.run();
}

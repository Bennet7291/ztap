//! Windows platform entry point -- TSF IME DLL.
//!
//! This crate is compiled as a `cdylib` and loaded by the Windows text stack.
//!
//! # WARNING: UNTESTED -- see tsf.rs's module doc comment
//!
//! Written without a Windows toolchain available; never compiled. The
//! DllGetClassObject / class factory pattern below follows the shape used
//! by other windows-rs-based COM servers (see e.g.
//! https://github.com/microsoft/windows-rs/issues/1819 for a worked
//! example this was modeled on), but has not been verified against an
//! actual `cargo build --target x86_64-pc-windows-msvc`. The registration
//! flow (RegisterServer / RegisterProfiles / RegisterCategories) follows
//! Microsoft's documented three-step TSF registration process
//! (https://learn.microsoft.com/windows/win32/tsf/text-service-registration)
//! but every registry path and flag value should be re-checked against a
//! working reference implementation (e.g. the Windows-classic-samples TSF
//! text service sample) before shipping.

pub mod candidate_window;
pub mod tsf;

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::{implement, IUnknown, Interface, Result, GUID, HRESULT};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, E_NOINTERFACE, E_POINTER, HINSTANCE, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

use tsf::{ZtapTextService, CLSID_ZTAP_TEXT_SERVICE, GUID_ZTAP_PROFILE, LANGID_ZH_CN};

/// Process-wide count of live COM object instances handed out by this DLL.
/// `DllCanUnloadNow` consults this so Windows never unloads the DLL while
/// TSF (or anything else) still holds a reference into it -- unloading
/// out from under a live COM object is a guaranteed crash.
static OBJECT_COUNT: AtomicU32 = AtomicU32::new(0);

/// Guard incrementing/decrementing `OBJECT_COUNT` for the lifetime of one
/// COM object. Every `IClassFactory::CreateInstance` call below wraps its
/// returned object in one of these via `ObjectCountGuard::attach`.
struct ObjectCountGuard;
impl ObjectCountGuard {
    fn attach() {
        OBJECT_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    fn detach() {
        OBJECT_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// DLL entry point. Windows calls this on process attach/detach and
/// thread attach/detach for every process that loads this DLL.
///
/// Ztap does no per-process global initialization here deliberately --
/// see `tsf::ZtapTextService::new`'s doc comment for why real setup
/// (dictionary load, learning store) happens in `Activate` instead, not
/// here. `DllMain` running arbitrary non-trivial code (loading files,
/// taking locks) is a well-known source of deadlocks under the loader
/// lock; keeping this a no-op sidesteps that whole class of bug.
#[no_mangle]
#[allow(non_snake_case, clippy::missing_safety_doc)]
extern "system" fn DllMain(_hinstance: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // Intentionally empty -- see doc comment above.
    }
    1 // TRUE
}

/// The class factory for `ZtapTextService`.
///
/// TSF (via `CoCreateInstance(CLSID_ZTAP_TEXT_SERVICE, ...)`) asks
/// `DllGetClassObject` for an `IClassFactory`, then calls
/// `IClassFactory::CreateInstance` on it to actually construct a
/// `ZtapTextService`. Kept as a separate zero-sized type (rather than
/// having `DllGetClassObject` construct a `ZtapTextService` directly)
/// because that's the shape `IClassFactory` requires -- COM always
/// separates "give me something that can make instances" from "make one
/// now".
#[implement(IClassFactory)]
struct ZtapClassFactory;

impl IClassFactory_Impl for ZtapClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: windows::core::Ref<'_, IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> Result<()> {
        // SAFETY: `ppv` is a valid out-pointer per COM's CreateInstance
        // contract; TSF (our only caller) never passes null here, but the
        // null check below is cheap insurance against a misbehaving caller
        // rather than relying purely on that assumption.
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };

        // Ztap's text service does not support COM aggregation (outer !=
        // None), matching the vast majority of TSF text service samples --
        // aggregation exists for COM composition scenarios Ztap has no use
        // for as a leaf IME implementation.
        if outer.is_some() {
            return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into());
        }

        let service = ZtapTextService::new();
        let unknown: IUnknown = service.into();

        // SAFETY: `riid` is a valid in-pointer per CreateInstance's
        // contract (TSF always supplies a real IID here).
        let riid = unsafe { *riid };
        // SAFETY: QueryInterface's documented contract: on success it
        // writes an owned, AddRef'd pointer into *ppv.
        let hr = unsafe { Interface::query(&unknown, &riid, ppv) };
        if hr.is_ok() {
            ObjectCountGuard::attach();
        }
        HRESULT(hr.0).ok()
    }

    fn LockServer(&self, flock: windows::Win32::Foundation::BOOL) -> Result<()> {
        // A minimal but correct LockServer: bump/drop the same object
        // count DllCanUnloadNow checks, so `LockServer(TRUE)` really does
        // keep the DLL alive the way callers expect.
        if flock.as_bool() {
            ObjectCountGuard::attach();
        } else {
            ObjectCountGuard::detach();
        }
        Ok(())
    }
}

/// Exported so `regsvr32`/the OS loader can locate a class factory for a
/// requested CLSID. This is the one export every in-proc COM server must
/// provide; see `DllRegisterServer`/`DllUnregisterServer`/`DllCanUnloadNow`
/// below for the other three.
///
/// # Safety
///
/// Called directly by the Windows COM runtime with raw pointers per the
/// standard `DllGetClassObject` ABI contract (`rclsid`/`riid` valid
/// in-pointers, `ppv` a valid out-pointer). Safe Rust code cannot express
/// this function signature; callers (the OS) are trusted to uphold the
/// documented contract, matching how every COM DLL export in the Windows
/// ecosystem is written.
#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if ppv.is_null() {
        return E_POINTER;
    }
    *ppv = std::ptr::null_mut();

    let rclsid = *rclsid;
    let riid = *riid;

    if rclsid != CLSID_ZTAP_TEXT_SERVICE {
        return CLASS_E_CLASSNOTAVAILABLE;
    }

    let factory: IClassFactory = ZtapClassFactory.into();
    let hr = Interface::query(&factory, &riid, ppv);
    if hr.is_ok() {
        HRESULT(0)
    } else {
        E_NOINTERFACE
    }
}

/// Tells the COM runtime whether it's safe to unload this DLL from the
/// process. Returns `S_OK` only when `OBJECT_COUNT` is zero; `S_FALSE`
/// otherwise. Getting this wrong in the "always say yes" direction is a
/// classic use-after-free (the OS unloads the DLL's code while a live
/// `ZtapTextService` still exists); getting it wrong in the "always say
/// no" direction just leaks the DLL in memory, which is unfortunate but
/// not a safety bug -- hence `OBJECT_COUNT` errs toward correctness over
/// eagerness to unload.
///
/// # Safety
/// Same ABI-boundary rationale as `DllGetClassObject` above.
#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECT_COUNT.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

/// Write the standard in-proc COM server registry entries
/// (`HKCR\CLSID\{...}\InprocServer32`) for `CLSID_ZTAP_TEXT_SERVICE`,
/// pointing at this DLL's own path.
fn register_com_server() -> Result<()> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CLASSES_ROOT, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let dll_path = current_dll_path()?;
    let clsid_str = guid_to_registry_string(&CLSID_ZTAP_TEXT_SERVICE);
    let key_path = format!("CLSID\\{clsid_str}\\InprocServer32");

    // SAFETY: all string arguments below are valid, NUL-terminated UTF-16
    // (via HSTRING/w!); RegCreateKeyExW/RegSetValueExW/RegCloseKey's other
    // preconditions (valid HKEY constants, matching close of every opened
    // key) are met by construction in this function.
    unsafe {
        let mut hkey = HKEY::default();
        RegCreateKeyExW(
            HKEY_CLASSES_ROOT,
            &windows::core::HSTRING::from(key_path.as_str()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()?;

        let dll_path_wide = windows::core::HSTRING::from(dll_path.as_str());
        let bytes = std::slice::from_raw_parts(
            dll_path_wide.as_ptr() as *const u8,
            (dll_path_wide.len() + 1) * 2, // include the NUL terminator
        );
        RegSetValueExW(hkey, None, None, REG_SZ, Some(bytes)).ok()?;

        // ThreadingModel=Apartment: TSF text services run single-threaded
        // apartment, matching ZtapTextService's documented non-Send/Sync
        // design (see tsf.rs's module doc comment).
        let apartment = windows::core::HSTRING::from("Apartment");
        let apartment_bytes = std::slice::from_raw_parts(
            apartment.as_ptr() as *const u8,
            (apartment.len() + 1) * 2,
        );
        RegSetValueExW(
            hkey,
            &windows::core::HSTRING::from("ThreadingModel"),
            None,
            REG_SZ,
            Some(apartment_bytes),
        )
        .ok()?;

        let _ = RegCloseKey(hkey);
    }

    Ok(())
}

fn unregister_com_server() -> Result<()> {
    use windows::Win32::System::Registry::{RegDeleteTreeW, HKEY_CLASSES_ROOT};
    let clsid_str = guid_to_registry_string(&CLSID_ZTAP_TEXT_SERVICE);
    let key_path = format!("CLSID\\{clsid_str}");
    // SAFETY: HSTRING is valid NUL-terminated UTF-16; RegDeleteTreeW
    // tolerates the key not existing (returns an ignorable error code
    // rather than crashing), which is the expected case on a
    // never-before-registered machine during an unregister-before-register
    // cleanup pass (see DllRegisterServer below).
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CLASSES_ROOT, &windows::core::HSTRING::from(key_path.as_str()));
    }
    Ok(())
}

/// Register the language profile via `ITfInputProcessorProfileMgr`, the
/// modern (Vista+) single-call registration API, per
/// https://learn.microsoft.com/windows/win32/tsf/text-service-registration.
fn register_profile() -> Result<()> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::TextServices::{
        ITfInputProcessorProfileMgr, CLSID_TF_InputProcessorProfiles,
    };

    // SAFETY: CoCreateInstance's standard contract; CLSID_TF_InputProcessorProfiles
    // is a well-known system CLSID always present on TSF-capable Windows.
    let profile_mgr: ITfInputProcessorProfileMgr = unsafe {
        CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
    };

    let icon_path = current_dll_path()?;
    let description = "Ztap Pinyin";

    // SAFETY: all pointer/slice arguments are valid for the duration of
    // this call (HSTRINGs and local variables live through the `unsafe`
    // block); RegisterProfile's other parameters are plain integers/flags
    // per its documented signature.
    unsafe {
        profile_mgr.RegisterProfile(
            &CLSID_ZTAP_TEXT_SERVICE,
            LANGID_ZH_CN,
            &GUID_ZTAP_PROFILE,
            &windows::core::HSTRING::from(description).as_wide(),
            &windows::core::HSTRING::from(icon_path.as_str()).as_wide(),
            0, // icon index
            None,
            0,
            false.into(), // not a keyboard-layout substitute
            0,
        )?;
    }

    Ok(())
}

fn unregister_profile() -> Result<()> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::TextServices::{
        ITfInputProcessorProfileMgr, CLSID_TF_InputProcessorProfiles,
    };

    let profile_mgr: ITfInputProcessorProfileMgr = unsafe {
        CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
    };
    unsafe {
        let _ = profile_mgr.UnregisterProfile(&CLSID_ZTAP_TEXT_SERVICE, LANGID_ZH_CN, &GUID_ZTAP_PROFILE, 0);
    }
    Ok(())
}

/// Register the TSF categories Ztap belongs to
/// (`GUID_TFCAT_CATEGORY_OF_TIP` + `GUID_TFCAT_TIP_KEYBOARD`) via
/// `ITfCategoryMgr`, so TSF and the language bar correctly classify Ztap
/// as a keyboard-input text service (as opposed to e.g. speech or
/// handwriting).
fn register_categories() -> Result<()> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::TextServices::{
        ITfCategoryMgr, CLSID_TF_CategoryMgr, GUID_TFCAT_CATEGORY_OF_TIP, GUID_TFCAT_TIP_KEYBOARD,
    };

    let category_mgr: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        category_mgr.RegisterCategory(
            &CLSID_ZTAP_TEXT_SERVICE,
            &GUID_TFCAT_CATEGORY_OF_TIP,
            &CLSID_ZTAP_TEXT_SERVICE,
        )?;
        category_mgr.RegisterCategory(
            &CLSID_ZTAP_TEXT_SERVICE,
            &GUID_TFCAT_TIP_KEYBOARD,
            &CLSID_ZTAP_TEXT_SERVICE,
        )?;
    }
    Ok(())
}

fn unregister_categories() -> Result<()> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::TextServices::{
        ITfCategoryMgr, CLSID_TF_CategoryMgr, GUID_TFCAT_CATEGORY_OF_TIP, GUID_TFCAT_TIP_KEYBOARD,
    };
    let category_mgr: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        let _ = category_mgr.UnregisterCategory(
            &CLSID_ZTAP_TEXT_SERVICE,
            &GUID_TFCAT_CATEGORY_OF_TIP,
            &CLSID_ZTAP_TEXT_SERVICE,
        );
        let _ = category_mgr.UnregisterCategory(
            &CLSID_ZTAP_TEXT_SERVICE,
            &GUID_TFCAT_TIP_KEYBOARD,
            &CLSID_ZTAP_TEXT_SERVICE,
        );
    }
    Ok(())
}

/// Full registration: COM server entries, TSF language profile, TSF
/// categories, in that order (matching the Microsoft sample's documented
/// `RegisterServer() || RegisterProfiles() || RegisterCategories()`
/// sequence). On any failure, unregisters everything that *did* succeed
/// rather than leaving a half-registered text service behind — a partial
/// registration (e.g. COM entries present but no language profile) is
/// worse than no registration, since it can make TSF believe a text
/// service exists that doesn't actually function.
///
/// # Safety
/// Standard COM DLL export ABI boundary; see `DllGetClassObject`'s doc comment.
#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    let result = (|| -> Result<()> {
        register_com_server()?;
        register_profile()?;
        register_categories()?;
        Ok(())
    })();

    match result {
        Ok(()) => S_OK,
        Err(e) => {
            let _ = unregister_categories();
            let _ = unregister_profile();
            let _ = unregister_com_server();
            HRESULT(e.code().0)
        }
    }
}

/// # Safety
/// Standard COM DLL export ABI boundary; see `DllGetClassObject`'s doc comment.
#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    let _ = unregister_categories();
    let _ = unregister_profile();
    let _ = unregister_com_server();
    S_OK
}

/// Full path to this DLL on disk, used both for the `InprocServer32`
/// registry value and as the icon path passed to `RegisterProfile`.
fn current_dll_path() -> Result<String> {
    use windows::Win32::Foundation::MAX_PATH;
    use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW,
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS};

    // SAFETY: passing the address of this very function as the lookup key
    // is the standard "find my own module handle from inside myself"
    // pattern; GetModuleHandleExW's documented behavior for
    // GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS is exactly this.
    let mut hmodule = Default::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            windows::core::PCWSTR(current_dll_path as *const _),
            &mut hmodule,
        )?;
    }

    let mut buf = [0u16; MAX_PATH as usize];
    // SAFETY: buf is sized to MAX_PATH; hmodule was just validated above.
    let len = unsafe { GetModuleFileNameW(Some(hmodule), &mut buf) };
    if len == 0 {
        return Err(windows::core::Error::from_win32());
    }
    Ok(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Format a `GUID` as the `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` string
/// the registry expects for CLSID key names.
fn guid_to_registry_string(guid: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7],
    )
}

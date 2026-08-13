use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::core::{implement, Error, IUnknown, Result, GUID, PCWSTR};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOINTERFACE, HMODULE, MAX_PATH,
};
use windows::Win32::System::Com::{
    CoCreateInstance, IClassFactory, IClassFactory_Impl, CLSCTX_INPROC_SERVER,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CLASSES_ROOT,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::TextServices::{
    ITfCategoryMgr, ITfInputProcessorProfiles, ITfTextInputProcessor, CLSID_TF_CategoryMgr,
    CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD,
};
use windows_core::{Interface, Ref, AsImpl, BOOL};

use crate::{ZtapTextService, CLSID_ZTAP_TEXT_SERVICE, GUID_ZTAP_PROFILE, LANGID_ZH_CN};

static DLL_MODULE: AtomicUsize = AtomicUsize::new(0);

const DLL_PROCESS_ATTACH: u32 = 1;

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    hinstance: HMODULE,
    fdw_reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if fdw_reason == DLL_PROCESS_ATTACH {
        DLL_MODULE.store(hinstance.0 as usize, Ordering::SeqCst);
    }
    1
}

fn dll_module_handle() -> HMODULE {
    HMODULE(DLL_MODULE.load(Ordering::SeqCst) as *mut c_void)
}

fn dll_path() -> Result<Vec<u16>> {
    let handle = dll_module_handle();
    let mut buf = vec![0u16; MAX_PATH as usize];
    loop {
        let len = unsafe { GetModuleFileNameW(Some(handle), &mut buf) };
        if len == 0 {
            return Err(Error::new(E_FAIL, "GetModuleFileNameW failed"));
        }
        if (len as usize) < buf.len() {
            buf.truncate(len as usize);
            return Ok(buf);
        }
        buf.resize(buf.len() * 2, 0);
    }
}

fn guid_to_reg_string(guid: &GUID) -> String {
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn write_reg_sz(root: HKEY, subkey: &str, value_name: Option<&str>, data: &str) -> Result<()> {
    let subkey_wide = wide(subkey);
    let mut hkey = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(subkey_wide.as_ptr()),
            Some(0),
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()?;
    }

    let value_wide = value_name.map(wide);
    let data_wide = wide(data);
    let data_bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(data_wide.as_ptr() as *const u8, data_wide.len() * 2) };

    let value_ptr = match &value_wide {
        Some(w) => PCWSTR(w.as_ptr()),
        None => PCWSTR::null(),
    };

    let result = unsafe { RegSetValueExW(hkey, value_ptr, Some(0), REG_SZ, Some(data_bytes)) };

    unsafe {
        let _ = RegCloseKey(hkey);
    }

    result.ok()
}

fn delete_reg_tree(root: HKEY, subkey: &str) {
    let subkey_wide = wide(subkey);
    unsafe {
        let _ = RegDeleteTreeW(root, PCWSTR(subkey_wide.as_ptr()));
    }
}

fn register_com_server() -> Result<()> {
    let clsid_str = guid_to_reg_string(&CLSID_ZTAP_TEXT_SERVICE);
    let path_wide = dll_path()?;
    let path_str = String::from_utf16_lossy(&path_wide[..path_wide.len().saturating_sub(1)]);

    let clsid_key = format!("CLSID\\{}", clsid_str);
    write_reg_sz(HKEY_CLASSES_ROOT, &clsid_key, None, "Ztap Text Service")?;

    let inproc_key = format!("CLSID\\{}\\InprocServer32", clsid_str);
    write_reg_sz(HKEY_CLASSES_ROOT, &inproc_key, None, &path_str)?;
    write_reg_sz(HKEY_CLASSES_ROOT, &inproc_key, Some("ThreadingModel"), "Apartment")?;

    Ok(())
}

fn unregister_com_server() {
    let clsid_str = guid_to_reg_string(&CLSID_ZTAP_TEXT_SERVICE);
    let clsid_key = format!("CLSID\\{}", clsid_str);
    delete_reg_tree(HKEY_CLASSES_ROOT, &clsid_key);
}

fn register_tsf_profile() -> Result<()> {
    unsafe {
        let category_mgr: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        category_mgr.RegisterCategory(
            &CLSID_ZTAP_TEXT_SERVICE,
            &GUID_TFCAT_TIP_KEYBOARD,
            &CLSID_ZTAP_TEXT_SERVICE,
        )?;

        let profiles: ITfInputProcessorProfiles =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        profiles.Register(&CLSID_ZTAP_TEXT_SERVICE)?;

        let description: Vec<u16> = "Ztap 拼音输入法".encode_utf16().collect();
        let icon_path_nul = dll_path()?;
        let icon_path: Vec<u16> = icon_path_nul[..icon_path_nul.len().saturating_sub(1)].to_vec();

        profiles.AddLanguageProfile(
            &CLSID_ZTAP_TEXT_SERVICE,
            LANGID_ZH_CN,
            &GUID_ZTAP_PROFILE,
            &description,
            &icon_path,
            0,
        )?;
    }
    Ok(())
}

fn unregister_tsf_profile() {
    unsafe {
        if let Ok(profiles) = CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            let _ = profiles.RemoveLanguageProfile(
                &CLSID_ZTAP_TEXT_SERVICE,
                LANGID_ZH_CN,
                &GUID_ZTAP_PROFILE,
            );
            let _ = profiles.Unregister(&CLSID_ZTAP_TEXT_SERVICE);
        }

        if let Ok(category_mgr) =
            CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
        {
            let _ = category_mgr.UnregisterCategory(
                &CLSID_ZTAP_TEXT_SERVICE,
                &GUID_TFCAT_TIP_KEYBOARD,
                &CLSID_ZTAP_TEXT_SERVICE,
            );
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> windows_core::HRESULT {
    match register_com_server().and_then(|_| register_tsf_profile()) {
        Ok(()) => windows_core::HRESULT(0),
        Err(e) => e.code(),
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> windows_core::HRESULT {
    unregister_tsf_profile();
    unregister_com_server();
    windows_core::HRESULT(0)
}

#[implement(IClassFactory)]
pub struct ZtapClassFactory;

impl IClassFactory_Impl for ZtapClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }

        let (processor, sink) = ZtapTextService::create();

        unsafe {
            let impl_ref = processor.as_impl();
            impl_ref.state.borrow_mut().self_as_sink = Some(sink);
        }

        let riid_ref = unsafe { &*riid };
        if riid_ref == &ITfTextInputProcessor::IID {
            unsafe {
                *ppvobject = std::mem::transmute(processor);
            }
            Ok(())
        } else {
            Err(Error::from(E_NOINTERFACE))
        }
    }

    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> windows_core::HRESULT {
    let clsid = unsafe { &*rclsid };
    if clsid != &CLSID_ZTAP_TEXT_SERVICE {
        return CLASS_E_CLASSNOTAVAILABLE;
    }

    let factory: IClassFactory = ZtapClassFactory.into();
    factory.query(unsafe { &*riid }, ppv)
}

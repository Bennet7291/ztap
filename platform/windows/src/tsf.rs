use windows::core::{implement, IUnknown, Result, GUID};
use windows::Win32::Foundation::CLASS_E_NOAGGREGATION;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::TextServices::ITfTextInputProcessor;
use windows_core::{Interface, Ref};

use crate::{ZtapTextService, CLSID_ZTAP_TEXT_SERVICE};

#[implement(IClassFactory)]
pub struct ZtapClassFactory;

impl IClassFactory_Impl for ZtapClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut std::ffi::c_void,
    ) -> Result<()> {
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }

        let (processor, sink) = ZtapTextService::create();

        {
            use windows_core::AsImpl;
            let impl_ref = unsafe { processor.as_impl() };
            impl_ref.state.borrow_mut().self_as_sink = Some(sink);
        }

        let riid = unsafe { &*riid };
        if riid == &ITfTextInputProcessor::IID {
            unsafe {
                *ppvobject = std::mem::transmute(processor);
            }
            Ok(())
        } else {
            Err(windows::core::Error::from(windows::Win32::Foundation::E_NOINTERFACE))
        }
    }

    fn LockServer(&self, _flock: windows_core::BOOL) -> Result<()> {
        Ok(())
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut std::ffi::c_void,
) -> windows_core::HRESULT {
    let clsid = unsafe { &*rclsid };
    if clsid != &CLSID_ZTAP_TEXT_SERVICE {
        return windows::Win32::Foundation::CLASS_E_CLASSNOTAVAILABLE;
    }

    let factory: IClassFactory = ZtapClassFactory.into();
    match factory.query(unsafe { &*riid }, ppv) {
        Ok(()) => windows_core::HRESULT(0),
        Err(e) => e.code(),
    }
}

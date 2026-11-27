//! Layouts from tv-native-apis (see LICENSE.tv-native-apis).
use std::ffi::{c_char, c_int, c_void};

#[allow(non_camel_case_types, clippy::undocumented_unsafe_blocks)]
mod native {
    include!("generated/vtcapture.rs");
}

#[cfg(test)]
pub(super) use native::VT_REGION as Region;
pub(super) use native::{
    _LibVtCaptureBufferInfo as BufferInfo, _LibVtCapturePlaneInfo as PlaneInfo,
    _LibVtCaptureProperties as Properties, VT_RESOLUTION as Resolution, prysm_vt_buffer,
    prysm_vt_client, prysm_vt_create, prysm_vt_init, prysm_vt_plane, prysm_vt_preprocess,
    prysm_vt_release,
};

pub type VtCreate = unsafe extern "C" fn() -> *mut c_void;
pub type VtInit = unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_char) -> c_int;
pub type VtPreprocess = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut Properties) -> c_int;
pub type VtPlane = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut PlaneInfo) -> c_int;
pub type VtBuffer = unsafe extern "C" fn(*mut c_void, *mut BufferInfo) -> c_int;
pub type VtClient = unsafe extern "C" fn(*mut c_void, *mut c_char) -> c_int;
pub type VtRelease = unsafe extern "C" fn(*mut c_void);

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        mem::{offset_of, size_of},
        ptr,
    };
    unsafe extern "C" {
        fn prysm_abi_layout(output: *mut usize);
        fn prysm_test_throw_create() -> *mut c_void;
        fn prysm_test_throw_call(driver: *mut c_void, client: *mut c_char) -> c_int;
        fn prysm_test_throw_release(driver: *mut c_void);
    }

    #[test]
    fn layouts_match_the_native_headers() {
        let expected = [
            size_of::<Properties>(),
            offset_of!(Properties, frm),
            size_of::<PlaneInfo>(),
            offset_of!(PlaneInfo, activeregion),
            size_of::<BufferInfo>(),
            offset_of!(BufferInfo, size0),
        ];
        let mut actual = [0; 6];
        // SAFETY: C++ writes exactly 6 size_t entries into this array.
        unsafe {
            prysm_abi_layout(actual.as_mut_ptr());
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn native_exceptions_never_cross_into_rust() {
        let mut error = [0; 512];
        let mut driver = ptr::null_mut();
        // SAFETY: the test functions throw before accessing arguments; their
        // actual signatures match the operation selected in the C++ bridge;
        // each message fits the buffer's 512 bytes.
        unsafe {
            assert_eq!(
                prysm_vt_create(
                    Some(prysm_test_throw_create),
                    &raw mut driver,
                    error.as_mut_ptr(),
                    error.len()
                ),
                -1000
            );
            assert!(
                CStr::from_ptr(error.as_ptr())
                    .to_str()
                    .unwrap()
                    .contains("LSRegisterPubPriv")
            );
            assert_eq!(
                prysm_vt_client(
                    Some(prysm_test_throw_call),
                    driver,
                    ptr::null_mut(),
                    error.as_mut_ptr(),
                    error.len()
                ),
                -1000
            );
            assert_eq!(
                CStr::from_ptr(error.as_ptr()).to_str().unwrap(),
                "native call failed"
            );
            let mut short = [1; 7];
            assert_eq!(
                prysm_vt_release(
                    Some(prysm_test_throw_release),
                    driver,
                    short.as_mut_ptr(),
                    short.len()
                ),
                -1000
            );
            assert_eq!(CStr::from_ptr(short.as_ptr()).to_str().unwrap(), "native");
        }
    }
    use std::ffi::CStr;
}

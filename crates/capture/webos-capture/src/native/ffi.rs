//! Layouts from tv-native-apis (see LICENSE.tv-native-apis).
use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
#[derive(Default)]
pub struct Properties {
    pub dump: i32,
    pub x: i16,
    pub y: i16,
    pub width: i16,
    pub height: i16,
    pub buffers: i32,
    pub fps: i32,
}

#[repr(C)]
#[derive(Default)]
pub struct PlaneInfo {
    pub stride: i32,
    pub region: [i16; 4],
    pub active: [i16; 4],
}

#[repr(C)]
#[derive(Default)]
pub struct BufferInfo {
    pub y: *mut u8,
    pub uv: *mut u8,
    pub y_len: i32,
    pub uv_len: i32,
}

#[repr(C)]
pub struct GmSurface {
    pub dummy: u32,
    pub framebuffer: *mut u8,
    pub reserved: [u32; 261],
    pub id: u32,
    pub tail: [u32; 2],
}

#[repr(C)]
pub struct HalSurface {
    pub offset: u32,
    pub physical_address: *mut u32,
    pub pitch: u16,
    pub bpp: u16,
    pub width: u16,
    pub height: u16,
    pub pixel_format: u16,
    pub palette: [u32; 257],
    pub property: u32,
    pub vendor_data: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct OutputState {
    pub enabled: u8,
    pub frozen: u8,
    pub pq: u8,
    pub unknown: u8,
    pub framerate: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct Framebuffer {
    pub format: u32,
    pub stride: u32,
    pub width: u32,
    pub height: u32,
    pub offsets: *mut *mut u32,
}

#[repr(C)]
#[derive(Default)]
pub struct Capability {
    pub buffers: u32,
    pub planes: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[repr(C)]
#[derive(Default)]
pub struct Limitation {
    pub max_resolution: Rect,
    pub flags: [u8; 4],
    pub scale_up_width: u32,
    pub scale_up_height: u32,
    pub scale_down_flags: [u8; 4],
    pub scale_down_width: u32,
    pub scale_down_height: u32,
}

pub type VtCreate = unsafe extern "C" fn() -> *mut c_void;
pub type VtInit = unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_char) -> c_int;
pub type VtPreprocess = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut Properties) -> c_int;
pub type VtPlane = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut PlaneInfo) -> c_int;
pub type VtBuffer = unsafe extern "C" fn(*mut c_void, *mut BufferInfo) -> c_int;
pub type VtClient = unsafe extern "C" fn(*mut c_void, *mut c_char) -> c_int;
pub type VtRelease = unsafe extern "C" fn(*mut c_void);

unsafe extern "C" {
    pub fn prysm_vt_create(
        function: VtCreate,
        driver: *mut *mut c_void,
        error: *mut c_char,
    ) -> c_int;
    pub fn prysm_vt_init(
        function: VtInit,
        driver: *mut c_void,
        caller: *const c_char,
        client: *mut c_char,
        error: *mut c_char,
    ) -> c_int;
    pub fn prysm_vt_preprocess(
        function: VtPreprocess,
        driver: *mut c_void,
        client: *mut c_char,
        properties: *mut Properties,
        error: *mut c_char,
    ) -> c_int;
    pub fn prysm_vt_plane(
        function: VtPlane,
        driver: *mut c_void,
        client: *mut c_char,
        plane: *mut PlaneInfo,
        error: *mut c_char,
    ) -> c_int;
    pub fn prysm_vt_buffer(
        function: VtBuffer,
        driver: *mut c_void,
        buffer: *mut BufferInfo,
        error: *mut c_char,
    ) -> c_int;
    pub fn prysm_vt_client(
        function: VtClient,
        driver: *mut c_void,
        client: *mut c_char,
        error: *mut c_char,
    ) -> c_int;
    pub fn prysm_vt_release(function: VtRelease, driver: *mut c_void, error: *mut c_char) -> c_int;
}

// These structs contain only integer fields and nullable raw pointers.
impl Default for GmSurface {
    fn default() -> Self {
        Self {
            dummy: 0,
            framebuffer: std::ptr::null_mut(),
            reserved: [0; 261],
            id: 0,
            tail: [0; 2],
        }
    }
}

impl Default for HalSurface {
    fn default() -> Self {
        Self {
            offset: 0,
            physical_address: std::ptr::null_mut(),
            pitch: 0,
            bpp: 0,
            width: 0,
            height: 0,
            pixel_format: 0,
            palette: [0; 257],
            property: 0,
            vendor_data: 0,
        }
    }
}

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
            offset_of!(Properties, fps),
            size_of::<PlaneInfo>(),
            offset_of!(PlaneInfo, active),
            size_of::<BufferInfo>(),
            offset_of!(BufferInfo, y_len),
            size_of::<GmSurface>(),
            offset_of!(GmSurface, framebuffer),
            offset_of!(GmSurface, id),
            size_of::<HalSurface>(),
            offset_of!(HalSurface, pitch),
            offset_of!(HalSurface, vendor_data),
            size_of::<OutputState>(),
            offset_of!(OutputState, framerate),
            size_of::<Framebuffer>(),
            offset_of!(Framebuffer, offsets),
            size_of::<Capability>(),
            size_of::<Rect>(),
            size_of::<Limitation>(),
            offset_of!(Limitation, scale_down_width),
        ];
        let mut actual = [0; 20];
        // SAFETY: C++ writes exactly 20 size_t entries into this array.
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
        // actual signatures match the operation selected in the C++ bridge.
        unsafe {
            assert_eq!(
                prysm_vt_create(prysm_test_throw_create, &raw mut driver, error.as_mut_ptr()),
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
                    prysm_test_throw_call,
                    driver,
                    ptr::null_mut(),
                    error.as_mut_ptr()
                ),
                -1000
            );
            assert_eq!(
                CStr::from_ptr(error.as_ptr()).to_str().unwrap(),
                "native call failed"
            );
            assert_eq!(
                prysm_vt_release(prysm_test_throw_release, driver, error.as_mut_ptr()),
                -1000
            );
            assert_eq!(
                CStr::from_ptr(error.as_ptr()).to_str().unwrap(),
                "native release failed"
            );
        }
    }
    use std::ffi::CStr;
}

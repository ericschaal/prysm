//! State-owned AVFoundation device connection notifications.

use av_foundation::capture_device::{AVCaptureDevice, AVCaptureDeviceWasConnectedNotification};
use objc2::rc::Retained;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObject, NSString};
use prysm_ports::Hangup;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use crate::{CaptureError, Result};

fn present(uid: &NSString) -> bool {
    AVCaptureDevice::device_with_unique_id(uid)
        .is_some_and(|device| device.is_connected().as_bool())
}

pub(crate) fn check_device(uid: &str) -> Result<()> {
    if !present(&NSString::from_str(uid)) {
        return Err(CaptureError::DeviceNotFound {
            device: Some(uid.to_owned()),
            source: None,
        });
    }
    super::access::check_access()
}

#[derive(Default)]
struct Wake {
    changed: Mutex<bool>,
    ready: Condvar,
}

impl Wake {
    fn signal(&self) {
        *self.changed.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.ready.notify_all();
    }
}

define_class!(
    // SAFETY: NSObject permits subclassing. The ivar and dealloc cannot panic.
    #[unsafe(super(NSObject))]
    #[name = "PrysmCaptureConnectionObserver"]
    #[ivars = Arc<Wake>]
    struct ConnectionObserver;

    impl ConnectionObserver {
        // SAFETY: This selector takes the notification posted by the center.
        #[unsafe(method(cameraConnected:))]
        fn connected(&self, _: &NSNotification) {
            self.ivars().signal();
        }
    }
);

impl Drop for ConnectionObserver {
    fn drop(&mut self) {
        // SAFETY: self is the live observer and can be removed from the center.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self) };
    }
}

pub(crate) fn watch_device(uid: &str, stop: &Hangup) -> bool {
    let wake = Arc::new(Wake::default());
    let cut = wake.clone();
    let Some(_registration) = stop.on_cut(move || cut.signal()) else {
        return false;
    };
    let this = ConnectionObserver::alloc().set_ivars(wake.clone());
    // SAFETY: NSObject initializes the allocated receiver with its ivars set.
    let observer: Retained<ConnectionObserver> = unsafe { msg_send![super(this), init] };
    // SAFETY: cameraConnected: accepts the notification and is removed on drop.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &observer,
            sel!(cameraConnected:),
            Some(AVCaptureDeviceWasConnectedNotification),
            None,
        );
    }
    let uid = NSString::from_str(uid);
    loop {
        if stop.is_cut() {
            return false;
        }
        if present(&uid) {
            return !stop.is_cut();
        }
        let changed = wake.changed.lock().unwrap_or_else(PoisonError::into_inner);
        *wake
            .ready
            .wait_while(changed, |changed| !*changed)
            .unwrap_or_else(PoisonError::into_inner) = false;
    }
}

//! macOS: CoreAudio process taps (macOS 14.2+) fronted by a private aggregate device.
//!
//! ## Exact API calls used (with availability)
//!
//! | Call | Availability | Source |
//! |---|---|---|
//! | `CATapDescription` (`-initStereoGlobalTapButExcludeProcesses:`, `setName:`, `setUUID:`, `setMuteBehavior:`, `setPrivateTap:`) | macOS 14.2+ | `CoreAudio/AudioHardwareTapping.h` (Objective-C class; reached via `objc2` message sends — there is no Rust binding crate for the 14.2 tap API, so `objc2::msg_send!` is used) |
//! | `AudioHardwareCreateProcessTap(CATapDescription*, AudioObjectID*)` | macOS 14.2+ | `CoreAudio/AudioHardwareTapping.h` |
//! | `AudioHardwareDestroyProcessTap(AudioObjectID)` | macOS 14.2+ | `CoreAudio/AudioHardwareTapping.h` |
//! | `AudioHardwareCreateAggregateDevice(CFDictionaryRef, AudioDeviceID*)` | macOS 10.9+ | `CoreAudio/AudioHardware.h` |
//! | `AudioHardwareDestroyAggregateDevice(AudioDeviceID)` | macOS 10.9+ | `CoreAudio/AudioHardware.h` |
//! | `AudioObjectGetPropertyData` | macOS 10.0+ | `CoreAudio/AudioHardware.h` |
//! | `kAudioTapPropertyUID` (`'tuid'`) | macOS 14.2+ | tap object property, `AudioHardwareTapping.h` |
//! | `kAudioDevicePropertyDeviceUID` (`'uid '`) / `kAudioDevicePropertyDeviceNameCFString` (`'lnam'`) | macOS 10.0+ | device properties, `AudioHardware.h` |
//! | `kAudioAggregateDeviceNameKey` / `…UIDKey` / `…IsPrivateKey` / `…TapListKey`, `kAudioSubTapUIDKey` | macOS 10.9+/14.2 | aggregate-device description keys — **string-literal macros** on the macOS 27 SDK (`"uid"` / `"name"` / `"private"` / `"taps"` / `"uid"`); see the note below |
//!
//! The two **14.2+ functions are resolved with `dlsym(RTLD_DEFAULT, …)` at call
//! time**, not linked directly: Rust cannot mark a symbol `weak_import`, and a
//! strong undefined reference would make the whole app fail to *load* on macOS
//! < 14.2. The `CATapDescription` class presence check via
//! `AnyClass::get("CATapDescription")` is the availability gate — on older macOS
//! `start()` returns [`TapError::OsTooOld`] before any symbol is called.
//!
//! The five aggregate-device description keys are **string-literal macros** in
//! the macOS 27 SDK and the macOS 27 runtime exports *no symbols* for them —
//! linking the old `extern` statics against CoreAudio is a hard link error on
//! macOS 27 SDKs (verified on a real SDK: `#define kAudioAggregateDeviceUIDKey "uid"`
//! etc. in `CoreAudio/AudioHardware.h`, and `dlsym` finds nothing in CoreAudio
//! or AudioToolbox). We therefore build the keys as plain CFStrings from those
//! literal values (see `KEY_*` below): the HAL matches aggregate-device
//! dictionary keys by string content, so this is exact on every macOS version.
//!
//! ## Teardown
//!
//! [`TapInner::stop`] destroys the aggregate device **first**, then the tap
//! (the aggregate references the tap), and is idempotent — `SystemTap::stop`
//! and `Drop` can both run in any order.

use std::ffi::c_void;
use std::ptr;

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSArray, NSNumber, NSString, NSUUID};

use crate::{ProcessSelection, TapConfig, TapError};

/// `AudioObjectID` / `AudioDeviceID` (CoreAudio `UInt32`).
type AudioObjectId = u32;
/// `OSStatus`.
type OsStatus = i32;

/// `kAudioObjectSystemObject`.
const AUDIO_OBJECT_SYSTEM_OBJECT: AudioObjectId = 1;
/// `kAudioObjectPropertyScopeGlobal` (`'glob'`).
const SCOPE_GLOBAL: u32 = fourcc(*b"glob");
/// `kAudioObjectPropertyElementMain` (0).
const ELEMENT_MAIN: u32 = 0;
/// `kAudioTapPropertyUID` (`'tuid'`, macOS 14.2+): UID of the tap object.
const K_AUDIO_TAP_PROPERTY_UID: u32 = fourcc(*b"tuid");
/// `kAudioDevicePropertyDeviceUID` (`'uid '`).
const K_AUDIO_DEVICE_PROPERTY_DEVICE_UID: u32 = fourcc(*b"uid ");
/// `kAudioHardwarePropertyTranslateUIDToDevice` (`'uidd'`, macOS 10.5+): UID → device id.
const K_AUDIO_HARDWARE_PROPERTY_TRANSLATE_UID_TO_DEVICE: u32 = fourcc(*b"uidd");

/// Aggregate-device description keys — string-literal macros in the macOS 27 SDK
/// (`CoreAudio/AudioHardware.h`): `#define kAudioAggregateDeviceUIDKey "uid"`,
/// `…NameKey "name"`, `…IsPrivateKey "private"`, `…TapListKey "taps"`.
/// Values taken verbatim from Apple's macOS 27 SDK header.
const KEY_UID: &str = "uid";
const KEY_NAME: &str = "name";
const KEY_PRIVATE: &str = "private";
const KEY_TAPS: &str = "taps";
/// `kAudioSubTapUIDKey` — same literal as `kAudioAggregateDeviceUIDKey`; the
/// sub-tap entry is its own dictionary level, so the key repeats there.
const KEY_SUBTAP_UID: &str = "uid";

/// Four-Charpack → `AudioObjectPropertySelector` (C multi-char constants are
/// big-endian packed; `'tuid'` → `0x74756964`).
const fn fourcc(bytes: [u8; 4]) -> u32 {
    u32::from_be_bytes(bytes)
}

/// `AudioObjectPropertyAddress` (`CoreAudio/AudioHardware.h`, macOS 10.0+).
#[repr(C)]
struct AudioObjectPropertyAddress {
    m_selector: u32,
    m_scope: u32,
    m_element: u32,
}

/// `unsafe extern "C" fn(CATapDescription *, AudioObjectID *) -> OSStatus` (macOS 14.2+).
type CreateProcessTapFn = unsafe extern "C" fn(*const AnyObject, *mut AudioObjectId) -> OsStatus;
/// `unsafe extern "C" fn(AudioObjectID) -> OSStatus` (macOS 14.2+).
type DestroyProcessTapFn = unsafe extern "C" fn(AudioObjectId) -> OsStatus;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    /// CoreAudio/AudioHardware.h, macOS 10.9+.
    fn AudioHardwareCreateAggregateDevice(
        description: CFDictionaryRef,
        out_device: *mut AudioObjectId,
    ) -> OsStatus;
    /// CoreAudio/AudioHardware.h, macOS 10.9+.
    fn AudioHardwareDestroyAggregateDevice(device: AudioObjectId) -> OsStatus;
    /// CoreAudio/AudioHardware.h, macOS 10.0+.
    fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        io_size: *mut u32,
        out: *mut c_void,
    ) -> OsStatus;
}

/// Resolves the macOS 14.2+ tap entry points at runtime (see module docs).
fn tap_entry_points() -> Result<(CreateProcessTapFn, DestroyProcessTapFn), TapError> {
    // SAFETY: `dlsym` returns either NULL or a pointer to the named symbol; the
    // names are exact C symbols of CoreAudio.framework.
    unsafe {
        let create = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"AudioHardwareCreateProcessTap".as_ptr(),
        );
        let destroy = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"AudioHardwareDestroyProcessTap".as_ptr(),
        );
        if create.is_null() || destroy.is_null() {
            return Err(TapError::OsTooOld);
        }
        // SAFETY: non-null and matching the documented C signatures above.
        Ok((
            std::mem::transmute::<*mut c_void, CreateProcessTapFn>(create),
            std::mem::transmute::<*mut c_void, DestroyProcessTapFn>(destroy),
        ))
    }
}

/// Builds the `CATapDescription` for `config` (macOS 14.2+).
///
/// `initStereoGlobalTapButExcludeProcesses:` creates a tap over the *entire*
/// system minus the listed pids; mute behavior `0` (`CATapMuteBehaviorUnmuted`)
/// keeps the tapped audio audible to the user, `privateTap = YES` keeps the
/// aggregate device out of other apps' device lists.
fn tap_description(config: &TapConfig) -> Result<Retained<AnyObject>, TapError> {
    let Some(cls) = AnyClass::get(c"CATapDescription") else {
        return Err(TapError::OsTooOld);
    };
    let ProcessSelection::AllExcept(pids) = &config.processes;
    let numbers: Vec<Retained<NSNumber>> = pids.iter().map(|pid| NSNumber::new_u32(*pid)).collect();
    let excluded = NSArray::from_retained_slice(&numbers);
    let name = NSString::from_str(&config.name);
    let uuid = NSUUID::new();

    // SAFETY: `alloc` + an `init…` initializer follow the +1 allocation contract;
    // property setters match the documented `CATapDescription` properties.
    unsafe {
        let allocated: Allocated<AnyObject> = msg_send![cls, alloc];
        let description: Retained<AnyObject> =
            msg_send![allocated, initStereoGlobalTapButExcludeProcesses: &*excluded];
        let _: () = msg_send![&*description, setName: &*name];
        let _: () = msg_send![&*description, setUUID: &*uuid];
        let _: () = msg_send![&*description, setMuteBehavior: 0isize];
        let _: () = msg_send![&*description, setPrivateTap: true];
        Ok(description)
    }
}

/// Reads a CFString-valued property of a CoreAudio object.
fn string_property(object: AudioObjectId, selector: u32) -> Result<String, TapError> {
    let address = AudioObjectPropertyAddress {
        m_selector: selector,
        m_scope: SCOPE_GLOBAL,
        m_element: ELEMENT_MAIN,
    };
    let mut value: CFStringRef = ptr::null();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;
    // SAFETY: `value` is a CFStringRef slot with the matching size; the address is
    // a live stack value for the duration of the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            &mut size,
            &mut value as *mut CFStringRef as *mut c_void,
        )
    };
    if status != 0 {
        return Err(TapError::CoreAudio {
            call: "AudioObjectGetPropertyData",
            status,
        });
    }
    if value.is_null() {
        return Err(TapError::Setup(
            "CoreAudio returned a null string property".to_string(),
        ));
    }
    // SAFETY: non-null CFStringRef owned by CoreAudio; `wrap_under_get_rule` retains.
    Ok(unsafe { CFString::wrap_under_get_rule(value) }.to_string())
}

/// Creates the private aggregate device that exposes `tap_uid` as an input device.
/// Returns its `AudioDeviceID`.
fn create_aggregate_device(name: &str, tap_uid: &str) -> Result<AudioObjectId, TapError> {
    // One sub-tap entry: the tap's UID.
    let sub_tap = {
        let key = CFString::new(KEY_SUBTAP_UID);
        let value = CFString::new(tap_uid);
        CFDictionary::from_CFType_pairs(&[(key, value)]).to_untyped()
    };
    let tap_list = CFArray::from_CFTypes(&[sub_tap.as_CFType()]);
    let device_uid = NSUUID::new().UUIDString().to_string();

    let dictionary = {
        let name_key = CFString::new(KEY_NAME);
        let uid_key = CFString::new(KEY_UID);
        let private_key = CFString::new(KEY_PRIVATE);
        let tap_list_key = CFString::new(KEY_TAPS);
        CFDictionary::from_CFType_pairs(&[
            (name_key, CFString::new(name).as_CFType()),
            (uid_key, CFString::new(&device_uid).as_CFType()),
            (private_key, CFBoolean::true_value().as_CFType()),
            (tap_list_key, tap_list.as_CFType()),
        ])
    };

    let mut device: AudioObjectId = 0;
    // SAFETY: `dictionary` is a live CFDictionary and `device` a matching out slot.
    let status = unsafe {
        AudioHardwareCreateAggregateDevice(dictionary.as_concrete_TypeRef(), &mut device)
    };
    if status != 0 {
        return Err(TapError::CoreAudio {
            call: "AudioHardwareCreateAggregateDevice",
            status,
        });
    }
    Ok(device)
}

/// Translates a device UID into a CoreAudio device id through the system object
/// (`kAudioHardwarePropertyTranslateUIDToDevice`, macOS 10.5+; the UID is passed
/// as the call qualifier). `None` when the system does not know the UID (yet).
fn device_id_for_uid(uid: &str) -> Option<AudioObjectId> {
    let address = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_HARDWARE_PROPERTY_TRANSLATE_UID_TO_DEVICE,
        m_scope: SCOPE_GLOBAL,
        m_element: ELEMENT_MAIN,
    };
    let cf_uid = CFString::new(uid);
    let mut device: AudioObjectId = 0;
    let mut size = std::mem::size_of::<AudioObjectId>() as u32;
    // SAFETY: the qualifier is a live CFStringRef and `device` a matching out slot.
    let status = unsafe {
        AudioObjectGetPropertyData(
            AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            std::mem::size_of::<CFStringRef>() as u32,
            cf_uid.as_concrete_TypeRef() as *const c_void,
            &mut size,
            &mut device as *mut AudioObjectId as *mut c_void,
        )
    };
    (status == 0 && device != 0).then_some(device)
}

/// macOS tap state.
pub struct TapInner {
    tap_id: AudioObjectId,
    aggregate_id: AudioObjectId,
    uid: String,
    name: String,
    destroy: DestroyProcessTapFn,
    stopped: bool,
}

impl TapInner {
    /// Creates the tap + private aggregate device.
    pub fn start(config: &TapConfig) -> Result<Self, TapError> {
        let (create, destroy) = tap_entry_points()?;
        let description = tap_description(config)?;

        let mut tap_id: AudioObjectId = 0;
        // SAFETY: `description` is a live `CATapDescription` instance.
        let status = unsafe { create(&*description, &mut tap_id) };
        if status != 0 {
            return Err(TapError::CoreAudio {
                call: "AudioHardwareCreateProcessTap",
                status,
            });
        }

        // The aggregate device refers to the tap by its own UID property.
        let tap_uid = match string_property(tap_id, K_AUDIO_TAP_PROPERTY_UID) {
            Ok(uid) => uid,
            Err(err) => {
                // SAFETY: the tap exists (created above) and was not destroyed yet.
                unsafe { destroy(tap_id) };
                return Err(err);
            }
        };

        let aggregate_id = match create_aggregate_device(&config.name, &tap_uid) {
            Ok(id) => id,
            Err(err) => {
                // SAFETY: as above — never leak the tap when the aggregate fails.
                unsafe { destroy(tap_id) };
                return Err(err);
            }
        };

        // The device UID is the handle callers get back (and what the aggregate was
        // created under); reading it back proves the device is live and addressable.
        let uid = match string_property(aggregate_id, K_AUDIO_DEVICE_PROPERTY_DEVICE_UID) {
            Ok(uid) => uid,
            Err(err) => {
                // SAFETY: both objects are live; tear down in dependency order.
                unsafe {
                    AudioHardwareDestroyAggregateDevice(aggregate_id);
                    destroy(tap_id);
                }
                return Err(err);
            }
        };

        Ok(Self {
            tap_id,
            aggregate_id,
            uid,
            name: config.name.clone(),
            destroy,
            stopped: false,
        })
    }

    /// UID of the private aggregate device.
    pub fn device_uid(&self) -> &str {
        &self.uid
    }

    /// Name the aggregate device was created with.
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// CoreAudio device id the UID currently resolves to
    /// (`kAudioHardwarePropertyTranslateUIDToDevice`, queried lazily; `None` when
    /// the system has not registered the device yet). This is the id a
    /// future/UID-aware cpal API would open the tap with.
    pub fn device_id(&self) -> Option<AudioObjectId> {
        device_id_for_uid(&self.uid)
    }

    /// Aggregate device first, then the tap. Idempotent.
    pub fn stop(&mut self) -> Result<(), TapError> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        // SAFETY: both objects were created by `start` and are destroyed once
        // (guarded by `stopped`).
        let aggregate_status = unsafe { AudioHardwareDestroyAggregateDevice(self.aggregate_id) };
        let tap_status = unsafe { (self.destroy)(self.tap_id) };
        if tap_status != 0 {
            return Err(TapError::CoreAudio {
                call: "AudioHardwareDestroyProcessTap",
                status: tap_status,
            });
        }
        if aggregate_status != 0 {
            return Err(TapError::CoreAudio {
                call: "AudioHardwareDestroyAggregateDevice",
                status: aggregate_status,
            });
        }
        Ok(())
    }
}

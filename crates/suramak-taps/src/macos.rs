//! macOS: CoreAudio process taps (macOS 14.2+) fronted by a private aggregate device.
//!
//! ## Exact API calls used (with availability)
//!
//! | Call | Availability | Source |
//! |---|---|---|
//! | `CATapDescription` (`-initStereoGlobalTapButExcludeProcesses:`, `setName:`, `setUUID:`, `setMuteBehavior:`, `setPrivate:`) | macOS 14.2+ | `CoreAudio/AudioHardwareTapping.h` (Objective-C class; reached via `objc2` message sends — there is no Rust binding crate for the 14.2 tap API, so `objc2::msg_send!` is used). Header truth: `@property (…, getter=isPrivate, setter=setPrivate:) BOOL privateTap` — the setter is `setPrivate:`, *not* `setPrivateTap:` |
//! | `AudioHardwareCreateProcessTap(CATapDescription*, AudioObjectID*)` | macOS 14.2+ | `CoreAudio/AudioHardwareTapping.h` |
//! | `AudioHardwareDestroyProcessTap(AudioObjectID)` | macOS 14.2+ | `CoreAudio/AudioHardwareTapping.h` |
//! | `AudioHardwareCreateAggregateDevice(CFDictionaryRef, AudioDeviceID*)` | macOS 10.9+ | `CoreAudio/AudioHardware.h` |
//! | `AudioHardwareDestroyAggregateDevice(AudioDeviceID)` | macOS 10.9+ | `CoreAudio/AudioHardware.h` |
//! | `AudioObjectGetPropertyData` | macOS 10.0+ | `CoreAudio/AudioHardware.h` |
//! | `kAudioTapPropertyUID` (`'tuid'`) | macOS 14.2+ | tap object property, `AudioHardwareTapping.h` |
//! | `kAudioDevicePropertyDeviceUID` (`'uid '`) / `kAudioDevicePropertyDeviceNameCFString` (`'lnam'`) | macOS 10.0+ | device properties, `AudioHardware.h` |
//! | `kAudioHardwarePropertyTranslatePIDToProcessObject` (selector `0x69643270`) | macOS 14.2+ | pid → process audio-object id; the tap exclusion array wants **process objects**, not pids |
//! | `kAudioHardwarePropertyProcessObjectList` (`'prs#'`, `0x70727323`) | macOS 14.2+ | every HAL process object, for the M6 per-app picker |
//! | `kAudioProcessPropertyPID` (`'ppid'`) / `…BundleID` (`'pbid'`) / `…IsRunningInput` (`'piri'`) / `…IsRunningOutput` (`'piro'`) | macOS 14.2+ | process-object fields; the `pbid`/`piri`/`piro` values are verbatim from the SDK-generated `objc2-core-audio` bindings |
//! | `AudioObjectGetPropertyDataSize` | macOS 10.0+ | size query for the variable-length process-object list |
//! | `-initStereoGlobalTapButExcludeProcesses:` (all except) / `-initStereoMixdownOfProcesses:` (only these — M6) | macOS 14.2+ | `CATapDescription` initializers, `CoreAudio/AudioHardwareTapping.h` |
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

use crate::{AudioProcessInfo, ProcessSelection, TapConfig, TapError};

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
/// `kAudioHardwarePropertyTranslatePIDToProcessObject` (macOS 14.2+): pid (as the
/// call qualifier) → that process's HAL **audio object** id; `0` when the process
/// has no audio object. Value `0x69643270`, verbatim from the SDK-generated
/// `objc2-core-audio` bindings (same value in the `core-audio` crate).
const K_AUDIO_HARDWARE_PROPERTY_TRANSLATE_PID_TO_PROCESS_OBJECT: u32 = 0x6964_3270;
/// `kAudioHardwarePropertyProcessObjectList` (`'prs#'`, macOS 14.2+): every HAL
/// process object (processes CoreAudio knows to have audio). Value verbatim from
/// `objc2-core-audio` (`kAudioHardwarePropertyProcessObjectList = 0x70727323`).
const K_AUDIO_HARDWARE_PROPERTY_PROCESS_OBJECT_LIST: u32 = 0x7072_7323;
/// `kAudioProcessPropertyPID` (`'ppid'`, macOS 14.2+): a process object's pid.
const K_AUDIO_PROCESS_PROPERTY_PID: u32 = 0x7070_6964;
/// `kAudioProcessPropertyBundleID` (`'pbid'`, macOS 14.2+): a process object's
/// bundle id (a CFString; may be absent for CLI helpers). Value verbatim from
/// `objc2-core-audio` — note it is `pbid`, not `pidb`.
const K_AUDIO_PROCESS_PROPERTY_BUNDLE_ID: u32 = 0x7062_6964;
/// `kAudioProcessPropertyIsRunningInput` (`'piri'`, macOS 14.2+).
const K_AUDIO_PROCESS_PROPERTY_IS_RUNNING_INPUT: u32 = 0x7069_7269;
/// `kAudioProcessPropertyIsRunningOutput` (`'piro'`, macOS 14.2+).
const K_AUDIO_PROCESS_PROPERTY_IS_RUNNING_OUTPUT: u32 = 0x7069_726f;

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
    /// CoreAudio/AudioHardware.h, macOS 10.0+ (variable-size properties, e.g.
    /// the process-object list).
    fn AudioObjectGetPropertyDataSize(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        out_size: *mut u32,
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
/// `AllExcept` → `initStereoGlobalTapButExcludeProcesses:` (the whole system
/// minus the listed processes); `Only` → `initStereoMixdownOfProcesses:` (a
/// stereo mixdown of exactly the listed processes — M6 per-app capture).
/// Mute behavior `0` (`CATapMuteBehaviorUnmuted`) keeps the tapped audio
/// audible to the user, `privateTap = YES` keeps the aggregate device out of
/// other apps' device lists.
fn tap_description(config: &TapConfig) -> Result<Retained<AnyObject>, TapError> {
    let Some(cls) = AnyClass::get(c"CATapDescription") else {
        return Err(TapError::OsTooOld);
    };
    // Apple: these arrays hold *audio process object* ids, **not** pids. Passing a
    // raw pid made tap creation fail with `kAudioHardwareBadObjectError` (`'!obj'`,
    // OSStatus 560947818) on the first real macOS run (2026-10-13). Translate
    // first; a pid with no audio object yet is not in the HAL's process list
    // (not an error — the same nil contract Apple's samples use) and is skipped.
    let pids = match &config.processes {
        ProcessSelection::AllExcept(pids) => pids,
        ProcessSelection::Only(pids) => pids,
    };
    let objects: Vec<AudioObjectId> = pids
        .iter()
        .filter_map(|pid| process_object_for_pid(*pid))
        .collect();
    let numbers: Vec<Retained<NSNumber>> = objects
        .iter()
        .map(|object| NSNumber::new_u32(*object))
        .collect();
    let selected = NSArray::from_retained_slice(&numbers);
    let name = NSString::from_str(&config.name);
    let uuid = NSUUID::new();

    // SAFETY: `alloc` + an `init…` initializer follow the +1 allocation contract;
    // property setters match the documented `CATapDescription` properties.
    unsafe {
        let allocated: Allocated<AnyObject> = msg_send![cls, alloc];
        let description: Retained<AnyObject> = match &config.processes {
            ProcessSelection::AllExcept(_) => {
                msg_send![allocated, initStereoGlobalTapButExcludeProcesses: &*selected]
            }
            // `-initStereoMixdownOfProcesses:` — "only these processes"
            // (verified against CATapDescription's documented initializers).
            ProcessSelection::Only(_) => {
                msg_send![allocated, initStereoMixdownOfProcesses: &*selected]
            }
        };
        let _: () = msg_send![&*description, setName: &*name];
        let _: () = msg_send![&*description, setUUID: &*uuid];
        let _: () = msg_send![&*description, setMuteBehavior: 0isize];
        // Apple's header: `@property (…, getter=isPrivate, setter=setPrivate:) BOOL privateTap`
        // — the setter is `setPrivate:`. `setPrivateTap:` does not exist and raises an
        // unrecognized-selector NSException, which aborts the process via objc2
        // ("Rust cannot catch foreign exceptions"). Caught by the user's first real
        // system-audio record (2026-10-13); regression-guarded by `tests::tap_description…`.
        let _: () = msg_send![&*description, setPrivate: true];
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

/// Reads an optional CFString-valued property, `None` when the object does not
/// have it (a process object without a bundle id) or the read fails.
///
/// Uses the **create rule**: CoreAudio property reads hand the caller a +1
/// CFStringRef (the same contract `corti-coreaudio`/`side-huddle` use for
/// `kAudioProcessPropertyBundleID`), so `wrap_under_create_rule` takes that
/// reference instead of leaking it.
fn optional_string_property(object: AudioObjectId, selector: u32) -> Option<String> {
    let address = AudioObjectPropertyAddress {
        m_selector: selector,
        m_scope: SCOPE_GLOBAL,
        m_element: ELEMENT_MAIN,
    };
    let mut value: CFStringRef = ptr::null();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;
    // SAFETY: `value` is a CFStringRef slot with the matching size; the address
    // is a live stack value for the duration of the call.
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
    if status != 0 || value.is_null() {
        return None;
    }
    // SAFETY: non-null CFStringRef owned by us (+1, create rule).
    Some(unsafe { CFString::wrap_under_create_rule(value) }.to_string())
}

/// Reads an optional `UInt32`-valued property (`None` on any failure).
fn optional_u32_property(object: AudioObjectId, selector: u32) -> Option<u32> {
    let address = AudioObjectPropertyAddress {
        m_selector: selector,
        m_scope: SCOPE_GLOBAL,
        m_element: ELEMENT_MAIN,
    };
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value` is a u32 slot with the matching size; the address is a
    // live stack value for the duration of the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            ptr::null(),
            &mut size,
            &mut value as *mut u32 as *mut c_void,
        )
    };
    (status == 0).then_some(value)
}

/// Every HAL process object (`kAudioHardwarePropertyProcessObjectList`,
/// macOS 14.2+). Empty (not an error) when the property is absent — there are
/// no process objects before the tap API exists.
fn process_object_list() -> Result<Vec<AudioObjectId>, TapError> {
    let address = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_HARDWARE_PROPERTY_PROCESS_OBJECT_LIST,
        m_scope: SCOPE_GLOBAL,
        m_element: ELEMENT_MAIN,
    };
    let mut size: u32 = 0;
    // SAFETY: the address is a live stack value; `size` is a matching out slot.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            0,
            ptr::null(),
            &mut size,
        )
    };
    if status != 0 {
        // The property does not exist on this OS (pre-14.2): no process objects.
        return Ok(Vec::new());
    }
    let count = size as usize / std::mem::size_of::<AudioObjectId>();
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut objects = vec![0 as AudioObjectId; count];
    let mut read_size = size;
    // SAFETY: `objects` holds `count` AudioObjectIds and `read_size` matches the
    // buffer; the address is a live stack value for the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            0,
            ptr::null(),
            &mut read_size,
            objects.as_mut_ptr() as *mut c_void,
        )
    };
    if status != 0 {
        return Err(TapError::CoreAudio {
            call: "AudioObjectGetPropertyData",
            status,
        });
    }
    objects.truncate(read_size as usize / std::mem::size_of::<AudioObjectId>());
    Ok(objects)
}

/// Lists the running processes with audio (M6 per-app picker): every HAL
/// process object with its pid, bundle id and running state.
///
/// Unreadable fields degrade to defaults (no bundle id, not running) rather
/// than failing the whole listing. Suramak's own process is skipped — a tap
/// must never capture itself. Sorted by pid for a stable UI order.
pub fn list_audio_processes() -> Result<Vec<AudioProcessInfo>, TapError> {
    let own_pid = std::process::id();
    let mut processes = Vec::new();
    for object in process_object_list()? {
        let Some(pid) = optional_u32_property(object, K_AUDIO_PROCESS_PROPERTY_PID) else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        processes.push(AudioProcessInfo {
            pid,
            bundle_id: optional_string_property(object, K_AUDIO_PROCESS_PROPERTY_BUNDLE_ID)
                .unwrap_or_default(),
            is_running_output: optional_u32_property(
                object,
                K_AUDIO_PROCESS_PROPERTY_IS_RUNNING_OUTPUT,
            )
            .is_some_and(|value| value != 0),
            is_running_input: optional_u32_property(
                object,
                K_AUDIO_PROCESS_PROPERTY_IS_RUNNING_INPUT,
            )
            .is_some_and(|value| value != 0),
        });
    }
    processes.sort_by_key(|process| process.pid);
    Ok(processes)
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

/// Translates a BSD pid to its HAL **audio process object** id
/// (`kAudioHardwarePropertyTranslatePIDToProcessObject`, macOS 14.2+).
///
/// `None` when the process has no audio object yet — a process that has never
/// done audio I/O is simply not in the HAL's process list, which is not an
/// error (the same "translate or nil" contract Apple's sample code uses).
fn process_object_for_pid(pid: u32) -> Option<AudioObjectId> {
    let address = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_HARDWARE_PROPERTY_TRANSLATE_PID_TO_PROCESS_OBJECT,
        m_scope: SCOPE_GLOBAL,
        m_element: ELEMENT_MAIN,
    };
    let mut object: AudioObjectId = 0;
    let mut size = std::mem::size_of::<AudioObjectId>() as u32;
    let mut qualifier: u32 = pid;
    // SAFETY: the qualifier is a live u32 holding the pid and `object` is a
    // matching out slot; the address is a live stack value for the call.
    let status = unsafe {
        AudioObjectGetPropertyData(
            AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            std::mem::size_of::<u32>() as u32,
            &mut qualifier as *mut u32 as *const c_void,
            &mut size,
            &mut object as *mut AudioObjectId as *mut c_void,
        )
    };
    (status == 0 && object != 0).then_some(object)
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
    ///
    /// Do NOT call right after [`SystemTap::start`]: translating the UID while
    /// the HAL device list was still settling after aggregate creation crashed
    /// inside `HALDeviceList::CopyDeviceByUID` (SIGSEGV, macOS 27, 2026-10-13 —
    /// probed live on the owner's Mac; the record path uses device enumeration
    /// instead). Diagnostics only, once the device has been up for a while.
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

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::sel;

    /// The `CATapDescription` message-send sequence must not raise ObjC
    /// exceptions: a bad selector aborts the whole process ("Rust cannot catch
    /// foreign exceptions") — that is exactly how `setPrivateTap:` (nonexistent,
    /// real setter is `setPrivate:`) killed the app on the first real
    /// system-audio record. Constructing the description needs no permission;
    /// only `AudioHardwareCreateProcessTap` does, so this test stays prompt-free.
    #[test]
    fn tap_description_builds_and_the_fixed_setter_exists() {
        let config = TapConfig::system_audio();
        let description = tap_description(&config).expect("CATapDescription builds");
        // Header truth: `@property (…, getter=isPrivate, setter=setPrivate:) BOOL privateTap`.
        let responds_private: bool =
            unsafe { msg_send![&*description, respondsToSelector: sel!(setPrivate:)] };
        assert!(
            responds_private,
            "CATapDescription must respond to setPrivate: — did Apple change the setter again?"
        );
        let responds_bogus: bool =
            unsafe { msg_send![&*description, respondsToSelector: sel!(setPrivateTap:)] };
        assert!(
            !responds_bogus,
            "setPrivateTap: unexpectedly exists; revisit the fix if so"
        );
    }

    /// The M6 per-app path builds a *mixdown* description for exactly the listed
    /// processes; the message-send sequence must not raise (a bad selector aborts
    /// the process). No permission is needed to construct the description.
    #[test]
    fn tap_description_builds_for_an_only_selection() {
        let config = TapConfig::app_audio(vec![std::process::id()]);
        let description = tap_description(&config).expect("CATapDescription builds");
        let responds_private: bool =
            unsafe { msg_send![&*description, respondsToSelector: sel!(setPrivate:)] };
        assert!(responds_private);
    }
}

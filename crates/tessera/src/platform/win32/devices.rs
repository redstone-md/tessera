// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What can be taken out of the machine, as the "Safely Remove Hardware"
//! icon of Explorer lists it: drives with a letter on a device that can be
//! unplugged (a USB stick, a disk on USB or Thunderbolt, a card reader on
//! USB), drives whose medium comes out of a drive that stays (a disc, a
//! card in a reader built in), and devices without a drive that Windows
//! wants to be told about before they are pulled.
//!
//! A device counts as one to eject the way Explorer's hotplug component
//! counts it: Windows says it is removable and not that it may be pulled
//! out without warning, and it is no docking station. Of a chain of such
//! devices, the one nearest the root of the device tree is ejected, which
//! takes the ones below it along.
//!
//! Ejecting asks Windows (`CM_Request_Device_EjectW`) with a buffer for the
//! reason of a refusal, so Windows shows no message of its own; the caller
//! tells the user. A medium is ejected by locking and dismounting its
//! volume and asking the drive to let it out.
//!
//! Changes come as `WM_DEVICECHANGE` to the hidden window of the shell
//! thread, which, being a top-level window, gets what Windows broadcasts:
//! a volume that arrived or left (drive letters, and media put in or taken
//! out) and a device tree that changed.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::forget;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use async_channel::Receiver;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DEVCAP_DOCKDEVICE, CM_DEVCAP_REMOVABLE, CM_DEVCAP_SURPRISEREMOVALOK,
    CM_DEVNODE_STATUS_FLAGS, CM_DRP_CAPABILITIES, CM_DRP_DEVICEDESC, CM_DRP_FRIENDLYNAME,
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_GETIDLIST_FILTER_PRESENT,
    CM_Get_DevNode_Registry_PropertyW, CM_Get_DevNode_Status, CM_Get_Device_ID_List_SizeW,
    CM_Get_Device_ID_ListW, CM_Get_Device_IDW, CM_Get_Device_Interface_List_SizeW,
    CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_PropertyW, CM_Get_Parent,
    CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CM_PROB, CM_Request_Device_EjectW, CONFIGRET,
    CR_BUFFER_SMALL, CR_SUCCESS, DN_STARTED, PNP_VETO_TYPE, PNP_VetoAlreadyRemoved,
    PNP_VetoInsufficientRights, PNP_VetoOutstandingOpen, PNP_VetoPendingClose, PNP_VetoWindowsApp,
    PNP_VetoWindowsService,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_InstanceId, DEVPROPTYPE};
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW,
    GetLogicalDrives, GetVolumeInformationW, OPEN_EXISTING,
};
use windows::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetThreadErrorMode, THREAD_ERROR_MODE,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, GUID_DEVINTERFACE_CDROM, GUID_DEVINTERFACE_DISK,
    GUID_DEVINTERFACE_FLOPPY, IOCTL_STORAGE_EJECT_MEDIA, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_MEDIA_REMOVAL, PREVENT_MEDIA_REMOVAL, STORAGE_DEVICE_NUMBER,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE, DBT_DEVNODES_CHANGED, WM_DEVICECHANGE,
};
use windows::core::{GUID, Owned, PCWSTR};

use super::files::{Drive, DriveKind, disk_space, text_in, wide};
use super::shell_thread::ShellThread;

/// What Windows calls the kinds of drives that are looked at.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_CDROM: u32 = 5;

/// The longest name Windows gives for what refused an ejection.
const VETO_NAME_LENGTH: usize = 260;

/// How often an ejection that was refused is asked for again, and how long
/// is waited in between: a program that just closed a file may hold it for
/// a moment longer, as Explorer's own retries assume.
const EJECT_TRIES: u32 = 3;
const EJECT_PAUSE: Duration = Duration::from_millis(500);

/// How often a volume is tried to be locked before its medium is ejected,
/// and how long is waited in between.
const LOCK_TRIES: u32 = 10;
const LOCK_PAUSE: Duration = Duration::from_millis(200);

/// How often a list of devices is asked for again where it grew between
/// asking for its size and reading it, as it does while devices change.
const LIST_TRIES: u32 = 5;

/// How many devices up the device tree are looked at, at the most.
const TREE_DEPTH: usize = 32;

/// A device that Windows wants to be told about before it is unplugged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovableDevice {
    /// What Windows knows the device by, its instance identifier.
    pub instance: String,
    /// What Windows calls it, as Device Manager does.
    pub name: String,
}

/// A drive with a letter that can be taken out of the machine.
#[derive(Clone, Debug)]
pub struct RemovableDrive {
    pub drive: Drive,
    /// The device ejecting it ejects, with every drive of it; `None` for
    /// a drive whose medium is ejected and which stays itself.
    pub device: Option<RemovableDevice>,
}

impl PartialEq for RemovableDrive {
    fn eq(&self, other: &Self) -> bool {
        let (one, two) = (&self.drive, &other.drive);
        one.root == two.root
            && one.label == two.label
            && one.kind == two.kind
            && one.total == two.total
            && one.free == two.free
            && self.device == other.device
    }
}

impl RemovableDrive {
    /// The letter of the drive, as `E`.
    pub fn letter(&self) -> char {
        self.drive
            .root
            .to_string_lossy()
            .chars()
            .next()
            .unwrap_or('?')
    }

    /// What ejecting this drive does.
    pub fn ejection(&self) -> Ejection {
        match &self.device {
            Some(device) => Ejection::Device(device.instance.clone()),
            None => Ejection::Medium(self.letter()),
        }
    }
}

/// Everything there is to take out of the machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Removables {
    /// The drives, in the order of their letters.
    pub drives: Vec<RemovableDrive>,
    /// The devices to eject that hold no drive with a letter.
    pub devices: Vec<RemovableDevice>,
}

impl Removables {
    /// What there is to take out now. It asks every drive with a letter
    /// for its volume, which can wait for a disc to spin up, so it is
    /// done away from the user interface.
    pub fn read() -> Self {
        let _quiet = QuietErrors::new();
        let disks = Disks::read();
        // The device Windows runs from is never offered, though a disk on a
        // port that can take disks while it runs counts as removable.
        let system = std::env::var("SystemDrive").ok();
        let system = system.and_then(|drive| drive.chars().next());
        let system = system.and_then(|letter| device_of(letter.to_ascii_uppercase(), &disks));
        let mut ejected: Vec<String> = system
            .iter()
            .map(|device| device.instance.clone())
            .collect();
        let mut drives = Vec::new();
        // SAFETY: the call takes no arguments.
        let letters = unsafe { GetLogicalDrives() };
        for letter in (0..26u8)
            .filter(|bit| letters & (1 << bit) != 0)
            .map(|bit| char::from(b'A' + bit))
        {
            // A network drive is not asked for its device, which could
            // wait for the network.
            let Some(kind) = drive_kind(letter) else {
                continue;
            };
            let device = device_of(letter, &disks);
            if device.is_some() && device == system {
                continue;
            }
            if let Some(drive) = removable_drive(letter, kind, device) {
                if let Some(device) = &drive.device {
                    ejected.push(device.instance.clone());
                }
                drives.push(drive);
            }
        }
        let devices = present_devices()
            .into_iter()
            .filter_map(Node::locate)
            // Of a chain of such devices, the one nearest the root.
            .filter(|node| {
                node.is_hotplug() && node.parent().and_then(Node::hotplug_root).is_none()
            })
            .filter_map(Node::device)
            .filter(|device| !ejected.contains(&device.instance))
            .collect();
        Self { drives, devices }
    }

    /// Whether there is nothing to take out.
    pub fn is_empty(&self) -> bool {
        self.drives.is_empty() && self.devices.is_empty()
    }

    /// Reads how full each drive is again, without looking for drives and
    /// devices anew, which takes far longer.
    pub fn read_space(&mut self) {
        let _quiet = QuietErrors::new();
        for drive in &mut self.drives {
            let space = disk_space(&drive.drive.root);
            drive.drive.total = space.map(|(total, _)| total);
            drive.drive.free = space.map(|(_, free)| free);
        }
    }
}

/// Keeps Windows from asking the user to put a disk in while a drive
/// without a medium is looked at, on this thread and until it is dropped.
struct QuietErrors(Option<THREAD_ERROR_MODE>);

impl QuietErrors {
    fn new() -> Self {
        let mut old = THREAD_ERROR_MODE::default();
        // SAFETY: `old` outlives the call; the mode is a thread's own.
        let set = unsafe {
            SetThreadErrorMode(
                SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX,
                Some(&mut old),
            )
        };
        Self(set.is_ok().then_some(old))
    }
}

impl Drop for QuietErrors {
    fn drop(&mut self) {
        if let Some(old) = self.0 {
            // SAFETY: the mode is the one the thread had before.
            unsafe { SetThreadErrorMode(old, None) }.ok();
        }
    }
}

/// What to eject.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ejection {
    /// The device with this instance identifier, with all of its drives.
    Device(String),
    /// The medium of the drive with this letter.
    Medium(char),
}

/// Why Windows did not eject something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EjectRefusal {
    /// A program or a service holds it, by the name Windows gives where it
    /// gives one; files on it may be open.
    InUse(Option<String>),
    /// The account may not eject it.
    NotAllowed,
    /// Windows or a driver refused for another reason, which it names.
    Refused(String),
}

impl Ejection {
    /// Ejects what this names on a thread of its own, which may wait for
    /// seconds while programs are asked to let go of it; the receiver
    /// yields how it went, or is closed without an answer where no thread
    /// could start, which is reported.
    pub fn start(&self) -> Receiver<Result<(), EjectRefusal>> {
        let (answer, answered) = async_channel::bounded(1);
        let ejection = self.clone();
        let started = thread::Builder::new()
            .name("eject".to_owned())
            .spawn(move || {
                answer.try_send(ejection.run()).ok();
            });
        if let Err(error) = started {
            report!("nothing is ejected: no thread starts for it: {error}");
        }
        answered
    }

    /// Ejects what this names, and waits for it.
    fn run(&self) -> Result<(), EjectRefusal> {
        match self {
            Self::Device(instance) => eject_device(instance),
            Self::Medium(letter) => eject_medium(*letter),
        }
    }
}

/// Tells of changes to drives and devices.
pub struct DeviceChanges;

impl DeviceChanges {
    /// Starts looking out for changes; the receiver yields whenever a
    /// volume arrived or left or the devices changed, with changes that
    /// come close together told once. Meant to be called once, by the one
    /// part of Tessera that keeps the list.
    pub fn watch() -> Receiver<()> {
        let (changed, changes) = async_channel::bounded(1);
        ShellThread::run(move || {
            let interception = ShellThread::intercept(move |message, wparam, _| {
                let event = wparam.0 as u32;
                let told = matches!(
                    event,
                    DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE | DBT_DEVNODES_CHANGED
                );
                if message == WM_DEVICECHANGE && told {
                    // A change waiting to be read covers this one too.
                    changed.try_send(()).ok();
                }
                None
            });
            // The watch lasts as long as the process.
            forget(interception);
        });
        changes
    }
}

/// The device to eject to take out the drive `letter`, if there is one.
fn device_of(letter: char, disks: &Disks) -> Option<RemovableDevice> {
    volume_number(letter)
        .and_then(|number| disks.node_of(number))
        .and_then(Node::hotplug_root)
        .and_then(Node::device)
}

/// What kind of drive the drive `letter` is, if it is one of those that
/// can be taken out or hold a medium; network drives, RAM disks and
/// letters without a volume are not.
fn drive_kind(letter: char) -> Option<DriveKind> {
    let root = wide(&format!("{letter}:\\"));
    // SAFETY: the root is NUL-terminated and outlives the call.
    match unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } {
        DRIVE_REMOVABLE => Some(DriveKind::Removable),
        DRIVE_FIXED => Some(DriveKind::Fixed),
        DRIVE_CDROM => Some(DriveKind::Optical),
        _ => None,
    }
}

/// The drive `letter`, of `kind` and on `device`, if it can be taken out
/// of the machine.
fn removable_drive(
    letter: char,
    kind: DriveKind,
    device: Option<RemovableDevice>,
) -> Option<RemovableDrive> {
    let root = format!("{letter}:\\");
    let root_wide = wide(&root);
    if device.is_none() && kind == DriveKind::Fixed {
        return None;
    }
    // A reader or a drive without a medium has nothing to eject.
    let label = volume_label(&root_wide)?;
    let space = disk_space(Path::new(&root));
    Some(RemovableDrive {
        drive: Drive {
            root: PathBuf::from(root),
            label,
            kind,
            total: space.map(|(total, _)| total),
            free: space.map(|(_, free)| free),
        },
        device,
    })
}

/// The name of the volume at `root`, empty where it has none; `None`
/// where there is no volume, as in a drive without a medium.
fn volume_label(root: &[u16]) -> Option<String> {
    let mut name = [0u16; 261];
    // SAFETY: the root is NUL-terminated, and the buffer carries its
    // length; every other part is left out.
    unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            Some(&mut name),
            None,
            None,
            None,
            None,
        )
    }
    .ok()?;
    Some(text_in(&name))
}

/// Which device of its kind a volume or disk is: the kind and the number,
/// as `IOCTL_STORAGE_GET_DEVICE_NUMBER` gives them.
type DeviceNumber = (u32, u32);

/// The disk, disc and floppy devices of the machine, by their numbers.
struct Disks {
    found: Vec<(DeviceNumber, Node)>,
}

impl Disks {
    fn read() -> Self {
        let classes = [
            GUID_DEVINTERFACE_DISK,
            GUID_DEVINTERFACE_CDROM,
            GUID_DEVINTERFACE_FLOPPY,
        ];
        let found = classes
            .iter()
            .flat_map(interfaces_of)
            .filter_map(|path| {
                let number = device_number(&wide(&path))?;
                let node = Node::locate(interface_instance(&path)?)?;
                Some((number, node))
            })
            .collect();
        Self { found }
    }

    fn node_of(&self, number: DeviceNumber) -> Option<Node> {
        let found = self.found.iter().find(|(known, _)| *known == number);
        found.map(|&(_, node)| node)
    }
}

/// The paths of the device interfaces of `class` that are present.
fn interfaces_of(class: &GUID) -> Vec<String> {
    read_list(
        // SAFETY: `length` outlives the call; no device is named.
        |length| unsafe {
            CM_Get_Device_Interface_List_SizeW(
                length,
                class,
                PCWSTR::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        },
        // SAFETY: the buffer carries its length; no device is named.
        |list| unsafe {
            CM_Get_Device_Interface_ListW(
                class,
                PCWSTR::null(),
                list,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        },
    )
}

/// A list of texts that Windows gives by its `size` first and then into a
/// buffer of that size with `list`; asked for again while it grew in
/// between, and empty where Windows does not give it.
fn read_list(
    size: impl Fn(&mut u32) -> CONFIGRET,
    list: impl Fn(&mut [u16]) -> CONFIGRET,
) -> Vec<String> {
    for _ in 0..LIST_TRIES {
        let mut length = 0u32;
        if size(&mut length) != CR_SUCCESS || length <= 1 {
            return Vec::new();
        }
        let mut buffer = vec![0u16; length as usize];
        match list(&mut buffer) {
            CR_SUCCESS => return texts_in(&buffer),
            CR_BUFFER_SMALL => {}
            _ => return Vec::new(),
        }
    }
    report!("the devices are not listed: the list kept growing");
    Vec::new()
}

/// The instance identifier of the device the interface `path` is of.
fn interface_instance(path: &str) -> Option<String> {
    let path = wide(path);
    let mut kind = DEVPROPTYPE::default();
    let mut buffer = [0u16; 512];
    let mut size = size_of_val(&buffer) as u32;
    // SAFETY: the path is NUL-terminated, and the buffer carries its size
    // in bytes; all of them outlive the call.
    let read = unsafe {
        CM_Get_Device_Interface_PropertyW(
            PCWSTR(path.as_ptr()),
            &DEVPKEY_Device_InstanceId,
            &mut kind,
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            0,
        )
    };
    (read == CR_SUCCESS).then(|| text_in(&buffer))
}

/// The instance identifiers of the devices that are present.
fn present_devices() -> Vec<String> {
    read_list(
        // SAFETY: `length` outlives the call; no filter is named.
        |length| unsafe {
            CM_Get_Device_ID_List_SizeW(length, PCWSTR::null(), CM_GETIDLIST_FILTER_PRESENT)
        },
        // SAFETY: the buffer carries its length; no filter is named.
        |list| unsafe { CM_Get_Device_ID_ListW(PCWSTR::null(), list, CM_GETIDLIST_FILTER_PRESENT) },
    )
}

/// A device of the device tree, by the number Windows gives it while it is
/// present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Node(u32);

impl Node {
    fn locate(instance: String) -> Option<Self> {
        let instance = wide(&instance);
        let mut node = 0u32;
        // SAFETY: the identifier is NUL-terminated and `node` outlives the
        // call.
        let found = unsafe {
            CM_Locate_DevNodeW(
                &mut node,
                PCWSTR(instance.as_ptr()),
                CM_LOCATE_DEVNODE_NORMAL,
            )
        };
        (found == CR_SUCCESS).then_some(Self(node))
    }

    fn parent(self) -> Option<Self> {
        let mut parent = 0u32;
        // SAFETY: `parent` outlives the call.
        let found = unsafe { CM_Get_Parent(&mut parent, self.0, 0) };
        (found == CR_SUCCESS).then_some(Self(parent))
    }

    /// What Windows says the device can do (`CM_DEVCAP_*`).
    fn capabilities(self) -> u32 {
        let mut capabilities = 0u32;
        let mut length = size_of::<u32>() as u32;
        // SAFETY: the number and its length outlive the call.
        let read = unsafe {
            CM_Get_DevNode_Registry_PropertyW(
                self.0,
                CM_DRP_CAPABILITIES,
                None,
                Some((&raw mut capabilities).cast::<c_void>()),
                &mut length,
                0,
            )
        };
        if read == CR_SUCCESS { capabilities } else { 0 }
    }

    /// Whether the device runs.
    fn is_started(self) -> bool {
        let mut status = CM_DEVNODE_STATUS_FLAGS::default();
        let mut problem = CM_PROB::default();
        // SAFETY: both outlive the call.
        let read = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, self.0, 0) };
        read == CR_SUCCESS && status.0 & DN_STARTED.0 != 0
    }

    /// Whether the device is one Windows wants to be told about before it
    /// is unplugged, as Explorer's hotplug component decides it.
    fn is_hotplug(self) -> bool {
        is_hotplug(self.capabilities()) && self.is_started()
    }

    /// The device to eject to take this one out: the one nearest the root
    /// of the tree among it and the devices above it that are hotplug.
    fn hotplug_root(self) -> Option<Self> {
        let mut node = Some(self);
        let mut root = None;
        for _ in 0..TREE_DEPTH {
            let Some(here) = node else {
                break;
            };
            if here.is_hotplug() {
                root = Some(here);
            }
            node = here.parent();
        }
        root
    }

    fn instance(self) -> Option<String> {
        let mut buffer = [0u16; 512];
        // SAFETY: the buffer carries its length.
        let read = unsafe { CM_Get_Device_IDW(self.0, &mut buffer, 0) };
        (read == CR_SUCCESS).then(|| text_in(&buffer))
    }

    /// A text property of the device's registry entry.
    fn text(self, property: u32) -> Option<String> {
        let mut buffer = [0u16; 256];
        let mut length = size_of_val(&buffer) as u32;
        // SAFETY: the buffer carries its length in bytes.
        let read = unsafe {
            CM_Get_DevNode_Registry_PropertyW(
                self.0,
                property,
                None,
                Some(buffer.as_mut_ptr().cast()),
                &mut length,
                0,
            )
        };
        let text = (read == CR_SUCCESS).then(|| text_in(&buffer))?;
        (!text.is_empty()).then_some(text)
    }

    /// The device as one to eject: its identifier and its name.
    fn device(self) -> Option<RemovableDevice> {
        let instance = self.instance()?;
        let name = self
            .text(CM_DRP_FRIENDLYNAME)
            .or_else(|| self.text(CM_DRP_DEVICEDESC))
            .unwrap_or_else(|| instance.clone());
        Some(RemovableDevice { instance, name })
    }
}

/// Whether a device with `capabilities` is to be ejected before it is
/// unplugged: it is removable, may not be pulled out without warning, and
/// is no docking station.
fn is_hotplug(capabilities: u32) -> bool {
    let removable = capabilities & CM_DEVCAP_REMOVABLE.0 != 0;
    let warned = CM_DEVCAP_SURPRISEREMOVALOK.0 | CM_DEVCAP_DOCKDEVICE.0;
    removable && capabilities & warned == 0
}

/// A volume or a device opened to be asked; closed when dropped.
struct Opened(Owned<HANDLE>);

impl Opened {
    /// Opens `path` (`\\.\E:` or the path of a device interface) with
    /// `access`; 0 is enough to ask for the device number.
    fn open(path: &[u16], access: u32) -> Option<Self> {
        // SAFETY: the path is NUL-terminated; the handle is owned and
        // closed once.
        unsafe {
            let handle = CreateFileW(
                PCWSTR(path.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
            .ok()?;
            Some(Self(Owned::new(handle)))
        }
    }

    /// Sends `code` with `input` and takes `output` back.
    fn control<I, O>(&self, code: u32, input: Option<&I>, output: Option<&mut O>) -> bool {
        self.ask(code, input, output).is_ok()
    }

    /// Sends `code` with `input`, takes `output` back and tells why Windows
    /// refused.
    fn ask<I, O>(
        &self,
        code: u32,
        input: Option<&I>,
        output: Option<&mut O>,
    ) -> windows::core::Result<()> {
        let mut returned = 0u32;
        let input_size = input.map_or(0, |_| size_of::<I>() as u32);
        let output_size = output.as_ref().map_or(0, |_| size_of::<O>() as u32);
        // SAFETY: the buffers are what their sizes say and outlive the
        // synchronous call.
        unsafe {
            DeviceIoControl(
                *self.0,
                code,
                input.map(|input| (input as *const I).cast::<c_void>()),
                input_size,
                output.map(|output| (output as *mut O).cast::<c_void>()),
                output_size,
                Some(&mut returned),
                None,
            )
        }
    }
}

/// The path of the volume of the drive `letter`, as `\\.\E:`.
fn volume_path(letter: char) -> Vec<u16> {
    wide(&format!("\\\\.\\{letter}:"))
}

fn volume_number(letter: char) -> Option<DeviceNumber> {
    device_number(&volume_path(letter))
}

fn device_number(path: &[u16]) -> Option<DeviceNumber> {
    let opened = Opened::open(path, 0)?;
    let mut number = STORAGE_DEVICE_NUMBER::default();
    let asked = opened.control::<(), _>(IOCTL_STORAGE_GET_DEVICE_NUMBER, None, Some(&mut number));
    asked.then_some((number.DeviceType, number.DeviceNumber))
}

/// What a refusal of `CM_Request_Device_EjectW` means.
fn refusal(veto: PNP_VETO_TYPE, name: String) -> Option<EjectRefusal> {
    const ALREADY_REMOVED: i32 = PNP_VetoAlreadyRemoved.0;
    const OUTSTANDING_OPEN: i32 = PNP_VetoOutstandingOpen.0;
    const PENDING_CLOSE: i32 = PNP_VetoPendingClose.0;
    const WINDOWS_APP: i32 = PNP_VetoWindowsApp.0;
    const WINDOWS_SERVICE: i32 = PNP_VetoWindowsService.0;
    const INSUFFICIENT_RIGHTS: i32 = PNP_VetoInsufficientRights.0;
    let named = (!name.is_empty()).then_some(name);
    match veto.0 {
        ALREADY_REMOVED => None,
        OUTSTANDING_OPEN | PENDING_CLOSE => Some(EjectRefusal::InUse(None)),
        WINDOWS_APP | WINDOWS_SERVICE => Some(EjectRefusal::InUse(named)),
        INSUFFICIENT_RIGHTS => Some(EjectRefusal::NotAllowed),
        other => Some(EjectRefusal::Refused(match named {
            Some(name) => format!("refusal {other} by {name}"),
            None => format!("refusal {other}"),
        })),
    }
}

fn eject_device(instance: &str) -> Result<(), EjectRefusal> {
    let mut last = None;
    for attempt in 0..EJECT_TRIES {
        if attempt > 0 {
            thread::sleep(EJECT_PAUSE);
        }
        // A device that is gone was taken out already.
        let Some(node) = Node::locate(instance.to_owned()) else {
            return Ok(());
        };
        let mut veto = PNP_VETO_TYPE::default();
        let mut name = [0u16; VETO_NAME_LENGTH];
        // SAFETY: the veto and its name buffer, which carries its length,
        // outlive the call. With a buffer Windows shows no message.
        let asked =
            unsafe { CM_Request_Device_EjectW(node.0, Some(&mut veto), Some(&mut name), 0) };
        if asked == CR_SUCCESS && veto.0 == 0 {
            return Ok(());
        }
        last = if veto.0 == 0 {
            Some(EjectRefusal::Refused(format!("error {}", asked.0)))
        } else {
            match refusal(veto, text_in(&name)) {
                None => return Ok(()),
                refused => refused,
            }
        };
        if last == Some(EjectRefusal::NotAllowed) {
            break;
        }
    }
    Err(last.unwrap_or_else(|| EjectRefusal::Refused(String::new())))
}

fn eject_medium(letter: char) -> Result<(), EjectRefusal> {
    let path = volume_path(letter);
    let written = Opened::open(&path, GENERIC_READ.0 | GENERIC_WRITE.0);
    let read_only = written.is_none();
    let opened = written
        .or_else(|| Opened::open(&path, GENERIC_READ.0))
        .ok_or(EjectRefusal::NotAllowed)?;
    // Only a disc may come out without its volume being locked: nothing
    // on it waits to be written.
    let disc = drive_kind(letter) == Some(DriveKind::Optical);
    let mut locked = false;
    for attempt in 0..LOCK_TRIES {
        if attempt > 0 {
            thread::sleep(LOCK_PAUSE);
        }
        match opened.ask::<(), ()>(FSCTL_LOCK_VOLUME, None, None) {
            Ok(()) => {
                locked = true;
                break;
            }
            // A volume opened only to be read cannot be locked: a disc is
            // ejected without, as Explorer does, and a card is not.
            Err(error) if read_only && error.code() == ERROR_ACCESS_DENIED.to_hresult() => {
                if disc {
                    break;
                }
                return Err(EjectRefusal::NotAllowed);
            }
            // Otherwise files on it are open, which a file system tells
            // as access denied too.
            Err(_) if attempt + 1 == LOCK_TRIES => return Err(EjectRefusal::InUse(None)),
            Err(_) => {}
        }
    }
    if locked {
        // A volume that is locked has no open files: dismounting it writes
        // what waits to be written and lets go of it. One that is not is
        // never dismounted, which would cut off the files open on it.
        opened.control::<(), ()>(FSCTL_DISMOUNT_VOLUME, None, None);
    }
    let allow = PREVENT_MEDIA_REMOVAL {
        PreventMediaRemoval: false,
    };
    opened.control::<_, ()>(IOCTL_STORAGE_MEDIA_REMOVAL, Some(&allow), None);
    opened
        .ask::<(), ()>(IOCTL_STORAGE_EJECT_MEDIA, None, None)
        .map_err(|error| EjectRefusal::Refused(error.message()))
}

/// The texts of a list of NUL-terminated texts that ends with one NUL
/// more.
fn texts_in(list: &[u16]) -> Vec<String> {
    list.split(|&unit| unit == 0)
        .take_while(|text| !text.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_is_ejected_when_it_is_removable_and_not_ready_to_be_pulled() {
        let removable = CM_DEVCAP_REMOVABLE.0;
        assert!(is_hotplug(removable));
        // A mouse may be pulled out at any time.
        assert!(!is_hotplug(removable | CM_DEVCAP_SURPRISEREMOVALOK.0));
        assert!(!is_hotplug(removable | CM_DEVCAP_DOCKDEVICE.0));
        assert!(!is_hotplug(0));
    }

    #[test]
    fn a_list_of_texts_ends_at_the_empty_one() {
        let list: Vec<u16> = "USB\\A\0SCSI\\B\0\0\0".encode_utf16().collect();
        assert_eq!(texts_in(&list), ["USB\\A", "SCSI\\B"]);
        assert!(texts_in(&[0, 0]).is_empty());
    }

    #[test]
    fn a_refusal_tells_who_holds_the_device() {
        assert_eq!(refusal(PNP_VetoAlreadyRemoved, String::new()), None);
        assert_eq!(
            refusal(PNP_VetoOutstandingOpen, "USB\\VID_1".to_owned()),
            Some(EjectRefusal::InUse(None))
        );
        assert_eq!(
            refusal(PNP_VetoWindowsApp, "winword.exe".to_owned()),
            Some(EjectRefusal::InUse(Some("winword.exe".to_owned())))
        );
        assert_eq!(
            refusal(PNP_VetoInsufficientRights, String::new()),
            Some(EjectRefusal::NotAllowed)
        );
        assert!(matches!(
            refusal(PNP_VETO_TYPE(6), "disk".to_owned()),
            Some(EjectRefusal::Refused(text)) if text.contains("disk")
        ));
    }

    #[test]
    fn a_drive_is_ejected_with_its_device_or_by_its_medium() {
        let drive = |device: Option<RemovableDevice>| RemovableDrive {
            drive: Drive {
                root: PathBuf::from("E:\\"),
                label: String::new(),
                kind: DriveKind::Removable,
                total: None,
                free: None,
            },
            device,
        };
        assert_eq!(drive(None).letter(), 'E');
        assert_eq!(drive(None).ejection(), Ejection::Medium('E'));
        let stick = RemovableDevice {
            instance: "USB\\VID_0781".to_owned(),
            name: "Stick".to_owned(),
        };
        assert_eq!(
            drive(Some(stick)).ejection(),
            Ejection::Device("USB\\VID_0781".to_owned())
        );
    }
}

// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One lazy, bounded owner of the fixed current-user Run registration. Registry
//! compare/write is not atomic against external editors: no transaction, retry,
//! or rollback is claimed. Windows may disable or delay execution independently
//! of this registration; StartupApproved and policy are never modified.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, TryLockError};

use tessera_system::startup::{
    StartupCompletion, StartupError, StartupHost, StartupRegistration, StartupSnapshot,
    StartupTarget,
};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, HLOCAL, LocalFree,
};
use windows::Win32::System::Environment::GetCommandLineW;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_CREATE_KEY_DISPOSITION,
    REG_CREATED_NEW_KEY, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::UI::Shell::CommandLineToArgvW;
use windows::core::{PCWSTR, w};

use crate::single_flight::{Flight, FlightGate};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Tessera");
// Microsoft documents Run command lines as no longer than 260 characters:
// https://learn.microsoft.com/windows/win32/setupapi/run-and-runonce-registry-keys
// Count UTF-16 units conservatively, including quotes and excluding final NUL.
const RUN_COMMAND_UNITS: usize = 260;
const REGISTRY_BYTES: usize = (RUN_COMMAND_UNITS + 1) * 2;
const PROCESS_COMMAND_UNITS: usize = 32_767;

#[derive(Default)]
pub(crate) struct NativeStartupHost {
    gate: FlightGate,
    worker: Mutex<Worker>,
}

#[derive(Default)]
struct Worker {
    sender: Option<SyncSender<Job>>,
    failed: bool,
}

impl NativeStartupHost {
    fn submit(
        &self,
        operation: Operation,
        completion: StartupCompletion,
    ) -> Result<(), StartupError> {
        let flight = self.gate.try_enter().ok_or(StartupError::Busy)?;
        let mut worker = self.worker.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => StartupError::Busy,
            TryLockError::Poisoned(_) => StartupError::Unavailable,
        })?;
        if worker.failed {
            return Err(StartupError::Unavailable);
        }
        if worker.sender.is_none() {
            // A forged set cannot start a native owner or acquire authority.
            if matches!(operation, Operation::Set { .. }) {
                return Err(StartupError::InvalidTarget);
            }
            let (sender, receiver) = sync_channel(1);
            if std::thread::Builder::new()
                .name("tessera-startup".into())
                .spawn(move || owner_loop(receiver))
                .is_err()
            {
                worker.failed = true;
                return Err(StartupError::Unavailable);
            }
            worker.sender = Some(sender);
        }
        let Some(sender) = &worker.sender else {
            return Err(StartupError::Unavailable);
        };
        match sender.try_send(Job {
            operation,
            completion,
            flight,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(StartupError::Busy),
            Err(TrySendError::Disconnected(_)) => {
                // Never grow or respawn workers after owner failure.
                worker.failed = true;
                worker.sender = None;
                Err(StartupError::Unavailable)
            }
        }
    }
}

impl StartupHost for NativeStartupHost {
    fn read(&self, completion: StartupCompletion) -> Result<(), StartupError> {
        self.submit(Operation::Read, completion)
    }

    fn set(
        &self,
        target: StartupTarget,
        registered: bool,
        completion: StartupCompletion,
    ) -> Result<(), StartupError> {
        self.submit(Operation::Set { target, registered }, completion)
    }
}

enum Operation {
    Read,
    Set {
        target: StartupTarget,
        registered: bool,
    },
}

struct Job {
    operation: Operation,
    completion: StartupCompletion,
    flight: Flight,
}

fn owner_loop(receiver: Receiver<Job>) {
    let mut owner = Owner::default();
    while let Ok(job) = receiver.recv() {
        let Job {
            operation,
            completion,
            flight,
        } = job;
        let failure = match &operation {
            Operation::Read => StartupError::Unavailable,
            Operation::Set { .. } => StartupError::Unconfirmed,
        };
        let result = catch_unwind(AssertUnwindSafe(|| match operation {
            Operation::Read => owner.read(),
            Operation::Set { target, registered } => owner.set(target, registered),
        }))
        .unwrap_or_else(|_| {
            owner.current = None;
            Err(failure)
        });
        // No issuer/mailbox borrow spans a callback. Reentrant requests can
        // submit the next single flight; accepted work needs no UI lifetime.
        drop(flight);
        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
    }
}

#[derive(Default)]
struct Owner {
    current: Option<Observation>,
}

struct Observation {
    target: StartupTarget,
    identity: Identity,
    value: Value,
}

#[derive(PartialEq, Eq)]
struct Identity {
    command: Vec<u8>,
    process_command: Vec<u16>,
}

#[derive(PartialEq, Eq)]
enum Value {
    MissingKey,
    MissingEntry,
    Bytes {
        kind: REG_VALUE_TYPE,
        bytes: Vec<u8>,
    },
    Foreign,
}

impl Value {
    fn registration(&self, identity: &Identity) -> StartupRegistration {
        match self {
            Self::MissingKey | Self::MissingEntry => StartupRegistration::Absent,
            Self::Bytes { kind, bytes } if *kind == REG_SZ && *bytes == identity.command => {
                StartupRegistration::Registered
            }
            _ => StartupRegistration::Foreign,
        }
    }
}

impl Owner {
    fn publish(&mut self, identity: Identity, value: Value) -> StartupSnapshot {
        let registration = value.registration(&identity);
        let target = (registration != StartupRegistration::Foreign).then(StartupTarget::new);
        self.current = target.as_ref().map(|target| Observation {
            target: target.clone(),
            identity,
            value,
        });
        StartupSnapshot {
            registration,
            target,
        }
    }

    fn read(&mut self) -> Result<StartupSnapshot, StartupError> {
        self.current = None;
        let (identity, value) = read_observation()?;
        Ok(self.publish(identity, value))
    }

    fn set(
        &mut self,
        target: StartupTarget,
        registered: bool,
    ) -> Result<StartupSnapshot, StartupError> {
        // One issuer entry only; every attempt consumes the observation.
        let observed = self.current.take().ok_or(StartupError::InvalidTarget)?;
        if observed.target != target || current_identity()? != observed.identity {
            return Err(StartupError::InvalidTarget);
        }
        let was_registered =
            observed.value.registration(&observed.identity) == StartupRegistration::Registered;
        if was_registered == registered {
            let (identity, value) = read_observation()?;
            if identity != observed.identity || value != observed.value {
                return Err(StartupError::InvalidTarget);
            }
            return Ok(self.publish(identity, value));
        }

        let key = match Key::open(KEY_QUERY_VALUE | KEY_SET_VALUE)? {
            Some(key) => {
                if observed.value == Value::MissingKey {
                    return Err(StartupError::InvalidTarget);
                }
                key
            }
            None => {
                if observed.value != Value::MissingKey || !registered {
                    return Err(StartupError::InvalidTarget);
                }
                // Recheck native identity before even creating the fixed key.
                if current_identity()? != observed.identity {
                    return Err(StartupError::InvalidTarget);
                }
                Key::create_missing()?
            }
        };
        let fresh = key.query()?;
        let matches = if observed.value == Value::MissingKey {
            fresh == Value::MissingEntry
        } else {
            fresh == observed.value
        };
        if !matches || current_identity()? != observed.identity {
            return Err(StartupError::InvalidTarget);
        }
        // SAFETY: fixed HKCU key/value, owned handle, exact bounded native
        // command. Foreign/malformed bytes and identities never reach here.
        // External writers can still race between this comparison and effect.
        let status = unsafe {
            if registered {
                RegSetValueExW(
                    key.0,
                    RUN_VALUE,
                    None,
                    REG_SZ,
                    Some(&observed.identity.command),
                )
            } else {
                RegDeleteValueW(key.0, RUN_VALUE)
            }
        };
        if status != ERROR_SUCCESS {
            return Err(if status == ERROR_FILE_NOT_FOUND {
                StartupError::InvalidTarget
            } else {
                StartupError::Unavailable
            });
        }
        drop(key);
        // Reopen the actual namespace; never infer success from the requested
        // boolean or roll back/retry a successful write with failed readback.
        let (identity, value) = read_observation().map_err(|_| StartupError::Unconfirmed)?;
        let expected = if registered {
            StartupRegistration::Registered
        } else {
            StartupRegistration::Absent
        };
        if identity != observed.identity || value.registration(&identity) != expected {
            return Err(StartupError::Unconfirmed);
        }
        Ok(self.publish(identity, value))
    }
}

fn read_observation() -> Result<(Identity, Value), StartupError> {
    let identity = current_identity()?;
    let value = match Key::open(KEY_QUERY_VALUE)? {
        Some(key) => key.query()?,
        None => Value::MissingKey,
    };
    if current_identity()? != identity {
        return Err(StartupError::Unavailable);
    }
    Ok((identity, value))
}

struct Key(HKEY);

impl Key {
    fn open(access: REG_SAM_FLAGS) -> Result<Option<Self>, StartupError> {
        let mut key = HKEY::default();
        // SAFETY: fixed current-user location, ordinary view, no enumeration.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(StartupError::Unavailable);
        }
        Ok(Some(Self(key)))
    }

    fn create_missing() -> Result<Self, StartupError> {
        let mut key = HKEY::default();
        let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
        // SAFETY: only an explicitly enabled, freshly observed missing fixed
        // HKCU key is created. Default security, no elevation or policy bypass.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                None,
                &mut key,
                Some(&mut disposition),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(StartupError::Unavailable);
        }
        let key = Self(key);
        if disposition != REG_CREATED_NEW_KEY {
            return Err(StartupError::InvalidTarget);
        }
        Ok(key)
    }

    fn query(&self) -> Result<Value, StartupError> {
        let mut bytes = [0_u8; REGISTRY_BYTES];
        let mut length = bytes.len() as u32;
        let mut kind = REG_VALUE_TYPE::default();
        // SAFETY: one bounded query into a live byte buffer; type and exact
        // returned bytes are preserved. No expansion or permissive decoding.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                RUN_VALUE,
                None,
                Some(&mut kind),
                Some(bytes.as_mut_ptr()),
                Some(&mut length),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(Value::MissingEntry);
        }
        if status == ERROR_MORE_DATA {
            return Ok(Value::Foreign);
        }
        if status != ERROR_SUCCESS {
            return Err(StartupError::Unavailable);
        }
        if length as usize > bytes.len() {
            return Ok(Value::Foreign);
        }
        Ok(Value::Bytes {
            kind,
            bytes: bytes[..length as usize].to_vec(),
        })
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: sole owner of a successfully opened/created registry handle.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn current_identity() -> Result<Identity, StartupError> {
    let mut filename = [0_u16; RUN_COMMAND_UNITS + 1];
    // SAFETY: current process image only; SDK writes into the bounded slice.
    let length = unsafe { GetModuleFileNameW(None, &mut filename) } as usize;
    if length == 0 || length >= filename.len() || filename[length] != 0 {
        return Err(StartupError::Unavailable);
    }
    let path = String::from_utf16(&filename[..length]).map_err(|_| StartupError::Unavailable)?;
    if length + 2 > RUN_COMMAND_UNITS
        || !std::path::Path::new(&path).is_absolute()
        || !path
            .rsplit('\\')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("tessera-desktop.exe"))
        || path
            .chars()
            .any(|character| character == '"' || character.is_control())
    {
        return Err(StartupError::Unavailable);
    }

    // SAFETY: the SDK's current-process command line is NUL-terminated and
    // valid for process lifetime. Bound the copy by Windows' command-line cap;
    // never call unbounded PCWSTR::to_string or trust UI/environment strings.
    let source = unsafe { GetCommandLineW() };
    if source.is_null() {
        return Err(StartupError::Unavailable);
    }
    let mut process_command = Vec::new();
    for index in 0..=PROCESS_COMMAND_UNITS {
        // SAFETY: within the SDK's documented maximum, stopping at first NUL.
        let unit = unsafe { *source.0.add(index) };
        process_command.push(unit);
        if unit == 0 {
            break;
        }
    }
    if process_command.len() <= 1 || process_command.last() != Some(&0) {
        return Err(StartupError::Unavailable);
    }
    String::from_utf16(&process_command[..process_command.len() - 1])
        .map_err(|_| StartupError::Unavailable)?;
    let mut count = 0;
    // SAFETY: owned, checked, bounded and NUL-terminated command-line buffer.
    let arguments = unsafe { CommandLineToArgvW(PCWSTR(process_command.as_ptr()), &mut count) };
    if arguments.is_null() {
        return Err(StartupError::Unavailable);
    }
    // SAFETY: CommandLineToArgvW returns one LocalAlloc allocation to free.
    let remaining = unsafe { LocalFree(Some(HLOCAL(arguments.cast()))) };
    if !remaining.0.is_null() || count != 1 {
        // No heartbeat, diagnostic, supervisor, CLI or guessed --desktop args.
        return Err(StartupError::Unavailable);
    }
    let command: Vec<u8> = std::iter::once('"' as u16)
        .chain(filename[..length].iter().copied())
        .chain(['"' as u16, 0])
        .flat_map(u16::to_le_bytes)
        .collect();
    Ok(Identity {
        command,
        process_command,
    })
}

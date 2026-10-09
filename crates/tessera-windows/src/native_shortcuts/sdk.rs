// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! SDK shapes audited against windows-sys 0.61.2. Microsoft documents that LL
//! hooks execute on the installing thread, before changed-key async state is
//! updated: <https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc>
//! Reserved Win chords use actual RegisterHotKey results, never hook bypass:
//! <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey>

use super::reducer::{BareWinReducer, KeyEvent};
use super::{Backend, BackendEvent, Gate, RawTrigger, Wake, unavailable};
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex, atomic::Ordering};
use tessera_system::shortcuts::{
    KeyChord, KeyModifiers, ShortcutAction, ShortcutError, ShortcutErrorKind, TriggerPoint,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, LPARAM, LRESULT, POINT, WAIT_FAILED, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetPhysicalCursorPos, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT,
    LLKHF_ALTDOWN, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, LLKHF_UP, MSG,
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
    WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const RING_CAPACITY: usize = 32;
// Failure retention is owner-side only, never a hook callback allocation/lock.
// Process-lifetime reachable storage is safer than freeing a borrowed context
// when Windows has not confirmed unhooking. No retry or replacement is installed.
struct QuarantinedOwner {
    _context: usize,
    _hook: usize,
    _settings: Option<(i32, u64, u32, u32)>,
}

static QUARANTINED_CONTEXTS: Mutex<Vec<QuarantinedOwner>> = Mutex::new(Vec::new());

thread_local! {
    static HOOK_CONTEXT: Cell<*mut HookContext> = const { Cell::new(ptr::null_mut()) };
}

// Async state aliases and mouse buttons do not represent distinct keys whose
// releases this keyboard-only hook can balance. Seed only canonical identities.
fn snapshot_key(key: usize) -> bool {
    if !(1..=255).contains(&key) || matches!(key, 0x01 | 0x02 | 0x04..=0x06 | 0x10..=0x12) {
        return false;
    }
    // Reuse the portable assigned-VK validator rather than duplicating its SDK
    // table. Modifier sides and F12 are held-state keys, not remappable mains.
    matches!(key, 0x5b..=0x5c | 0x7b | 0xa0..=0xa5)
        || KeyChord::new(KeyModifiers::default(), key as u16).is_ok()
}

fn canonical_key(payload: &KBDLLHOOKSTRUCT) -> u32 {
    match payload.vkCode {
        0x10 => match payload.scanCode {
            0x2a => 0xa0,
            0x36 => 0xa1,
            _ => 0, // Unrecognized Shift side: fail closed, never guess.
        },
        0x11 => {
            if payload.flags & LLKHF_EXTENDED != 0 {
                0xa3
            } else {
                0xa2
            }
        }
        0x12 => {
            if payload.flags & LLKHF_EXTENDED != 0 {
                0xa5
            } else {
                0xa4
            }
        }
        key => key,
    }
}

struct Event(HANDLE);
// SAFETY: A Windows event handle supports SetEvent from arbitrary threads. Arc
// keeps the handle open across a concurrent wake; this is not a hook/context.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: This is the one owned handle, with no surviving Arc borrowers.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct HookContext {
    gate: Arc<Gate>,
    reducer: BareWinReducer,
    enabled: bool,
    faulted: bool,
    ring: [Option<RawTrigger>; RING_CAPACITY],
    head: usize,
    length: usize,
}

impl HookContext {
    fn reset_snapshot(&mut self, generation: u64, held: [bool; 256], enabled: bool) {
        // The snapshot proves only down, never hardware origin. The reducer's
        // unknown-origin ledger prevents any gesture until balanced safely;
        // ambiguous releases request owner-side retirement and explicit rearm.
        self.enabled = enabled;
        self.reducer.reset(enabled, generation, held);
    }

    fn next_event(&mut self) -> Result<Option<BackendEvent>, ShortcutError> {
        if self.faulted {
            return Err(unavailable());
        }
        if self.reducer.rearm_required() {
            return Ok(Some(BackendEvent::RearmRequired));
        }
        Ok(self.pop().map(BackendEvent::Triggered))
    }

    fn pop(&mut self) -> Option<RawTrigger> {
        if self.length == 0 {
            return None;
        }
        let trigger = self.ring[self.head].take();
        self.head = (self.head + 1) % RING_CAPACITY;
        self.length -= 1;
        trigger
    }

    fn clear(&mut self) {
        self.enabled = false;
        self.reducer.reset(false, 0, [false; 256]);
        while let Some(trigger) = self.pop() {
            if trigger.reserved {
                self.gate.events.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    fn key(&mut self, payload: KBDLLHOOKSTRUCT, message: u32) -> bool {
        let known_message = matches!(message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP);
        let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
        let known_flags =
            LLKHF_ALTDOWN | LLKHF_EXTENDED | LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED | LLKHF_UP;
        let active = self.enabled
            && self.gate.listening.load(Ordering::Acquire)
            && !self.gate.closed.load(Ordering::Acquire);
        let generation = self.reducer.key(KeyEvent {
            key: canonical_key(&payload),
            down,
            injected: payload.flags & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0,
            unknown: !active
                || !known_message
                || payload.flags & !known_flags != 0
                || (payload.flags & LLKHF_UP != 0) == down
                || payload.flags & LLKHF_ALTDOWN != 0,
        });
        let Some(generation) = generation else {
            return false;
        };
        if !self.enabled
            || self.faulted
            || self.length == RING_CAPACITY
            || self.gate.closed.load(Ordering::Acquire)
            || !self.gate.listening.load(Ordering::Acquire)
            || self.gate.generation.load(Ordering::Acquire) != generation
        {
            return false;
        }
        // Reserve the eventual relay slot before admitting release suppression.
        // Only atomic operations and fixed storage occur inside the callback.
        if super::actor::reserve_event_slot(&self.gate.events).is_err() {
            return false;
        }
        if self.gate.closed.load(Ordering::Acquire)
            || !self.gate.listening.load(Ordering::Acquire)
            || self.gate.generation.load(Ordering::Acquire) != generation
        {
            self.gate.events.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        let index = (self.head + self.length) % RING_CAPACITY;
        self.ring[index] = Some(RawTrigger {
            generation,
            action: ShortcutAction::ToggleLauncher,
            reserved: true,
        });
        self.length += 1;
        true
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        // SAFETY: Required negative-code passthrough; no context access.
        return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
    }
    let handled = catch_unwind(AssertUnwindSafe(|| {
        HOOK_CONTEXT.with(|slot| {
            let context = slot.get();
            if context.is_null() {
                return false;
            }
            if code != HC_ACTION as i32 || lparam == 0 {
                // No payload may be copied for an unknown hook code. Cancel
                // conservatively until an authoritative owner-side rearm.
                unsafe {
                    (&mut *context).reducer.key(KeyEvent {
                        key: 0,
                        down: false,
                        injected: false,
                        unknown: true,
                    });
                }
                return false;
            }
            // SAFETY: Windows supplies this payload for HC_ACTION. The context
            // allocation stays owned/reachable until checked unhook on this same
            // installing thread. No SDK call occurs while this borrow is live.
            let payload = unsafe { (lparam as *const KBDLLHOOKSTRUCT).read() };
            unsafe { (&mut *context).key(payload, u32::try_from(wparam).unwrap_or(u32::MAX)) }
        })
    }));
    match handled {
        Ok(true) => 1,
        Ok(false) => unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) },
        Err(_) => {
            // Contain unwind at FFI. Report outside the callback on the next pump
            // turn, and never suppress another key after a reducer failure.
            let _ = HOOK_CONTEXT.try_with(|slot| {
                let context = slot.get();
                if !context.is_null() {
                    unsafe {
                        (*context).enabled = false;
                        (*context).faulted = true;
                    }
                }
            });
            unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
        }
    }
}

pub(super) struct SdkBackend {
    context: NonNull<HookContext>,
    event: Arc<Event>,
    hook: HHOOK,
    settings: Option<(i32, u64, u32, u32)>,
    next_id: i32,
    poisoned: bool,
}

impl SdkBackend {
    pub(super) fn new(gate: Arc<Gate>) -> Result<Self, ShortcutError> {
        // SAFETY: Fixed unnamed auto-reset event, with no security attributes.
        let handle = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if handle.is_null() {
            return Err(last_error());
        }
        let context = Box::new(HookContext {
            gate,
            reducer: BareWinReducer::new(),
            enabled: false,
            faulted: false,
            ring: [None; RING_CAPACITY],
            head: 0,
            length: 0,
        });
        // Non-null by construction; raw ownership avoids retaining a Rust borrow
        // across SDK functions that synchronously invoke the thread's callback.
        let context = unsafe { NonNull::new_unchecked(Box::into_raw(context)) };
        let mut message = MSG::default();
        // SAFETY: Creates the current owner thread's queue; no input generation.
        unsafe {
            PeekMessageW(&mut message, ptr::null_mut(), 0, 0, 0);
        }
        Ok(Self {
            context,
            event: Arc::new(Event(handle)),
            hook: ptr::null_mut(),
            settings: None,
            next_id: 0,
            poisoned: false,
        })
    }

    fn pop(&mut self) -> Result<Option<BackendEvent>, ShortcutError> {
        // SAFETY: Transient access outside native calls on the installing thread.
        unsafe { self.context.as_mut().next_event() }
    }

    fn seed(&mut self, generation: u64) {
        let mut held = [false; 256];
        // Finite owner-side snapshot only on explicit configuration, never in a
        // callback or idle timer. Mouse keys/aliases cannot be balanced here.
        for (key, down) in held.iter_mut().enumerate().skip(1) {
            if !snapshot_key(key) {
                continue;
            }
            // SAFETY: Bounded read-only state query on this native owner thread.
            *down = unsafe { GetAsyncKeyState(key as i32) } < 0;
        }
        // SAFETY: No native call while this context borrow is live.
        let context = unsafe { self.context.as_mut() };
        context.reset_snapshot(generation, held, true);
    }
}

impl Backend for SdkBackend {
    fn wake(&self) -> Wake {
        let event = self.event.clone();
        Arc::new(move || {
            // SAFETY: The closure's Arc prevents a concurrent event close.
            let _ = unsafe { SetEvent(event.0) };
        })
    }

    fn install_launcher(&mut self, generation: u64) -> Result<(), ShortcutError> {
        if self.poisoned || !self.hook.is_null() {
            return Err(unavailable());
        }
        if HOOK_CONTEXT.with(|slot| !slot.get().is_null()) {
            return Err(unavailable());
        }
        // The callback stays disabled until one finite post-install snapshot.
        // No initial state is labeled as callback-observed physical input.
        unsafe {
            self.context.as_mut().clear();
        }
        HOOK_CONTEXT.with(|slot| slot.set(self.context.as_ptr()));
        // SAFETY: The genuine SDK callback has the HOOKPROC ABI. It is global,
        // low-level, installed and pumped exclusively on this dedicated thread.
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                GetModuleHandleW(ptr::null()),
                0,
            )
        };
        if hook.is_null() {
            let error = last_error();
            HOOK_CONTEXT.with(|slot| slot.set(ptr::null_mut()));
            unsafe {
                self.context.as_mut().clear();
            }
            return Err(error);
        }
        self.hook = hook;
        // Seed unknown-origin preheld keys once outside the callback. Their
        // first release cannot toggle. Ambiguous origin causes RearmRequired,
        // not a permanently fake Registered status or a synthetic-key bypass.
        self.seed(generation);
        Ok(())
    }

    fn register_settings(
        &mut self,
        chord: &KeyChord,
        generation: u64,
    ) -> Result<(), ShortcutError> {
        if self.poisoned || self.settings.is_some() {
            return Err(unavailable());
        }
        chord.validate()?;
        let KeyChord::Chord { modifiers, key } = chord else {
            return Err(ShortcutError::new(
                ShortcutErrorKind::InvalidData,
                None,
                "Settings requires a chord",
            ));
        };
        let Some(id) = self.next_id.checked_add(1).filter(|id| *id <= 0xbfff) else {
            self.poisoned = true;
            return Err(ShortcutError::new(
                ShortcutErrorKind::Other,
                None,
                "Native hotkey identifiers exhausted",
            ));
        };
        self.next_id = id;
        let mut flags = MOD_NOREPEAT;
        if modifiers.control {
            flags |= MOD_CONTROL;
        }
        if modifiers.alt {
            flags |= MOD_ALT;
        }
        if modifiers.shift {
            flags |= MOD_SHIFT;
        }
        if modifiers.win {
            flags |= MOD_WIN;
        }
        // SAFETY: Fixed app-owned unique ID on the installing thread. Reserved
        // OS Win chords are not bypassed; the actual SDK error is authoritative.
        if unsafe { RegisterHotKey(ptr::null_mut(), id, flags, u32::from(*key)) } == 0 {
            return Err(last_error());
        }
        self.settings = Some((id, generation, flags & !MOD_NOREPEAT, u32::from(*key)));
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), ShortcutError> {
        // Disable delivery before either native retirement operation. Transient
        // callbacks during unhook still see reachable, disabled context storage.
        unsafe {
            self.context.as_mut().clear();
        }
        let mut error = None;
        if let Some((id, _, _, _)) = self.settings {
            // SAFETY: Unregister exactly the ID owned by this installing thread.
            if unsafe { UnregisterHotKey(ptr::null_mut(), id) } == 0 {
                error = Some(last_error());
            } else {
                self.settings = None;
            }
        }
        if !self.hook.is_null() {
            // SAFETY: Context storage remains alive during this checked unhook.
            if unsafe { UnhookWindowsHookEx(self.hook) } == 0 {
                error.get_or_insert_with(last_error);
            } else {
                self.hook = ptr::null_mut();
                HOOK_CONTEXT.with(|slot| slot.set(ptr::null_mut()));
            }
        }
        if let Some(error) = error {
            self.poisoned = true;
            Err(error)
        } else {
            Ok(())
        }
    }

    fn wait(&mut self) -> Result<Option<BackendEvent>, ShortcutError> {
        if let Some(trigger) = self.pop()? {
            return Ok(Some(trigger));
        }
        let handle = self.event.0;
        // SAFETY: One live event plus the owner's message queue, infinite idle
        // wait (not polling). Sent LL callbacks are dispatched by the pump.
        if unsafe {
            MsgWaitForMultipleObjectsEx(1, &handle, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        } == WAIT_FAILED
        {
            return Err(last_error());
        }
        let mut message = MSG::default();
        for _ in 0..64 {
            // SAFETY: Retrieves current-thread messages; may dispatch LL hook
            // sent messages. No context borrow exists across this native call.
            let has_message =
                unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) } != 0;
            if !has_message {
                return self.pop();
            }
            if message.message == WM_QUIT {
                return Err(unavailable());
            }
            if message.message == WM_HOTKEY {
                if let Some((id, generation, modifiers, key)) = self.settings {
                    let packed = message.lParam as u32;
                    if message.wParam == id as usize
                        && packed & 0xffff == modifiers
                        && packed >> 16 == key
                    {
                        return Ok(Some(BackendEvent::Triggered(RawTrigger {
                            generation,
                            action: ShortcutAction::OpenSettings,
                            reserved: false,
                        })));
                    }
                }
            } else {
                // SAFETY: Ordinary messages receive their ordinary dispatch.
                unsafe {
                    DispatchMessageW(&message);
                }
            }
            if let Some(trigger) = self.pop()? {
                return Ok(Some(trigger));
            }
        }
        Ok(None)
    }

    fn cursor(&mut self) -> Option<TriggerPoint> {
        let mut point = POINT::default();
        // SAFETY: Initialized output; the physical-screen API matches monitor
        // bounds without depending on the owner's DPI context. Failure stays
        // Unknown, with no conversion or primary-monitor fallback.
        // https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getphysicalcursorpos
        (unsafe { GetPhysicalCursorPos(&mut point) } != 0).then_some(TriggerPoint {
            x: point.x,
            y: point.y,
        })
    }
}

impl Drop for SdkBackend {
    fn drop(&mut self) {
        let cleanup = self.cleanup();
        if cleanup.is_ok() && self.hook.is_null() {
            // Checked unhook and same-thread callback serialization prove no
            // remaining callback borrow of this allocation.
            unsafe {
                drop(Box::from_raw(self.context.as_ptr()));
            }
        } else {
            QUARANTINED_CONTEXTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(QuarantinedOwner {
                    _context: self.context.as_ptr() as usize,
                    _hook: self.hook as usize,
                    _settings: self.settings,
                });
            // Keep the disabled allocation process-lifetime reachable. The
            // thread may retire, but no borrowed storage is prematurely freed.
        }
    }
}

fn last_error() -> ShortcutError {
    // SAFETY: Immediately follows the failing SDK call on the same thread.
    let code = unsafe { GetLastError() };
    let kind = match code {
        5 => ShortcutErrorKind::AccessDenied,
        1409 => ShortcutErrorKind::Conflict,
        8 | 14 | 1450 => ShortcutErrorKind::Busy,
        _ => ShortcutErrorKind::Other,
    };
    ShortcutError::new(kind, Some(code), "Native shortcut operation failed")
}

#[cfg(test)]
#[path = "callback_tests.rs"]
mod callback_tests;

//! How often the windows of the shell are asked to draw.
//!
//! GPUI asks every window for a frame at every refresh of the display,
//! whether or not anything changed. Each asking wakes the thread of the
//! windows, and with a dozen windows on fast displays that is some two
//! thousand wakings a second while nothing moves. Here the askings are let
//! through at the full rate only while something moves, and a few times a
//! second otherwise, so that what a timer changes still shows at once to
//! the eye.
//!
//! GPUI offers no way to say so, and the askings come from a thread of its
//! own. They are therefore caught where the program calls `RedrawWindow`:
//! the address of that function in the import table of the program is
//! replaced with [`paced`]. Where the address is not found nothing is
//! replaced and every asking goes through, as before.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, MSG, SetWindowsHookExW, WH_GETMESSAGE, WM_KEYFIRST, WM_KEYLAST, WM_MOUSEFIRST,
    WM_MOUSELAST, WM_NCMOUSEMOVE, WM_NCXBUTTONDBLCLK,
};
use windows::core::{s, w};

/// For how long the windows are asked at the full rate after something
/// moved.
const AWAKE: Duration = Duration::from_millis(400);

/// How far apart the askings are let through while nothing moves.
const REST: Duration = Duration::from_millis(48);

/// For how long the askings are let through once they are: long enough for
/// every window to be asked in the same refresh, shorter than a refresh.
const GATE: Duration = Duration::from_millis(2);

/// The flags GPUI asks a frame with: `RDW_INVALIDATE` alone.
const ASKS_A_FRAME: u32 = 1;

/// `WM_MOUSELEAVE` and the messages of a pen or a finger, which the
/// ranges of the mouse do not hold.
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_POINTER_FIRST: u32 = 0x0241;
const WM_POINTER_LAST: u32 = 0x024F;

type Redraw = unsafe extern "system" fn(HWND, *const c_void, *mut c_void, u32) -> i32;

/// The `RedrawWindow` of Windows, once [`paced`] stands in its place.
static REDRAW: AtomicUsize = AtomicUsize::new(0);

/// The thread of the windows, whose own askings are never held back.
static WINDOWS_THREAD: AtomicU64 = AtomicU64::new(0);

/// Until when the askings go through at the full rate, and until when the
/// gate that was last opened stays open; in microseconds since [`start`].
static AWAKE_UNTIL: AtomicU64 = AtomicU64::new(0);
static GATE_UNTIL: AtomicU64 = AtomicU64::new(0);

fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn now() -> u64 {
    start().elapsed().as_micros() as u64
}

/// The pace of the frames of the shell.
pub struct Pace;

impl Pace {
    /// Begins to hold the askings back. Called once, on the thread of the
    /// windows, before any window is opened.
    pub fn keep() {
        start();
        // SAFETY: the calls have no preconditions; the import table that is
        // walked is that of this program, which stays loaded.
        unsafe {
            WINDOWS_THREAD.store(u64::from(GetCurrentThreadId()), Ordering::Relaxed);
            let Ok(user) = GetModuleHandleW(w!("user32.dll")) else {
                return;
            };
            let Some(redraw) = GetProcAddress(user, s!("RedrawWindow")) else {
                return;
            };
            let Ok(program) = GetModuleHandleW(None) else {
                return;
            };
            REDRAW.store(redraw as usize, Ordering::SeqCst);
            if !replace(
                program.0 as *const u8,
                redraw as usize,
                paced as *const () as usize,
            ) {
                return;
            }
            // What the pointer and the keys do is seen here, where the
            // thread takes its messages.
            let _ = SetWindowsHookExW(WH_GETMESSAGE, Some(taken), None, GetCurrentThreadId());
        }
        Self::quicken();
    }

    /// Says that something moves, so that the windows are asked at the
    /// full rate for a while. Cheap, and called at every frame of what is
    /// animated.
    pub fn quicken() {
        AWAKE_UNTIL.store(now() + AWAKE.as_micros() as u64, Ordering::Relaxed);
    }
}

/// Puts `with` wherever the import table of the program at `image` holds
/// `what`. Whether it was found.
///
/// # Safety
///
/// `image` must be the base of a loaded 64-bit program.
unsafe fn replace(image: *const u8, what: usize, with: usize) -> bool {
    let read = |at: usize| -> u32 {
        // SAFETY: the offsets read lie within the headers and the import
        // directory of the loaded program.
        unsafe { image.add(at).cast::<u32>().read_unaligned() }
    };
    let headers = read(0x3C) as usize;
    // The data directories of a 64-bit program begin 112 bytes into its
    // optional header, which follows 24 bytes of signature and file header;
    // the second directory is that of the imports.
    let imports = headers + 24 + 112 + 8;
    let (mut entry, size) = (read(imports) as usize, read(imports + 4) as usize);
    if entry == 0 {
        return false;
    }
    let end = entry + size;
    let mut found = false;
    // An entry is twenty bytes, the last four the place of its addresses,
    // and an entry of zeroes ends them.
    while entry + 20 <= end && read(entry + 12) != 0 {
        // SAFETY: the table of addresses of an import entry ends with zero.
        let mut slot = unsafe { image.add(read(entry + 16) as usize) } as *mut usize;
        // SAFETY: as above; the page is made writable for the one write.
        unsafe {
            while slot.read() != 0 {
                if slot.read() == what {
                    let mut old = PAGE_PROTECTION_FLAGS(0);
                    if VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old).is_ok() {
                        slot.write(with);
                        let _ = VirtualProtect(slot.cast(), 8, old, &mut old);
                        found = true;
                    }
                }
                slot = slot.add(1);
            }
        }
        entry += 20;
    }
    found
}

/// Stands in the place of `RedrawWindow`.
unsafe extern "system" fn paced(
    window: HWND,
    area: *const c_void,
    region: *mut c_void,
    flags: u32,
) -> i32 {
    // SAFETY: the address was that of `RedrawWindow` when it was stored.
    let redraw: Redraw = unsafe { std::mem::transmute(REDRAW.load(Ordering::SeqCst)) };
    // SAFETY: no preconditions.
    let thread = u64::from(unsafe { GetCurrentThreadId() });
    let asks_a_frame = flags == ASKS_A_FRAME && area.is_null() && region.is_null();
    if asks_a_frame && thread != WINDOWS_THREAD.load(Ordering::Relaxed) && !goes_through(now()) {
        return 1;
    }
    // SAFETY: the arguments are passed on as they came.
    unsafe { redraw(window, area, region, flags) }
}

/// Whether an asking at `now` is let through.
fn goes_through(now: u64) -> bool {
    if now < AWAKE_UNTIL.load(Ordering::Relaxed) || now < GATE_UNTIL.load(Ordering::Relaxed) {
        return true;
    }
    // The gate was closed for a rest: it opens for one refresh.
    let opened = GATE_UNTIL.load(Ordering::Relaxed);
    let rested = opened + (REST - GATE).as_micros() as u64;
    if now >= rested {
        GATE_UNTIL.store(now + GATE.as_micros() as u64, Ordering::Relaxed);
        return true;
    }
    false
}

/// Sees each message the thread of the windows takes from its queue.
unsafe extern "system" fn taken(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && lparam.0 != 0 {
        // SAFETY: for this hook `lparam` points at the message taken.
        let message = unsafe { (*(lparam.0 as *const MSG)).message };
        let moves = (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&message)
            || (WM_KEYFIRST..=WM_KEYLAST).contains(&message)
            || (WM_NCMOUSEMOVE..=WM_NCXBUTTONDBLCLK).contains(&message)
            || (WM_POINTER_FIRST..=WM_POINTER_LAST).contains(&message)
            || message == WM_MOUSELEAVE;
        if moves {
            Pace::quicken();
        }
    }
    // SAFETY: passing the message on as it came.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn askings_are_far_apart_while_nothing_moves() {
        AWAKE_UNTIL.store(0, Ordering::Relaxed);
        GATE_UNTIL.store(0, Ordering::Relaxed);
        let refresh = 6_000_usize;
        let second = 1_000_000_usize;
        let through = (second..2 * second)
            .step_by(refresh)
            .filter(|&now| goes_through(now as u64))
            .count();
        // One refresh in every 48 ms, of the 166 in the second.
        assert!((18..=22).contains(&through), "{through}");

        // Every window is asked in the refresh that goes through.
        GATE_UNTIL.store(0, Ordering::Relaxed);
        assert!(goes_through(3 * second as u64));
        assert!(goes_through(3 * second as u64 + 500));
        assert!(!goes_through(3 * second as u64 + refresh as u64));

        AWAKE_UNTIL.store(5 * second as u64, Ordering::Relaxed);
        let through = (4 * second..5 * second)
            .step_by(refresh)
            .filter(|&now| goes_through(now as u64))
            .count();
        assert_eq!(through, second.div_ceil(refresh));
    }
}

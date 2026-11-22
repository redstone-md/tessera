// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Windows where the wallpaper is that a video plays in, again and again
//! and without its sound.
//!
//! The windows lie where a [`DesktopBackdrop`](super::DesktopBackdrop)
//! does, one over each display. The media engine of Windows reads the
//! file and decodes it once, with the graphics card where that can; a
//! thread of its own has each frame scaled into every window by the
//! graphics card. Nothing of a frame passes through the memory of
//! Tessera, and a video on three displays is decoded as often as one on
//! one.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, RECT, S_OK, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionEffectGroup, IDCompositionTarget,
    IDCompositionVisual,
};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGIOutput,
    IDXGISwapChain1,
};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine,
    IMFMediaEngineClassFactory, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl,
    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER, MF_MEDIA_ENGINE_EVENT_ERROR,
    MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA, MF_MEDIA_ENGINE_FORCEMUTE,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION, MFARGB, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFSTARTUP_LITE, MFStartup, MFVideoNormalizedRect,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, LWA_ALPHA, SW_SHOWNA, SetLayeredWindowAttributes, ShowWindow,
    WM_ERASEBKGND,
};
use windows::core::{BSTR, HRESULT, Interface, implement, w};

use super::ScreenRect;
use super::backdrop::{backdrops_changed, open_behind_icons};

/// The engine knows the size of the video.
const MEASURED: u32 = 1;
/// A frame was drawn into the windows.
const SHOWING: u32 = 2;
/// The video cannot be played.
const FAILED: u32 = 4;

/// How long the thread that draws sleeps while no display shows the
/// video, and the longest the thread of the interface waits for it to
/// end.
const IDLE: Duration = Duration::from_millis(100);

/// How long the thread that draws sleeps where it cannot wait for the
/// compositor.
const FRAME: Duration = Duration::from_millis(8);

/// What the thread of the interface and the threads of the video tell
/// each other.
#[derive(Default)]
struct Told {
    events: AtomicU32,
    /// Why the engine gave up, as Windows numbers it.
    error: AtomicU32,
    /// Why the video could not be set up.
    reason: Mutex<Option<String>>,
    /// The video is to end.
    stop: AtomicBool,
    /// The displays, counted by bits, that show nothing of the video.
    hidden: AtomicU32,
    /// How much of the video is shown over what was there, of 255.
    shown: AtomicU32,
}

#[implement(IMFMediaEngineNotify)]
struct Listener(Arc<Told>);

impl IMFMediaEngineNotify_Impl for Listener_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> windows::core::Result<()> {
        let told = &self.0;
        let event = event as i32;
        if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 {
            // The second number is the error of Windows, where there is
            // one; the first only says which kind of failure it was.
            let error = if param2 != 0 { param2 } else { param1 as u32 };
            told.error.store(error, Ordering::Relaxed);
            told.events.fetch_or(FAILED, Ordering::Relaxed);
        } else if event == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 {
            told.events.fetch_or(MEASURED, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// How far a video has come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoState {
    /// The file is being read.
    Loading,
    /// Frames are drawn.
    Playing,
    /// The file cannot be played, and why.
    Failed(String),
}

/// The part of a video of `width` by `height` that covers a window of
/// `over_width` by `over_height` when scaled, the rest cut off evenly on
/// two sides: its left, top, right and bottom as shares of the video.
fn covering_share(width: u32, height: u32, over_width: u32, over_height: u32) -> [f32; 4] {
    let (wide, high) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let (over_wide, over_high) = (f64::from(over_width.max(1)), f64::from(over_height.max(1)));
    let (shape, over_shape) = (wide / high, over_wide / over_high);
    if shape > over_shape {
        // Wider in shape: the sides go.
        let cut = ((1. - over_shape / shape) / 2.) as f32;
        [cut, 0., 1. - cut, 1.]
    } else {
        let cut = ((1. - shape / over_shape) / 2.) as f32;
        [0., cut, 1., 1. - cut]
    }
}

/// Whether the media library of Windows could be started, which is done
/// once and holds for as long as the program runs.
fn media_started() -> bool {
    static STARTED: OnceLock<bool> = OnceLock::new();
    // SAFETY: the call takes numbers; the library is never shut down.
    *STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE).is_ok() })
}

/// A window the thread that draws is given: its handle as a number, and
/// its size.
#[derive(Clone, Copy)]
struct Target {
    window: isize,
    width: i32,
    height: i32,
}

/// What the thread that draws keeps for a window.
struct Screen {
    chain: IDXGISwapChain1,
    buffer: ID3D11Texture2D,
    /// What binds the frames to the window, kept for as long as they
    /// are shown.
    _bound: (IDCompositionTarget, IDCompositionVisual),
    target: Target,
    /// The part of the video shown, once the size of the video is known.
    part: Option<MFVideoNormalizedRect>,
}

/// Plays the video in the file `path` into the windows of `targets` until
/// told to stop. Runs on a thread of its own.
fn draw(targets: &[Target], path: &Path, told: &Arc<Told>) -> Result<()> {
    if !media_started() {
        bail!("the media library of Windows does not start");
    }
    // SAFETY: the objects are made and used on this thread with arguments
    // that outlive the calls; the windows outlive the thread, which the
    // thread of the interface waits for before it destroys them.
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            None,
        )
        .context("the graphics card gives no device for video")?;
        let device = device.context("the graphics card gives no device")?;
        // The engine decodes on threads of its own with this device.
        let _ = device
            .cast::<ID3D11Multithread>()?
            .SetMultithreadProtected(true);
        let mut token = 0;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&raw mut token, &raw mut manager)?;
        let manager = manager.context("Windows shares no device with the media engine")?;
        manager.ResetDevice(&device, token)?;

        let shared = device.cast::<IDXGIDevice>()?;
        let factory: IDXGIFactory2 = shared.GetAdapter()?.GetParent()?;
        // The compositor of Windows shows the frames in the windows: a
        // window that is a layer of the desktop shows none drawn into it
        // directly.
        let composition: IDCompositionDevice = DCompositionCreateDevice(&shared)
            .context("the compositor of Windows takes no frames")?;
        // One for all the windows: they come into sight together.
        let fade = composition.CreateEffectGroup()?;
        fade.SetOpacity2(0.)?;
        let mut screens = Vec::new();
        for &target in targets {
            let description = DXGI_SWAP_CHAIN_DESC1 {
                Width: target.width.unsigned_abs(),
                Height: target.height.unsigned_abs(),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let chain = factory
                .CreateSwapChainForComposition(
                    &device,
                    &raw const description,
                    None::<&IDXGIOutput>,
                )
                .context("the graphics card gives no frames for a wallpaper window")?;
            let bound = composition
                .CreateTargetForHwnd(HWND(target.window as *mut _), true)
                .context("a wallpaper window takes no frames of the compositor")?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&chain)?;
            visual.SetEffect(&fade)?;
            bound.SetRoot(&visual)?;
            let buffer: ID3D11Texture2D = chain.GetBuffer(0)?;
            screens.push(Screen {
                chain,
                buffer,
                _bound: (bound, visual),
                target,
                part: None,
            });
        }
        composition.Commit()?;

        let classes: IMFMediaEngineClassFactory =
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                .context("Windows has no media engine")?;
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&raw mut attributes, 3)?;
        let attributes = attributes.context("the media engine takes no settings")?;
        let listener: IMFMediaEngineNotify = Listener(told.clone()).into();
        attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &listener)?;
        attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager)?;
        attributes.SetUINT32(
            &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
            DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
        )?;
        // No sound is asked of the engine at all.
        let engine = classes
            .CreateInstance(MF_MEDIA_ENGINE_FORCEMUTE.0 as u32, &attributes)
            .context("the media engine could not be made")?;
        let played = play(&engine, &mut screens, (&composition, &fade), path, told);
        let _ = engine.Shutdown();
        played
    }
}

/// Has `engine` play the file `path` and draws its frames into `screens`
/// until told to stop or the engine gives up, and has `fade` show as
/// much of them as was asked for.
///
/// # Safety
///
/// The windows of `screens` are alive.
unsafe fn play(
    engine: &IMFMediaEngine,
    screens: &mut [Screen],
    fade: (&IDCompositionDevice, &IDCompositionEffectGroup),
    path: &Path,
    told: &Told,
) -> Result<()> {
    let black = MFARGB {
        rgbBlue: 0,
        rgbGreen: 0,
        rgbRed: 0,
        rgbAlpha: 255,
    };
    // SAFETY: the arguments outlive the calls; see the function.
    unsafe {
        engine.SetMuted(true)?;
        engine.SetLoop(true)?;
        engine.SetAutoPlay(true)?;
        engine.SetSource(&BSTR::from(path.to_string_lossy().as_ref()))?;

        let all = u32::MAX >> (32 - screens.len().clamp(1, 32));
        let mut paused = false;
        let mut faded = 0;
        while !told.stop.load(Ordering::Relaxed) {
            if told.events.load(Ordering::Relaxed) & FAILED != 0 {
                return Ok(());
            }
            let shown = told.shown.load(Ordering::Relaxed);
            if shown != faded {
                let (composition, group) = fade;
                if group.SetOpacity2(shown as f32 / 255.).is_ok() && composition.Commit().is_ok() {
                    faded = shown;
                }
            }
            let hidden = told.hidden.load(Ordering::Relaxed);
            if hidden & all == all {
                // Nobody sees the video: it is not decoded.
                if !paused {
                    paused = engine.Pause().is_ok();
                }
                std::thread::sleep(IDLE);
                continue;
            }
            if paused {
                paused = engine.Play().is_err();
            }

            // Whether there is a new frame is told by the kind of
            // success, which the wrapper of the call does not give.
            let mut time = 0_i64;
            let tick: HRESULT = (Interface::vtable(engine).OnVideoStreamTick)(
                Interface::as_raw(engine),
                &raw mut time,
            );
            if tick == S_OK {
                let mut drawn = false;
                for (index, screen) in screens.iter_mut().enumerate() {
                    if hidden & (1 << index.min(31)) != 0 {
                        continue;
                    }
                    let Target { width, height, .. } = screen.target;
                    if screen.part.is_none() {
                        let (mut wide, mut high) = (0, 0);
                        let size =
                            engine.GetNativeVideoSize(Some(&raw mut wide), Some(&raw mut high));
                        if size.is_err() || wide == 0 || high == 0 {
                            continue;
                        }
                        let [left, top, right, bottom] =
                            covering_share(wide, high, width.unsigned_abs(), height.unsigned_abs());
                        screen.part = Some(MFVideoNormalizedRect {
                            left,
                            top,
                            right,
                            bottom,
                        });
                    }
                    let part = screen.part.as_ref().map(|part| part as *const _);
                    let window = RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    };
                    let moved = engine.TransferVideoFrame(
                        &screen.buffer,
                        part,
                        &raw const window,
                        Some(&raw const black),
                    );
                    if moved.is_ok() && screen.chain.Present(0, DXGI_PRESENT(0)).is_ok() {
                        drawn = true;
                    }
                }
                if drawn {
                    told.events.fetch_or(SHOWING, Ordering::Relaxed);
                }
            }
            // Until the compositor has shown what there is: a frame of
            // the display.
            if DwmFlush().is_err() {
                std::thread::sleep(FRAME);
            }
        }
        Ok(())
    }
}

/// The wallpaper windows of a video, one for each display. They belong to
/// the thread that opened them.
pub struct DesktopVideo {
    windows: Vec<HWND>,
    told: Arc<Told>,
    thread: Option<JoinHandle<()>>,
    /// Whether the windows were shown.
    revealed: Cell<bool>,
}

impl DesktopVideo {
    /// Opens a window over each of the displays at `frames`, not yet to
    /// be seen, and begins to read the video in the file `path`. Fails
    /// where the desktop is not laid out as Windows 11 24H2 does; what
    /// else keeps the video from playing is told by [`Self::state`].
    pub fn open(frames: &[ScreenRect], path: &Path) -> Result<Self> {
        let mut video = Self {
            windows: Vec::new(),
            told: Arc::new(Told::default()),
            thread: None,
            revealed: Cell::new(false),
        };
        let mut targets = Vec::new();
        for frame in frames {
            // Those opened before go with `video` where one fails.
            let window = open_behind_icons(*frame, w!("TesseraVideo"), respond)?;
            video.windows.push(window);
            targets.push(Target {
                window: window.0 as isize,
                width: frame.right - frame.left,
                height: frame.bottom - frame.top,
            });
        }
        let told = video.told.clone();
        let path: PathBuf = path.to_path_buf();
        let thread = std::thread::Builder::new().name("video".into());
        let thread = thread.spawn(move || {
            // SAFETY: the thread is given to the library and taken back
            // in a pair, after everything made on it is let go of.
            let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            if let Err(error) = draw(&targets, &path, &told) {
                let mut reason = told.reason.lock().unwrap_or_else(PoisonError::into_inner);
                *reason = Some(format!("{error:#}"));
                told.events.fetch_or(FAILED, Ordering::Relaxed);
            }
            if entered {
                // SAFETY: as above.
                unsafe { CoUninitialize() };
            }
        });
        video.thread = Some(thread.context("no thread could be started for the video")?);
        Ok(video)
    }

    /// How far the video has come.
    pub fn state(&self) -> VideoState {
        let events = self.told.events.load(Ordering::Relaxed);
        if events & FAILED != 0 {
            let reason = self.told.reason.lock();
            let reason = reason.unwrap_or_else(PoisonError::into_inner).clone();
            return VideoState::Failed(reason.unwrap_or_else(|| {
                let error = self.told.error.load(Ordering::Relaxed);
                let message = HRESULT(error as i32).message();
                format!("{} (0x{error:08X})", message.trim())
            }));
        }
        if events & SHOWING != 0 {
            VideoState::Playing
        } else {
            VideoState::Loading
        }
    }

    /// Shows `to` of the video, from 0 for none of it to 1 for all, over
    /// what was there.
    pub fn advance(&self, to: f32) {
        let shown = (to.clamp(0., 1.) * 255.).round() as u32;
        self.told.shown.store(shown, Ordering::Relaxed);
        if !self.revealed.replace(true) {
            for &window in &self.windows {
                // SAFETY: the window is of this thread. The compositor
                // fades what is in it; the window itself hides nothing.
                unsafe {
                    let _ = SetLayeredWindowAttributes(window, COLORREF(0), 255, LWA_ALPHA);
                    let _ = ShowWindow(window, SW_SHOWNA);
                }
            }
        }
        if to >= 1. {
            backdrops_changed();
        }
    }

    /// Says which displays, counted by bits in the order they were given
    /// in, show nothing of the video: no frame is drawn there, and where
    /// that is all of them, the video is held still and not decoded.
    pub fn hide(&self, displays: u32) {
        self.told.hidden.store(displays, Ordering::Relaxed);
    }
}

impl Drop for DesktopVideo {
    fn drop(&mut self) {
        self.told.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // It ends within a frame, or within `IDLE` where it sleeps.
            let _ = thread.join();
        }
        for &window in &self.windows {
            // SAFETY: the window belongs to this thread, and what drew
            // into it has gone with the thread.
            unsafe {
                let _ = DestroyWindow(window);
            }
        }
        backdrops_changed();
    }
}

/// The window procedure of a video window: the frames are drawn by the
/// graphics card, and nothing is erased under them.
unsafe extern "system" fn respond(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_ERASEBKGND {
        return LRESULT(1);
    }
    // SAFETY: the arguments are those the procedure was called with.
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_is_cut_evenly_to_the_shape_of_the_display() {
        assert_eq!(covering_share(1920, 1080, 2560, 1440), [0., 0., 1., 1.]);
        // A display twice as wide in shape keeps the middle half of the rows.
        assert_eq!(covering_share(1000, 1000, 2000, 1000), [0., 0.25, 1., 0.75]);
        assert_eq!(covering_share(2000, 1000, 1000, 1000), [0.25, 0., 0.75, 1.]);
        assert_eq!(covering_share(0, 0, 0, 0), [0., 0., 1., 1.]);
    }
}

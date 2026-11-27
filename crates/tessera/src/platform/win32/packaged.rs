// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Packaged (Store) applications, which have no shortcut file and often no
//! readable program file: Windows knows them by an application identifier.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, ReleaseDC,
};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItemImageFactory,
    KF_FLAG_DEFAULT, SHCreateItemFromParsingName, SHGetKnownFolderItem, SIGDN, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING, SIIGBF_ICONONLY,
};
use windows::core::{BSTR, HSTRING, PWSTR};

use super::{IconPixels, Shortcut};

/// What the shell calls the folder of every installed application; an
/// application identifier after it names one of them.
const APPS_FOLDER: &str = "shell:AppsFolder";

/// How long a list of the installed applications is given out again.
const FRESH: Duration = Duration::from_secs(30);

/// The application identifier `path` names, if it is of the form
/// `shell:AppsFolder\identifier`, with either kind of slash.
pub fn app_id_in(path: &Path) -> Option<&str> {
    let path = path.to_str()?;
    let (folder, id) = path.split_at_checked(APPS_FOLDER.len())?;
    let id = id.strip_prefix(['\\', '/'])?;
    (folder.eq_ignore_ascii_case(APPS_FOLDER) && !id.is_empty()).then_some(id)
}

/// The path that names the application `id`.
pub fn app_path(id: &str) -> PathBuf {
    PathBuf::from(format!("{APPS_FOLDER}\\{id}"))
}

/// Whether `id` is that of a packaged application: a package family and
/// the name of an application in it. Other identifiers are those of
/// ordinary programs, which have shortcut files.
fn is_packaged(id: &str) -> bool {
    id.split_once('!')
        .is_some_and(|(family, app)| family.contains('_') && !app.is_empty())
}

/// The string Windows allocated at `text`, which is freed.
pub(super) fn take(text: PWSTR) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the string is NUL-terminated and was allocated by COM for the
    // caller, which frees it once.
    unsafe {
        let string = text.to_string().ok();
        CoTaskMemFree(Some(text.0.cast()));
        string
    }
}

impl Shortcut {
    /// Every packaged application Windows lists among the installed ones,
    /// as something to start; empty if Windows does not tell. Asking takes
    /// a noticeable part of a second, so an answer is given again to those
    /// who ask shortly after, as every display's widgets do.
    pub fn packaged() -> Vec<Self> {
        static LISTED: Mutex<Option<(Instant, Vec<Shortcut>)>> = Mutex::new(None);
        let mut listed = LISTED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((when, apps)) = &*listed
            && when.elapsed() < FRESH
        {
            return apps.clone();
        }
        let apps = Self::read_packaged().unwrap_or_else(|error| {
            report!("packaged applications are not listed: {error}");
            Vec::new()
        });
        *listed = Some((Instant::now(), apps.clone()));
        apps
    }

    fn read_packaged() -> windows::core::Result<Vec<Self>> {
        let mut found = Vec::new();
        // SAFETY: COM is initialized on the calling thread; every interface
        // is released when it goes out of scope.
        unsafe {
            let folder: IShellItem =
                SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
            let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
            loop {
                let mut item = [None];
                let mut fetched = 0;
                if items.Next(&mut item, Some(&raw mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                let Some(item) = item[0].take() else {
                    break;
                };
                let text = |kind: SIGDN| item.GetDisplayName(kind).ok().and_then(take);
                let (Some(name), Some(id)) =
                    (text(SIGDN_NORMALDISPLAY), text(SIGDN_PARENTRELATIVEPARSING))
                else {
                    continue;
                };
                if is_packaged(&id) {
                    found.push(Self {
                        name,
                        path: app_path(&id),
                    });
                }
            }
        }
        Ok(found)
    }
}

/// The program whose windows show applications of the Universal Windows
/// Platform, each of which runs in a process of its own.
const FRAME_HOST: &str = "ApplicationFrameHost.exe";

/// The identifier of the packaged application `window` belongs to, which
/// is a window of `process`, whose program file is `executable`; `None`
/// for the window of an ordinary program.
pub(super) fn app_id_of(
    window: HWND,
    process: HANDLE,
    executable: Option<&Path>,
) -> Option<String> {
    let mut buffer = [0u16; 256];
    let mut length = buffer.len() as u32;
    // SAFETY: the buffer and its length describe the same allocation.
    let failed = unsafe {
        GetApplicationUserModelId(process, &raw mut length, Some(PWSTR(buffer.as_mut_ptr())))
    };
    if failed.is_ok() && length > 1 {
        let id = String::from_utf16_lossy(&buffer[..length as usize - 1]);
        return is_packaged(&id).then_some(id);
    }
    // A window of the frame host carries the identifier of what it shows.
    let name = executable?.file_name()?;
    if !name.eq_ignore_ascii_case(FRAME_HOST) {
        return None;
    }
    // SAFETY: COM is initialized on the calling thread, and the call
    // tolerates a window that has since been destroyed.
    let id = unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(window).ok()?;
        let value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        BSTR::try_from(&value).ok()?.to_string()
    };
    is_packaged(&id).then_some(id)
}

impl IconPixels {
    /// The icon of the application `id` as the shell draws it, `size`
    /// pixels wide and high; `None` if the shell has none.
    pub fn of_app(id: &str, size: u32) -> Option<Self> {
        Self::of_item(&app_path(id), size)
    }

    /// The icon the shell draws for the file or folder at `path`, `size`
    /// pixels wide and high; `None` if the shell has none.
    pub fn of_item(path: &Path, size: u32) -> Option<Self> {
        // The shell takes no path written with forward slashes.
        let name = HSTRING::from(path.to_string_lossy().replace('/', "\\"));
        let side = size as i32;
        // SAFETY: COM is initialized on the calling thread, and the bitmap
        // is deleted once its pixels are copied.
        unsafe {
            let item: IShellItemImageFactory = SHCreateItemFromParsingName(&name, None).ok()?;
            let bitmap = item
                .GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY)
                .ok()?;
            let pixels = Self::of_bitmap(bitmap);
            let _ = DeleteObject(bitmap.into());
            pixels
        }
    }

    /// The pixels of `bitmap`, which holds colors already multiplied by
    /// their opacity.
    pub(super) fn of_bitmap(bitmap: HBITMAP) -> Option<Self> {
        // SAFETY: every structure passed is correctly sized and the pixel
        // buffer matches the dimensions given to GetDIBits.
        let (width, height, mut bgra) = unsafe {
            let mut described = BITMAP::default();
            let read = GetObjectW(
                bitmap.into(),
                size_of::<BITMAP>() as i32,
                Some((&raw mut described).cast()),
            );
            let (width, height) = (described.bmWidth, described.bmHeight.abs());
            if read == 0 || width <= 0 || height <= 0 {
                return None;
            }
            let mut bgra = vec![0u8; (width * height * 4) as usize];
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let screen = GetDC(None);
            let copied = GetDIBits(
                screen,
                bitmap,
                0,
                height as u32,
                Some(bgra.as_mut_ptr().cast()),
                &raw mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, screen);
            if copied == 0 {
                return None;
            }
            (width as u32, height as u32, bgra)
        };
        straighten(&mut bgra);
        Some(Self {
            width,
            height,
            bgra,
        })
    }
}

/// Divides the colors of `bgra` by their opacity, which they were
/// multiplied by. Pixels that are all transparent make an opaque picture
/// without an opacity channel, which is left as it is but for that.
fn straighten(bgra: &mut [u8]) {
    let pixels = bgra.as_chunks_mut::<4>().0;
    if pixels.iter().all(|pixel| pixel[3] == 0) {
        pixels.iter_mut().for_each(|pixel| pixel[3] = 0xff);
        return;
    }
    for pixel in pixels {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 0xff {
            continue;
        }
        for color in &mut pixel[..3] {
            *color = (u32::from(*color) * 0xff / alpha).min(0xff) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_is_named_by_a_path_in_the_folder_of_applications() {
        let id = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
        let path = app_path(id);
        assert_eq!(app_id_in(&path), Some(id));
        // The configuration file writes the other slash.
        let written = format!("shell:appsfolder/{id}");
        assert_eq!(app_id_in(Path::new(&written)), Some(id));
        assert_eq!(app_id_in(Path::new("C:/Windows/notepad.exe")), None);
        assert_eq!(app_id_in(Path::new("shell:AppsFolder")), None);
        assert_eq!(app_id_in(Path::new("shell:AppsFolder/")), None);
    }

    #[test]
    fn only_identifiers_of_a_package_count_as_packaged() {
        assert!(is_packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        // Ordinary programs are listed by a path or a name of their own.
        assert!(!is_packaged(
            "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}/notepad.exe"
        ));
        assert!(!is_packaged("Chrome"));
        assert!(!is_packaged("Vendor.Tool!"));
    }

    /// Reads what is installed on the machine it runs on, so it is run by
    /// hand: `cargo test packaged -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the applications installed on this machine"]
    fn the_packaged_applications_of_this_machine_have_icons() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        // SAFETY: the thread of the test uses COM until it ends.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let started = std::time::Instant::now();
        let apps = Shortcut::packaged();
        println!("{} applications in {:?}", apps.len(), started.elapsed());
        assert!(!apps.is_empty());
        let mut drawn = 0;
        for app in &apps {
            let id = app_id_in(&app.path).expect("the path names an application");
            let icon = IconPixels::of_app(id, 64);
            drawn += usize::from(icon.is_some());
            let size = icon.map(|icon| (icon.width, icon.height));
            println!("{:40} {size:?} {id}", app.name);
        }
        println!("{drawn} icons in {:?}", started.elapsed());
        assert!(drawn > 0);
    }

    #[test]
    fn colors_are_divided_by_their_opacity() {
        let mut pixels = [40, 20, 10, 0x80, 0, 0, 0, 0, 9, 9, 9, 0xff];
        straighten(&mut pixels);
        assert_eq!(pixels, [79, 39, 19, 0x80, 0, 0, 0, 0, 9, 9, 9, 0xff]);
        // A picture without opacities is opaque.
        let mut opaque = [1, 2, 3, 0, 4, 5, 6, 0];
        straighten(&mut opaque);
        assert_eq!(opaque, [1, 2, 3, 0xff, 4, 5, 6, 0xff]);
    }
}

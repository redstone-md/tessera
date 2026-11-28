// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The pictures of toasts: read from where the toast names them, decoded
//! and scaled away from the user interface, and kept by the application
//! and the name the toast gives them.
//!
//! A toast names a file (`file:///` or a path), a file of the package of
//! its application (`ms-appx:///`, in the folder the package is installed
//! in, where Windows also looks for the variants of a picture by scale or
//! size), a file of the package's own data (`ms-appdata:///local/`), or a
//! picture on the web, which is fetched only under
//! `[notifications] web_images = true`, into a cache of limited size in
//! `%LOCALAPPDATA%\tessera\notification-pictures`.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use gpui::{App, AsyncApp};
use sha2::{Digest, Sha256};

use super::{LOGO_PIXELS, NotificationStore, Watch};
use crate::icons::image_of_bytes;
use crate::platform::{NotificationSound, package_data_folder, package_folder};
use crate::toast::{Source, Toast};

/// The side of the square a wide picture (the hero, a picture in the
/// content) is scaled to fit, in pixels: the width of a banner at 200 %.
const WIDE_PIXELS: u32 = 720;

/// The side of the picture of a button.
const BUTTON_PIXELS: u32 = 32;

/// The largest picture file that is read, in bytes; Windows takes at
/// most 3 MB from the web.
const PICTURE_LIMIT: u64 = 3 << 20;

/// How many bytes the cache of pictures of the web keeps at most.
const CACHE_LIMIT: u64 = 32 << 20;

/// The folder of the cache, in the `tessera` folder of the local
/// application data.
const CACHE_FOLDER: &str = "notification-pictures";

/// The pictures `toast` names, each with the side it is scaled to fit.
fn sources(toast: &Toast) -> Vec<(String, u32)> {
    let logo = toast
        .logo
        .iter()
        .map(|image| (image.src.clone(), LOGO_PIXELS));
    let wide =
        { toast.hero.iter().chain(&toast.images) }.map(|image| (image.src.clone(), WIDE_PIXELS));
    let buttons = { toast.actions.iter() }
        .filter_map(|action| action.image.clone())
        .map(|src| (src, BUTTON_PIXELS));
    logo.chain(wide).chain(buttons).collect()
}

/// Reads the pictures of `toast`, of the application `app_id`, that were
/// not read before; the store gets each once it is ready.
pub(super) fn look_for(app_id: &str, toast: &Toast, cx: &mut App) {
    let web = cx.global::<Watch>().web_images;
    let asked = &mut cx.global_mut::<Watch>().pictures;
    let wanted: Vec<(String, u32)> = { sources(toast).into_iter() }
        .filter(|(src, _)| asked.insert((app_id.to_owned(), src.clone())))
        .collect();
    if wanted.is_empty() {
        return;
    }
    let app_id = app_id.to_owned();
    cx.spawn(async move |cx: &mut AsyncApp| {
        for (src, side) in wanted {
            let (id, name) = (app_id.clone(), src.clone());
            let picture = cx.background_executor().spawn(async move {
                let bytes = read(&name, &id, web)?;
                image_of_bytes(&bytes, side).ok().flatten()
            });
            let Some(picture) = picture.await else {
                continue;
            };
            let stored = cx.update(|cx| {
                let pictures = &mut cx.global_mut::<NotificationStore>().pictures;
                pictures.insert((app_id.clone(), src), Some(picture));
            });
            if stored.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// The bytes of the picture `src` of a toast of the application `app_id`;
/// one of the web only where `web` lets it be fetched.
fn read(src: &str, app_id: &str, web: bool) -> Option<Vec<u8>> {
    match Source::of(src)? {
        Source::File(path) => read_file(Path::new(&path)),
        Source::Package(path) => read_file(&variant_in(&package_folder(app_id)?, &path)?),
        Source::PackageData(path) => read_file(&inside(&package_data_folder(app_id)?, &path)?),
        Source::Web(address) if web => fetch(&address),
        Source::Web(_) | Source::SoundEvent(_) => None,
    }
}

/// The file `path` holds, unless it is larger than a picture may be.
fn read_file(path: &Path) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(PICTURE_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= PICTURE_LIMIT).then_some(bytes)
}

/// `relative`, a path with forward or backward slashes, inside `base`;
/// `None` where it would lead out of it.
fn inside(base: &Path, relative: &str) -> Option<PathBuf> {
    let relative = PathBuf::from(relative.replace('/', "\\"));
    let plain = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    (plain && relative.components().next().is_some()).then(|| base.join(relative))
}

/// The file of the package at `relative` in its install folder `base`:
/// the file itself, or else the variant of it with the largest scale or
/// size, as `Logo.scale-200.png` or `Logo.targetsize-48.png` stand for
/// `Logo.png`.
fn variant_in(base: &Path, relative: &str) -> Option<PathBuf> {
    let path = inside(base, relative)?;
    if path.is_file() {
        return Some(path);
    }
    let folder = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    let extension = path.extension()?.to_str()?;
    let names: Vec<String> = { fs::read_dir(folder).ok()?.flatten() }
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    best_variant(stem, extension, &names).map(|name| folder.join(name))
}

/// Of `names`, the variant of `stem.extension` with the largest scale or
/// size qualifier.
fn best_variant<'a>(stem: &str, extension: &str, names: &'a [String]) -> Option<&'a str> {
    let lower = |text: &str| text.to_lowercase();
    let (stem, extension) = (lower(stem), lower(extension));
    names
        .iter()
        .filter_map(|name| {
            let small = lower(name);
            let qualifiers = small
                .strip_prefix(&format!("{stem}."))?
                .strip_suffix(&format!(".{extension}"))?
                .to_owned();
            // The size it is for: the largest number of its qualifiers.
            let size = { qualifiers.split(['.', '_']) }
                .filter_map(|qualifier| {
                    let (_, number) = qualifier.split_once('-')?;
                    number.parse::<u32>().ok()
                })
                .max()
                .unwrap_or(0);
            Some((size, name.as_str()))
        })
        .max_by_key(|(size, _)| *size)
        .map(|(_, name)| name)
}

/// Where the cache keeps the picture of `address`.
fn cached_path(address: &str) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").filter(|local| !local.is_empty())?;
    let digest = Sha256::digest(address.as_bytes());
    let name: String = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Some(
        PathBuf::from(local)
            .join("tessera")
            .join(CACHE_FOLDER)
            .join(name),
    )
}

/// The picture at `address`, from the cache or else fetched into it.
fn fetch(address: &str) -> Option<Vec<u8>> {
    let path = cached_path(address)?;
    if let Some(bytes) = read_file(&path) {
        return Some(bytes);
    }
    let bytes = match crate::extension::http::download(address, PICTURE_LIMIT) {
        Ok(bytes) => bytes,
        Err(error) => {
            report!("a picture of a notification was not fetched from {address}: {error}");
            return None;
        }
    };
    if let Some(folder) = path.parent()
        && fs::create_dir_all(folder).is_ok()
        && fs::write(&path, &bytes).is_ok()
    {
        trim_cache(folder);
    }
    Some(bytes)
}

/// Removes the oldest pictures of the cache until it holds no more than
/// it may.
fn trim_cache(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = { entries.flatten() }
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let when = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            meta.is_file().then(|| (when, meta.len(), entry.path()))
        })
        .collect();
    for path in over_limit(&mut files, CACHE_LIMIT) {
        fs::remove_file(path).ok();
    }
}

/// The oldest of `files` (when written, bytes, path) to remove so that
/// the rest hold no more than `limit` bytes.
fn over_limit(files: &mut [(SystemTime, u64, PathBuf)], limit: u64) -> Vec<PathBuf> {
    files.sort_by_key(|file| std::cmp::Reverse(file.0));
    let mut kept = 0u64;
    let mut gone = Vec::new();
    for (_, size, path) in files.iter() {
        kept += size;
        if kept > limit {
            gone.push(path.clone());
        }
    }
    gone
}

/// The sound file a banner of a toast of the application `app_id` plays,
/// and whether it plays again and again: what `toast` names, or the
/// default sound of notifications for a toast that names none and for a
/// notice of Tessera's own (no toast); none for a toast that is silent.
/// It reads the registry and asks for the folder of a package, so it is
/// for a thread away from the user interface.
pub fn sound_file(toast: Option<&Toast>, app_id: &str) -> Option<(PathBuf, bool)> {
    let audio = toast.map(|toast| toast.audio.clone()).unwrap_or_default();
    if audio.silent {
        return None;
    }
    let file = match audio.src.as_deref().and_then(Source::of) {
        None => NotificationSound::of_event("Notification.Default")?,
        Some(Source::SoundEvent(event)) => NotificationSound::of_event(&event)?,
        Some(Source::File(path)) => PathBuf::from(path),
        Some(Source::Package(path)) => inside(&package_folder(app_id)?, &path)?,
        Some(Source::PackageData(path)) => inside(&package_data_folder(app_id)?, &path)?,
        Some(Source::Web(_)) => return None,
    };
    let looping = audio.looping && toast.is_some_and(|toast| toast.scenario.stays());
    Some((file, looping))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_path_of_a_package_stays_inside_it() {
        let base = Path::new(r"C:\Program Files\WindowsApps\Contoso");
        assert_eq!(
            inside(base, "Assets/Logo.png"),
            Some(base.join(r"Assets\Logo.png"))
        );
        assert_eq!(inside(base, "../../Windows/x.png"), None);
        assert_eq!(inside(base, r"C:\x.png"), None);
        assert_eq!(inside(base, ""), None);
    }

    #[test]
    fn the_largest_variant_of_a_picture_is_taken() {
        let names: Vec<String> = [
            "Logo.scale-100.png",
            "Logo.scale-200.png",
            "Logo.targetsize-48_altform-unplated.png",
            "LogoWide.scale-400.png",
            "Logo.scale-400.jpg",
        ]
        .iter()
        .map(|name| name.to_string())
        .collect();
        assert_eq!(
            best_variant("logo", "png", &names),
            Some("Logo.scale-200.png")
        );
        assert_eq!(best_variant("Other", "png", &names), None);
    }

    #[test]
    fn the_cache_lets_go_of_its_oldest_pictures() {
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        let mut files = vec![
            (at(1), 10, PathBuf::from("old")),
            (at(3), 10, PathBuf::from("new")),
            (at(2), 10, PathBuf::from("middle")),
        ];
        assert_eq!(over_limit(&mut files, 25), [PathBuf::from("old")]);
        assert!(over_limit(&mut files, 30).is_empty());
    }
}

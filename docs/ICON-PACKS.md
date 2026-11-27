# Icon packs

An icon pack replaces the icons Tessera shows for applications (in the dock, the launcher, the Start menu, the window switcher and the overview) and for kinds of files (in the file manager and the launcher). It is a directory of `%APPDATA%\tessera\icon-packs\` (or of `icon-packs` in the directory `TESSERA_CONFIG_DIR` names) holding a manifest, `icon-pack.toml`, and the pictures it names.

Written against the Windows API and checked to build for Windows; not run on Windows yet.

## Using packs

`icon_packs` in `config.toml` names the packs in use by the names of their directories, the first one first:

```toml
icon_packs = ["line", "my-own"]
```

For each icon the packs are asked in that order and the first that has one gives it, so a small pack of a few favourites can stand before a large one. The `[icons]` table of `config.toml` stands above every pack; the icon an application or a file brings is used where no pack has one. A pack that is missing or whose manifest cannot be used is reported among the problems and left out; the others still count.

The marketplace installs packs from the registries (see [Registries](EXTENSIONS.md#registries)) and its **Use** button puts a pack first among those in use; **Stop using** takes it out. Changing the order rewrites `icon_packs`, which is applied at once; as with the `[icons]` table, the widgets are built again for it and an open popup, the settings included, closes. The files of the packs in use are looked at every two seconds; a picture that is added, replaced or removed is shown about two seconds later without a restart, and the bars and docks are built again for it, as after a change of the `[icons]` table, without loading the extensions again. An open popup stays open, so updating a pack in use from the marketplace leaves it open, and shows the new pictures the next time it opens; the cards keep theirs until they are built again.

## The manifest

```toml
id = "line"                    # a-z, 0-9 and -; the name of the directory
name = "Line"
author = "Someone"             # optional
version = "1.0.0"
description = "Shown in the marketplace."   # optional

[apps]
"notepad.exe" = "apps/notepad.png"
"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App" = { icon = "apps/calculator.svg", monochrome = true }
"Visual Studio Code" = "apps/code.ico"

[files]
folder = { icon = "files/folder.png", mask = "files/folder-tint.png" }
drive = "files/drive.svg"
recycle-bin-empty = "files/bin.svg"
recycle-bin-full = "files/bin-full.svg"
txt = "files/text.svg"
".md" = "files/text.svg"

[shell]
# Kept for icons of the shell itself; nothing reads it yet.
```

A key of the manifest that is not listed here makes the whole manifest unusable, which is reported.

`[apps]` names an application by any of:

- the file name of its program, as `notepad.exe`;
- the identifier of a packaged (Store) application, as Windows gives it (`Package_family!App`), for the icons Tessera shows for packaged applications;
- the name of its shortcut in the Start menu (`Visual Studio Code`), or the file name of the shortcut without `.lnk`.

Names are compared without regard to case. For a shortcut the program it opens is asked for first, then its name. A packaged application is only known by its identifier.

`[files]` names a kind of file by its extension, with or without the dot, or one of the kinds `folder`, `drive` (the root of a drive), `recycle-bin-full` and `recycle-bin-empty`. Tessera's own file manager and launcher ask for the extensions and for `folder` and `drive`; the two recycle bin kinds are there for the icons of the desktop, which ask for them once they are drawn by Tessera.

Each value is the path of a picture within the pack, with `/` between directories and without `..`: a PNG, ICO, SVG or JPEG file of at most 4 MiB. It is fitted into a square of 64 pixels, which is large enough at 200% scaling. An SVG is drawn by the renderer GPUI already brings (resvg), at that size, so it stays sharp. A picture that cannot be read is reported once, and the icon the application or the file brings is shown instead.

## Icons in the colors of the theme

An icon can follow the colors of the theme in use, as the icons of the shell do, in two ways:

- `monochrome = true` draws the icon by its shape alone: every pixel in the color of text of the theme (`on_surface`), as opaque as the picture is there. Line icons drawn in any one color are meant for this.
- A mask paints part of a picture that keeps its own colors: where the mask covers, the color of text of the theme is laid over the picture, as strongly as the mask covers. The mask is the picture `mask` names, or, without the key, a PNG next to the icon named like it with `-mask` before the extension (`folder.png` and `folder-mask.png`). How much a mask covers is its alpha channel, or its brightness where it has none.

Where the icon stands for something active, such as an application that runs, it is drawn in the primary color of the theme instead; the part of Tessera that draws it has to ask for that (`AppIcons::of_executable_as`, `of_shortcut_as`), which the dock does not do yet. When the theme changes, such icons are drawn again in the new colors the next time they are asked for; a widget that kept the picture it was given shows the new colors once it is built again.

## A pack in a registry

A registry offers a pack as an `[[icon_pack]]` entry with the manifest and every file of the pack, each with its SHA-256 sum; the paths of the files are relative to the directory of the manifest. Installing checks every sum, that the manifest is usable and has the offered `id`, and that every picture it names is listed, then writes the files to `icon-packs\<id>\` with `installed.toml`, which records the version, the registry and the files written. An update deletes the files of the earlier version that the new one no longer has. Removing deletes only those files, and nothing at all if the directory holds a file that did not come from the registry. A pack copied in by hand (without `installed.toml`) is neither replaced nor removed.

[`registry/icon-packs/line`](../registry/icon-packs/line) is a pack to start from: line icons for a few common programs and kinds of files, drawn in the colors of the theme, and a folder in its own colors whose front the theme paints through a mask next to it.

## What is not done

- No pack has been tried on a running shell: the lookup, the masks and the drawing of SVG files were tried in tests only, and the sample pack's SVG files with resvg outside Tessera.
- The dock does not ask for the active color yet, and the icons of the desktop and the thumbnails of the file manager do not ask the packs yet, as those parts come separately.
- Icons of the shell itself (`[shell]`), of the notification area, of notifications and of extensions are not replaced.
- Light and dark variants of a picture are not chosen between; a pack that wants to fit both uses `monochrome` or a mask.

// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the system monitor writes, and which windows it lists, apart from
//! Tessera so that it can be tested anywhere.

use tessera_extension_kit::WindowInfo;

/// A share from 0 to 1 as a whole percent, as `42%`.
pub fn percent(share: f32) -> String {
    let share = if share.is_finite() {
        share.clamp(0., 1.)
    } else {
        0.
    };
    format!("{:.0}%", share * 100.)
}

/// A number of bytes in the largest unit that keeps it above 1, as
/// `7.8 GB`.
pub fn bytes(count: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = count.max(0.);
    let mut unit = 0;
    while value >= 1000. && unit + 1 < UNITS.len() {
        value /= 1000.;
        unit += 1;
    }
    match unit {
        0 => format!("{value:.0} {}", UNITS[unit]),
        _ if value < 10. => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

/// A rate of bytes per second, as `1.2 MB/s`.
pub fn rate(per_second: f64) -> String {
    format!("{}/s", bytes(per_second))
}

/// What a window's `app` is called for short: the name of its program
/// file without `.exe`, or the application of a packaged one.
pub fn app_name(app: &str) -> &str {
    if let Some((family, application)) = app.split_once('!') {
        let name = family.split('_').next().unwrap_or(family);
        let name = name.rsplit('.').next().unwrap_or(name);
        return if name.is_empty() { application } else { name };
    }
    let file = app.rsplit(['\\', '/']).next().unwrap_or(app);
    let stem = file
        .len()
        .checked_sub(4)
        .filter(|&at| file.is_char_boundary(at) && file[at..].eq_ignore_ascii_case(".exe"));
    match stem {
        Some(at) => &file[..at],
        None => file,
    }
}

/// The windows whose title or application has `filter` in it, in any
/// case, at most `count` of them, in the order given.
pub fn matching<'a>(windows: &'a [WindowInfo], filter: &str, count: usize) -> Vec<&'a WindowInfo> {
    let filter = filter.trim().to_lowercase();
    let matches = |window: &&WindowInfo| {
        filter.is_empty()
            || window.title.to_lowercase().contains(&filter)
            || app_name(&window.app).to_lowercase().contains(&filter)
    };
    windows.iter().filter(matches).take(count).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64, title: &str, app: &str) -> WindowInfo {
        WindowInfo {
            id,
            title: title.to_owned(),
            app: app.to_owned(),
            display: 1,
            workspace: None,
            minimized: false,
            focused: false,
        }
    }

    #[test]
    fn amounts_are_written_short() {
        assert_eq!(percent(0.424), "42%");
        assert_eq!(percent(f32::NAN), "0%");
        assert_eq!(percent(3.), "100%");
        assert_eq!(bytes(512.), "512 B");
        assert_eq!(bytes(7_800_000_000.), "7.8 GB");
        assert_eq!(bytes(64_000_000.), "64 MB");
        assert_eq!(rate(1_234_000.), "1.2 MB/s");
        assert_eq!(rate(-5.), "0 B/s");
    }

    #[test]
    fn applications_are_named_for_short() {
        assert_eq!(app_name(r"C:\Windows\notepad.EXE"), "notepad");
        assert_eq!(app_name("C:/Tools/btop"), "btop");
        assert_eq!(
            app_name("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            "WindowsCalculator"
        );
        assert_eq!(app_name(""), "");
    }

    #[test]
    fn windows_are_filtered_by_title_and_application() {
        let windows = [
            window(1, "Notes - Notepad", r"C:\Windows\notepad.exe"),
            window(2, "Inbox", r"C:\Mail\mail.exe"),
            window(3, "Report.docx", r"C:\Office\WINWORD.EXE"),
        ];
        let ids =
            |found: Vec<&WindowInfo>| found.iter().map(|window| window.id).collect::<Vec<_>>();
        assert_eq!(ids(matching(&windows, "", 2)), [1, 2]);
        assert_eq!(ids(matching(&windows, " NOTE ", 9)), [1]);
        assert_eq!(ids(matching(&windows, "winword", 9)), [3]);
        assert!(matching(&windows, "nothing", 9).is_empty());
    }
}

// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The session, the settings and the tools of Windows: locking, sleep,
//! signing out, restarting and shutting down, each but locking after a
//! confirmation; the pages of the Settings app; the items of the Control
//! Panel and the tools of Windows such as the Registry Editor. Each is
//! found by its name in the language of Tessera, in English, and by the
//! names Windows users know it by.

use crate::platform::{PowerAction, expand_variables};

use super::command::Command;
use super::{Answer, AnswerAction, Query, Runner, rank, ranked_score};

const ICON_SETTINGS: &str = "\u{e713}";
const ICON_TOOL: &str = "\u{e90f}";
const ICON_PANEL: &str = "\u{e80f}";

/// How many letters the pages and the actions wait for.
const LETTERS: usize = 2;

/// The scores of an action of the session and of a page whose name
/// starts with what is typed; pages come after the applications.
const SCORE_POWER: u16 = 800;
const SCORE_PAGE: u16 = 400;

/// What can be done to the session: the action, what it is called, its
/// icon, and other names it is known by.
const POWER: [(PowerAction, &str, &str, &str); 6] = [
    (PowerAction::Lock, "Lock the session", "\u{e72e}", "lock"),
    (
        PowerAction::Sleep,
        "Put the computer to sleep",
        "\u{e708}",
        "sleep,suspend",
    ),
    (
        PowerAction::Hibernate,
        "Hibernate the computer",
        "\u{e74e}",
        "hibernate",
    ),
    (
        PowerAction::SignOut,
        "Sign out of Windows",
        "\u{f3b1}",
        "sign out,log out,logout,log off,logoff",
    ),
    (
        PowerAction::Restart,
        "Restart the computer",
        "\u{e777}",
        "restart,reboot",
    ),
    (
        PowerAction::ShutDown,
        "Shut down the computer",
        "\u{e7e8}",
        "shut down,shutdown,power off,turn off",
    ),
];

/// The pages of the Settings app, the items of the Control Panel and the
/// tools of Windows: the name, what opens it, its arguments, and other
/// names it is known by, in English.
#[rustfmt::skip]
pub const PAGES: [(&str, &str, &str, &str); 100] = [
    ("Display", "ms-settings:display", "", "screen,resolution,scale"),
    ("Night light", "ms-settings:nightlight", "", "blue light"),
    ("Sound", "ms-settings:sound", "", "audio,speakers,volume"),
    ("Notifications", "ms-settings:notifications", "", "do not disturb"),
    ("Power and battery", "ms-settings:powersleep", "", "power,sleep,battery"),
    ("Storage", "ms-settings:storagesense", "", "disk space,storage sense"),
    ("Bluetooth and devices", "ms-settings:bluetooth", "", "devices"),
    ("Printers and scanners", "ms-settings:printers", "", "printer,scanner"),
    ("Mouse", "ms-settings:mousetouchpad", "", "pointer"),
    ("Network and internet", "ms-settings:network-status", "", "network"),
    ("Wi-Fi", "ms-settings:network-wifi", "", "wireless,wlan"),
    ("VPN", "ms-settings:network-vpn", "", ""),
    ("Personalization", "ms-settings:personalization", "", ""),
    ("Background", "ms-settings:personalization-background", "", "wallpaper"),
    ("Colors", "ms-settings:colors", "", "accent,dark mode,light mode"),
    ("Themes", "ms-settings:themes", "", "desktop icons"),
    ("Lock screen", "ms-settings:lockscreen", "", ""),
    ("Installed apps", "ms-settings:appsfeatures", "", "uninstall,programs"),
    ("Default apps", "ms-settings:defaultapps", "", "file associations"),
    ("Startup apps", "ms-settings:startupapps", "", "autostart"),
    ("Your account", "ms-settings:yourinfo", "", "profile"),
    ("Sign-in options", "ms-settings:signinoptions", "", "password,pin,hello"),
    ("Date and time", "ms-settings:dateandtime", "", "clock,time zone"),
    ("Language and region", "ms-settings:regionlanguage", "", "language"),
    ("Typing", "ms-settings:typing", "", "autocorrect,keyboard"),
    ("Accessibility", "ms-settings:easeofaccess", "", "ease of access"),
    ("Privacy and security", "ms-settings:privacy", "", "permissions"),
    ("Windows Update", "ms-settings:windowsupdate", "", "updates"),
    ("About this computer", "ms-settings:about", "", "about,pc name,specifications"),
    ("Start menu", "ms-settings:personalization-start", "", "start"),
    ("Taskbar", "ms-settings:taskbar", "", ""),
    ("Fonts", "ms-settings:fonts", "", ""),
    ("Multitasking", "ms-settings:multitasking", "", "snap,alt+tab"),
    ("Touchpad", "ms-settings:devices-touchpad", "", "trackpad"),
    ("Pen and Windows Ink", "ms-settings:pen", "", "stylus"),
    ("AutoPlay", "ms-settings:autoplay", "", ""),
    ("USB", "ms-settings:usb", "", ""),
    ("Cameras", "ms-settings:camera", "", "webcam"),
    ("Ethernet", "ms-settings:network-ethernet", "", "cable,lan"),
    ("Mobile hotspot settings", "ms-settings:network-mobilehotspot", "", "hotspot,tethering"),
    ("Airplane mode settings", "ms-settings:network-airplanemode", "", "airplane,flight mode"),
    ("Proxy", "ms-settings:network-proxy", "", ""),
    ("Advanced network settings", "ms-settings:network-advancedsettings", "", "adapters"),
    ("Data usage", "ms-settings:datausage", "", ""),
    ("Graphics", "ms-settings:display-advancedgraphics", "", "gpu"),
    ("Optional features", "ms-settings:optionalfeatures", "", ""),
    ("Apps for websites", "ms-settings:appsforwebsites", "", ""),
    ("Video playback", "ms-settings:videoplayback", "", "hdr"),
    ("Email and accounts", "ms-settings:emailandaccounts", "", "mail"),
    ("Family", "ms-settings:family-group", "", "parental"),
    ("Windows Backup", "ms-settings:backup", "", "onedrive"),
    ("Other users", "ms-settings:otherusers", "", "accounts"),
    ("Access work or school", "ms-settings:workplace", "", "domain"),
    ("Game Bar", "ms-settings:gaming-gamebar", "", "gaming,xbox"),
    ("Game Mode", "ms-settings:gaming-gamemode", "", "gaming"),
    ("Captures", "ms-settings:gaming-gamedvr", "", "recording,screenshots"),
    ("Text size", "ms-settings:easeofaccess-display", "", "font size"),
    ("Visual effects", "ms-settings:easeofaccess-visualeffects", "", "animations,transparency"),
    ("Mouse pointer and touch", "ms-settings:easeofaccess-mousepointer", "", "cursor"),
    ("Magnifier", "ms-settings:easeofaccess-magnifier", "", "zoom"),
    ("Color filters", "ms-settings:easeofaccess-colorfilter", "", "color blindness"),
    ("Narrator", "ms-settings:easeofaccess-narrator", "", "screen reader"),
    ("Speech", "ms-settings:speech", "", "voice"),
    ("Captions", "ms-settings:easeofaccess-closedcaptioning", "", "subtitles"),
    ("Location permissions", "ms-settings:privacy-location", "", "location,gps"),
    ("Camera permissions", "ms-settings:privacy-webcam", "", "camera"),
    ("Microphone permissions", "ms-settings:privacy-microphone", "", "microphone"),
    ("Windows Security", "windowsdefender:", "", "defender,antivirus,firewall"),
    ("Find my device", "ms-settings:findmydevice", "", ""),
    ("For developers", "ms-settings:developers", "", "developer mode"),
    ("Recovery", "ms-settings:recovery", "", "reset this pc"),
    ("Activation", "ms-settings:activation", "", "product key,license"),
    ("Troubleshoot", "ms-settings:troubleshoot", "", ""),
    ("Remote Desktop", "ms-settings:remotedesktop", "", "rdp"),
    ("Clipboard settings", "ms-settings:clipboard", "", "clipboard"),
    ("Nearby sharing", "ms-settings:crossdevice", "", "share"),
    ("Region", "ms-settings:regionformatting", "", "formats,country"),
    ("Disks and volumes", "ms-settings:disksandvolumes", "", "partitions,drives"),
    ("Control Panel", r"%SystemRoot%\System32\control.exe", "", "control"),
    ("Manage devices and drivers", r"%SystemRoot%\System32\devmgmt.msc", "", "device manager,devmgmt"),
    ("Events and logs of Windows", r"%SystemRoot%\System32\eventvwr.msc", "", "event viewer,eventvwr"),
    ("Manage disks and partitions", r"%SystemRoot%\System32\diskmgmt.msc", "", "disk management,diskmgmt"),
    ("Manage the computer", r"%SystemRoot%\System32\compmgmt.msc", "", "computer management,compmgmt"),
    ("Network adapters", r"%SystemRoot%\System32\control.exe", "ncpa.cpl", "network connections,ncpa"),
    ("Running programs and their load", r"%SystemRoot%\System32\taskmgr.exe", "", "task manager,taskmgr"),
    ("Services", r"%SystemRoot%\System32\services.msc", "", "services.msc"),
    ("Registry Editor", r"%SystemRoot%\regedit.exe", "", "regedit"),
    ("System information", r"%SystemRoot%\System32\msinfo32.exe", "", "msinfo32"),
    ("System configuration", r"%SystemRoot%\System32\msconfig.exe", "", "msconfig"),
    ("Task Scheduler", r"%SystemRoot%\System32\taskschd.msc", "", "taskschd"),
    ("Programs and Features", r"%SystemRoot%\System32\control.exe", "appwiz.cpl", "appwiz,uninstall"),
    ("User accounts", r"%SystemRoot%\System32\control.exe", "/name Microsoft.UserAccounts", "netplwiz"),
    ("Credential Manager", r"%SystemRoot%\System32\control.exe", "/name Microsoft.CredentialManager", "passwords"),
    ("File Explorer options", r"%SystemRoot%\System32\control.exe", "folders", "folder options"),
    ("Environment variables", r"%SystemRoot%\System32\rundll32.exe", "sysdm.cpl,EditEnvironmentVariables", "path,env"),
    ("Advanced system settings", r"%SystemRoot%\System32\SystemPropertiesAdvanced.exe", "", "system properties,sysdm"),
    ("Windows Tools", r"%SystemRoot%\System32\control.exe", "/name Microsoft.AdministrativeTools", "administrative tools"),
    ("Windows Defender Firewall", r"%SystemRoot%\System32\control.exe", "firewall.cpl", "firewall"),
    ("Resource Monitor", r"%SystemRoot%\System32\resmon.exe", "", "resmon"),
    ("Character Map", r"%SystemRoot%\System32\charmap.exe", "", "charmap,symbols"),
];

/// What a page of [`PAGES`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A page of the Settings app.
    Page,
    /// An item of the Control Panel.
    Panel,
    /// A tool of Windows.
    Tool,
}

impl Kind {
    fn of(target: &str, arguments: &str) -> Self {
        let lower = target.to_ascii_lowercase();
        if !lower.contains('\\') {
            Self::Page
        } else if lower.ends_with("control.exe") || arguments.contains(".cpl") {
            Self::Panel
        } else {
            Self::Tool
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Page => ICON_SETTINGS,
            Self::Panel => ICON_PANEL,
            Self::Tool => ICON_TOOL,
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Page => "Settings",
            Self::Panel => "Control Panel",
            Self::Tool => "Windows tool",
        }
    }
}

/// The best rank among `names` and the comma-separated `keywords` of
/// how they answer `query`, which is lowercase.
fn best_rank(names: [&str; 2], keywords: &str, query: &str) -> Option<u8> {
    let keywords = keywords.split(',').filter(|word| !word.is_empty());
    { names.into_iter().chain(keywords) }
        .filter_map(|name| rank(name, query))
        .min()
}

/// The positions in [`PAGES`] of those whose name in `language`, in
/// English or another name of theirs answers `query`, the best first.
fn pages_matching(query: &Query) -> Vec<(usize, u8)> {
    let mut found: Vec<(usize, u8)> = { PAGES.iter().enumerate() }
        .filter_map(|(index, &(english, _, _, keywords))| {
            let names = [query.say(english), english];
            Some((index, best_rank(names, keywords, query.lower())?))
        })
        .collect();
    found.sort_by_key(|&(_, rank)| rank);
    found
}

pub struct Settings;

impl Runner for Settings {
    fn id(&self) -> &'static str {
        "settings"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        if query.letters() < LETTERS {
            return Vec::new();
        }
        { pages_matching(query).into_iter() }
            .map(|(index, best)| {
                let (english, target, arguments, _) = PAGES[index];
                let kind = Kind::of(target, arguments);
                let target = expand_variables(target);
                let command = Command {
                    typed: format!("{target} {arguments}").trim().to_owned(),
                    target,
                    arguments: (!arguments.is_empty()).then(|| arguments.to_owned()),
                    folder: false,
                    terminal: false,
                };
                let score = ranked_score(SCORE_PAGE, best);
                let detail = query.say(kind.detail());
                Answer::new(query.say(english), detail, kind.glyph(), score, {
                    AnswerAction::Run(command)
                })
                .keyed(format!("settings:{english}"))
            })
            .collect()
    }
}

pub struct Power;

impl Runner for Power {
    fn id(&self) -> &'static str {
        "power"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        if query.letters() < LETTERS {
            return Vec::new();
        }
        { POWER.iter() }
            .filter_map(|&(action, english, glyph, keywords)| {
                let names = [query.say(english), english];
                let best = best_rank(names, keywords, query.lower())?;
                let detail = query.say("Session and power");
                let score = ranked_score(SCORE_POWER, best);
                let answer = Answer::new(query.say(english), detail, glyph, score, {
                    AnswerAction::Power(action)
                });
                Some(answer.keyed(format!("power:{english}")))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;

    #[test]
    fn the_pages_are_found_by_their_names_in_both_languages() {
        let russian = Query::new("зву", Language::Russian);
        let sound = Settings.answer(&russian);
        assert_eq!(sound[0].title, "Звук");
        let update = Settings.answer(&Query::new("windows up", Language::Russian));
        let AnswerAction::Run(command) = &update[0].action else {
            panic!("a page opens");
        };
        assert_eq!(command.target, "ms-settings:windowsupdate");
        let manager = Settings.answer(&Query::new("device manager", Language::English));
        assert_eq!(manager[0].title, "Manage devices and drivers");
        assert_eq!(manager[0].detail, "Windows tool");
        assert!(
            Settings
                .answer(&Query::new("s", Language::English))
                .is_empty()
        );
    }

    #[test]
    fn every_page_has_its_own_name_and_an_opener() {
        let mut names: Vec<&str> = PAGES.iter().map(|page| page.0).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PAGES.len());
        for (name, target, arguments, _) in PAGES {
            assert!(!target.is_empty(), "{name} opens nothing");
            let kind = Kind::of(target, arguments);
            assert_eq!(
                kind == Kind::Page,
                target.ends_with(':') || target.starts_with("ms-settings:")
            );
        }
        assert_eq!(
            Kind::of(r"%SystemRoot%\System32\control.exe", "ncpa.cpl"),
            Kind::Panel
        );
        assert_eq!(Kind::of(r"%SystemRoot%\regedit.exe", ""), Kind::Tool);
    }

    #[test]
    fn the_session_is_ended_by_its_names() {
        let reboot = Power.answer(&Query::new("reboot", Language::English));
        assert_eq!(reboot.len(), 1);
        assert_eq!(reboot[0].action, AnswerAction::Power(PowerAction::Restart));
        let off = Power.answer(&Query::new("shut", Language::English));
        assert_eq!(off[0].action, AnswerAction::Power(PowerAction::ShutDown));
        assert!(Power.answer(&Query::new("l", Language::English)).is_empty());
    }
}

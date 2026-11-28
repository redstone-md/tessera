// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! `tessera pin <path|link>` and `tessera unpin <path|link>`: they hand
//! the request to the Tessera that runs, as the actions `pin` and `unpin`,
//! the way `tessera do` does. A relative path is made absolute here, in
//! the folder the command runs in, which the running Tessera does not
//! know. The verb "Pin to Tessera dock" of Explorer's menus runs
//! `tessera pin`.

use std::path::Path;

use crate::platform::Commands;

/// The commands this file answers, each with the action it hands over.
const COMMANDS: [(&str, &str); 2] = [("pin", "pin"), ("unpin", "unpin")];

/// Hands `words` to the running Tessera if they are `pin` or `unpin` with
/// what to pin, and says whether that worked; nothing if they are not.
pub fn send_if_requested(words: impl Iterator<Item = String>) -> Option<bool> {
    let words: Vec<String> = words.collect();
    let text = request(&words, |path| std::path::absolute(path).ok())?;
    Some(match Commands::send(&text) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Tessera does not run, or did not take the request: {error}");
            false
        }
    })
}

/// What `words` hand over as `do`, with each relative path that names
/// something made absolute by `absolute`: `None` where they are not
/// `pin` or `unpin` with something after.
fn request(
    words: &[String],
    absolute: impl Fn(&Path) -> Option<std::path::PathBuf>,
) -> Option<String> {
    let (first, rest) = words.split_first()?;
    let (_, action) = COMMANDS.iter().find(|(command, _)| command == first)?;
    let named = rest.join(" ");
    let named = named.trim();
    if named.is_empty() {
        return None;
    }
    let path = Path::new(named);
    let is_link = super::pins::scheme_of(named).is_some() && !path.is_absolute();
    let made = match (is_link, path.is_relative() && path.exists()) {
        (false, true) => absolute(path).map(|path| path.to_string_lossy().into_owned()),
        _ => None,
    };
    let named = made.unwrap_or_else(|| named.to_owned());
    Some(format!("do\n{action}\n{named}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &[&str]) -> Vec<String> {
        text.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn pin_and_unpin_are_handed_over_as_their_actions() {
        let none = |_: &Path| None;
        assert_eq!(
            request(&words(&["pin", r"C:\Apps\app.exe"]), none).as_deref(),
            Some("do\npin\nC:\\Apps\\app.exe")
        );
        assert_eq!(
            request(&words(&["unpin", "https://example.org"]), none).as_deref(),
            Some("do\nunpin\nhttps://example.org")
        );
        assert_eq!(
            request(&words(&["pin", "Visual", "Studio", "Code"]), none).as_deref(),
            Some("do\npin\nVisual Studio Code")
        );
        assert_eq!(request(&words(&["pin"]), none), None);
        assert_eq!(request(&words(&["popup", "calendar"]), none), None);
        assert_eq!(request(&[], none), None);
    }
}

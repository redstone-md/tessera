// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The folder a shell says it is in, which the terminal itself does not
//! keep: OSC 7 (`ESC ] 7 ; file://host/path ST`), which shells of Linux
//! and configured prompts of PowerShell write, and OSC 9;9
//! (`ESC ] 9 ; 9 ; "C:\path" ST`), which Windows Terminal taught cmd and
//! PowerShell prompts to write. The output of the shell is looked at for
//! them as it is read, before the terminal parses it.

/// The longest command of the operating system kept; a longer one is no
/// folder and is let go.
const LONGEST: usize = 4096;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Where in a command the output is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// After an escape.
    Escape,
    /// In a command, after `ESC ]`.
    Command,
    /// After an escape in a command, which `\` ends.
    CommandEscape,
    /// In a command too long to keep, until it ends.
    Skipping,
    /// After an escape in such a command.
    SkippingEscape,
}

/// Finds the commands to the operating system (OSC) in output that comes
/// piece by piece; one may be cut between two pieces.
#[derive(Debug, Default)]
pub struct OscScanner {
    state: State,
    command: Vec<u8>,
}

impl OscScanner {
    /// Looks at `bytes`, the next piece of the output, and hands `found`
    /// what each command that ends in it says, without `ESC ]` and its end.
    pub fn feed(&mut self, bytes: &[u8], mut found: impl FnMut(&[u8])) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (State::Ground, ESC) => State::Escape,
                (State::Ground, _) => State::Ground,
                (State::Escape, b']') => {
                    self.command.clear();
                    State::Command
                }
                (State::Escape, ESC) => State::Escape,
                (State::Escape, _) => State::Ground,
                (State::Command, BEL) => {
                    found(&self.command);
                    State::Ground
                }
                (State::Command, ESC) => State::CommandEscape,
                // Cancel and substitute end a sequence unfinished.
                (State::Command | State::Skipping, 0x18 | 0x1a) => State::Ground,
                (State::Command, _) if self.command.len() >= LONGEST => State::Skipping,
                (State::Command, _) => {
                    self.command.push(byte);
                    State::Command
                }
                (State::CommandEscape, b'\\') => {
                    found(&self.command);
                    State::Ground
                }
                // Another escape begins: the command ended there.
                (State::CommandEscape, b']') => {
                    self.command.clear();
                    State::Command
                }
                (State::CommandEscape, _) => State::Ground,
                (State::Skipping, BEL) => State::Ground,
                (State::Skipping, ESC) => State::SkippingEscape,
                (State::Skipping, _) => State::Skipping,
                (State::SkippingEscape, b'\\') => State::Ground,
                (State::SkippingEscape, _) => State::Skipping,
            };
        }
    }
}

/// The folder a command to the operating system reports, as the shell
/// wrote it: the path of an OSC 7 address (`/C:/Users/me`, `/home/me`)
/// with its escapes undone, or the path of OSC 9;9 (`C:\Users\me`).
/// None for any other command.
pub fn reported_folder(command: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(command).ok()?;
    if let Some(address) = text.strip_prefix("7;") {
        let rest = address.strip_prefix("file://")?;
        // The host comes before the path; the path begins at the first `/`.
        let path = &rest[rest.find('/')?..];
        let path = percent_decoded(path)?;
        return (!path.is_empty()).then_some(path);
    }
    if let Some(path) = text.strip_prefix("9;9;") {
        let path = path.trim();
        let path = { path.strip_prefix('"') }
            .and_then(|path| path.strip_suffix('"'))
            .unwrap_or(path);
        return (!path.is_empty()).then(|| path.to_owned());
    }
    None
}

/// `text` with its `%XX` escapes made the bytes they stand for; none
/// where they make no UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let escape = (byte == b'%')
            .then(|| text.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(value) => {
                decoded.push(value);
                at += 3;
            }
            None => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(pieces: &[&[u8]]) -> Vec<String> {
        let mut scanner = OscScanner::default();
        let mut found = Vec::new();
        for piece in pieces {
            scanner.feed(piece, |command| {
                found.push(String::from_utf8_lossy(command).into_owned())
            });
        }
        found
    }

    #[test]
    fn commands_are_found_whatever_ends_them_and_wherever_they_are_cut() {
        assert_eq!(
            commands(&[b"ab\x1b]0;title\x07cd\x1b]7;file://pc/C:/x\x1b\\"]),
            ["0;title", "7;file://pc/C:/x"]
        );
        assert_eq!(
            commands(&[b"\x1b]9;9;", b"\"C:\\a b\"", b"\x1b", b"\\done"]),
            ["9;9;\"C:\\a b\""]
        );
        // Other sequences are no commands.
        assert!(commands(&[b"\x1b[31mred\x1b[0m\x1bPq\x1b\\"]).is_empty());
        // A cancelled one is dropped.
        assert!(commands(&[b"\x1b]7;file://x\x18\x07"]).is_empty());
    }

    #[test]
    fn a_command_too_long_is_let_go() {
        let mut long = b"\x1b]7;".to_vec();
        long.extend(std::iter::repeat_n(b'a', LONGEST + 10));
        long.extend(b"\x07\x1b]2;next\x07");
        assert_eq!(commands(&[&long]), ["2;next"]);
    }

    #[test]
    fn the_folder_is_read_from_osc_7_and_osc_9_9() {
        let folder = |text: &str| reported_folder(text.as_bytes());
        assert_eq!(
            folder("7;file://pc/C:/Users/me/My%20files"),
            Some("/C:/Users/me/My files".to_owned())
        );
        assert_eq!(
            folder("7;file:///home/me/%D0%B4"),
            Some("/home/me/д".to_owned())
        );
        assert_eq!(
            folder("9;9;\"C:\\Program Files\""),
            Some("C:\\Program Files".to_owned())
        );
        assert_eq!(folder("9;9;D:\\work"), Some("D:\\work".to_owned()));
        assert_eq!(folder("0;a title"), None);
        assert_eq!(folder("7;https://example.com/"), None);
        assert_eq!(folder("9;4;1;50"), None);
    }
}

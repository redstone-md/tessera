// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What a key pressed in the terminal sends to the shell, as xterm sends
//! it: the arrows, the keys of a page and the function keys as escape
//! sequences (the arrows in the form of the application cursor mode when
//! a program asks for it), Ctrl with a letter as its control character,
//! Alt as an escape before the key, and what is pasted.

/// The keys held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Held {
    /// The number xterm puts in a sequence for the keys held: one more
    /// than Shift as 1, Alt as 2 and Ctrl as 4 added up; 1 for none.
    fn parameter(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.control)
    }

    fn any(self) -> bool {
        self.control || self.alt || self.shift
    }
}

/// What a key press sends to the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    /// These bytes.
    Bytes(Vec<u8>),
    /// The text the key writes, which comes through the text input of the
    /// window, so that dead keys and input methods work.
    Text,
    /// Nothing: the key means nothing to a terminal.
    Nothing,
}

const ESC: u8 = 0x1b;

/// What the key `key`, as GPUI names it, sends with `held`. `typed` is the
/// text the key writes with the keys held, where it writes one.
/// `app_cursor` is whether a program asked for the arrows of the
/// application cursor mode.
pub fn encode(key: &str, typed: Option<&str>, held: Held, app_cursor: bool) -> Sent {
    if let Some(sent) = named(key, held, app_cursor) {
        return sent;
    }
    let mut letters = key.chars();
    let (Some(letter), None) = (letters.next(), letters.next()) else {
        return Sent::Nothing;
    };
    let typed = typed.filter(|typed| !typed.chars().any(char::is_control));
    // Ctrl and Alt together write letters on some keyboards (AltGr).
    if held.control && held.alt && typed.is_some() {
        return Sent::Text;
    }
    if held.control {
        let mut bytes = Vec::new();
        if held.alt {
            bytes.push(ESC);
        }
        match control_of(letter) {
            Some(control) => bytes.push(control),
            None => bytes.extend(letter.to_string().as_bytes()),
        }
        return Sent::Bytes(bytes);
    }
    if held.alt {
        let shown = typed.map_or_else(|| letter.to_string(), str::to_owned);
        let mut bytes = vec![ESC];
        bytes.extend(shown.as_bytes());
        return Sent::Bytes(bytes);
    }
    Sent::Text
}

/// What a key that has a name sends; none for a key that writes a letter.
fn named(key: &str, held: Held, app_cursor: bool) -> Option<Sent> {
    let with_alt = |bytes: &[u8]| {
        let mut sent = Vec::new();
        if held.alt {
            sent.push(ESC);
        }
        sent.extend(bytes);
        Sent::Bytes(sent)
    };
    let sent = match key {
        "enter" => with_alt(b"\r"),
        "backspace" if held.control => with_alt(&[0x08]),
        "backspace" => with_alt(&[0x7f]),
        "tab" if held.shift => Sent::Bytes(b"\x1b[Z".to_vec()),
        "tab" => with_alt(b"\t"),
        "escape" => with_alt(&[ESC]),
        "space" if held.control => with_alt(&[0]),
        "space" if held.alt => with_alt(b" "),
        "space" => Sent::Text,
        "up" => cursor_key(b'A', held, app_cursor),
        "down" => cursor_key(b'B', held, app_cursor),
        "right" => cursor_key(b'C', held, app_cursor),
        "left" => cursor_key(b'D', held, app_cursor),
        "home" => cursor_key(b'H', held, app_cursor),
        "end" => cursor_key(b'F', held, app_cursor),
        "insert" => tilde_key(2, held),
        "delete" => tilde_key(3, held),
        "pageup" => tilde_key(5, held),
        "pagedown" => tilde_key(6, held),
        "f1" => function_key(b'P', held),
        "f2" => function_key(b'Q', held),
        "f3" => function_key(b'R', held),
        "f4" => function_key(b'S', held),
        "f5" => tilde_key(15, held),
        "f6" => tilde_key(17, held),
        "f7" => tilde_key(18, held),
        "f8" => tilde_key(19, held),
        "f9" => tilde_key(20, held),
        "f10" => tilde_key(21, held),
        "f11" => tilde_key(23, held),
        "f12" => tilde_key(24, held),
        _ if key.chars().count() > 1 => Sent::Nothing,
        _ => return None,
    };
    Some(sent)
}

/// An arrow, Home or End: `ESC [ A`, `ESC O A` in the application cursor
/// mode, `ESC [ 1 ; m A` with keys held.
fn cursor_key(last: u8, held: Held, app_cursor: bool) -> Sent {
    let bytes = match (held.any(), app_cursor) {
        (false, true) => vec![ESC, b'O', last],
        (false, false) => vec![ESC, b'[', last],
        (true, _) => {
            let mut bytes = format!("\x1b[1;{}", held.parameter()).into_bytes();
            bytes.push(last);
            bytes
        }
    };
    Sent::Bytes(bytes)
}

/// F1 to F4: `ESC O P`, `ESC [ 1 ; m P` with keys held.
fn function_key(last: u8, held: Held) -> Sent {
    let bytes = match held.any() {
        false => vec![ESC, b'O', last],
        true => {
            let mut bytes = format!("\x1b[1;{}", held.parameter()).into_bytes();
            bytes.push(last);
            bytes
        }
    };
    Sent::Bytes(bytes)
}

/// A key sent as `ESC [ n ~`, `ESC [ n ; m ~` with keys held.
fn tilde_key(number: u8, held: Held) -> Sent {
    let text = match held.any() {
        false => format!("\x1b[{number}~"),
        true => format!("\x1b[{number};{}~", held.parameter()),
    };
    Sent::Bytes(text.into_bytes())
}

/// The control character Ctrl makes of `letter`, as xterm has it.
fn control_of(letter: char) -> Option<u8> {
    let letter = letter.to_ascii_lowercase();
    match letter {
        'a'..='z' => Some(letter as u8 - b'a' + 1),
        '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' | '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// What pasting `text` sends: its line ends as Enter sends them, and with
/// `bracketed`, as a program that asked for bracketed paste reads it,
/// between the marks of a paste and without escapes that could end it.
pub fn pasted(text: &str, bracketed: bool) -> Vec<u8> {
    let lines = text.replace("\r\n", "\r").replace('\n', "\r");
    match bracketed {
        false => lines.into_bytes(),
        true => {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend(lines.bytes().filter(|&byte| byte != ESC));
            bytes.extend(b"\x1b[201~");
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Held = Held {
        control: false,
        alt: false,
        shift: false,
    };
    const CTRL: Held = Held {
        control: true,
        ..NONE
    };
    const ALT: Held = Held { alt: true, ..NONE };
    const SHIFT: Held = Held {
        shift: true,
        ..NONE
    };

    fn bytes(sent: Sent) -> Vec<u8> {
        match sent {
            Sent::Bytes(bytes) => bytes,
            other => panic!("{other:?} is no bytes"),
        }
    }

    #[test]
    fn the_arrows_follow_the_application_cursor_mode() {
        assert_eq!(bytes(encode("up", None, NONE, false)), b"\x1b[A");
        assert_eq!(bytes(encode("up", None, NONE, true)), b"\x1bOA");
        assert_eq!(bytes(encode("left", None, NONE, false)), b"\x1b[D");
        assert_eq!(bytes(encode("home", None, NONE, true)), b"\x1bOH");
        // Keys held name themselves whatever the mode.
        assert_eq!(bytes(encode("right", None, CTRL, true)), b"\x1b[1;5C");
        assert_eq!(bytes(encode("end", None, SHIFT, false)), b"\x1b[1;2F");
        let both = Held { shift: true, ..ALT };
        assert_eq!(bytes(encode("down", None, both, false)), b"\x1b[1;4B");
    }

    #[test]
    fn the_keys_of_a_page_and_the_function_keys_are_sequences() {
        assert_eq!(bytes(encode("delete", None, NONE, false)), b"\x1b[3~");
        assert_eq!(bytes(encode("pageup", None, CTRL, false)), b"\x1b[5;5~");
        assert_eq!(bytes(encode("f1", None, NONE, false)), b"\x1bOP");
        assert_eq!(bytes(encode("f4", None, SHIFT, false)), b"\x1b[1;2S");
        assert_eq!(bytes(encode("f5", None, NONE, false)), b"\x1b[15~");
        assert_eq!(bytes(encode("f12", None, ALT, false)), b"\x1b[24;3~");
        assert_eq!(encode("f13", None, NONE, false), Sent::Nothing);
        assert_eq!(encode("menu", None, NONE, false), Sent::Nothing);
    }

    #[test]
    fn ctrl_makes_control_characters() {
        assert_eq!(bytes(encode("c", Some("c"), CTRL, false)), [3]);
        assert_eq!(bytes(encode("a", None, CTRL, false)), [1]);
        assert_eq!(bytes(encode("[", None, CTRL, false)), [0x1b]);
        assert_eq!(bytes(encode("-", None, CTRL, false)), [0x1f]);
        assert_eq!(bytes(encode("space", None, CTRL, false)), [0]);
        let shifted = Held {
            shift: true,
            ..CTRL
        };
        assert_eq!(bytes(encode("z", Some("Z"), shifted, false)), [26]);
        // Ctrl with a key that has no control character sends the key.
        assert_eq!(bytes(encode("1", None, CTRL, false)), b"1");
        let both = Held { alt: true, ..CTRL };
        assert_eq!(bytes(encode("x", None, both, false)), [0x1b, 0x18]);
    }

    #[test]
    fn alt_puts_an_escape_before_the_key() {
        assert_eq!(bytes(encode("b", Some("b"), ALT, false)), b"\x1bb");
        let shifted = Held { shift: true, ..ALT };
        assert_eq!(bytes(encode("b", Some("B"), shifted, false)), b"\x1bB");
        assert_eq!(bytes(encode("enter", None, ALT, false)), b"\x1b\r");
        assert_eq!(bytes(encode("backspace", None, ALT, false)), b"\x1b\x7f");
    }

    #[test]
    fn letters_come_through_the_text_input() {
        assert_eq!(encode("a", Some("a"), NONE, false), Sent::Text);
        assert_eq!(encode("a", Some("A"), SHIFT, false), Sent::Text);
        assert_eq!(encode("space", None, SHIFT, false), Sent::Text);
        // AltGr+Q writes @ on a German keyboard.
        let altgr = Held { alt: true, ..CTRL };
        assert_eq!(encode("q", Some("@"), altgr, false), Sent::Text);
    }

    #[test]
    fn the_keys_that_edit_send_what_shells_expect() {
        assert_eq!(bytes(encode("enter", None, NONE, false)), b"\r");
        assert_eq!(bytes(encode("backspace", None, NONE, false)), [0x7f]);
        assert_eq!(bytes(encode("backspace", None, CTRL, false)), [0x08]);
        assert_eq!(bytes(encode("tab", None, NONE, false)), b"\t");
        assert_eq!(bytes(encode("tab", None, SHIFT, false)), b"\x1b[Z");
        assert_eq!(bytes(encode("escape", None, NONE, false)), [0x1b]);
    }

    #[test]
    fn a_paste_ends_its_lines_as_enter_does_and_is_bracketed_when_asked() {
        assert_eq!(pasted("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(pasted("x\x1b[201~y", true), b"\x1b[200~x[201~y\x1b[201~");
    }
}

// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The calculator: arithmetic typed into the launcher is answered with
//! its result, which Enter copies.
//!
//! It knows `+ - * / ^ %` (`**` for `^`, `×` and `÷` too), `mod`, `!`,
//! parentheses, the functions of [`FUNCTIONS`], the constants `pi`, `e`
//! and `tau`, and numbers written in hexadecimal (`0xff`), binary
//! (`0b101`) or octal (`0o17`). `%` after a number is a percentage, and
//! between two numbers the remainder. Angles are in radians. A trailing
//! `to hex`, `in bin`, `to oct` or `to dec` gives the result in that
//! base. A text is answered only where it calculates something, so that
//! a plain number or word is left to the other answers; a leading `=`
//! has any expression answered.

use super::{Answer, AnswerAction, Query, Runner};

const GLYPH: &str = "\u{e8ef}";

/// How sure the calculator is that a text is meant for it.
const SCORE: u16 = 900;

/// The functions, each with how many arguments it takes at the least
/// and at the most.
const FUNCTIONS: [(&str, usize, usize); 23] = [
    ("sin", 1, 1),
    ("cos", 1, 1),
    ("tan", 1, 1),
    ("asin", 1, 1),
    ("acos", 1, 1),
    ("atan", 1, 1),
    ("sinh", 1, 1),
    ("cosh", 1, 1),
    ("tanh", 1, 1),
    ("sqrt", 1, 1),
    ("cbrt", 1, 1),
    ("ln", 1, 1),
    ("log", 1, 2),
    ("log2", 1, 1),
    ("log10", 1, 1),
    ("exp", 1, 1),
    ("abs", 1, 1),
    ("floor", 1, 1),
    ("ceil", 1, 1),
    ("round", 1, 1),
    ("trunc", 1, 1),
    ("min", 1, usize::MAX),
    ("max", 1, usize::MAX),
];

/// The largest whole number shown without an exponent, and the largest
/// a factorial is taken of.
const LARGEST_WHOLE: f64 = 1e15;
const LARGEST_FACTORIAL: f64 = 170.;

/// The significant digits a result is shown with.
const DIGITS: i32 = 12;

/// The base a number is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Decimal,
    Hexadecimal,
    Binary,
    Octal,
}

impl Base {
    fn radix(self) -> u32 {
        match self {
            Self::Decimal => 10,
            Self::Hexadecimal => 16,
            Self::Binary => 2,
            Self::Octal => 8,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Decimal => "",
            Self::Hexadecimal => "0x",
            Self::Binary => "0b",
            Self::Octal => "0o",
        }
    }

    /// The base a conversion names: `hex`, `bin`, `oct`, `dec` and their
    /// long names.
    fn named(name: &str) -> Option<Self> {
        match name {
            "hex" | "hexadecimal" => Some(Self::Hexadecimal),
            "bin" | "binary" => Some(Self::Binary),
            "oct" | "octal" => Some(Self::Octal),
            "dec" | "decimal" => Some(Self::Decimal),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    /// One of `+ - * / ^ % !`.
    Operator(char),
    Open,
    Close,
    Comma,
}

/// The tokens of `text`, and the base of the last number written in
/// another base than ten, if one is; `None` where something in it is
/// no part of an expression.
fn tokens(text: &str) -> Option<(Vec<Token>, Option<Base>)> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut based = None;
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        match c {
            _ if c.is_whitespace() => at += 1,
            '0' if matches!(chars.get(at + 1), Some('x' | 'X' | 'b' | 'B' | 'o' | 'O')) => {
                let base = match chars[at + 1].to_ascii_lowercase() {
                    'x' => Base::Hexadecimal,
                    'b' => Base::Binary,
                    _ => Base::Octal,
                };
                let start = at + 2;
                let mut end = start;
                while end < chars.len() && chars[end].is_digit(base.radix()) {
                    end += 1;
                }
                let digits: String = chars[start..end].iter().collect();
                let value = u64::from_str_radix(&digits, base.radix()).ok()?;
                tokens.push(Token::Number(value as f64));
                based = Some(base);
                at = end;
            }
            '0'..='9' | '.' => {
                let start = at;
                while at < chars.len() && (chars[at].is_ascii_digit() || chars[at] == '.') {
                    at += 1;
                }
                // An exponent: e, a sign maybe, and digits.
                if matches!(chars.get(at), Some('e' | 'E')) {
                    let sign = usize::from(matches!(chars.get(at + 1), Some('+' | '-')));
                    if chars.get(at + 1 + sign).is_some_and(char::is_ascii_digit) {
                        at += 1 + sign;
                        while at < chars.len() && chars[at].is_ascii_digit() {
                            at += 1;
                        }
                    }
                }
                let number: String = chars[start..at].iter().collect();
                tokens.push(Token::Number(number.parse().ok()?));
            }
            _ if c.is_alphabetic() || c == '_' => {
                let start = at;
                while at < chars.len() && (chars[at].is_alphanumeric() || chars[at] == '_') {
                    at += 1;
                }
                let name: String = chars[start..at].iter().collect();
                tokens.push(Token::Name(name.to_lowercase()));
            }
            '*' if chars.get(at + 1) == Some(&'*') => {
                tokens.push(Token::Operator('^'));
                at += 2;
            }
            '+' | '-' | '*' | '/' | '^' | '%' | '!' => {
                tokens.push(Token::Operator(c));
                at += 1;
            }
            '×' | '·' | '∙' => {
                tokens.push(Token::Operator('*'));
                at += 1;
            }
            '÷' | '∕' => {
                tokens.push(Token::Operator('/'));
                at += 1;
            }
            '−' => {
                tokens.push(Token::Operator('-'));
                at += 1;
            }
            '(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            ',' | ';' => {
                tokens.push(Token::Comma);
                at += 1;
            }
            _ => return None,
        }
    }
    Some((tokens, based))
}

/// A Pratt parser that works the value out as it reads.
struct Parser {
    tokens: Vec<Token>,
    at: usize,
    /// Whether anything was calculated: an operator between values, a
    /// function, a percentage or a factorial.
    calculated: bool,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    /// Whether the token after the one being looked at starts a value,
    /// which makes a `%` before it the remainder instead of a percentage.
    fn value_follows(&self) -> bool {
        match self.tokens.get(self.at + 1) {
            Some(Token::Number(_) | Token::Open) => true,
            Some(Token::Name(name)) => name != "mod",
            _ => false,
        }
    }

    /// The value of the expression from here, of the operators that bind
    /// more tightly than `least`.
    fn expression(&mut self, least: u8) -> Option<f64> {
        let mut value = self.operand()?;
        loop {
            let (operator, left, right) = match self.peek() {
                Some(Token::Operator('!')) => {
                    self.next();
                    value = factorial(value)?;
                    self.calculated = true;
                    continue;
                }
                Some(Token::Operator('%')) if !self.value_follows() => {
                    self.next();
                    value /= 100.;
                    self.calculated = true;
                    continue;
                }
                Some(Token::Operator(operator @ ('+' | '-'))) => (*operator, 1, 2),
                Some(Token::Operator(operator @ ('*' | '/' | '%'))) => (*operator, 3, 4),
                Some(Token::Name(name)) if name == "mod" => ('%', 3, 4),
                // Raising binds to the right: 2^3^2 is 2^9.
                Some(Token::Operator('^')) => ('^', 8, 7),
                _ => break,
            };
            if left < least {
                break;
            }
            self.next();
            let other = self.expression(right)?;
            self.calculated = true;
            value = match operator {
                '+' => value + other,
                '-' => value - other,
                '*' => value * other,
                '/' if other == 0. => return None,
                '/' => value / other,
                '%' if other == 0. => return None,
                '%' => value % other,
                _ => value.powf(other),
            };
        }
        Some(value)
    }

    /// A number, a constant, a function with its arguments, an
    /// expression in parentheses, or one of these with a sign.
    fn operand(&mut self) -> Option<f64> {
        match self.next()? {
            Token::Number(number) => Some(number),
            // A sign binds less tightly than raising: -2^2 is -4.
            Token::Operator('-') => Some(-self.expression(6)?),
            Token::Operator('+') => self.expression(6),
            Token::Open => {
                let value = self.expression(0)?;
                (self.next()? == Token::Close).then_some(value)
            }
            Token::Name(name) => match name.as_str() {
                "pi" | "π" => Some(std::f64::consts::PI),
                "e" => Some(std::f64::consts::E),
                "tau" | "τ" => Some(std::f64::consts::TAU),
                _ => self.call(&name),
            },
            _ => None,
        }
    }

    /// The function `name` applied to the arguments in parentheses that
    /// follow it.
    fn call(&mut self, name: &str) -> Option<f64> {
        let &(_, least, most) = FUNCTIONS.iter().find(|(known, ..)| *known == name)?;
        if self.next()? != Token::Open {
            return None;
        }
        let mut arguments = vec![self.expression(0)?];
        loop {
            match self.next()? {
                Token::Comma => arguments.push(self.expression(0)?),
                Token::Close => break,
                _ => return None,
            }
        }
        if arguments.len() < least || arguments.len() > most {
            return None;
        }
        self.calculated = true;
        let x = arguments[0];
        Some(match name {
            "sin" => x.sin(),
            "cos" => x.cos(),
            "tan" => x.tan(),
            "asin" => x.asin(),
            "acos" => x.acos(),
            "atan" => x.atan(),
            "sinh" => x.sinh(),
            "cosh" => x.cosh(),
            "tanh" => x.tanh(),
            "sqrt" => x.sqrt(),
            "cbrt" => x.cbrt(),
            "ln" => x.ln(),
            "log" => match arguments.get(1) {
                Some(&base) => x.log(base),
                None => x.log10(),
            },
            "log2" => x.log2(),
            "log10" => x.log10(),
            "exp" => x.exp(),
            "abs" => x.abs(),
            "floor" => x.floor(),
            "ceil" => x.ceil(),
            "round" => x.round(),
            "trunc" => x.trunc(),
            "min" => arguments.into_iter().fold(f64::INFINITY, f64::min),
            _ => arguments.into_iter().fold(f64::NEG_INFINITY, f64::max),
        })
    }
}

/// `n!` for a whole `n` from 0 to 170.
fn factorial(n: f64) -> Option<f64> {
    if n < 0. || n.fract() != 0. || n > LARGEST_FACTORIAL {
        return None;
    }
    Some((1..=n as u32).fold(1., |product, k| product * f64::from(k)))
}

/// What the calculator makes of a text.
#[derive(Clone, Debug, PartialEq)]
pub struct Calculation {
    /// The expression, without a leading `=` or a conversion.
    pub expression: String,
    pub value: f64,
    /// The base the result is shown in besides ten, where one was asked
    /// for or a number was written in it.
    pub base: Option<Base>,
    /// Whether the base was asked for with `to hex` and the like, which
    /// has the result shown in it first.
    pub converted: bool,
}

impl Calculation {
    /// What `text` calculates to, if it is an expression that calculates
    /// something, or one after `=`.
    pub fn of(text: &str) -> Option<Self> {
        let text = text.trim();
        let (forced, text) = match text.strip_prefix('=') {
            Some(rest) => (true, rest.trim()),
            None => (false, text),
        };
        let (text, asked) = split_conversion(text);
        let (tokens, written) = tokens(text)?;
        let mut parser = Parser {
            tokens,
            at: 0,
            calculated: false,
        };
        let value = parser.expression(0)?;
        if parser.at != parser.tokens.len() || !value.is_finite() {
            return None;
        }
        let meant = forced || parser.calculated || asked.is_some() || written.is_some();
        meant.then(|| Self {
            expression: text.to_owned(),
            value,
            base: asked.or(written),
            converted: asked.is_some(),
        })
    }

    /// The result as it is shown and copied: in the base asked for where
    /// one was and the result is whole, else in base ten.
    pub fn result(&self) -> String {
        match self.based() {
            Some(based) if self.converted => based,
            _ => format_number(self.value),
        }
    }

    /// The result in the other base, where there is one and it is whole.
    pub fn based(&self) -> Option<String> {
        let base = self.base.filter(|&base| base != Base::Decimal)?;
        in_base(self.value, base)
    }
}

/// `text` without a trailing `to hex`, `in bin` and the like, and the
/// base it names. The words are ASCII, so an ASCII lowercase copy keeps
/// the byte offsets of `text` it is sliced at.
fn split_conversion(text: &str) -> (&str, Option<Base>) {
    let lower = text.to_ascii_lowercase();
    for word in [" to ", " in ", " as "] {
        if let Some(at) = lower.rfind(word)
            && let Some(base) = Base::named(lower[at + word.len()..].trim())
        {
            return (text[..at].trim(), Some(base));
        }
    }
    (text, None)
}

/// `value` written in `base` with its prefix, where it is whole and
/// small enough to be exact.
fn in_base(value: f64, base: Base) -> Option<String> {
    if value.fract() != 0. || value.abs() >= 2f64.powi(63) {
        return None;
    }
    let whole = value.abs() as u64;
    let digits = match base {
        Base::Decimal => whole.to_string(),
        Base::Hexadecimal => format!("{whole:X}"),
        Base::Binary => format!("{whole:b}"),
        Base::Octal => format!("{whole:o}"),
    };
    let sign = if value < 0. { "-" } else { "" };
    Some(format!("{sign}{}{digits}", base.prefix()))
}

/// `value` as a person writes it: whole numbers without a fraction,
/// others with up to twelve significant digits and no zeros at the end,
/// very large and very small ones with an exponent.
pub fn format_number(value: f64) -> String {
    format_digits(value, DIGITS)
}

/// `value` as [`format_number`] writes it, with up to `digits`
/// significant digits.
pub fn format_digits(value: f64, digits: i32) -> String {
    if value == 0. {
        return "0".to_owned();
    }
    let magnitude = value.abs();
    if value.fract() == 0. && magnitude < LARGEST_WHOLE {
        return format!("{value:.0}");
    }
    if !(1e-9..LARGEST_WHOLE).contains(&magnitude) {
        let written = format!("{value:.*e}", (digits - 1).max(0) as usize);
        let (mantissa, exponent) = written.split_once('e').unwrap_or((&written, "0"));
        return format!("{}e{exponent}", trimmed(mantissa));
    }
    let whole_digits = magnitude.log10().floor() as i32 + 1;
    let decimals = (digits - whole_digits).clamp(0, 15) as usize;
    trimmed(&format!("{value:.decimals$}")).to_owned()
}

/// A decimal number without the zeros at the end of its fraction, or
/// its point where nothing is left after it.
fn trimmed(number: &str) -> &str {
    if !number.contains('.') {
        return number;
    }
    number.trim_end_matches('0').trim_end_matches('.')
}

pub struct Calculator;

impl Runner for Calculator {
    fn id(&self) -> &'static str {
        "calculator"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let Some(calculation) = Calculation::of(query.text()) else {
            return Vec::new();
        };
        let result = calculation.result();
        let mut detail = format!("{} =", calculation.expression);
        if let Some(based) = calculation.based().filter(|based| *based != result) {
            detail = format!("{detail} {based}");
        } else if calculation.converted {
            detail = format!("{detail} {}", format_number(calculation.value));
        }
        let detail = format!("{detail} · {}", query.say("Calculator"));
        let title = format!("= {result}");
        let copy = AnswerAction::Copy(result.clone());
        vec![Answer::new(title, detail, GLYPH, SCORE, copy).copying(result)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> Option<f64> {
        Calculation::of(text).map(|calculation| calculation.value)
    }

    fn shown(text: &str) -> Option<String> {
        Calculation::of(text).map(|calculation| calculation.result())
    }

    #[test]
    fn arithmetic_keeps_the_order_of_operations() {
        assert_eq!(value("2 + 3 * 4"), Some(14.));
        assert_eq!(value("(2 + 3) * 4"), Some(20.));
        assert_eq!(value("10 - 4 - 3"), Some(3.));
        assert_eq!(value("2 ^ 3 ^ 2"), Some(512.));
        assert_eq!(value("2 ** 10"), Some(1024.));
        assert_eq!(value("-2 ^ 2"), Some(-4.));
        assert_eq!(value("2 ^ -1"), Some(0.5));
        assert_eq!(value("7 × 6 ÷ 3"), Some(14.));
        assert_eq!(value("3 − 5"), Some(-2.));
        assert_eq!(value("1.5e3 / 3"), Some(500.));
        assert_eq!(value(".5 + .25"), Some(0.75));
    }

    #[test]
    fn percent_is_a_percentage_or_the_remainder() {
        assert_eq!(value("15%"), Some(0.15));
        assert_eq!(value("200 * 15%"), Some(30.));
        assert_eq!(value("17 % 5"), Some(2.));
        assert_eq!(value("17 mod 5"), Some(2.));
        assert_eq!(value("(10)% + 1"), Some(1.1));
        assert_eq!(value("5!"), Some(120.));
        assert_eq!(value("3! + 1"), Some(7.));
    }

    #[test]
    fn functions_and_constants_are_known() {
        assert_eq!(value("sqrt(16)"), Some(4.));
        assert!((value("log(1000)").unwrap() - 3.).abs() < 1e-12);
        assert!((value("log(8, 2)").unwrap() - 3.).abs() < 1e-12);
        assert_eq!(value("max(1, 7, 3)"), Some(7.));
        assert_eq!(value("abs(-3) + round(2.5)"), Some(6.));
        assert!((value("sin(pi / 2)").unwrap() - 1.).abs() < 1e-12);
        assert!((value("= tau").unwrap() - std::f64::consts::TAU).abs() < 1e-12);
        assert!((value("ln(e)").unwrap() - 1.).abs() < 1e-12);
        assert_eq!(value("sqrt 16"), None);
        assert_eq!(value("max()"), None);
        assert_eq!(value("frob(2)"), None);
    }

    #[test]
    fn only_what_calculates_something_is_answered() {
        assert_eq!(value("42"), None);
        assert_eq!(value("pi"), None);
        assert_eq!(value("= 42"), Some(42.));
        assert_eq!(value("notepad"), None);
        assert_eq!(value("2 pi"), None);
        assert_eq!(value("1 / 0"), None);
        assert_eq!(value("(2 + 3"), None);
        assert_eq!(value("2 +"), None);
        assert_eq!(value("sqrt(-1) + 1"), None);
        assert_eq!(value("C:\\Windows"), None);
    }

    #[test]
    fn numbers_of_other_bases_are_read_and_written() {
        assert_eq!(value("0xff + 1"), Some(256.));
        assert_eq!(value("0b101 * 2"), Some(10.));
        assert_eq!(value("0o17"), Some(15.));
        assert_eq!(shown("255 to hex").as_deref(), Some("0xFF"));
        assert_eq!(shown("10 in bin").as_deref(), Some("0b1010"));
        assert_eq!(shown("0x10 to dec").as_deref(), Some("16"));
        assert_eq!(shown("-8 as oct").as_deref(), Some("-0o10"));
        let hex = Calculation::of("0xff + 1").unwrap();
        assert_eq!(hex.result(), "256");
        assert_eq!(hex.based().as_deref(), Some("0x100"));
        // A fraction has no other base to be shown in.
        assert_eq!(shown("1 / 2 to hex").as_deref(), Some("0.5"));
        assert_eq!(shown("255 TO HEX").as_deref(), Some("0xFF"));
        // Letters that grow when lowercased leave the offsets as they are.
        assert_eq!(shown("İİİİİİİİ to hex"), None);
    }

    #[test]
    fn results_are_written_as_a_person_writes_them() {
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(4.), "4");
        assert_eq!(format_number(-12.5), "-12.5");
        assert_eq!(format_number(1. / 3.), "0.333333333333");
        assert_eq!(format_number(2f64.powi(60)), "1.15292150461e18");
        assert_eq!(format_number(1.5e-12), "1.5e-12");
        assert_eq!(format_number(123456.789), "123456.789");
        assert_eq!(format_digits(3.106855961, 7), "3.106856");
        assert_eq!(format_digits(1234.5678, 3), "1235");
    }
}

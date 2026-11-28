// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is written in the search field of the file manager, read as the
//! Advanced Query Syntax of Explorer reads it: bare words and `"quoted
//! phrases"` that names must hold, and the filters `kind:`, `modified:`
//! (or `date:`), `size:`, `author:`, `tag:`, `ext:` (or `type:`) and
//! `content:`. A filter whose value cannot be read is taken as words, as
//! a path such as `C:\Windows` is.
//!
//! Nothing here touches the disk: the same [`Query`] is asked of the index
//! of Windows Search ([`Query::index_search`]) and of the folders read one
//! after another ([`Query::fits`], [`Query::name_matches`]), and the chips
//! under the field change the text through [`with_filter`], so that what
//! they set can be read and changed by hand like anything typed.

use std::path::Path;

use chrono::{Datelike, Days, Months, NaiveDate};

use crate::platform::FileSearch;

/// The kinds of files the chips offer and `kind:` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Documents,
    Pictures,
    Music,
    Videos,
    Folders,
    Archives,
}

impl Kind {
    pub const ALL: [Self; 6] = [
        Self::Documents,
        Self::Pictures,
        Self::Music,
        Self::Videos,
        Self::Folders,
        Self::Archives,
    ];

    /// What the chips call it, in English, as the table of texts has it.
    pub fn title(self) -> &'static str {
        match self {
            Self::Documents => "Documents",
            Self::Pictures => "Pictures",
            Self::Music => "Music",
            Self::Videos => "Videos",
            Self::Folders => "Folders",
            Self::Archives => "Archives",
        }
    }

    /// What `kind:` is written with for it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Documents => "documents",
            Self::Pictures => "pictures",
            Self::Music => "music",
            Self::Videos => "videos",
            Self::Folders => "folders",
            Self::Archives => "archives",
        }
    }

    /// The kind `word` names, in any case, with other words for some.
    pub fn named(word: &str) -> Option<Self> {
        let word = word.trim().to_lowercase();
        let kind = match word.as_str() {
            "document" | "documents" | "doc" | "docs" | "text" => Self::Documents,
            "picture" | "pictures" | "image" | "images" | "photo" | "photos" => Self::Pictures,
            "music" | "audio" | "song" | "songs" => Self::Music,
            "video" | "videos" | "movie" | "movies" => Self::Videos,
            "folder" | "folders" | "dir" => Self::Folders,
            "archive" | "archives" | "zip" | "compressed" => Self::Archives,
            _ => return None,
        };
        Some(kind)
    }

    /// What Windows Search calls the kind (`System.Kind`); archives are
    /// none of its kinds and are told by their extensions.
    pub fn index_name(self) -> Option<&'static str> {
        match self {
            Self::Documents => Some("document"),
            Self::Pictures => Some("picture"),
            Self::Music => Some("music"),
            Self::Videos => Some("video"),
            Self::Folders => Some("folder"),
            Self::Archives => None,
        }
    }

    /// The extensions files of the kind have, in small letters, for where
    /// the index is not asked; none for folders.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Documents => &[
                "doc", "docx", "docm", "dot", "dotx", "odt", "rtf", "txt", "md", "pdf", "xps",
                "oxps", "xls", "xlsx", "xlsm", "xlsb", "ods", "csv", "ppt", "pptx", "pptm", "odp",
                "one", "epub", "msg", "eml", "tex", "log",
            ],
            Self::Pictures => &[
                "jpg", "jpeg", "jfif", "jpe", "png", "gif", "bmp", "dib", "tif", "tiff", "webp",
                "heic", "heif", "avif", "ico", "svg", "raw", "cr2", "cr3", "nef", "arw", "dng",
                "orf", "rw2", "psd", "jxr", "wdp",
            ],
            Self::Music => &[
                "mp3", "wav", "wma", "flac", "aac", "m4a", "ogg", "oga", "opus", "aiff", "aif",
                "mid", "midi", "ape", "alac", "m3u",
            ],
            Self::Videos => &[
                "mp4", "m4v", "mkv", "avi", "mov", "wmv", "webm", "mpg", "mpeg", "3gp", "3g2",
                "ts", "mts", "m2ts", "flv", "vob", "ogv",
            ],
            Self::Folders => &[],
            Self::Archives => &[
                "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst",
                "lz", "lzma", "cab", "iso", "arj", "z",
            ],
        }
    }

    /// Whether the item `name`, a folder or not, is of the kind.
    pub fn holds(self, name: &str, folder: bool) -> bool {
        match self {
            Self::Folders => folder,
            _ if folder => false,
            kind => extension_of(name).is_some_and(|ext| kind.extensions().contains(&ext.as_str())),
        }
    }
}

/// How a value is compared, as `>`, `>=`, `<`, `<=` or `=` before it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Above,
    AtLeast,
    Below,
    AtMost,
    Equal,
}

impl Comparison {
    /// The comparison at the start of `value`, and the rest; equality
    /// where none is written.
    fn read(value: &str) -> (Self, &str) {
        for (written, comparison) in [
            (">=", Self::AtLeast),
            ("<=", Self::AtMost),
            (">", Self::Above),
            ("<", Self::Below),
            ("=", Self::Equal),
        ] {
            if let Some(rest) = value.strip_prefix(written) {
                return (comparison, rest.trim_start());
            }
        }
        (Self::Equal, value)
    }

    fn written(self) -> &'static str {
        match self {
            Self::Above => ">",
            Self::AtLeast => ">=",
            Self::Below => "<",
            Self::AtMost => "<=",
            Self::Equal => "",
        }
    }
}

/// When a file was changed, as `modified:` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateSpan {
    Today,
    Yesterday,
    /// From the Monday of this week.
    ThisWeek,
    LastWeek,
    ThisMonth,
    LastMonth,
    ThisYear,
    LastYear,
    /// The seven days, the month or the year back from today, as the
    /// launcher writes `week`, `month` and `year`.
    PastWeek,
    PastMonth,
    PastYear,
    /// From the first day to the last, both included.
    Between(NaiveDate, NaiveDate),
    Compared(Comparison, NaiveDate),
}

impl DateSpan {
    /// The spans the chips offer, with their English titles.
    pub const CHIPS: [(Self, &'static str); 4] = [
        (Self::Today, "Today"),
        (Self::ThisWeek, "This week"),
        (Self::ThisMonth, "This month"),
        (Self::ThisYear, "This year"),
    ];

    /// The span `value` writes, as `today`, `this-week`, `"this week"`,
    /// `2026-01-31`, `>=2026-01-31` or `2026-01-01..2026-01-31`.
    pub fn read(value: &str) -> Option<Self> {
        let value = value.trim();
        let named = value.to_lowercase().replace([' ', '_'], "-");
        let span = match named.as_str() {
            "today" => Self::Today,
            "yesterday" => Self::Yesterday,
            "this-week" | "thisweek" => Self::ThisWeek,
            "last-week" | "lastweek" => Self::LastWeek,
            "this-month" | "thismonth" => Self::ThisMonth,
            "last-month" | "lastmonth" => Self::LastMonth,
            "this-year" | "thisyear" => Self::ThisYear,
            "last-year" | "lastyear" => Self::LastYear,
            "week" => Self::PastWeek,
            "month" => Self::PastMonth,
            "year" => Self::PastYear,
            _ => {
                if let Some((from, to)) = value.split_once("..") {
                    let (from, to) = (day_of(from)?, day_of(to)?);
                    return Some(Self::Between(from.min(to), from.max(to)));
                }
                let (comparison, day) = Comparison::read(value);
                return day_of(day).map(|day| Self::Compared(comparison, day));
            }
        };
        Some(span)
    }

    /// How `modified:` writes the span.
    pub fn written(self) -> String {
        let day = |day: NaiveDate| day.format("%Y-%m-%d").to_string();
        match self {
            Self::Today => "today".to_owned(),
            Self::Yesterday => "yesterday".to_owned(),
            Self::ThisWeek => "this-week".to_owned(),
            Self::LastWeek => "last-week".to_owned(),
            Self::ThisMonth => "this-month".to_owned(),
            Self::LastMonth => "last-month".to_owned(),
            Self::ThisYear => "this-year".to_owned(),
            Self::LastYear => "last-year".to_owned(),
            Self::PastWeek => "week".to_owned(),
            Self::PastMonth => "month".to_owned(),
            Self::PastYear => "year".to_owned(),
            Self::Between(from, to) => format!("{}..{}", day(from), day(to)),
            Self::Compared(comparison, at) => format!("{}{}", comparison.written(), day(at)),
        }
    }

    /// The days the span covers, `today` being today: from the first,
    /// until the one after the last; `None` where it is open that way.
    pub fn days(self, today: NaiveDate) -> (Option<NaiveDate>, Option<NaiveDate>) {
        let next = |day: NaiveDate| day.checked_add_days(Days::new(1));
        let monday = today
            .checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday())))
            .unwrap_or(today);
        let first_of_month = today.with_day(1).unwrap_or(today);
        let first_of_year = first_of_month.with_month(1).unwrap_or(first_of_month);
        match self {
            Self::Today => (Some(today), next(today)),
            Self::Yesterday => (today.checked_sub_days(Days::new(1)), Some(today)),
            Self::ThisWeek => (Some(monday), None),
            Self::LastWeek => (monday.checked_sub_days(Days::new(7)), Some(monday)),
            Self::ThisMonth => (Some(first_of_month), None),
            Self::LastMonth => (
                first_of_month.checked_sub_months(Months::new(1)),
                Some(first_of_month),
            ),
            Self::ThisYear => (Some(first_of_year), None),
            Self::LastYear => (
                first_of_year.checked_sub_months(Months::new(12)),
                Some(first_of_year),
            ),
            Self::PastWeek => (today.checked_sub_days(Days::new(7)), None),
            Self::PastMonth => (today.checked_sub_months(Months::new(1)), None),
            Self::PastYear => (today.checked_sub_months(Months::new(12)), None),
            Self::Between(from, to) => (Some(from), next(to)),
            Self::Compared(comparison, at) => match comparison {
                Comparison::Equal => (Some(at), next(at)),
                Comparison::Above => (next(at), None),
                Comparison::AtLeast => (Some(at), None),
                Comparison::Below => (None, Some(at)),
                Comparison::AtMost => (None, next(at)),
            },
        }
    }

    /// Whether a file changed on `day` is in the span.
    pub fn holds(self, day: NaiveDate, today: NaiveDate) -> bool {
        let (from, until) = self.days(today);
        from.is_none_or(|from| day >= from) && until.is_none_or(|until| day < until)
    }
}

/// The day `text` writes as `2026-01-31`.
fn day_of(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()
}

/// The sizes Explorer's filter of the size tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeTier {
    Empty,
    Tiny,
    Small,
    Medium,
    Large,
    Huge,
    Gigantic,
}

const KB: u64 = 1024;
const MB: u64 = 1024 * KB;
const GB: u64 = 1024 * MB;

impl SizeTier {
    pub const ALL: [Self; 7] = [
        Self::Empty,
        Self::Tiny,
        Self::Small,
        Self::Medium,
        Self::Large,
        Self::Huge,
        Self::Gigantic,
    ];

    /// What `size:` is written with for it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Tiny => "tiny",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
            Self::Huge => "huge",
            Self::Gigantic => "gigantic",
        }
    }

    /// What the chips call it, in English, with the sizes it covers, as
    /// the table of texts has it.
    pub fn title(self) -> &'static str {
        match self {
            Self::Empty => "Empty (0 KB)",
            Self::Tiny => "Tiny (0 – 10 KB)",
            Self::Small => "Small (10 – 100 KB)",
            Self::Medium => "Medium (100 KB – 1 MB)",
            Self::Large => "Large (1 – 16 MB)",
            Self::Huge => "Huge (16 – 128 MB)",
            Self::Gigantic => "Gigantic (over 128 MB)",
        }
    }

    fn named(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|tier| tier.word().eq_ignore_ascii_case(word.trim()))
    }

    /// The sizes in bytes it covers: from the first, until the one after
    /// the last.
    fn bounds(self) -> (u64, Option<u64>) {
        match self {
            Self::Empty => (0, Some(1)),
            Self::Tiny => (1, Some(10 * KB + 1)),
            Self::Small => (10 * KB + 1, Some(100 * KB + 1)),
            Self::Medium => (100 * KB + 1, Some(MB + 1)),
            Self::Large => (MB + 1, Some(16 * MB + 1)),
            Self::Huge => (16 * MB + 1, Some(128 * MB + 1)),
            Self::Gigantic => (128 * MB + 1, None),
        }
    }
}

/// How large a file is, as `size:` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeSpan {
    Tier(SizeTier),
    Compared(Comparison, u64),
    /// From the first number of bytes to the last, both included.
    Between(u64, u64),
}

impl SizeSpan {
    /// The span `value` writes: a tier as `large`, a size with a
    /// comparison as `>10MB`, or two sizes as `1MB..10MB`.
    pub fn read(value: &str) -> Option<Self> {
        if let Some(tier) = SizeTier::named(value) {
            return Some(Self::Tier(tier));
        }
        if let Some((from, to)) = value.split_once("..") {
            let (from, to) = (bytes_written(from)?, bytes_written(to)?);
            return Some(Self::Between(from.min(to), from.max(to)));
        }
        let (comparison, size) = Comparison::read(value.trim());
        bytes_written(size).map(|bytes| Self::Compared(comparison, bytes))
    }

    pub fn written(self) -> String {
        match self {
            Self::Tier(tier) => tier.word().to_owned(),
            Self::Compared(comparison, bytes) => {
                format!("{}{}", comparison.written(), bytes_text(bytes))
            }
            Self::Between(from, to) => format!("{}..{}", bytes_text(from), bytes_text(to)),
        }
    }

    /// The sizes in bytes it covers: from the first, until the one after
    /// the last, `None` where there is no last.
    pub fn bounds(self) -> (u64, Option<u64>) {
        match self {
            Self::Tier(tier) => tier.bounds(),
            Self::Between(from, to) => (from, to.checked_add(1)),
            Self::Compared(comparison, bytes) => match comparison {
                Comparison::Equal => (bytes, bytes.checked_add(1)),
                Comparison::Above => (bytes.saturating_add(1), None),
                Comparison::AtLeast => (bytes, None),
                Comparison::Below => (0, Some(bytes)),
                Comparison::AtMost => (0, bytes.checked_add(1)),
            },
        }
    }

    pub fn holds(self, size: u64) -> bool {
        let (from, until) = self.bounds();
        size >= from && until.is_none_or(|until| size < until)
    }
}

/// The number of bytes `text` writes, as `10MB`, `1.5 GB` or `4096`, in
/// units of 1024.
fn bytes_written(text: &str) -> Option<u64> {
    let text = text.trim().to_uppercase();
    let digits = text.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','));
    let (number, unit) = text.split_at(digits.unwrap_or(text.len()));
    let number: f64 = number.replace(',', ".").parse().ok()?;
    let power = match unit.trim() {
        "" | "B" => 0,
        "K" | "KB" | "KIB" => 1,
        "M" | "MB" | "MIB" => 2,
        "G" | "GB" | "GIB" => 3,
        "T" | "TB" | "TIB" => 4,
        _ => return None,
    };
    let bytes = number * 1024f64.powi(power);
    (bytes.is_finite() && bytes >= 0. && bytes < u64::MAX as f64).then_some(bytes as u64)
}

/// `bytes` as `size:` writes it: in the largest unit it is a whole
/// number of.
fn bytes_text(bytes: u64) -> String {
    for (unit, size) in [("GB", GB), ("MB", MB), ("KB", KB)] {
        if bytes >= size && bytes.is_multiple_of(size) {
            return format!("{}{unit}", bytes / size);
        }
    }
    bytes.to_string()
}

/// The families of filters the chips set, each by the keys that write it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Kind,
    Modified,
    Size,
    Tag,
    /// Whether bare words are looked for in what files hold too:
    /// `content:on`.
    Contents,
}

impl Filter {
    /// The key the chips write it with.
    fn key(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::Modified => "modified",
            Self::Size => "size",
            Self::Tag => "tag",
            Self::Contents => "content",
        }
    }

    /// Whether `token` sets this filter.
    fn set_by(self, token: &Token) -> bool {
        let Some(key) = token.key.as_deref() else {
            return false;
        };
        match self {
            Self::Kind => key == "kind",
            Self::Modified => matches!(key, "modified" | "date" | "datemodified"),
            Self::Size => key == "size",
            Self::Tag => matches!(key, "tag" | "tags" | "keywords"),
            Self::Contents => key == "content" && switch_of(&token.value).is_some(),
        }
    }
}

/// What `content:` says alone, where it says on or off.
fn switch_of(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "" | "on" | "yes" | "true" => Some(true),
        "off" | "no" | "false" => Some(false),
        _ => None,
    }
}

/// One part of what is written: a word, a quoted phrase, or a filter
/// with its key in small letters and its value without quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    key: Option<String>,
    value: String,
    /// Whether the value was written in quotes.
    quoted: bool,
    /// The token as it was written.
    written: String,
}

/// The keys a token may start with; anything else before a colon, as the
/// letter of a drive, is part of a word.
const KEYS: [&str; 15] = [
    "kind",
    "modified",
    "date",
    "datemodified",
    "size",
    "author",
    "authors",
    "tag",
    "tags",
    "keywords",
    "ext",
    "type",
    "content",
    "contents",
    "name",
];

/// The parts of `text`: separated by spaces outside quotes, a part in
/// quotes being one, also after the colon of a key, as `tag:"my trip"`.
fn tokens(text: &str) -> Vec<Token> {
    let mut written = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                token.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    written.push(std::mem::take(&mut token));
                }
            }
            c => token.push(c),
        }
    }
    if !token.is_empty() {
        written.push(token);
    }
    { written.into_iter() }
        .map(|written| {
            let keyed = written.split_once(':').and_then(|(key, value)| {
                let key = key.to_lowercase();
                KEYS.contains(&key.as_str()).then_some((key, value))
            });
            let (key, value) = match keyed {
                Some((key, value)) => (Some(key), value.to_owned()),
                None => (None, written.clone()),
            };
            let quoted = value.len() >= 2 && value.starts_with('"') && value.ends_with('"');
            let value = value.trim_matches('"').to_owned();
            Token {
                key,
                value,
                quoted,
                written,
            }
        })
        .collect()
}

/// What a search looks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The words and phrases a name must hold, in small letters; with
    /// [`Self::contents`], what a file holds may hold them instead.
    pub words: Vec<String>,
    pub contents: bool,
    /// What a file must hold, whatever its name, in small letters.
    pub content_words: Vec<String>,
    /// Any of which a file may be.
    pub kinds: Vec<Kind>,
    pub modified: Option<DateSpan>,
    pub size: Option<SizeSpan>,
    /// Words the authors of a file must hold, in small letters.
    pub authors: Vec<String>,
    /// Tags a file must have, in any case.
    pub tags: Vec<String>,
    /// Any of which a file may have, in small letters, without the dot.
    pub extensions: Vec<String>,
}

impl Query {
    pub fn parse(text: &str) -> Self {
        let mut query = Self::default();
        for token in tokens(text) {
            if !query.take(&token) {
                query.add_words(&token.written);
            }
        }
        query
    }

    /// Takes the filter `token` writes; `false` where it is none that can
    /// be read.
    fn take(&mut self, token: &Token) -> bool {
        let Some(key) = token.key.as_deref() else {
            self.add_words(&token.value);
            return true;
        };
        let value = token.value.trim();
        if value.is_empty() && key != "content" {
            return false;
        }
        match key {
            "kind" => match Kind::named(value) {
                Some(kind) if !self.kinds.contains(&kind) => self.kinds.push(kind),
                Some(_) => {}
                None => return false,
            },
            "modified" | "date" | "datemodified" => match DateSpan::read(value) {
                Some(span) => self.modified = Some(span),
                None => return false,
            },
            "size" => match SizeSpan::read(value) {
                Some(span) => self.size = Some(span),
                None => return false,
            },
            "author" | "authors" => self.authors.push(value.to_lowercase()),
            "tag" | "tags" | "keywords" => {
                let tags = value.split([',', ';']).map(str::trim);
                self.tags
                    .extend(tags.filter(|tag| !tag.is_empty()).map(str::to_owned));
            }
            "ext" | "type" => {
                let extensions: Vec<String> = { value.split([',', ';']) }
                    .map(|ext| ext.trim().trim_start_matches("*.").trim_start_matches('.'))
                    .map(str::to_lowercase)
                    .filter(|ext| !ext.is_empty() && ext.chars().all(is_name_letter))
                    .collect();
                if extensions.is_empty() {
                    return false;
                }
                self.extensions.extend(extensions);
            }
            "content" | "contents" => match (token.quoted, switch_of(value)) {
                (false, Some(on)) => self.contents = on,
                _ => self.content_words.push(value.to_lowercase()),
            },
            "name" => self.add_words(value),
            _ => return false,
        }
        true
    }

    fn add_words(&mut self, text: &str) {
        let text = text.trim().trim_matches('"').trim();
        if !text.is_empty() {
            self.words.push(text.to_lowercase());
        }
    }

    /// Whether it asks for nothing at all.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
            || (*self
                == Self {
                    contents: true,
                    ..Self::default()
                })
    }

    /// Whether the properties of files are to be read to answer it.
    pub fn reads_properties(&self) -> bool {
        !self.authors.is_empty() || !self.tags.is_empty()
    }

    /// Whether `name` (or the name it is shown with) holds every word.
    pub fn name_matches(&self, names: &[&str]) -> bool {
        { self.words.iter() }
            .all(|word| { names.iter() }.any(|name| name.to_lowercase().contains(word.as_str())))
    }

    /// Whether an item named `name`, a folder or not, `size` bytes large
    /// and changed on `day`, fits the filters that need no reading of the
    /// file: its kind, extension, size and date. `today` is today.
    pub fn fits(
        &self,
        name: &str,
        folder: bool,
        size: u64,
        day: Option<NaiveDate>,
        today: NaiveDate,
    ) -> bool {
        let kind = self.kinds.is_empty() || self.kinds.iter().any(|kind| kind.holds(name, folder));
        let extension = self.extensions.is_empty()
            || (!folder && extension_of(name).is_some_and(|ext| self.extensions.contains(&ext)));
        // A size is that of a file; folders have none.
        let size = self.size.is_none_or(|span| !folder && span.holds(size));
        let modified = match (self.modified, day) {
            (None, _) => true,
            (Some(span), Some(day)) => span.holds(day, today),
            (Some(_), None) => false,
        };
        // What a folder holds is not read, nor are its properties.
        let read = !folder || (self.content_words.is_empty() && !self.reads_properties());
        kind && extension && size && modified && read
    }

    /// Whether `authors` and `tags`, as a file has them, fit.
    pub fn properties_fit(&self, authors: &[String], tags: &[String]) -> bool {
        let authors: Vec<String> = authors.iter().map(|author| author.to_lowercase()).collect();
        let authored = { self.authors.iter() }.all(|wanted| {
            authors
                .iter()
                .any(|author| author.contains(wanted.as_str()))
        });
        let tagged = { self.tags.iter() }
            .all(|wanted| tags.iter().any(|tag| tag.eq_ignore_ascii_case(wanted)));
        authored && tagged
    }

    /// The question of the index of Windows Search below `root`, `today`
    /// being today.
    pub fn index_search(&self, root: &Path, today: NaiveDate) -> FileSearch {
        let mut kinds = Vec::new();
        let mut kind_extensions = Vec::new();
        for kind in &self.kinds {
            match kind.index_name() {
                Some(name) => kinds.push(name),
                None => {
                    kind_extensions.extend(kind.extensions().iter().map(|ext| (*ext).to_owned()))
                }
            }
        }
        FileSearch {
            words: self.words.clone(),
            name_or_content: self.contents,
            content_words: self.content_words.clone(),
            kinds,
            kind_extensions,
            extensions: self.extensions.clone(),
            modified_days: self.modified.map(|span| span.days(today)),
            size_bounds: self.size.map(SizeSpan::bounds),
            authors: self.authors.clone(),
            keywords: self.tags.clone(),
            folder: Some(root.to_owned()),
            ..FileSearch::default()
        }
    }
}

/// Whether `c` may stand in an extension.
fn is_name_letter(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// The extension of `name`, in small letters, where it has one.
pub fn extension_of(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_lowercase())
}

/// `text` with the filter `filter` set to `value`, or taken out without
/// one: every part that sets it goes, and the new one is put at the end.
/// The rest is kept as it was written.
pub fn with_filter(text: &str, filter: Filter, value: Option<&str>) -> String {
    let mut parts: Vec<String> = { tokens(text).into_iter() }
        .filter(|token| !filter.set_by(token))
        .map(|token| token.written)
        .collect();
    if let Some(value) = value {
        let value = value.trim();
        let value = match value.contains(char::is_whitespace) {
            true => format!("\"{}\"", value.replace('"', "")),
            false => value.to_owned(),
        };
        parts.push(format!("{}:{value}", filter.key()));
    }
    parts.join(" ")
}

/// The value the last part of `text` that sets `filter` gives it.
pub fn filter_value(text: &str, filter: Filter) -> Option<String> {
    { tokens(text).into_iter() }
        .rfind(|token| filter.set_by(token))
        .map(|token| token.value)
}

/// What a search is called where it is listed: what was written, without
/// the spaces around it.
pub fn shown(text: &str) -> String {
    { tokens(text).into_iter() }
        .map(|token| token.written)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn words_phrases_and_filters_are_read() {
        let query = Query::parse(
            r#"Budget "Annual Report" kind:pictures modified:this-week size:large author:Anna tag:"my trip" ext:.PDF,docx content:invoice"#,
        );
        assert_eq!(query.words, ["budget", "annual report"]);
        assert_eq!(query.kinds, [Kind::Pictures]);
        assert_eq!(query.modified, Some(DateSpan::ThisWeek));
        assert_eq!(query.size, Some(SizeSpan::Tier(SizeTier::Large)));
        assert_eq!(query.authors, ["anna"]);
        assert_eq!(query.tags, ["my trip"]);
        assert_eq!(query.extensions, ["pdf", "docx"]);
        assert_eq!(query.content_words, ["invoice"]);
        assert!(!query.contents);
        assert!(query.reads_properties());
    }

    #[test]
    fn what_cannot_be_read_is_words() {
        let query = Query::parse(r"kind:spaceship size:big C:\Windows date:soon ext:pdf'--");
        assert_eq!(
            query.words,
            [
                "kind:spaceship",
                "size:big",
                r"c:\windows",
                "date:soon",
                "ext:pdf'--"
            ]
        );
        assert!(query.kinds.is_empty() && query.size.is_none() && query.modified.is_none());
        assert!(query.extensions.is_empty());
        assert!(Query::parse("  ").is_empty());
        assert!(Query::parse("content:on").is_empty());
        assert!(!Query::parse("kind:music").is_empty());
    }

    #[test]
    fn contents_are_switched_by_content_alone() {
        let on = Query::parse("plan content:on");
        assert!(on.contents && on.content_words.is_empty());
        assert!(Query::parse("content:on").words.is_empty());
        let quoted = Query::parse(r#"content:"on""#);
        assert_eq!(quoted.content_words, ["on"]);
        assert!(!Query::parse("plan content:off").contents);
    }

    #[test]
    fn dates_cover_their_days() {
        let today = day(2026, 3, 18); // a Wednesday
        let days = |text: &str| DateSpan::read(text).unwrap().days(today);
        assert_eq!(days("today"), (Some(today), Some(day(2026, 3, 19))));
        assert_eq!(days("yesterday"), (Some(day(2026, 3, 17)), Some(today)));
        assert_eq!(days("this week"), (Some(day(2026, 3, 16)), None));
        assert_eq!(
            days("last-week"),
            (Some(day(2026, 3, 9)), Some(day(2026, 3, 16)))
        );
        assert_eq!(days("this-month"), (Some(day(2026, 3, 1)), None));
        assert_eq!(
            days("last-month"),
            (Some(day(2026, 2, 1)), Some(day(2026, 3, 1)))
        );
        assert_eq!(days("this-year"), (Some(day(2026, 1, 1)), None));
        assert_eq!(days("week"), (Some(day(2026, 3, 11)), None));
        assert_eq!(
            days("2026-02-10..2026-02-01"),
            (Some(day(2026, 2, 1)), Some(day(2026, 2, 11)))
        );
        assert_eq!(days(">2026-02-01"), (Some(day(2026, 2, 2)), None));
        assert_eq!(days("<=2026-02-01"), (None, Some(day(2026, 2, 2))));
        let span = DateSpan::read("this-week").unwrap();
        assert!(span.holds(day(2026, 3, 16), today));
        assert!(!span.holds(day(2026, 3, 15), today));
        for text in [
            "today",
            "this-week",
            "2026-01-01..2026-02-01",
            ">=2026-01-01",
        ] {
            assert_eq!(DateSpan::read(text).unwrap().written(), text);
        }
    }

    #[test]
    fn sizes_go_by_explorers_tiers_and_comparisons() {
        let size = |text: &str| SizeSpan::read(text).unwrap();
        assert!(size("empty").holds(0) && !size("empty").holds(1));
        assert!(size("tiny").holds(10 * KB) && !size("tiny").holds(10 * KB + 1));
        assert!(size("large").holds(16 * MB) && !size("large").holds(MB));
        assert!(size("gigantic").holds(GB));
        assert_eq!(size(">10MB").bounds(), (10 * MB + 1, None));
        assert_eq!(size("<=1KB").bounds(), (0, Some(KB + 1)));
        assert_eq!(size("1MB..2MB").bounds(), (MB, Some(2 * MB + 1)));
        assert_eq!(size(">=1.5MB").written(), ">=1536KB");
        assert_eq!(size("10MB").written(), "10MB");
        assert!(SizeSpan::read("3 parsecs").is_none());
    }

    #[test]
    fn names_and_kinds_are_matched_without_reading_files() {
        let today = day(2026, 3, 18);
        let query = Query::parse("trip kind:pictures modified:today size:>1KB");
        assert!(query.name_matches(&["Trip to Rome.JPG"]));
        assert!(query.name_matches(&["IMG_1.jpg", "My trip"]));
        assert!(!query.name_matches(&["Rome.jpg"]));
        assert!(query.fits("a.jpg", false, 4 * KB, Some(today), today));
        assert!(!query.fits("a.mp3", false, 4 * KB, Some(today), today));
        assert!(!query.fits("a.jpg", false, 10, Some(today), today));
        assert!(!query.fits("a.jpg", false, 4 * KB, Some(day(2026, 3, 1)), today));
        assert!(!query.fits("a.jpg", false, 4 * KB, None, today));
        let folders = Query::parse("kind:folders");
        assert!(folders.fits("Work", true, 0, None, today));
        assert!(!folders.fits("work.txt", false, 0, None, today));
        let archives = Query::parse("kind:archives ext:zip");
        assert!(archives.fits("x.ZIP", false, 0, None, today));
        assert!(!archives.fits("x.7z", false, 0, None, today));
        // Folders have no size, nor contents or properties to read.
        assert!(!Query::parse("size:empty").fits("Work", true, 0, None, today));
        assert!(!Query::parse("tag:sea").fits("Work", true, 0, None, today));
    }

    #[test]
    fn properties_are_matched_in_any_case() {
        let query = Query::parse("author:ann tag:Sea");
        let authors = ["Anna Karenina".to_owned()];
        assert!(query.properties_fit(&authors, &["sea".to_owned(), "sun".to_owned()]));
        assert!(!query.properties_fit(&authors, &["seaside".to_owned()]));
        assert!(!query.properties_fit(&[], &["sea".to_owned()]));
    }

    #[test]
    fn the_chips_change_only_their_filter() {
        let text = r#"report  "q1 plan" kind:music date:today"#;
        assert_eq!(
            with_filter(text, Filter::Kind, Some("pictures")),
            r#"report "q1 plan" date:today kind:pictures"#
        );
        assert_eq!(
            with_filter(text, Filter::Modified, None),
            r#"report "q1 plan" kind:music"#
        );
        assert_eq!(
            with_filter("x", Filter::Tag, Some("my trip")),
            r#"x tag:"my trip""#
        );
        let on = with_filter("x content:budget", Filter::Contents, Some("on"));
        assert_eq!(on, "x content:budget content:on");
        assert_eq!(with_filter(&on, Filter::Contents, None), "x content:budget");
        assert_eq!(filter_value(text, Filter::Kind).as_deref(), Some("music"));
        assert_eq!(filter_value(text, Filter::Size), None);
        assert_eq!(shown("  a   b "), "a b");
    }

    #[test]
    fn the_index_is_asked_for_the_same() {
        let query = Query::parse("plan kind:archives kind:docs size:small content:on");
        let search = query.index_search(Path::new(r"D:\Work"), day(2026, 3, 18));
        assert_eq!(search.kinds, ["document"]);
        assert!(search.kind_extensions.contains(&"zip".to_owned()));
        assert_eq!(search.size_bounds, Some((10 * KB + 1, Some(100 * KB + 1))));
        assert!(search.name_or_content);
        assert_eq!(search.folder.as_deref(), Some(Path::new(r"D:\Work")));
    }
}

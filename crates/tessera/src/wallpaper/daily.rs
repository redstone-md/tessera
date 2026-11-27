// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The picture of the day of Bing or of Wikimedia Commons, fetched once a
//! day and kept for a week in `%LOCALAPPDATA%\tessera\daily`.
//!
//! Nothing is fetched unless the configuration asks for a picture of the
//! day. Where the network cannot be reached, the newest picture kept is
//! shown. The addresses are those the two sites offer without a key; they
//! may change without notice, which is then reported.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use chrono::NaiveDate;
use serde::Deserialize;

use crate::extension::http::{download, download_into};

/// How many pictures are kept.
const KEPT: usize = 7;

/// The largest answer of a site that is read, in bytes.
const ANSWER_LIMIT: u64 = 1 << 20;

/// The largest picture that is downloaded, in bytes.
const PICTURE_LIMIT: u64 = 40 << 20;

/// How wide a picture of Wikimedia Commons is asked for, in pixels. The
/// files themselves are often much larger.
const COMMONS_WIDTH: u32 = 3840;

/// The kinds of file a downloaded picture is kept as, by the end of its
/// address.
const KINDS: [&str; 6] = ["jpg", "jpeg", "png", "webp", "gif", "tiff"];

/// Where the picture of the day comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Daily {
    /// The picture of the home page of Bing, in the size for 4K displays.
    #[default]
    Bing,
    /// The Picture of the Day of Wikimedia Commons.
    Wikimedia,
}

/// A picture of the day that was fetched: its file and what the site
/// says of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DailyPicture {
    pub file: PathBuf,
    pub caption: Option<String>,
}

impl Daily {
    fn name(self) -> &'static str {
        match self {
            Self::Bing => "bing",
            Self::Wikimedia => "wikimedia",
        }
    }

    /// The directory the pictures are kept in.
    fn directory() -> Option<PathBuf> {
        Some(super::local_data()?.join("daily"))
    }

    /// How the files of the picture of `date` begin.
    fn prefix(self, date: NaiveDate) -> String {
        format!("{}-{}.", self.name(), date.format("%Y-%m-%d"))
    }

    /// The pictures of this site that are kept, the newest first.
    fn kept(self, directory: &Path) -> Vec<PathBuf> {
        let start = format!("{}-", self.name());
        let mut found: Vec<PathBuf> = fs::read_dir(directory)
            .into_iter()
            .flatten()
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| {
                let name = path.file_name().and_then(|name| name.to_str());
                name.is_some_and(|name| name.starts_with(&start)) && is_kept_picture(path)
            })
            .collect();
        // The date in the name sorts as the days do.
        found.sort();
        found.reverse();
        found
    }

    /// The picture of `date`, if it was fetched.
    pub fn of(self, date: NaiveDate) -> Option<DailyPicture> {
        let prefix = self.prefix(date);
        let directory = Self::directory()?;
        let file = self.kept(&directory).into_iter().find(|path| {
            let name = path.file_name().and_then(|name| name.to_str());
            name.is_some_and(|name| name.starts_with(&prefix))
        })?;
        Some(DailyPicture::of_file(file))
    }

    /// The newest picture that is kept, of whatever day.
    pub fn newest(self) -> Option<DailyPicture> {
        let file = self.kept(&Self::directory()?).into_iter().next()?;
        Some(DailyPicture::of_file(file))
    }

    /// The address that tells which picture is the one of `date`.
    fn question(self, date: NaiveDate, locale: Option<&str>) -> String {
        match self {
            Self::Bing => {
                let market = locale.filter(|locale| is_market(locale)).unwrap_or("en-US");
                format!("https://www.bing.com/HPImageArchive.aspx?format=js&idx=0&n=1&mkt={market}")
            }
            Self::Wikimedia => format!(
                "https://commons.wikimedia.org/w/api.php?action=query&format=json\
                 &formatversion=2&generator=images&gimlimit=1&prop=imageinfo&iiprop=url\
                 &iiurlwidth={COMMONS_WIDTH}&titles=Template:Potd/{}",
                date.format("%Y-%m-%d")
            ),
        }
    }

    /// Fetches the picture of `date` and keeps it, with what the site says
    /// of it, and lets go of all but the newest pictures kept. `locale`,
    /// as `en-US`, has Bing give the picture of that market. It waits for
    /// the network: not for the thread of the interface.
    pub fn fetch(self, date: NaiveDate, locale: Option<&str>) -> Result<DailyPicture> {
        let directory = Self::directory().context("there is no directory for local data")?;
        fs::create_dir_all(&directory)
            .with_context(|| format!("{} cannot be made", directory.display()))?;
        let question = self.question(date, locale);
        let answer = download(&question, ANSWER_LIMIT).map_err(|error| anyhow!(error))?;
        let answer = String::from_utf8_lossy(&answer);
        let (address, caption) = self
            .answered(&answer)
            .with_context(|| format!("{} gave no picture", self.name()))?;
        let kind = kind_of(&address);
        let file = directory.join(format!("{}{kind}", self.prefix(date)));
        let partial = file.with_extension("part");
        let fetched = (|| {
            let mut sink = fs::File::create(&partial)?;
            download_into(&address, PICTURE_LIMIT, &mut sink).map_err(|error| anyhow!(error))?;
            sink.flush()?;
            drop(sink);
            fs::rename(&partial, &file)?;
            anyhow::Ok(())
        })();
        if let Err(error) = fetched {
            let _ = fs::remove_file(&partial);
            return Err(error.context(format!("the picture at {address} was not fetched")));
        }
        if let Some(caption) = &caption {
            let _ = fs::write(caption_file(&file), caption);
        }
        for old in self.kept(&directory).into_iter().skip(KEPT) {
            let _ = fs::remove_file(caption_file(&old));
            let _ = fs::remove_file(old);
        }
        Ok(DailyPicture { file, caption })
    }

    /// The address of the picture and what is said of it, read from the
    /// `answer` of the site.
    fn answered(self, answer: &str) -> Option<(String, Option<String>)> {
        match self {
            Self::Bing => {
                let base = json_string(answer, "urlbase");
                let address = match base {
                    Some(base) => format!("{base}_UHD.jpg"),
                    None => json_string(answer, "url")?,
                };
                let address = if address.starts_with('/') {
                    format!("https://www.bing.com{address}")
                } else {
                    address
                };
                Some((address, json_string(answer, "copyright")))
            }
            Self::Wikimedia => {
                let address =
                    json_string(answer, "thumburl").or_else(|| json_string(answer, "url"))?;
                let caption = json_string(answer, "title").map(|title| {
                    let title = title.strip_prefix("File:").unwrap_or(&title);
                    let title = Path::new(title).file_stem().map_or_else(
                        || title.to_owned(),
                        |stem| stem.to_string_lossy().into_owned(),
                    );
                    title.replace('_', " ")
                });
                Some((address, caption))
            }
        }
        .filter(|(address, _)| address.starts_with("https://"))
    }
}

impl DailyPicture {
    fn of_file(file: PathBuf) -> Self {
        let caption = fs::read_to_string(caption_file(&file)).ok();
        let caption = caption.map(|caption| caption.trim().to_owned());
        Self {
            caption: caption.filter(|caption| !caption.is_empty()),
            file,
        }
    }
}

/// The file what a site says of the picture `file` is kept in.
fn caption_file(file: &Path) -> PathBuf {
    file.with_extension("txt")
}

/// Whether `path` is a picture kept here, not what is said of one nor one
/// still being downloaded.
fn is_kept_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| KINDS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// The kind of file the picture at `address` is kept as.
fn kind_of(address: &str) -> &'static str {
    let path = address.split(['?', '#']).next().unwrap_or_default();
    let extension = path
        .rsplit_once('.')
        .map(|(_, end)| end.to_ascii_lowercase());
    // Bing names the kind only in the query of its addresses.
    KINDS
        .iter()
        .find(|kind| extension.as_deref() == Some(**kind))
        .copied()
        .unwrap_or("jpg")
}

/// Whether `locale` looks like a market Bing knows, as `en-US`: letters
/// and one hyphen, nothing that would change the address it goes into.
fn is_market(locale: &str) -> bool {
    let mut parts = locale.split('-');
    let language = parts.next().unwrap_or_default();
    let region = parts.next().unwrap_or_default();
    parts.next().is_none()
        && (2..=3).contains(&language.len())
        && region.len() == 2
        && language.chars().all(|c| c.is_ascii_alphabetic())
        && region.chars().all(|c| c.is_ascii_alphabetic())
}

/// The text of the first string value of the key `key` in the JSON
/// `text`, with its escapes undone. This reads just enough of JSON for the
/// two answers above, which are not checked further.
fn json_string(text: &str, key: &str) -> Option<String> {
    let quoted = format!("\"{key}\"");
    let mut rest = text;
    loop {
        let at = rest.find(&quoted)?;
        rest = &rest[at + quoted.len()..];
        let after = rest.trim_start();
        let Some(after) = after.strip_prefix(':') else {
            continue;
        };
        let after = after.trim_start();
        let Some(value) = after.strip_prefix('"') else {
            continue;
        };
        return unescape(value);
    }
}

/// The text of a JSON string from its first character up to its closing
/// quote, with its escapes undone.
fn unescape(value: &str) -> Option<String> {
    let mut text = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(text),
            '\\' => match chars.next()? {
                'n' => text.push('\n'),
                't' => text.push('\t'),
                'r' => text.push('\r'),
                'b' => text.push('\u{8}'),
                'f' => text.push('\u{c}'),
                'u' => {
                    let unit = |chars: &mut std::str::Chars| {
                        let digits: String = chars.by_ref().take(4).collect();
                        u16::from_str_radix(&digits, 16).ok()
                    };
                    let first = unit(&mut chars)?;
                    if (0xD800..0xDC00).contains(&first) {
                        // A pair of units for a character beyond the first
                        // plane: the second follows as `\uXXXX`.
                        if chars.next()? != '\\' || chars.next()? != 'u' {
                            return None;
                        }
                        let second = unit(&mut chars)?;
                        for c in char::decode_utf16([first, second]) {
                            text.push(c.ok()?);
                        }
                    } else {
                        text.push(char::from_u32(u32::from(first))?);
                    }
                }
                other => text.push(other),
            },
            c => text.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_of_json_is_found_by_its_key_and_unescaped() {
        let text = r#"{"images":[{"startdate":"20261127","url":"/th?id=OHR.Fox_EN-US1_1920x1080.jpg&rf=x","urlbase" : "\/th?id=OHR.Fox_EN-US1","copyright":"A fox © Someone \"quoted\" 🦊"}]}"#;
        assert_eq!(
            json_string(text, "url").as_deref(),
            Some("/th?id=OHR.Fox_EN-US1_1920x1080.jpg&rf=x")
        );
        assert_eq!(
            json_string(text, "urlbase").as_deref(),
            Some("/th?id=OHR.Fox_EN-US1")
        );
        assert_eq!(
            json_string(text, "copyright").as_deref(),
            Some("A fox \u{a9} Someone \"quoted\" \u{1f98a}")
        );
        assert_eq!(json_string(text, "missing"), None);
        // A key whose value is no string is passed over.
        assert_eq!(
            json_string(r#"{"n":1,"x":{"n":"two"}}"#, "n").as_deref(),
            Some("two")
        );
        assert_eq!(json_string(r#"{"n":"open"#, "n"), None);
    }

    #[test]
    fn bing_gives_its_picture_in_the_size_for_4k() {
        let answer = r#"{"images":[{"url":"/th?id=OHR.Fox_1920x1080.jpg","urlbase":"/th?id=OHR.Fox","copyright":"A fox"}]}"#;
        assert_eq!(
            Daily::Bing.answered(answer),
            Some((
                "https://www.bing.com/th?id=OHR.Fox_UHD.jpg".to_owned(),
                Some("A fox".to_owned())
            ))
        );
        let older = r#"{"images":[{"url":"/th?id=OHR.Fox_1920x1080.jpg"}]}"#;
        assert_eq!(
            Daily::Bing.answered(older).unwrap().0,
            "https://www.bing.com/th?id=OHR.Fox_1920x1080.jpg"
        );
        assert_eq!(Daily::Bing.answered("{}"), None);
    }

    #[test]
    fn wikimedia_gives_a_scaled_picture_and_its_name() {
        let answer = r#"{"query":{"pages":[{"title":"File:Red_fox_in_snow.jpg","imageinfo":[{"thumburl":"https://upload.wikimedia.org/a/3840px-Red_fox_in_snow.jpg","url":"https://upload.wikimedia.org/a/Red_fox_in_snow.jpg"}]}]}}"#;
        assert_eq!(
            Daily::Wikimedia.answered(answer),
            Some((
                "https://upload.wikimedia.org/a/3840px-Red_fox_in_snow.jpg".to_owned(),
                Some("Red fox in snow".to_owned())
            ))
        );
        // Plain HTTP is not followed.
        let plain = r#"{"thumburl":"http://example.com/a.jpg"}"#;
        assert_eq!(Daily::Wikimedia.answered(plain), None);
    }

    #[test]
    fn the_question_names_the_day_or_the_market() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
        assert!(
            Daily::Bing
                .question(date, Some("ru-RU"))
                .ends_with("&mkt=ru-RU")
        );
        // A locale that is no market is not put into the address.
        assert!(
            Daily::Bing
                .question(date, Some("x&y=z"))
                .ends_with("&mkt=en-US")
        );
        assert!(
            Daily::Wikimedia
                .question(date, None)
                .ends_with("&titles=Template:Potd/2026-11-27")
        );
        assert_eq!(Daily::Bing.prefix(date), "bing-2026-11-27.");
    }

    #[test]
    fn a_picture_is_kept_as_the_kind_its_address_names() {
        assert_eq!(kind_of("https://upload.wikimedia.org/a/x.PNG"), "png");
        assert_eq!(kind_of("https://www.bing.com/th?id=OHR.Fox_UHD.jpg"), "jpg");
        assert_eq!(kind_of("https://example.com/picture"), "jpg");
        assert!(is_kept_picture(Path::new("bing-2026-11-27.jpg")));
        assert!(!is_kept_picture(Path::new("bing-2026-11-27.txt")));
        assert!(!is_kept_picture(Path::new("bing-2026-11-27.part")));
    }

    #[test]
    fn the_newest_pictures_come_first() {
        let directory = std::env::temp_dir().join(format!("tessera-daily-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        for name in [
            "bing-2026-11-25.jpg",
            "bing-2026-11-27.jpg",
            "bing-2026-11-26.png",
            "bing-2026-11-27.txt",
            "wikimedia-2026-11-28.jpg",
        ] {
            fs::write(directory.join(name), []).unwrap();
        }
        let kept = Daily::Bing.kept(&directory);
        fs::remove_dir_all(&directory).unwrap();
        let names: Vec<_> = kept
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "bing-2026-11-27.jpg",
                "bing-2026-11-26.png",
                "bing-2026-11-25.jpg"
            ]
        );
    }
}

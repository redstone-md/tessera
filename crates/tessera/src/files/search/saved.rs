// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Saved searches: a search of the file manager written as the
//! `.search-ms` file Windows writes (a `persistedQuery`: how its results
//! are shown, the conditions of its query, the kinds of items and the
//! folder it looks below) into the Searches folder of the user
//! (`FOLDERID_SavedSearches`), where Explorer, the Searches of Tessera
//! Files and the Start menu all find it.
//!
//! What Tessera writes also carries what was typed, in a comment Windows
//! passes over, so that Tessera reads its own searches back exactly. A
//! search another program saved is read where its conditions are ones
//! Tessera can write as words and filters, below one folder; any other is
//! left to the Shell to show.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use chrono::NaiveDate;

use super::aqs::{Kind, Query, SizeSpan};
use crate::files::Location;
use crate::platform::{known_folder_path, unique_name};

/// The extension of saved searches.
pub const EXTENSION: &str = "search-ms";

/// What begins the comment that holds what was typed.
const MARK: &str = "tessera-search";

/// The folder saved searches go to: the user's Searches, made where it is
/// missing. It reads the disk: never call it on the thread of the user
/// interface.
pub fn folder() -> Result<PathBuf> {
    if let Some(folder) = known_folder_path("Searches") {
        return Ok(folder);
    }
    let profile = std::env::var_os("USERPROFILE").context("there is no folder of the user")?;
    let folder = PathBuf::from(profile).join("Searches");
    fs::create_dir_all(&folder).with_context(|| format!("{} cannot be made", folder.display()))?;
    Ok(folder)
}

/// Saves the search for `text` below `root` as `title`, with a number
/// where the name is taken, and says where it went. `today` is today. It
/// writes the disk: never call it on the thread of the user interface.
pub fn save(root: &Path, text: &str, title: &str, today: NaiveDate) -> Result<PathBuf> {
    let folder = folder()?;
    let stem: String = { title.chars() }
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let stem = stem.trim().trim_end_matches('.');
    let stem = if stem.is_empty() { "Search" } else { stem };
    let name = unique_name(&format!("{stem}.{EXTENSION}"), |name| {
        folder.join(name).exists()
    });
    let path = folder.join(name);
    fs::write(&path, document(root, text, title, today))
        .with_context(|| format!("{} cannot be written", path.display()))?;
    Ok(path)
}

/// Where the saved search at `path` leads in Tessera: a search below its
/// folder, where it is one Tessera can express. It reads the disk: never
/// call it on the thread of the user interface.
#[allow(
    dead_code,
    reason = "files-namespace opens a .search-ms through it once both are merged"
)]
pub fn location_of(path: &Path) -> Option<Location> {
    let text = fs::read_to_string(path).ok()?;
    let (root, query) = read(&text)?;
    Some(Location::Search { root, query })
}

/// `text` with what XML gives a meaning to written as it is to be read.
fn escaped(text: &str) -> String {
    let mut written = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => written.push_str("&amp;"),
            '<' => written.push_str("&lt;"),
            '>' => written.push_str("&gt;"),
            '"' => written.push_str("&quot;"),
            '\'' => written.push_str("&apos;"),
            c => written.push(c),
        }
    }
    written
}

/// `text` as it may stand in a comment of XML, which cannot hold `--`,
/// between the quotes of a value of the comment.
fn commented(text: &str) -> String {
    { text.replace('%', "%25").replace('-', "%2D") }.replace('"', "%22")
}

fn uncommented(text: &str) -> String {
    { text.replace("%22", "\"").replace("%2D", "-") }.replace("%25", "%")
}

/// One condition of the query of a saved search.
fn leaf(property: &str, operator: &str, kind: &str, value: &str) -> String {
    format!(
        "<condition type=\"leafCondition\" property=\"{property}\" operator=\"{operator}\" propertyType=\"{kind}\" value=\"{}\"/>",
        escaped(value)
    )
}

/// Conditions any of which is to hold.
fn any(conditions: Vec<String>) -> Option<String> {
    match conditions.len() {
        0 => None,
        1 => conditions.into_iter().next(),
        _ => Some(format!(
            "<condition type=\"orCondition\">{}</condition>",
            conditions.concat()
        )),
    }
}

/// The file Windows reads for the search of `text` below `root`, shown
/// as `title`; `today` gives the days a span such as "this week" covers
/// as Windows is to read them.
pub fn document(root: &Path, text: &str, title: &str, today: NaiveDate) -> String {
    let query = Query::parse(text);
    let mut conditions = Vec::new();
    for word in &query.words {
        let property = match query.contents {
            true => "System.Generic.String",
            false => "System.ItemNameDisplay",
        };
        conditions.push(leaf(property, "wordmatch", "string", word));
    }
    for word in &query.content_words {
        conditions.push(leaf("System.Search.Contents", "wordmatch", "string", word));
    }
    let kinds = { query.kinds.iter() }.flat_map(|kind| match kind.index_name() {
        Some(name) => vec![leaf("System.Kind", "eq", "string", name)],
        None => { kind.extensions().iter() }
            .map(|ext| leaf("System.FileExtension", "eq", "string", &format!(".{ext}")))
            .collect(),
    });
    conditions.extend(any(kinds.collect()));
    let extensions = { query.extensions.iter() }
        .map(|ext| leaf("System.FileExtension", "eq", "string", &format!(".{ext}")));
    conditions.extend(any(extensions.collect()));
    for author in &query.authors {
        conditions.push(leaf("System.Author", "wordmatch", "string", author));
    }
    for tag in &query.tags {
        conditions.push(leaf("System.Keywords", "wordmatch", "string", tag));
    }
    if let Some(span) = query.size {
        let (from, until) = span.bounds();
        if from > 0 {
            conditions.push(leaf("System.Size", "gte", "uint64", &from.to_string()));
        }
        if let Some(until) = until {
            conditions.push(leaf("System.Size", "lt", "uint64", &until.to_string()));
        }
    }
    if let Some(span) = query.modified {
        let at = |day: NaiveDate| format!("{}T00:00:00", day.format("%Y-%m-%d"));
        let (from, until) = span.days(today);
        if let Some(from) = from {
            conditions.push(leaf("System.DateModified", "gte", "datetime", &at(from)));
        }
        if let Some(until) = until {
            conditions.push(leaf("System.DateModified", "lt", "datetime", &at(until)));
        }
    }
    let conditions = match conditions.len() {
        0 => String::new(),
        1 => conditions.concat(),
        _ => format!(
            "<condition type=\"andCondition\">{}</condition>",
            conditions.concat()
        ),
    };
    let root_text = root.display().to_string();
    format!(
        "<?xml version=\"1.0\"?>\n\
         <!--{MARK} root=\"{}\" query=\"{}\"-->\n\
         <persistedQuery version=\"1.0\">\
         <viewInfo viewMode=\"details\" iconSize=\"16\" stackIconSize=\"0\" displayName=\"{}\" autoListFlags=\"0\">\
         <visibleColumns><column viewField=\"System.ItemNameDisplay\"/><column viewField=\"System.DateModified\"/><column viewField=\"System.ItemFolderPathDisplay\"/></visibleColumns>\
         <sortList><sort viewField=\"System.DateModified\" direction=\"descending\"/></sortList>\
         </viewInfo>\
         <query><conditions>{conditions}</conditions><kindList><kind name=\"item\"/></kindList>\
         <scope><include path=\"{}\"/></scope></query>\
         </persistedQuery>\n",
        commented(&root_text),
        commented(text),
        escaped(title),
        escaped(&root_text),
    )
}

/// The folder and what Tessera writes of the search `xml` holds, where it
/// is one Tessera can express.
pub fn read(xml: &str) -> Option<(PathBuf, String)> {
    let document = roxmltree::Document::parse(xml).ok()?;
    if let Some(own) = own_search(&document) {
        return Some(own);
    }
    let root = document.root_element();
    if !root.has_tag_name("persistedQuery") {
        return None;
    }
    let query = root.children().find(|node| node.has_tag_name("query"))?;
    let scope = query.children().find(|node| node.has_tag_name("scope"))?;
    let includes: Vec<&str> = { scope.children() }
        .filter(|node| node.has_tag_name("include"))
        .filter_map(|node| node.attribute("path"))
        .collect();
    let excludes = scope.children().any(|node| node.has_tag_name("exclude"));
    let [folder] = includes.as_slice() else {
        return None;
    };
    let folder = PathBuf::from(folder);
    if excludes || !folder.is_absolute() {
        return None;
    }
    let mut words = Vec::new();
    if let Some(conditions) = query
        .children()
        .find(|node| node.has_tag_name("conditions"))
    {
        for condition in conditions.children().filter(|node| node.is_element()) {
            words.extend(condition_words(condition)?);
        }
    }
    Some((folder, words.join(" ")))
}

/// What Tessera wrote of its own search in the comment it left.
fn own_search(document: &roxmltree::Document) -> Option<(PathBuf, String)> {
    let comment = { document.root().children() }
        .filter(|node| node.is_comment())
        .filter_map(|node| node.text())
        .find(|text| text.trim_start().starts_with(MARK))?;
    let value = |name: &str| {
        let start = comment.find(&format!("{name}=\""))? + name.len() + 2;
        let end = comment[start..].find('"')? + start;
        Some(uncommented(&comment[start..end]))
    };
    Some((PathBuf::from(value("root")?), value("query")?))
}

/// What Tessera writes for `condition`, where it can write it: words and
/// filters, all of which must hold.
fn condition_words(condition: roxmltree::Node) -> Option<Vec<String>> {
    match condition.attribute("type")? {
        "andCondition" => {
            let mut words = Vec::new();
            for child in condition.children().filter(|node| node.is_element()) {
                words.extend(condition_words(child)?);
            }
            Some(words)
        }
        "orCondition" => {
            let children: Vec<roxmltree::Node> = condition
                .children()
                .filter(|node| node.is_element())
                .collect();
            let leaves: Option<Vec<(&str, &str)>> = { children.iter() }
                .map(|child| {
                    let leaf = child.attribute("type")? == "leafCondition";
                    let operator = child.attribute("operator")?;
                    let wanted = leaf && operator == "eq";
                    wanted.then(|| child.attribute("property").zip(child.attribute("value")))?
                })
                .collect();
            let leaves = leaves?;
            if leaves
                .iter()
                .all(|(property, _)| *property == "System.FileExtension")
            {
                let extensions: Vec<&str> = { leaves.iter() }
                    .map(|(_, value)| value.trim_start_matches('.'))
                    .collect();
                // The extensions of a kind of Tessera's are that kind.
                let kind = { Kind::ALL.into_iter() }.find(|kind| {
                    !kind.extensions().is_empty()
                        && kind.extensions().len() == extensions.len()
                        && { extensions.iter() }
                            .all(|ext| kind.extensions().contains(&ext.to_lowercase().as_str()))
                });
                return Some(vec![match kind {
                    Some(kind) => format!("kind:{}", kind.word()),
                    None => format!("ext:{}", extensions.join(",")),
                }]);
            }
            if leaves
                .iter()
                .all(|(property, _)| *property == "System.Kind")
            {
                let kinds: Option<Vec<String>> = { leaves.iter() }
                    .map(|(_, value)| {
                        Kind::named(value).map(|kind| format!("kind:{}", kind.word()))
                    })
                    .collect();
                // Any of the kinds: only one is written as Tessera reads it.
                return kinds.filter(|kinds| kinds.len() == 1);
            }
            None
        }
        "leafCondition" => leaf_words(
            condition.attribute("property")?,
            condition.attribute("operator")?,
            condition.attribute("value")?,
        )
        .map(|word| vec![word]),
        _ => None,
    }
}

/// What Tessera writes for one condition of a saved search.
fn leaf_words(property: &str, operator: &str, value: &str) -> Option<String> {
    let quoted = |value: &str| match value.contains(char::is_whitespace) {
        true => format!("\"{}\"", value.replace('"', "")),
        false => value.to_owned(),
    };
    let word = match (property, operator) {
        ("System.Generic.String" | "System.ItemNameDisplay" | "System.FileName", _) => {
            quoted(value)
        }
        ("System.Search.Contents", _) => format!("content:{}", quoted(value)),
        ("System.Kind", "eq") => format!("kind:{}", Kind::named(value)?.word()),
        ("System.FileExtension", "eq") => format!("ext:{}", value.trim_start_matches('.')),
        ("System.Author", _) => format!("author:{}", quoted(value)),
        ("System.Keywords", _) => format!("tag:{}", quoted(value)),
        ("System.Size", operator) => {
            let bytes: u64 = value.trim().parse().ok()?;
            let comparison = match operator {
                "gt" => ">",
                "gte" => ">=",
                "lt" => "<",
                "lte" => "<=",
                "eq" => "",
                _ => return None,
            };
            let written = format!("{comparison}{bytes}");
            SizeSpan::read(&written)?;
            format!("size:{written}")
        }
        _ => return None,
    };
    Some(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 18).unwrap()
    }

    #[test]
    fn a_saved_search_is_read_back_as_it_was_typed() {
        let text = r#"report "q1 -- plan" kind:pictures size:large modified:this-week tag:"a&b""#;
        let xml = document(
            Path::new(r"C:\Users\Ann\Work & Play"),
            text,
            "My <search>",
            today(),
        );
        assert!(roxmltree::Document::parse(&xml).is_ok(), "{xml}");
        assert!(xml.contains("displayName=\"My &lt;search&gt;\""));
        assert!(xml.contains("<include path=\"C:\\Users\\Ann\\Work &amp; Play\"/>"));
        assert!(xml.contains("property=\"System.Kind\" operator=\"eq\""));
        assert!(xml.contains("value=\"2026-03-16T00:00:00\""));
        let comment = xml
            .split("<!--")
            .nth(1)
            .unwrap()
            .split("-->")
            .next()
            .unwrap();
        assert!(!comment.contains("--"));
        let (root, query) = read(&xml).expect("it is Tessera's");
        assert_eq!(root, PathBuf::from(r"C:\Users\Ann\Work & Play"));
        assert_eq!(query, text);
    }

    #[test]
    fn a_search_saved_by_windows_is_read_where_it_can_be() {
        let xml = r#"<?xml version="1.0"?>
<persistedQuery version="1.0"><viewInfo viewMode="details"/><query><conditions>
<condition type="andCondition">
<condition type="leafCondition" property="System.Generic.String" operator="wordmatch" propertyType="string" value="annual report"/>
<condition type="orCondition">
<condition type="leafCondition" property="System.FileExtension" operator="eq" propertyType="string" value=".pdf"/>
<condition type="leafCondition" property="System.FileExtension" operator="eq" propertyType="string" value=".docx"/>
</condition>
<condition type="leafCondition" property="System.Size" operator="gt" propertyType="uint64" value="1048576"/>
</condition></conditions>
<kindList><kind name="item"/></kindList><scope><include path="D:\Work"/></scope></query></persistedQuery>"#;
        let (root, query) = read(xml).expect("it can be expressed");
        assert_eq!(root, PathBuf::from(r"D:\Work"));
        assert_eq!(query, r#""annual report" ext:pdf,docx size:>1048576"#);
        let parsed = Query::parse(&query);
        assert_eq!(parsed.extensions, ["pdf", "docx"]);
        // Two places, or a condition Tessera has no word for, are left to
        // the Shell.
        let two = xml.replace(
            r#"<include path="D:\Work"/>"#,
            r#"<include path="D:\Work"/><include path="E:\"/>"#,
        );
        assert!(read(&two).is_none());
        let rated = xml.replace("System.Size", "System.Rating");
        assert!(read(&rated).is_none());
        assert!(read("<nothing/>").is_none());
    }
}

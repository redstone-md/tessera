// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What is asked of the Hacker News API and how its answers read.

use serde_json::Value;

pub const TOP_STORIES: &str = "https://hacker-news.firebaseio.com/v0/topstories.json";

pub fn item_address(id: u64) -> String {
    format!("https://hacker-news.firebaseio.com/v0/item/{id}.json")
}

/// The first `count` ids of the list of top stories.
pub fn read_top(body: &str, count: usize) -> Result<Vec<u64>, String> {
    let ids: Vec<u64> = serde_json::from_str(body)
        .map_err(|_| "The list of top stories is unreadable.".to_owned())?;
    if ids.is_empty() {
        return Err("Hacker News listed no stories.".to_owned());
    }
    Ok(ids.into_iter().take(count).collect())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Story {
    pub title: String,
    pub score: u64,
    pub comments: u64,
    pub by: String,
}

impl Story {
    pub fn read(body: &str) -> Result<Self, String> {
        let item: Value =
            serde_json::from_str(body).map_err(|_| "A story is unreadable.".to_owned())?;
        let gone = item["deleted"].as_bool() == Some(true) || item["dead"].as_bool() == Some(true);
        let title = item["title"].as_str().filter(|_| !gone);
        let title = title.ok_or_else(|| "A story is gone.".to_owned())?;
        Ok(Self {
            title: title.to_owned(),
            score: item["score"].as_u64().unwrap_or(0),
            comments: item["descendants"].as_u64().unwrap_or(0),
            by: item["by"].as_str().unwrap_or_default().to_owned(),
        })
    }

    /// The points, comments and author, as `104 points · 71 comments · by dhouston`.
    pub fn about(&self) -> String {
        let points = if self.score == 1 { "point" } else { "points" };
        let comments = if self.comments == 1 {
            "comment"
        } else {
            "comments"
        };
        let mut about = format!("{} {points} · {} {comments}", self.score, self.comments);
        if !self.by.is_empty() {
            about.push_str(&format!(" · by {}", self.by));
        }
        about
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_top_of_the_list_is_taken() {
        assert_eq!(read_top("[5, 4, 3, 2]", 2), Ok(vec![5, 4]));
        assert!(read_top("[]", 2).is_err());
        assert!(read_top("null", 2).is_err());
        assert_eq!(
            item_address(8863),
            "https://hacker-news.firebaseio.com/v0/item/8863.json"
        );
    }

    #[test]
    fn stories_are_read_and_gone_ones_refused() {
        let body = r#"{"by":"dhouston","descendants":71,"id":8863,"score":104,"title":"My YC app: Dropbox","type":"story"}"#;
        let story = Story::read(body).unwrap();
        assert_eq!(story.title, "My YC app: Dropbox");
        assert_eq!(story.about(), "104 points · 71 comments · by dhouston");
        assert!(Story::read("null").is_err());
        assert!(Story::read(r#"{"id":1,"deleted":true}"#).is_err());
        assert!(Story::read(r#"{"title":"x","dead":true}"#).is_err());
    }
}

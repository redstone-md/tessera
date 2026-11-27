// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Asking services: when to ask again, and requests waiting their turn.

use std::collections::VecDeque;

use crate::{Header, Request, Response};

/// A `GET` request for `url` with the given headers.
pub fn get(url: impl Into<String>, headers: &[(&str, &str)]) -> Request {
    let headers = headers.iter().map(|(name, value)| Header {
        name: (*name).to_owned(),
        value: (*value).to_owned(),
    });
    Request {
        method: "GET".to_owned(),
        url: url.into(),
        headers: headers.collect(),
        body: None,
    }
}

/// The body of a successful answer from `service`, or what went wrong,
/// said so that it can be shown.
pub fn body(outcome: Result<Response, String>, service: &str) -> Result<String, String> {
    let response = outcome.map_err(|error| format!("{service} did not answer: {error}"))?;
    match response.status {
        200..=299 => Ok(response.body),
        401 | 403 => Err(format!(
            "{service} refused the request ({}).",
            response.status
        )),
        404 => Err(format!("{service} knows no such thing (404).")),
        429 => Err(format!("{service} was asked too often (429).")),
        status => Err(format!("{service} answered {status}.")),
    }
}

/// When to ask a service again: every `interval` seconds, or after
/// `retry` seconds when the last question failed.
#[derive(Clone, Debug)]
pub struct Schedule {
    interval: u64,
    retry: u64,
    asked: Option<u64>,
    failing: bool,
}

impl Schedule {
    pub fn new(interval: u64, retry: u64) -> Self {
        Self {
            interval,
            retry,
            asked: None,
            failing: false,
        }
    }

    /// Whether it is time to ask.
    pub fn due(&self, now: u64) -> bool {
        let wait = if self.failing {
            self.retry
        } else {
            self.interval
        };
        self.asked
            .is_none_or(|asked| now >= asked.saturating_add(wait))
    }

    /// Notes that the question was asked at `now`.
    pub fn asked(&mut self, now: u64) {
        self.asked = Some(now);
    }

    /// Notes how the question went.
    pub fn settle(&mut self, succeeded: bool) {
        self.failing = !succeeded;
    }

    /// Makes the next question due at once.
    pub fn hurry(&mut self) {
        self.asked = None;
    }

    /// Takes changed settings: asks every `interval` seconds from now on,
    /// the next time at once.
    pub fn retune(&mut self, interval: u64) {
        self.interval = interval;
        self.failing = false;
        self.hurry();
    }

    pub fn interval(&self) -> u64 {
        self.interval
    }
}

/// One question asked of a service on a [`Schedule`], and the last answer
/// read from it, as a `T`, with when it came.
#[derive(Debug)]
pub struct Feed<T> {
    schedule: Schedule,
    pending: Option<u32>,
    latest: Option<(T, u64)>,
    error: Option<String>,
}

impl<T> Feed<T> {
    /// Asks every `interval` seconds, or `retry` seconds after a failure.
    pub fn new(interval: u64, retry: u64) -> Self {
        Self {
            schedule: Schedule::new(interval, retry),
            pending: None,
            latest: None,
            error: None,
        }
    }

    /// Sends the request `request` makes when it is time to ask.
    pub fn ask_due(&mut self, now: u64, request: impl FnOnce() -> Request) {
        self.ask_due_with(now, request, crate::fetch);
    }

    /// [`Feed::ask_due`], sending with `send`.
    pub fn ask_due_with(
        &mut self,
        now: u64,
        request: impl FnOnce() -> Request,
        send: impl FnOnce(&Request) -> Result<u32, String>,
    ) {
        if self.pending.is_some() || !self.schedule.due(now) {
            return;
        }
        self.schedule.asked(now);
        match send(&request()) {
            Ok(number) => self.pending = Some(number),
            Err(error) => self.fail(error),
        }
    }

    /// Takes the outcome of the request numbered `number` when it is this
    /// feed's, reading a successful body from `service` with `read`.
    /// Whether it was this feed's.
    pub fn answered(
        &mut self,
        number: u32,
        outcome: Result<Response, String>,
        now: u64,
        service: &str,
        read: impl FnOnce(&str) -> Result<T, String>,
    ) -> bool {
        if self.pending != Some(number) {
            return false;
        }
        self.pending = None;
        match body(outcome, service).and_then(|body| read(&body)) {
            Ok(value) => {
                self.latest = Some((value, now));
                self.error = None;
                self.schedule.settle(true);
            }
            Err(error) => self.fail(error),
        }
        true
    }

    fn fail(&mut self, error: String) {
        crate::log(&error);
        self.error = Some(error);
        self.schedule.settle(false);
    }

    /// Makes the next question due at once.
    pub fn hurry(&mut self) {
        self.schedule.hurry();
    }

    /// Takes changed settings: asks every `interval` seconds from now on,
    /// the next time at once. With `fresh`, what is asked changed, so the
    /// last answer, its error and the question under way, which were about
    /// what was asked before, are forgotten; the answer to that question
    /// is then not this feed's.
    pub fn retune(&mut self, interval: u64, fresh: bool) {
        self.schedule.retune(interval);
        if fresh {
            self.pending = None;
            self.latest = None;
            self.error = None;
        }
    }

    /// The last answer and when it came.
    pub fn latest(&self) -> Option<(&T, u64)> {
        self.latest.as_ref().map(|(value, came)| (value, *came))
    }

    /// What went wrong with the last question, when it did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// What to say while there is no answer to show: the error, or that
    /// the question is under way.
    pub fn excuse(&self) -> String {
        match &self.error {
            Some(error) => error.clone(),
            None => "Asking…".to_owned(),
        }
    }
}

/// Requests sent a few at a time, each with a `T` telling what it was
/// for; Tessera lets only a few be under way.
#[derive(Debug)]
pub struct Fetcher<T> {
    limit: usize,
    waiting: VecDeque<(Request, T)>,
    under_way: Vec<(u32, T)>,
}

impl<T> Fetcher<T> {
    /// Sends at most `limit` requests at a time.
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            waiting: VecDeque::new(),
            under_way: Vec::new(),
        }
    }

    /// Queues `request`, to be sent by [`Fetcher::pump`].
    pub fn queue(&mut self, request: Request, tag: T) {
        self.waiting.push_back((request, tag));
    }

    /// Sends what waits while fewer than the limit are under way, and
    /// gives back the tags of the requests that could not be sent, with
    /// why.
    pub fn pump(&mut self) -> Vec<(T, String)> {
        self.pump_with(crate::fetch)
    }

    /// [`Fetcher::pump`], sending with `send`.
    pub fn pump_with(
        &mut self,
        mut send: impl FnMut(&Request) -> Result<u32, String>,
    ) -> Vec<(T, String)> {
        let mut failed = Vec::new();
        while self.under_way.len() < self.limit {
            let Some((request, tag)) = self.waiting.pop_front() else {
                break;
            };
            match send(&request) {
                Ok(number) => self.under_way.push((number, tag)),
                Err(error) => failed.push((tag, error)),
            }
        }
        failed
    }

    /// The tag of the request numbered `number`, which is no longer under
    /// way; `None` when it is not one of these.
    pub fn finish(&mut self, number: u32) -> Option<T> {
        let at = self.under_way.iter().position(|(own, _)| *own == number)?;
        Some(self.under_way.swap_remove(at).1)
    }

    /// Whether a request waits or is under way.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty() || !self.under_way.is_empty()
    }

    /// Forgets every request, as after changed settings made them ask
    /// for the wrong things: those waiting are not sent, and those under
    /// way are no longer these, so [`Fetcher::finish`] does not know them.
    pub fn clear(&mut self) {
        self.waiting.clear();
        self.under_way.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schedule_asks_on_time_and_sooner_after_a_failure() {
        let mut schedule = Schedule::new(600, 60);
        assert!(schedule.due(1000));
        schedule.asked(1000);
        assert!(!schedule.due(1059));
        schedule.settle(false);
        assert!(schedule.due(1060));
        schedule.settle(true);
        assert!(!schedule.due(1599));
        assert!(schedule.due(1600));
        schedule.hurry();
        assert!(schedule.due(1001));
    }

    #[test]
    fn a_feed_keeps_the_last_answer_and_the_last_error() {
        let mut feed: Feed<usize> = Feed::new(600, 60);
        let request = || get("https://a.example/", &[]);
        feed.ask_due_with(0, request, |_| Ok(7));
        feed.ask_due_with(1, request, |_| panic!("asked twice"));
        let answer = |status| {
            Ok(Response {
                status,
                body: "four".to_owned(),
            })
        };
        assert!(!feed.answered(8, answer(200), 5, "S", |body| Ok(body.len())));
        assert!(feed.answered(7, answer(200), 5, "S", |body| Ok(body.len())));
        assert_eq!(feed.latest(), Some((&4, 5)));
        feed.ask_due_with(600, request, |_| Ok(9));
        feed.answered(9, answer(200), 601, "S", |_| Err("unreadable".to_owned()));
        assert_eq!(feed.latest(), Some((&4, 5)));
        assert_eq!(feed.error(), Some("unreadable"));
        feed.ask_due_with(659, request, |_| panic!("asked before the retry"));
        feed.ask_due_with(660, request, |_| Err("refused".to_owned()));
        assert_eq!(feed.excuse(), "refused");
    }

    #[test]
    fn changed_settings_ask_at_once_and_forget_what_no_longer_fits() {
        let mut feed: Feed<usize> = Feed::new(600, 60);
        let request = || get("https://a.example/", &[]);
        let answer = || {
            Ok(Response {
                status: 200,
                body: "four".to_owned(),
            })
        };
        feed.ask_due_with(0, request, |_| Ok(1));
        assert!(feed.answered(1, answer(), 1, "S", |body| Ok(body.len())));
        feed.retune(1200, false);
        assert_eq!(feed.latest(), Some((&4, 1)));
        feed.ask_due_with(2, request, |_| Ok(2));
        feed.retune(1200, true);
        assert_eq!(feed.latest(), None);
        assert!(!feed.answered(2, answer(), 3, "S", |body| Ok(body.len())));
        feed.ask_due_with(3, request, |_| Ok(3));
        assert!(feed.answered(3, answer(), 4, "S", |body| Ok(body.len())));
        feed.ask_due_with(1202, request, |_| panic!("asked before the new interval"));

        let mut fetcher = Fetcher::new(1);
        fetcher.queue(request(), 'a');
        fetcher.queue(request(), 'b');
        fetcher.pump_with(|_| Ok(7));
        fetcher.clear();
        assert!(!fetcher.busy());
        assert_eq!(fetcher.finish(7), None);
    }

    #[test]
    fn a_fetcher_sends_a_few_at_a_time() {
        let mut fetcher = Fetcher::new(2);
        for tag in 0..5 {
            fetcher.queue(get(format!("https://a.example/{tag}"), &[]), tag);
        }
        let mut next = 10;
        let mut send = |_: &Request| {
            next += 1;
            Ok(next)
        };
        assert!(fetcher.pump_with(&mut send).is_empty());
        assert_eq!(fetcher.finish(11), Some(0));
        assert_eq!(fetcher.finish(11), None);
        fetcher.pump_with(&mut send);
        assert_eq!(fetcher.finish(13), Some(2));
        assert_eq!(fetcher.finish(12), Some(1));
        let failed = fetcher.pump_with(|_| Err("no".to_owned()));
        let failed: Vec<i32> = failed.into_iter().map(|(tag, _)| tag).collect();
        assert_eq!(failed, [3, 4]);
        assert!(!fetcher.busy());
    }

    #[test]
    fn answers_are_read_or_explained() {
        let answer = |status| {
            Ok(Response {
                status,
                body: "x".to_owned(),
            })
        };
        assert_eq!(body(answer(200), "S"), Ok("x".to_owned()));
        assert!(body(answer(429), "S").unwrap_err().contains("too often"));
        assert!(body(answer(500), "S").unwrap_err().contains("500"));
        assert!(
            body(Err("timeout".to_owned()), "S")
                .unwrap_err()
                .contains("timeout")
        );
    }
}

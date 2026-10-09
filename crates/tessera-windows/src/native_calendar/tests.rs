// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::calendar::worker;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
struct Reply {
    count: i32,
    units: Vec<u16>,
}

impl Reply {
    fn text(value: &str) -> Self {
        let units: Vec<_> = value.encode_utf16().chain(std::iter::once(0)).collect();
        Self {
            count: units.len() as i32,
            units,
        }
    }

    fn write(&self, output: &mut [u16]) -> i32 {
        for (target, source) in output.iter_mut().zip(&self.units) {
            *target = *source;
        }
        self.count
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Time,
    Locale(usize),
    Label {
        locale: Vec<u16>,
        calendar: u32,
        field: CalendarField,
        capacity: usize,
    },
    LocaleLabel {
        locale: Vec<u16>,
        field: CalendarField,
        capacity: usize,
    },
    FirstDay(Vec<u16>, usize),
}

struct Recording {
    time: LocalTime,
    locale: Result<Reply, u32>,
    label_size: Option<Result<i32, u32>>,
    label_read: Option<Result<Reply, u32>>,
    inherited_fields: Vec<CalendarField>,
    inherited_size: Option<Result<i32, u32>>,
    inherited_read: Option<Result<Reply, u32>>,
    first_day: Result<Reply, u32>,
    calls: Vec<Call>,
}

impl Default for Recording {
    fn default() -> Self {
        Self {
            time: LocalTime {
                year: 2024,
                month: 2,
                day: 29,
                weekday: 4,
                hour: 23,
                minute: 59,
                second: 59,
                milliseconds: 999,
            },
            locale: Ok(Reply::text("ja-JP")),
            label_size: None,
            label_read: None,
            inherited_fields: Vec::new(),
            inherited_size: None,
            inherited_read: None,
            first_day: Ok(Reply::text("0")),
            calls: Vec::new(),
        }
    }
}

fn field_text(field: CalendarField) -> String {
    match field {
        CalendarField::Month(index) => format!("月{}", index + 1),
        CalendarField::Weekday(index) => format!("曜日{}", index + 1),
        CalendarField::AbbreviatedWeekday(index) => format!("略{}", index + 1),
    }
}

impl Queries for Recording {
    fn local_time(&mut self) -> LocalTime {
        self.calls.push(Call::Time);
        self.time
    }

    fn user_locale(&mut self, output: &mut [u16]) -> Result<i32, u32> {
        self.calls.push(Call::Locale(output.len()));
        self.locale
            .as_ref()
            .map(|reply| reply.write(output))
            .map_err(|code| *code)
    }

    fn calendar_info(
        &mut self,
        locale: &[u16],
        calendar: u32,
        field: CalendarField,
        output: &mut [u16],
    ) -> Result<i32, u32> {
        self.calls.push(Call::Label {
            locale: locale.to_vec(),
            calendar,
            field,
            capacity: output.len(),
        });
        if field == CalendarField::Month(0) {
            if output.is_empty() {
                if let Some(reply) = self.label_size {
                    return reply;
                }
            } else if let Some(reply) = &self.label_read {
                return reply
                    .as_ref()
                    .map(|reply| reply.write(output))
                    .map_err(|code| *code);
            }
        }
        if self.inherited_fields.contains(&field) {
            return Ok(Reply::text("").write(output));
        }
        let reply = Reply::text(&field_text(field));
        Ok(reply.write(output))
    }

    fn locale_label(
        &mut self,
        locale: &[u16],
        field: CalendarField,
        output: &mut [u16],
    ) -> Result<i32, u32> {
        self.calls.push(Call::LocaleLabel {
            locale: locale.to_vec(),
            field,
            capacity: output.len(),
        });
        if output.is_empty() {
            if let Some(reply) = self.inherited_size {
                return reply;
            }
        } else if let Some(reply) = &self.inherited_read {
            return reply
                .as_ref()
                .map(|reply| reply.write(output))
                .map_err(|code| *code);
        }
        Ok(Reply::text(&format!("共通{}", field_text(field))).write(output))
    }

    fn first_day(&mut self, locale: &[u16], output: &mut [u16]) -> Result<i32, u32> {
        self.calls
            .push(Call::FirstDay(locale.to_vec(), output.len()));
        self.first_day
            .as_ref()
            .map(|reply| reply.write(output))
            .map_err(|code| *code)
    }
}

fn sample() -> Result<CalendarSnapshot, CalendarError> {
    acquire(&mut Recording::default())
}

fn receive<T>(receiver: &Receiver<T>) -> T {
    receiver
        .recv_timeout(DEADLINE)
        .expect("worker must complete accepted read")
}

fn assert_drained<T>(receiver: &Receiver<T>) {
    assert!(
        matches!(
            receiver.recv_timeout(DEADLINE),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        ),
        "completed reads must release every callback"
    );
}

#[test]
fn explicit_gregorian_all_labels_are_monday_first_and_locale_retains_nul() {
    let mut queries = Recording::default();
    let snapshot = acquire(&mut queries).unwrap();
    assert_eq!(snapshot.today(), CivilDate::new(2024, 2, 29).unwrap());
    assert_eq!(snapshot.locale_name(), "ja-JP");
    assert_eq!(
        snapshot.months(),
        &std::array::from_fn(|index| field_text(CalendarField::Month(index)))
    );
    assert_eq!(
        snapshot.weekdays_full(),
        &std::array::from_fn(|index| field_text(CalendarField::Weekday(index)))
    );
    assert_eq!(
        snapshot.weekdays_abbreviated(),
        &std::array::from_fn(|index| field_text(CalendarField::AbbreviatedWeekday(index)))
    );
    assert_eq!(queries.calls[0], Call::Time);
    assert_eq!(queries.calls[1], Call::Locale(locale_capacity()));
    let locale = Reply::text("ja-JP").units;
    let fields = (0..12)
        .map(CalendarField::Month)
        .chain((0..7).map(CalendarField::Weekday))
        .chain((0..7).map(CalendarField::AbbreviatedWeekday));
    for (pair, field) in queries.calls[2..54].as_chunks::<2>().0.iter().zip(fields) {
        assert_eq!(
            pair[0],
            Call::Label {
                locale: locale.clone(),
                calendar: 1,
                field,
                capacity: 0
            }
        );
        assert_eq!(
            pair[1],
            Call::Label {
                locale: locale.clone(),
                calendar: 1,
                field,
                capacity: Reply::text(&field_text(field)).units.len(),
            }
        );
    }
    assert_eq!(queries.calls.len(), 55);
    assert_eq!(queries.calls[54], Call::FirstDay(locale, 2));
}

#[test]
fn every_native_week_start_is_preserved() {
    let expected = [
        WeekStart::Monday,
        WeekStart::Tuesday,
        WeekStart::Wednesday,
        WeekStart::Thursday,
        WeekStart::Friday,
        WeekStart::Saturday,
        WeekStart::Sunday,
    ];
    for (digit, expected) in expected.into_iter().enumerate() {
        let mut queries = Recording {
            first_day: Ok(Reply::text(&digit.to_string())),
            ..Recording::default()
        };
        assert_eq!(acquire(&mut queries).unwrap().week_start(), expected);
    }
}

#[test]
fn documented_systemtime_year_bounds_are_not_a_9999_cutoff() {
    for year in [1601, 9999, 10000, 30827] {
        let mut queries = Recording::default();
        queries.time.year = year;
        queries.time.month = 12;
        queries.time.day = 31;
        assert_eq!(
            acquire(&mut queries).unwrap().today().year(),
            i32::from(year)
        );
    }
    for year in [0, 1600, 30828, u16::MAX] {
        let mut queries = Recording::default();
        queries.time.year = year;
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert_eq!(queries.calls, [Call::Time]);
    }
}

#[test]
fn invalid_gregorian_and_systemtime_fields_fail_before_locale_queries() {
    let valid = Recording::default().time;
    let times = [
        LocalTime {
            year: 2023,
            ..valid
        },
        LocalTime { month: 13, ..valid },
        LocalTime { day: 0, ..valid },
        LocalTime { day: 30, ..valid },
        LocalTime {
            weekday: 7,
            ..valid
        },
        LocalTime { hour: 24, ..valid },
        LocalTime {
            minute: 60,
            ..valid
        },
        LocalTime {
            second: 60,
            ..valid
        },
        LocalTime {
            milliseconds: 1000,
            ..valid
        },
    ];
    for time in times {
        let mut queries = Recording {
            time,
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert_eq!(queries.calls, [Call::Time]);
    }
}

#[test]
fn strict_utf16_counts_terminators_and_surrogates_are_rejected() {
    for (units, count) in [
        (vec![65, 0], -1),
        (vec![65, 0], 0),
        (vec![0], 1),
        (vec![65, 0], 3),
        (vec![65, 66], 2),
        (vec![65, 0, 66, 0], 4),
        (vec![0xD800, 0], 2),
        (vec![0xDC00, 0], 2),
    ] {
        assert_eq!(
            decode(&units, count).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
    }
    let reply = Reply::text("شهر\u{200f} 🌙");
    assert_eq!(decode(&reply.units, reply.count).unwrap(), "شهر\u{200f} 🌙");
}

#[test]
fn label_size_is_bounded_and_read_races_are_not_truncated() {
    for size in [i32::MIN, 0, 257, i32::MAX] {
        let mut queries = Recording {
            label_size: Some(Ok(size)),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert_eq!(queries.calls.len(), 3, "invalid size must not start a read");
    }
    for count in [0, 1, 3, 5, i32::MAX] {
        let mut queries = Recording {
            label_size: Some(Ok(4)),
            label_read: Some(Ok(Reply {
                count,
                units: vec![65, 66, 67, 0],
            })),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
    }
    let reply = Reply::text(&"界".repeat(255));
    let mut queries = Recording {
        label_size: Some(Ok(reply.count)),
        label_read: Some(Ok(reply)),
        ..Recording::default()
    };
    assert_eq!(
        acquire(&mut queries).unwrap().months()[0]
            .encode_utf16()
            .count(),
        255
    );
}

#[test]
fn malformed_or_control_labels_never_become_a_snapshot() {
    for units in [
        vec![0xD800, 0],
        vec![65, 0, 66, 0],
        vec![65, 66],
        vec![10, 0],
        vec![32, 0],
    ] {
        let count = units.len() as i32;
        let mut queries = Recording {
            label_size: Some(Ok(count)),
            label_read: Some(Ok(Reply { units, count })),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert!(
            !queries
                .calls
                .iter()
                .any(|call| matches!(call, Call::LocaleLabel { .. }))
        );
    }
    let mut queries = Recording {
        label_size: Some(Ok(3)),
        label_read: Some(Ok(Reply {
            count: 3,
            units: vec![65],
        })),
        ..Recording::default()
    };
    assert_eq!(
        acquire(&mut queries).unwrap_err().kind,
        CalendarErrorKind::InvalidData
    );
}

#[test]
fn native_query_failures_keep_unsigned_error_and_operation() {
    for (operation, mut queries) in [
        (
            "User locale",
            Recording {
                locale: Err(0xF1234567),
                ..Recording::default()
            },
        ),
        (
            "Calendar label size",
            Recording {
                label_size: Some(Err(0xF1234567)),
                ..Recording::default()
            },
        ),
        (
            "Calendar label read",
            Recording {
                label_read: Some(Err(0xF1234567)),
                ..Recording::default()
            },
        ),
        (
            "First weekday",
            Recording {
                first_day: Err(0xF1234567),
                ..Recording::default()
            },
        ),
    ] {
        let error = acquire(&mut queries).unwrap_err();
        assert_eq!(error.kind, CalendarErrorKind::Unavailable);
        assert!(error.message.contains(operation));
        assert!(error.message.contains("0xF1234567"));
        assert!(!error.message.contains("-"));
        assert!(
            !queries
                .calls
                .iter()
                .any(|call| matches!(call, Call::LocaleLabel { .. }))
        );
    }
}

#[test]
fn malformed_locale_and_first_weekday_are_not_guessed() {
    for reply in [
        Reply {
            count: 86,
            units: vec![],
        },
        Reply::text(""),
        Reply::text("\n"),
        Reply {
            count: 2,
            units: vec![0xD800, 0],
        },
    ] {
        let mut queries = Recording {
            locale: Ok(reply),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert!(
            !queries
                .calls
                .iter()
                .any(|call| matches!(call, Call::LocaleLabel { .. }))
        );
    }
    for reply in [
        Reply::text("7"),
        Reply::text("-1"),
        Reply::text(""),
        Reply {
            count: 2,
            units: vec![48, 49],
        },
        Reply {
            count: 1,
            units: vec![48, 0],
        },
        Reply {
            count: 3,
            units: vec![48, 0],
        },
    ] {
        let mut queries = Recording {
            first_day: Ok(reply),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
    }
}

#[test]
fn accepted_failure_is_once_and_the_next_read_retries_real_acquisition() {
    let mut queries = Recording {
        locale: Err(5),
        ..Recording::default()
    };
    let host = worker::start(move || {
        let result = acquire(&mut queries);
        queries.locale = Ok(Reply::text("ja-JP"));
        result
    })
    .unwrap();
    let (send, receive_results) = channel();
    for _ in 0..2 {
        let send = send.clone();
        host.read(Box::new(move |result| send.send(result).unwrap()))
            .unwrap();
    }
    drop(send);
    drop(host);
    assert_eq!(
        receive(&receive_results).unwrap_err().kind,
        CalendarErrorKind::Unavailable
    );
    assert!(receive(&receive_results).is_ok());
    assert_drained(&receive_results);
}

#[test]
fn worker_and_consumer_panics_do_not_strand_later_reads() {
    let mut first = true;
    let host = worker::start(move || {
        if std::mem::take(&mut first) {
            panic!("recorded acquisition panic");
        }
        sample()
    })
    .unwrap();
    let (send, results) = channel();
    let first_send = send.clone();
    host.read(Box::new(move |result| first_send.send(result).unwrap()))
        .unwrap();
    let panic_send = send.clone();
    host.read(Box::new(move |result| {
        panic_send.send(result).unwrap();
        panic!("recorded consumer panic");
    }))
    .unwrap();
    host.read(Box::new(move |result| send.send(result).unwrap()))
        .unwrap();
    drop(host);
    assert_eq!(
        receive(&results).unwrap_err().kind,
        CalendarErrorKind::Other
    );
    assert!(receive(&results).is_ok());
    assert!(receive(&results).is_ok());
    assert_drained(&results);
}

#[test]
fn bounded_queue_rejects_without_callback_and_drop_drains_without_join() {
    let (started_send, started) = channel();
    let (release_send, release) = channel();
    let mut first = true;
    let count = Arc::new(AtomicUsize::new(0));
    let worker_count = Arc::clone(&count);
    let host = worker::start(move || {
        worker_count.fetch_add(1, Ordering::SeqCst);
        if std::mem::take(&mut first) {
            started_send.send(()).unwrap();
            receive(&release);
        }
        sample()
    })
    .unwrap();
    let (send, results) = channel();
    let first_send = send.clone();
    host.read(Box::new(move |result| first_send.send(result).unwrap()))
        .unwrap();
    receive(&started);
    for _ in 0..worker::QUEUE_CAPACITY {
        let send = send.clone();
        host.read(Box::new(move |result| send.send(result).unwrap()))
            .unwrap();
    }
    let rejected = Arc::new(AtomicBool::new(false));
    let rejected_callback = Arc::clone(&rejected);
    assert_eq!(
        host.read(Box::new(move |_| {
            rejected_callback.store(true, Ordering::SeqCst);
        }))
        .unwrap_err()
        .kind,
        CalendarErrorKind::Busy
    );
    // The blocked acquisition makes this drop hang if the host joins its worker.
    drop(host);
    assert!(!rejected.load(Ordering::SeqCst));
    drop(send);
    release_send.send(()).unwrap();
    for _ in 0..=worker::QUEUE_CAPACITY {
        assert!(receive(&results).is_ok());
    }
    assert_eq!(count.load(Ordering::SeqCst), worker::QUEUE_CAPACITY + 1);
    assert_drained(&results);
    assert!(!rejected.load(Ordering::SeqCst));
}

#[test]
fn callbacks_may_reenter_the_same_provider() {
    let host = worker::start(sample).unwrap();
    let callback_host = Arc::clone(&host);
    let (send, results) = channel();
    host.read(Box::new(move |first| {
        assert!(first.is_ok());
        callback_host
            .read(Box::new(move |result| send.send(result).unwrap()))
            .unwrap();
    }))
    .unwrap();
    drop(host);
    assert!(receive(&results).is_ok());
    assert_drained(&results);
}

#[cfg(not(windows))]
#[test]
fn non_windows_factory_reports_unsupported_without_fake_data() {
    let error = crate::calendar::native_calendar_host()
        .err()
        .expect("non-Windows must be unsupported");
    assert_eq!(error.kind, CalendarErrorKind::Unsupported);
}

#[test]
fn valid_empty_calendar_names_inherit_each_native_label_family_same_locale() {
    let fields: Vec<_> = (0..12)
        .map(CalendarField::Month)
        .chain((0..7).map(CalendarField::Weekday))
        .chain((0..7).map(CalendarField::AbbreviatedWeekday))
        .collect();
    let mut queries = Recording {
        inherited_fields: fields.clone(),
        ..Recording::default()
    };
    let snapshot = acquire(&mut queries).unwrap();
    assert_eq!(
        snapshot.months(),
        &std::array::from_fn(|index| {
            format!("共通{}", field_text(CalendarField::Month(index)))
        })
    );
    assert_eq!(
        snapshot.weekdays_full(),
        &std::array::from_fn(|index| {
            format!("共通{}", field_text(CalendarField::Weekday(index)))
        })
    );
    assert_eq!(
        snapshot.weekdays_abbreviated(),
        &std::array::from_fn(|index| {
            format!(
                "共通{}",
                field_text(CalendarField::AbbreviatedWeekday(index))
            )
        })
    );
    let locale = Reply::text("ja-JP").units;
    assert_eq!(queries.calls.len(), 107);
    for (calls, field) in queries.calls[2..106].as_chunks::<4>().0.iter().zip(fields) {
        assert_eq!(
            calls[0],
            Call::Label {
                locale: locale.clone(),
                calendar: 1,
                field,
                capacity: 0,
            }
        );
        assert_eq!(
            calls[1],
            Call::Label {
                locale: locale.clone(),
                calendar: 1,
                field,
                capacity: 1,
            }
        );
        assert_eq!(
            calls[2],
            Call::LocaleLabel {
                locale: locale.clone(),
                field,
                capacity: 0,
            }
        );
        assert_eq!(
            calls[3],
            Call::LocaleLabel {
                locale: locale.clone(),
                field,
                capacity: Reply::text(&format!("共通{}", field_text(field)))
                    .units
                    .len(),
            }
        );
    }
}

#[test]
fn size_one_requires_actual_successful_nul_before_native_inheritance() {
    for reply in [
        Reply {
            count: 1,
            units: vec![],
        },
        Reply {
            count: 1,
            units: vec![65],
        },
        Reply {
            count: 0,
            units: vec![0],
        },
        Reply {
            count: 2,
            units: vec![0],
        },
    ] {
        let mut queries = Recording {
            label_size: Some(Ok(1)),
            label_read: Some(Ok(reply)),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert!(
            !queries
                .calls
                .iter()
                .any(|call| matches!(call, Call::LocaleLabel { .. }))
        );
    }
    let mut queries = Recording {
        label_size: Some(Ok(1)),
        label_read: Some(Err(5)),
        ..Recording::default()
    };
    assert_eq!(
        acquire(&mut queries).unwrap_err().kind,
        CalendarErrorKind::Unavailable
    );
    assert!(
        !queries
            .calls
            .iter()
            .any(|call| matches!(call, Call::LocaleLabel { .. }))
    );
}

#[test]
fn inherited_query_errors_remain_unavailable_not_fabricated_labels() {
    for (operation, inherited_size, inherited_read) in [
        ("Locale label size", Some(Err(0xF1234567)), None),
        ("Locale label read", None, Some(Err(0xF1234567))),
    ] {
        let mut queries = Recording {
            inherited_fields: vec![CalendarField::Month(0)],
            inherited_size,
            inherited_read,
            ..Recording::default()
        };
        let error = acquire(&mut queries).unwrap_err();
        assert_eq!(error.kind, CalendarErrorKind::Unavailable);
        assert!(error.message.contains(operation));
        assert!(error.message.contains("0xF1234567"));
    }
}

#[test]
fn inherited_label_bounds_races_utf16_nul_and_nonempty_are_strict() {
    for size in [i32::MIN, 0, 257, i32::MAX] {
        let mut queries = Recording {
            inherited_fields: vec![CalendarField::Month(0)],
            inherited_size: Some(Ok(size)),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert_eq!(
            queries.calls.len(),
            5,
            "invalid inherited size must not allocate or read"
        );
    }
    for (size, reply) in [
        (1, Reply::text("")),
        (
            2,
            Reply {
                count: 1,
                units: vec![0],
            },
        ),
        (
            2,
            Reply {
                count: 3,
                units: vec![65, 0],
            },
        ),
        (
            2,
            Reply {
                count: 2,
                units: vec![0xD800, 0],
            },
        ),
        (
            2,
            Reply {
                count: 2,
                units: vec![65, 66],
            },
        ),
        (
            3,
            Reply {
                count: 3,
                units: vec![65],
            },
        ),
        (
            4,
            Reply {
                count: 4,
                units: vec![65, 0, 66, 0],
            },
        ),
        (2, Reply::text("\n")),
        (2, Reply::text(" ")),
    ] {
        let mut queries = Recording {
            inherited_fields: vec![CalendarField::Month(0)],
            inherited_size: Some(Ok(size)),
            inherited_read: Some(Ok(reply)),
            ..Recording::default()
        };
        assert_eq!(
            acquire(&mut queries).unwrap_err().kind,
            CalendarErrorKind::InvalidData
        );
        assert_eq!(
            queries
                .calls
                .iter()
                .filter(|call| matches!(call, Call::LocaleLabel { .. }))
                .count(),
            2,
            "invalid inheritance must not recurse or try another locale"
        );
    }
    let reply = Reply::text(&"界".repeat(255));
    let mut queries = Recording {
        inherited_fields: vec![CalendarField::Month(0)],
        inherited_size: Some(Ok(reply.count)),
        inherited_read: Some(Ok(reply)),
        ..Recording::default()
    };
    assert_eq!(
        acquire(&mut queries).unwrap().months()[0]
            .encode_utf16()
            .count(),
        255
    );
}

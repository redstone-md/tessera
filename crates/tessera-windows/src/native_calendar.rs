// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Strict native-query decoding; no locale fallback, COM, or OS mutations.

use tessera_system::calendar::{
    CalendarError, CalendarErrorKind, CalendarSnapshot, CivilDate, WeekStart,
};

const LABEL_CAPACITY: usize = 256;
#[cfg(windows)]
use windows_sys::Win32::Globalization::CAL_GREGORIAN;
#[cfg(not(windows))]
const CAL_GREGORIAN: u32 = 1;

#[derive(Clone, Copy, Debug)]
struct LocalTime {
    year: u16,
    month: u16,
    day: u16,
    weekday: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalendarField {
    Month(usize),
    Weekday(usize),
    AbbreviatedWeekday(usize),
}

/// Internal recording seam preserves native counts and captured unsigned errors.
/// Empty output requests the required size, including the terminal NUL.
trait Queries {
    fn local_time(&mut self) -> LocalTime;
    fn user_locale(&mut self, output: &mut [u16]) -> Result<i32, u32>;
    fn calendar_info(
        &mut self,
        locale: &[u16],
        calendar: u32,
        field: CalendarField,
        output: &mut [u16],
    ) -> Result<i32, u32>;
    fn locale_label(
        &mut self,
        locale: &[u16],
        field: CalendarField,
        output: &mut [u16],
    ) -> Result<i32, u32>;
    fn first_day(&mut self, locale: &[u16], output: &mut [u16]) -> Result<i32, u32>;
}

fn invalid(message: &str) -> CalendarError {
    CalendarError::new(CalendarErrorKind::InvalidData, message)
}

fn native_error(operation: &str, code: u32) -> CalendarError {
    CalendarError::new(
        CalendarErrorKind::Unavailable,
        format!("{operation} failed (Win32 0x{code:08X})."),
    )
}

fn decode(output: &[u16], count: i32) -> Result<String, CalendarError> {
    let count = usize::try_from(count).map_err(|_| invalid("Invalid native calendar count."))?;
    if count < 2 || count > output.len() {
        return Err(invalid("Invalid native calendar count."));
    }
    let value = &output[..count];
    if value[count - 1] != 0 || value[..count - 1].contains(&0) {
        return Err(invalid("Invalid native calendar string terminator."));
    }
    String::from_utf16(&value[..count - 1]).map_err(|_| invalid("Invalid native calendar UTF-16."))
}

fn today(time: LocalTime) -> Result<CivilDate, CalendarError> {
    // GetLocalTime returns void, so no stale last-error is consulted here.
    if !(1601..=30827).contains(&time.year)
        || time.weekday > 6
        || time.hour > 23
        || time.minute > 59
        || time.second > 59
        || time.milliseconds > 999
    {
        return Err(invalid("Invalid native local time."));
    }
    CivilDate::new(
        i32::from(time.year),
        u32::from(time.month),
        u32::from(time.day),
    )
    .ok_or_else(|| invalid("Invalid native Gregorian date."))
}

fn label<Q: Queries>(
    queries: &mut Q,
    locale: &[u16],
    field: CalendarField,
) -> Result<String, CalendarError> {
    let value = bounded_label("Calendar label", |output| {
        queries.calendar_info(locale, CAL_GREGORIAN, field, output)
    })?;
    if !value.is_empty() {
        return Ok(value);
    }
    // Win32 omits duplicated names with a successful empty calendar string.
    // Resolve only that documented representation from the same locale:
    // https://learn.microsoft.com/en-us/windows/win32/intl/calendar-type-information
    let inherited = bounded_label("Locale label", |output| {
        queries.locale_label(locale, field, output)
    })?;
    if inherited.is_empty() {
        return Err(invalid("Empty inherited native calendar label."));
    }
    Ok(inherited)
}

fn bounded_label(
    operation: &str,
    mut query: impl FnMut(&mut [u16]) -> Result<i32, u32>,
) -> Result<String, CalendarError> {
    let required =
        query(&mut []).map_err(|code| native_error(&format!("{operation} size"), code))?;
    let size =
        usize::try_from(required).map_err(|_| invalid("Invalid native calendar label size."))?;
    if !(1..=LABEL_CAPACITY).contains(&size) {
        return Err(invalid("Invalid native calendar label size."));
    }
    // A sentinel prevents unwritten bytes from passing as a valid terminator.
    let mut buffer = vec![u16::MAX; size];
    let written =
        query(&mut buffer).map_err(|code| native_error(&format!("{operation} read"), code))?;
    if written != required {
        return Err(invalid("Native calendar label changed during acquisition."));
    }
    if written == 1 && buffer[0] == 0 {
        return Ok(String::new());
    }
    decode(&buffer, written)
}

fn labels<Q: Queries, const N: usize>(
    queries: &mut Q,
    locale: &[u16],
    field: impl Fn(usize) -> CalendarField,
) -> Result<[String; N], CalendarError> {
    let mut values = std::array::from_fn(|_| String::new());
    for (index, value) in values.iter_mut().enumerate() {
        *value = label(queries, locale, field(index))?;
    }
    Ok(values)
}

fn locale_capacity() -> usize {
    #[cfg(windows)]
    {
        windows_sys::Win32::System::SystemServices::LOCALE_NAME_MAX_LENGTH as usize
    }
    #[cfg(not(windows))]
    {
        // Recorded Win32 buffer contract; never used as a runtime OS substitute.
        85
    }
}

fn acquire<Q: Queries>(queries: &mut Q) -> Result<CalendarSnapshot, CalendarError> {
    let date = today(queries.local_time())?;
    let mut locale = vec![u16::MAX; locale_capacity()];
    let count = queries
        .user_locale(&mut locale)
        .map_err(|code| native_error("User locale", code))?;
    let locale_name = decode(&locale, count)?;
    // decode has already established count is positive and within this buffer.
    locale.truncate(count as usize);
    let months = labels(queries, &locale, CalendarField::Month)?;
    let weekdays_full = labels(queries, &locale, CalendarField::Weekday)?;
    let weekdays_abbreviated = labels(queries, &locale, CalendarField::AbbreviatedWeekday)?;
    let mut first_day = [u16::MAX; 2];
    let count = queries
        .first_day(&locale, &mut first_day)
        .map_err(|code| native_error("First weekday", code))?;
    if count != 2 || first_day[1] != 0 {
        return Err(invalid("Invalid native first weekday."));
    }
    let week_start = match first_day[0] {
        0x30 => WeekStart::Monday,
        0x31 => WeekStart::Tuesday,
        0x32 => WeekStart::Wednesday,
        0x33 => WeekStart::Thursday,
        0x34 => WeekStart::Friday,
        0x35 => WeekStart::Saturday,
        0x36 => WeekStart::Sunday,
        _ => return Err(invalid("Invalid native first weekday.")),
    };
    CalendarSnapshot::new(
        date,
        locale_name,
        months,
        weekdays_full,
        weekdays_abbreviated,
        week_start,
    )
}

#[cfg(windows)]
pub(crate) fn read_snapshot() -> Result<CalendarSnapshot, CalendarError> {
    acquire(&mut WindowsQueries)
}

#[cfg(windows)]
struct WindowsQueries;

#[cfg(windows)]
impl Queries for WindowsQueries {
    fn local_time(&mut self) -> LocalTime {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut time = SYSTEMTIME::default();
        // SAFETY: output is a live, correctly sized SYSTEMTIME. No OS mutation.
        unsafe { GetLocalTime(&mut time) };
        LocalTime {
            year: time.wYear,
            month: time.wMonth,
            day: time.wDay,
            weekday: time.wDayOfWeek,
            hour: time.wHour,
            minute: time.wMinute,
            second: time.wSecond,
            milliseconds: time.wMilliseconds,
        }
    }

    fn user_locale(&mut self, output: &mut [u16]) -> Result<i32, u32> {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
        // SAFETY: buffer is writable for its bounded advertised UTF-16 capacity.
        let count = unsafe { GetUserDefaultLocaleName(output.as_mut_ptr(), output.len() as i32) };
        if count == 0 {
            // SAFETY: immediately captures this call's unsigned thread-local error.
            Err(unsafe { GetLastError() })
        } else {
            Ok(count)
        }
    }

    fn calendar_info(
        &mut self,
        locale: &[u16],
        calendar: u32,
        field: CalendarField,
        output: &mut [u16],
    ) -> Result<i32, u32> {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Globalization::{
            CAL_SABBREVDAYNAME1, CAL_SABBREVDAYNAME2, CAL_SABBREVDAYNAME3, CAL_SABBREVDAYNAME4,
            CAL_SABBREVDAYNAME5, CAL_SABBREVDAYNAME6, CAL_SABBREVDAYNAME7, CAL_SDAYNAME1,
            CAL_SDAYNAME2, CAL_SDAYNAME3, CAL_SDAYNAME4, CAL_SDAYNAME5, CAL_SDAYNAME6,
            CAL_SDAYNAME7, CAL_SMONTHNAME1, CAL_SMONTHNAME2, CAL_SMONTHNAME3, CAL_SMONTHNAME4,
            CAL_SMONTHNAME5, CAL_SMONTHNAME6, CAL_SMONTHNAME7, CAL_SMONTHNAME8, CAL_SMONTHNAME9,
            CAL_SMONTHNAME10, CAL_SMONTHNAME11, CAL_SMONTHNAME12, GetCalendarInfoEx,
        };
        // Win32 day-name constants are Monday-first, independent of week start.
        let field = match field {
            CalendarField::Month(index) => [
                CAL_SMONTHNAME1,
                CAL_SMONTHNAME2,
                CAL_SMONTHNAME3,
                CAL_SMONTHNAME4,
                CAL_SMONTHNAME5,
                CAL_SMONTHNAME6,
                CAL_SMONTHNAME7,
                CAL_SMONTHNAME8,
                CAL_SMONTHNAME9,
                CAL_SMONTHNAME10,
                CAL_SMONTHNAME11,
                CAL_SMONTHNAME12,
            ][index],
            CalendarField::Weekday(index) => [
                CAL_SDAYNAME1,
                CAL_SDAYNAME2,
                CAL_SDAYNAME3,
                CAL_SDAYNAME4,
                CAL_SDAYNAME5,
                CAL_SDAYNAME6,
                CAL_SDAYNAME7,
            ][index],
            CalendarField::AbbreviatedWeekday(index) => [
                CAL_SABBREVDAYNAME1,
                CAL_SABBREVDAYNAME2,
                CAL_SABBREVDAYNAME3,
                CAL_SABBREVDAYNAME4,
                CAL_SABBREVDAYNAME5,
                CAL_SABBREVDAYNAME6,
                CAL_SABBREVDAYNAME7,
            ][index],
        };
        let pointer = if output.is_empty() {
            std::ptr::null_mut()
        } else {
            output.as_mut_ptr()
        };
        // SAFETY: locale retains its validated NUL; reserved and numeric output
        // are null. Empty output is the documented size query, otherwise the
        // buffer is writable for its bounded capacity. Calendar is Gregorian.
        let count = unsafe {
            GetCalendarInfoEx(
                locale.as_ptr(),
                calendar,
                std::ptr::null(),
                field,
                pointer,
                output.len() as i32,
                std::ptr::null_mut(),
            )
        };
        if count == 0 {
            // SAFETY: capture immediately, before any other native call.
            Err(unsafe { GetLastError() })
        } else {
            Ok(count)
        }
    }

    fn locale_label(
        &mut self,
        locale: &[u16],
        field: CalendarField,
        output: &mut [u16],
    ) -> Result<i32, u32> {
        use windows_sys::Win32::Globalization::{
            LOCALE_SABBREVDAYNAME1, LOCALE_SABBREVDAYNAME2, LOCALE_SABBREVDAYNAME3,
            LOCALE_SABBREVDAYNAME4, LOCALE_SABBREVDAYNAME5, LOCALE_SABBREVDAYNAME6,
            LOCALE_SABBREVDAYNAME7, LOCALE_SDAYNAME1, LOCALE_SDAYNAME2, LOCALE_SDAYNAME3,
            LOCALE_SDAYNAME4, LOCALE_SDAYNAME5, LOCALE_SDAYNAME6, LOCALE_SDAYNAME7,
            LOCALE_SMONTHNAME1, LOCALE_SMONTHNAME2, LOCALE_SMONTHNAME3, LOCALE_SMONTHNAME4,
            LOCALE_SMONTHNAME5, LOCALE_SMONTHNAME6, LOCALE_SMONTHNAME7, LOCALE_SMONTHNAME8,
            LOCALE_SMONTHNAME9, LOCALE_SMONTHNAME10, LOCALE_SMONTHNAME11, LOCALE_SMONTHNAME12,
        };
        // Locale selectors differ numerically from calendar selectors. Both
        // native day-name families retain their documented Monday-first order.
        let field = match field {
            CalendarField::Month(index) => [
                LOCALE_SMONTHNAME1,
                LOCALE_SMONTHNAME2,
                LOCALE_SMONTHNAME3,
                LOCALE_SMONTHNAME4,
                LOCALE_SMONTHNAME5,
                LOCALE_SMONTHNAME6,
                LOCALE_SMONTHNAME7,
                LOCALE_SMONTHNAME8,
                LOCALE_SMONTHNAME9,
                LOCALE_SMONTHNAME10,
                LOCALE_SMONTHNAME11,
                LOCALE_SMONTHNAME12,
            ][index],
            CalendarField::Weekday(index) => [
                LOCALE_SDAYNAME1,
                LOCALE_SDAYNAME2,
                LOCALE_SDAYNAME3,
                LOCALE_SDAYNAME4,
                LOCALE_SDAYNAME5,
                LOCALE_SDAYNAME6,
                LOCALE_SDAYNAME7,
            ][index],
            CalendarField::AbbreviatedWeekday(index) => [
                LOCALE_SABBREVDAYNAME1,
                LOCALE_SABBREVDAYNAME2,
                LOCALE_SABBREVDAYNAME3,
                LOCALE_SABBREVDAYNAME4,
                LOCALE_SABBREVDAYNAME5,
                LOCALE_SABBREVDAYNAME6,
                LOCALE_SABBREVDAYNAME7,
            ][index],
        };
        query_locale_info(locale, field, output)
    }

    fn first_day(&mut self, locale: &[u16], output: &mut [u16]) -> Result<i32, u32> {
        query_locale_info(
            locale,
            windows_sys::Win32::Globalization::LOCALE_IFIRSTDAYOFWEEK,
            output,
        )
    }
}

#[cfg(windows)]
fn query_locale_info(locale: &[u16], field: u32, output: &mut [u16]) -> Result<i32, u32> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Globalization::GetLocaleInfoEx;
    let pointer = if output.is_empty() {
        std::ptr::null_mut()
    } else {
        output.as_mut_ptr()
    };
    // SAFETY: validated terminated locale and bounded writable output; empty
    // output is the documented size query. No NOUSEROVERRIDE/RETURN_NUMBER
    // flags: preserve genuine same-locale labels and all seven user week starts.
    let count = unsafe { GetLocaleInfoEx(locale.as_ptr(), field, pointer, output.len() as i32) };
    if count == 0 {
        // SAFETY: capture immediately, before any other native call.
        Err(unsafe { GetLastError() })
    } else {
        Ok(count)
    }
}

#[cfg(test)]
#[path = "native_calendar/tests.rs"]
mod tests;

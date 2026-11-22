// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The language Tessera speaks in its settings, its marketplace and its
//! popups. Texts are written in English where they are used; a table gives
//! them in the other languages, and a text it lacks stays English. English
//! and Russian are compiled in; a file of the `languages` directory of the
//! configuration gives another.

use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::format::{Fixed, Item, StrftimeItems};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};
use gpui::{App, Global};
use serde::Deserialize;

use crate::config::config_dir;

/// The directory of the language files, within the configuration.
const DIRECTORY: &str = "languages";

/// A language of the user interface, named in the configuration by its
/// letters: `en`, `ru`, or the name of a language file without `.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(from = "String")]
pub enum Language {
    #[default]
    English,
    Russian,
    /// One that a language file gives.
    File(&'static Tongue),
}

impl Global for Language {}

impl From<String> for Language {
    /// The language the configuration calls `code`; English, with a
    /// report, where there is none.
    fn from(code: String) -> Self {
        Self::named(&code).unwrap_or_else(|| {
            report!("there is no language `{code}`: no file {DIRECTORY}/{code}.toml can be used");
            Self::English
        })
    }
}

/// What a language file holds.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct TongueFile {
    /// What the language calls itself.
    name: String,
    /// The months as a calendar is headed by them.
    months: Option<[String; 12]>,
    /// The months as they follow a day; `months` where it is absent.
    months_of: Option<[String; 12]>,
    short_months: Option<[String; 12]>,
    /// The days of the week, Monday first.
    weekdays: Option<[String; 7]>,
    short_weekdays: Option<[String; 7]>,
    /// The English texts and what they are in the language.
    #[serde(default)]
    texts: HashMap<String, String>,
}

/// A language read from a file. It is kept for as long as Tessera runs.
#[derive(Debug)]
pub struct Tongue {
    code: String,
    file: TongueFile,
}

impl PartialEq for Tongue {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
    }
}

impl Eq for Tongue {}

impl Hash for Tongue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.code.hash(state);
    }
}

/// The language files read so far, each with the time it was written at.
static TONGUES: Mutex<Vec<(SystemTime, &'static Tongue)>> = Mutex::new(Vec::new());

impl Tongue {
    /// The language of the file `code` names, read again if the file was
    /// written since; a file that cannot be used is reported.
    fn read(code: &str) -> Option<&'static Self> {
        // A code is a file name, never a path.
        let plain = |letter: char| letter.is_ascii_alphanumeric() || letter == '-' || letter == '_';
        if code.is_empty() || !code.chars().all(plain) {
            return None;
        }
        let path = config_dir()?.join(DIRECTORY).join(format!("{code}.toml"));
        let written = fs::metadata(&path).and_then(|file| file.modified()).ok()?;
        let mut tongues = TONGUES.lock().ok()?;
        let known =
            |&&(at, tongue): &&(SystemTime, &'static Self)| at == written && tongue.code == code;
        if let Some(&(_, tongue)) = tongues.iter().find(known) {
            return Some(tongue);
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| report!("{}: {error}", path.display()))
            .ok()?;
        let tongue = Self::parse(code, &text)
            .map_err(|error| report!("{}: {error}", path.display()))
            .ok()?;
        // The texts are handed out for good, so the language stays.
        let tongue: &'static Self = Box::leak(Box::new(tongue));
        tongues.retain(|(_, other)| other.code != code);
        tongues.push((written, tongue));
        Some(tongue)
    }

    fn parse(code: &str, text: &str) -> Result<Self, toml::de::Error> {
        Ok(Self {
            code: code.to_owned(),
            file: toml::from_str(text)?,
        })
    }
}

/// A word of the calendar.
#[derive(Clone, Copy)]
enum Word {
    /// A month as a calendar is headed by it.
    Month,
    /// A month as it follows a day.
    MonthOf,
    ShortMonth,
    Weekday,
    ShortWeekday,
}

impl Language {
    /// The languages compiled in.
    const BUILTIN: [Self; 2] = [Self::English, Self::Russian];

    /// The language in use.
    pub fn of(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// The language the configuration calls `code`.
    pub fn named(code: &str) -> Option<Self> {
        let builtin = Self::BUILTIN.into_iter().find(|known| known.code() == code);
        builtin.or_else(|| Tongue::read(code).map(Self::File))
    }

    /// Every language there is to choose: the ones compiled in, then the
    /// ones of the language files, by name of file.
    pub fn installed() -> Vec<Self> {
        let entries = config_dir()
            .and_then(|directory| fs::read_dir(directory.join(DIRECTORY)).ok())
            .into_iter()
            .flatten();
        let mut codes: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "toml")
            })
            .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
            .filter(|code| Self::BUILTIN.iter().all(|known| known.code() != code))
            .collect();
        codes.sort();
        let files = codes.iter().filter_map(|code| Tongue::read(code));
        let mut all = Self::BUILTIN.to_vec();
        all.extend(files.map(Self::File));
        all
    }

    /// What the configuration calls it.
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
            Self::File(tongue) => &tongue.code,
        }
    }

    /// What it calls itself.
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Русский",
            Self::File(tongue) => &tongue.file.name,
        }
    }

    /// `text` in this language, or as it is where the table lacks it.
    pub fn say(self, text: &'static str) -> &'static str {
        let table = match self {
            Self::English => return text,
            Self::Russian => RUSSIAN,
            Self::File(tongue) => {
                return tongue.file.texts.get(text).map_or(text, String::as_str);
            }
        };
        let found = table.iter().find(|(english, _)| *english == text);
        found.map_or(text, |&(_, said)| said)
    }

    /// The word of the calendar with the number `index`, counted from
    /// January or Monday as 0; `None` where it is the English one.
    fn word(self, word: Word, index: usize) -> Option<&'static str> {
        let tongue = match self {
            Self::English => return None,
            Self::Russian => {
                return Some(match word {
                    Word::Month => RUSSIAN_MONTHS[index],
                    Word::MonthOf => RUSSIAN_MONTHS_OF[index],
                    Word::ShortMonth => RUSSIAN_SHORT_MONTHS[index],
                    Word::Weekday => RUSSIAN_LONG_WEEKDAYS[index],
                    Word::ShortWeekday => RUSSIAN_WEEKDAYS[index],
                });
            }
            Self::File(tongue) => &tongue.file,
        };
        let words: Option<&'static [String]> = match word {
            Word::Month => tongue.months.as_ref().map(|words| &words[..]),
            Word::MonthOf => { tongue.months_of.as_ref() }
                .or(tongue.months.as_ref())
                .map(|words| &words[..]),
            Word::ShortMonth => tongue.short_months.as_ref().map(|words| &words[..]),
            Word::Weekday => tongue.weekdays.as_ref().map(|words| &words[..]),
            Word::ShortWeekday => tongue.short_weekdays.as_ref().map(|words| &words[..]),
        };
        Some(words?.get(index)?.as_str())
    }

    /// The month `date` is in and its year, as a calendar is headed.
    pub fn month(self, date: NaiveDate) -> String {
        match self.word(Word::Month, date.month0() as usize) {
            Some(month) => format!("{month} {}", date.year()),
            None => date.format("%B %Y").to_string(),
        }
    }

    /// The moment `now` written after the `strftime` pattern `format`, the
    /// names of days and months in this language.
    pub fn moment(self, now: NaiveDateTime, format: &str) -> String {
        let items = StrftimeItems::new(format).map(|item| {
            let (day, month) = (
                now.weekday().num_days_from_monday() as usize,
                now.month0() as usize,
            );
            let named = match &item {
                Item::Fixed(Fixed::ShortWeekdayName) => self.word(Word::ShortWeekday, day),
                Item::Fixed(Fixed::LongWeekdayName) => self.word(Word::Weekday, day),
                Item::Fixed(Fixed::ShortMonthName) => self.word(Word::ShortMonth, month),
                Item::Fixed(Fixed::LongMonthName) => self.word(Word::MonthOf, month),
                _ => None,
            };
            named.map_or(item, Item::Literal)
        });
        now.format_with_items(items).to_string()
    }

    /// A day of the week in a few letters.
    pub fn weekday(self, day: Weekday) -> String {
        let named = self.word(Word::ShortWeekday, day.num_days_from_monday() as usize);
        named.map_or_else(|| day.to_string(), str::to_owned)
    }
}

const RUSSIAN_MONTHS: [&str; 12] = [
    "Январь",
    "Февраль",
    "Март",
    "Апрель",
    "Май",
    "Июнь",
    "Июль",
    "Август",
    "Сентябрь",
    "Октябрь",
    "Ноябрь",
    "Декабрь",
];

/// As a date names its month: the day "of" it.
const RUSSIAN_MONTHS_OF: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

const RUSSIAN_SHORT_MONTHS: [&str; 12] = [
    "янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
];

/// From Monday on.
const RUSSIAN_LONG_WEEKDAYS: [&str; 7] = [
    "понедельник",
    "вторник",
    "среда",
    "четверг",
    "пятница",
    "суббота",
    "воскресенье",
];

/// From Monday on.
const RUSSIAN_WEEKDAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

const RUSSIAN: &[(&str, &str)] = &[
    // The notifications and the problems.
    ("Notifications", "Уведомления"),
    ("No notifications", "Уведомлений нет"),
    ("Clear all", "Очистить"),
    ("Banners", "Баннеры"),
    ("Do not disturb", "Не беспокоить"),
    ("Seconds shown", "Секунд на экране"),
    ("Volume flyout", "Индикатор громкости"),
    ("Problems", "Проблемы"),
    ("No problem is on record", "Проблем не записано"),
    ("now", "сейчас"),
    ("min", "мин"),
    ("h", "ч"),
    ("d", "д"),
    // The sound.
    ("No audio", "Нет звука"),
    ("No audio output", "Нет устройства вывода"),
    ("No microphone", "Нет микрофона"),
    ("Applications", "Приложения"),
    ("System sounds", "Системные звуки"),
    (
        "No application plays sound",
        "Ни одно приложение не играет звук",
    ),
    ("Output", "Вывод"),
    ("In use", "Используется"),
    // The connections.
    ("Connections", "Подключения"),
    ("Nothing is connected", "Нет подключений"),
    ("Internet", "Интернет"),
    ("No internet", "Без интернета"),
    ("Not connected", "Не подключено"),
    ("Connecting", "Подключение"),
    ("Connected", "Подключено"),
    ("Disconnecting", "Отключение"),
    (
        "Add a VPN in the Windows settings",
        "Добавить VPN в параметрах Windows",
    ),
    ("Network settings", "Параметры сети"),
    // The Wi-Fi networks and the Bluetooth devices.
    ("Joining", "Подключение…"),
    ("Saved", "Сохранена"),
    (
        "Not saved; join it in the Windows settings first",
        "Не сохранена; сначала подключитесь в параметрах Windows",
    ),
    (
        "Open; join it in the Windows settings first",
        "Открытая; сначала подключитесь в параметрах Windows",
    ),
    ("Looking for networks", "Поиск сетей"),
    (
        "This machine has no Wi-Fi adapter",
        "В компьютере нет адаптера Wi-Fi",
    ),
    ("Wi-Fi is switched off", "Wi-Fi выключен"),
    ("No network is in reach", "Сетей поблизости нет"),
    ("Looking for devices", "Поиск устройств"),
    (
        "No Bluetooth device is paired",
        "Нет сопряжённых устройств Bluetooth",
    ),
    ("was", "было"),
    // Settings: the parts.
    ("General", "Общие"),
    ("Bars", "Панели"),
    ("Desktop", "Рабочий стол"),
    ("Home", "Домашняя папка"),
    ("Documents", "Документы"),
    ("Downloads", "Загрузки"),
    ("Pictures", "Изображения"),
    ("Music", "Музыка"),
    ("Videos", "Видео"),
    (
        "This display does not let its brightness be set",
        "Этот экран не даёт менять яркость",
    ),
    // Settings: the cards of the desktop.
    ("Cards", "Карточки"),
    ("Card", "Карточка"),
    (
        "Panels of widgets that lie on the desktop, below every window.",
        "Панели с виджетами, которые лежат на рабочем столе под всеми окнами.",
    ),
    ("Add a card", "Добавить карточку"),
    ("Remove the card", "Убрать карточку"),
    ("Place", "Место"),
    ("Top left", "Сверху слева"),
    ("Top right", "Сверху справа"),
    ("Bottom left", "Снизу слева"),
    ("Bottom right", "Снизу справа"),
    ("In the center", "По центру"),
    ("Displays", "Экраны"),
    ("All displays", "Все экраны"),
    ("Primary display", "Основной экран"),
    ("Width", "Ширина"),
    ("Height", "Высота"),
    ("Horizontal distance", "Отступ по горизонтали"),
    ("Vertical distance", "Отступ по вертикали"),
    ("Widgets of the card", "Виджеты карточки"),
    ("Widgets", "Виджеты"),
    ("Dock", "Док"),
    ("Tray", "Трей"),
    ("Windows", "Окна"),
    ("Themes", "Темы"),
    ("Extensions", "Расширения"),
    ("Quit", "Выйти"),
    ("Settings", "Настройки"),
    // Settings: the cards.
    ("Arrangement", "Расположение"),
    (
        "Replaces the bars and popups of the layout; pinned applications stay.",
        "Заменяет панели и попапы раскладки; закреплённые приложения остаются.",
    ),
    ("Language", "Язык"),
    (
        "Of the settings, the marketplace, the calendar, the clock and the notifications.",
        "Язык настроек, маркетплейса, календаря, часов и уведомлений.",
    ),
    ("Windows key", "Клавиша Windows"),
    ("Contrast", "Контраст"),
    (
        "Keep text readable (WCAG 2.1 AA)",
        "Держать текст читаемым (WCAG 2.1 AA)",
    ),
    ("Opens the launcher", "Открывает лаунчер"),
    ("Theme", "Тема"),
    ("Built-in", "Встроенная"),
    ("Windows accent", "Акцент Windows"),
    ("Pinned applications", "Закреплённые приложения"),
    ("Nothing is pinned.", "Ничего не закреплено."),
    (
        "Running now, click to pin:",
        "Запущены сейчас, нажмите, чтобы закрепить:",
    ),
    (
        "Applications are pinned to the `windows` widget, which the layout lacks.",
        "Приложения закрепляются в виджете `windows`, которого нет в раскладке.",
    ),
    ("Add", "Добавить"),
    // Arranging the widgets on the bars.
    ("Done", "Готово"),
    ("By hand", "Вручную"),
    ("Arrange on the bars", "Расставить на панелях"),
    (
        "Drag a widget to another place of its bar, or off the bar to take it out; the plus adds one.",
        "Перетащите виджет на другое место его панели или за панель, чтобы убрать; плюс добавляет новый.",
    ),
    ("Start", "Начало"),
    ("Center", "Центр"),
    ("End", "Конец"),
    ("Top bar", "Верхняя панель"),
    ("Bottom bar", "Нижняя панель"),
    ("Hiding", "Скрытие"),
    ("Always shown", "Всегда видна"),
    ("Hides by itself", "Скрывается сама"),
    ("Hides under windows", "Скрывается под окнами"),
    ("every display", "все экраны"),
    ("the primary display", "основной экран"),
    ("Not in the layout", "Нет в раскладке"),
    // Settings: options of widgets.
    ("Clock", "Часы"),
    ("Format", "Формат"),
    ("Calendar", "Календарь"),
    ("Week starts on", "Неделя начинается с"),
    ("Monday", "понедельника"),
    ("Sunday", "воскресенья"),
    ("Processor", "Процессор"),
    ("Memory", "Память"),
    ("Battery", "Батарея"),
    ("Style", "Вид"),
    ("Wave", "Волна"),
    ("Text", "Текст"),
    ("Shows", "Показывает"),
    ("Percent", "Проценты"),
    ("Gigabytes", "Гигабайты"),
    ("Network", "Сеть"),
    ("Name", "Имя"),
    ("Icon", "Значок"),
    ("Start menu", "Меню «Пуск»"),
    ("Columns", "Столбцы"),
    ("Icons shown", "Значков видно"),
    ("All", "Все"),
    ("Descriptions under the pointer", "Подсказки под указателем"),
    ("Tiling", "Тайлинг"),
    ("Tile windows", "Раскладывать окна"),
    ("Layout", "Раскладка"),
    ("Master", "Главное окно"),
    ("Rows", "Строки"),
    ("Grid", "Сетка"),
    ("Monocle", "Одно окно"),
    ("Gap", "Отступ"),
    ("Border of the window in front", "Рамка активного окна"),
    ("Workspaces", "Рабочие столы"),
    ("Own workspaces", "Свои рабочие столы"),
    ("Keys with a digit", "Клавиши с цифрой"),
    ("Default", "По умолчанию"),
    // The marketplace.
    ("Refresh", "Обновить"),
    ("Install", "Установить"),
    ("Update", "Обновить"),
    ("Remove", "Удалить"),
    ("Cancel", "Отмена"),
    ("Allow and install", "Разрешить и установить"),
    ("Downloading…", "Загрузка…"),
    ("Reading the registries…", "Читаю реестры…"),
    (
        "The registries offer no extensions",
        "Реестры не предлагают расширений",
    ),
    (
        "The registries offer no themes",
        "Реестры не предлагают тем",
    ),
    (
        "Deletes the extension and what it remembered.",
        "Удаляет расширение и всё, что оно запомнило.",
    ),
    ("Deletes the theme file.", "Удаляет файл темы."),
    (
        "A theme changes how Tessera looks and nothing else.",
        "Тема меняет только внешний вид Tessera.",
    ),
    ("From the registries", "Из реестров"),
    ("Installed", "Установлено"),
    ("Installed by hand", "Установлено вручную"),
    ("Widgets for the layout:", "Виджеты для раскладки:"),
    (
        "Asks for nothing beyond showing its widgets.",
        "Не просит ничего, кроме показа своих виджетов.",
    ),
    ("Asks to", "Просит разрешения"),
    ("and to", "и"),
    ("contact", "обращаться к"),
    ("read", "читать"),
    ("be told of the devices", "узнавать об устройствах"),
    // Settings: what is said beside the choices.
    ("Off", "Выкл."),
    (
        "Theme files are looked for in the themes directory of the configuration directory.",
        "Файлы тем ищутся в папке themes в папке конфигурации.",
    ),
    ("Keys", "Клавиши"),
    (
        "Key combinations Tessera answers wherever they are pressed. One that Windows or another program holds does nothing and is reported as a problem.",
        "Сочетания клавиш, на которые Tessera отвечает, где бы их ни нажали. Сочетание, занятое Windows или другой программой, ничего не делает и попадает в список проблем.",
    ),
    ("No key combination is set.", "Сочетаний нет."),
    ("Add a key", "Добавить сочетание"),
    ("Remove the key", "Убрать сочетание"),
    ("Held", "Удерживать"),
    ("Key", "Клавиша"),
    ("Does", "Действие"),
    ("Popup", "Попап"),
    ("Focus", "Фокус"),
    ("Move", "Перенос"),
    ("Program", "Программа"),
    ("Next", "Следующий"),
    ("Previous", "Предыдущий"),
    ("Left", "Влево"),
    ("Right", "Вправо"),
    ("Up", "Вверх"),
    ("Down", "Вниз"),
    (
        "The program is written in the file by hand.",
        "Программа пишется в файле вручную.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_the_table_lacks_stays_english() {
        assert_eq!(Language::Russian.say("Quit"), "Выйти");
        assert_eq!(Language::Russian.say("No such text"), "No such text");
        assert_eq!(Language::English.say("Quit"), "Quit");
    }

    #[test]
    fn a_moment_names_its_day_and_month_in_the_language() {
        let now = NaiveDate::from_ymd_opt(2026, 11, 21)
            .and_then(|date| date.and_hms_opt(9, 5, 0))
            .unwrap();
        let moment = |language: Language, format| language.moment(now, format);
        assert_eq!(
            moment(Language::English, "%a %d %b %H:%M"),
            "Sat 21 Nov 09:05"
        );
        assert_eq!(
            moment(Language::Russian, "%a %d %b %H:%M"),
            "Сб 21 ноя 09:05"
        );
        assert_eq!(
            moment(Language::Russian, "%A, %-d %B"),
            "суббота, 21 ноября"
        );
    }

    #[test]
    fn a_language_file_gives_its_name_its_texts_and_its_calendar() {
        let file = r#"
            name = "Deutsch"
            months = ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"]
            short-weekdays = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"]

            [texts]
            "Quit" = "Beenden"
        "#;
        let tongue: &'static Tongue = Box::leak(Box::new(Tongue::parse("de", file).unwrap()));
        let german = Language::File(tongue);
        assert_eq!((german.code(), german.name()), ("de", "Deutsch"));
        assert_eq!(german.say("Quit"), "Beenden");
        assert_eq!(german.say("No such text"), "No such text");
        let day = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        assert_eq!(german.month(day), "November 2026");
        assert_eq!(german.weekday(Weekday::Sat), "Sa");
        let now = day.and_hms_opt(9, 5, 0).unwrap();
        // The month after a day is the heading one where no other is
        // given, and what the file leaves out stays English.
        assert_eq!(
            german.moment(now, "%a %-d %B, %A %b"),
            "Sa 21 November, Saturday Nov"
        );
    }

    #[test]
    fn a_language_file_with_an_unknown_key_or_too_few_months_is_refused() {
        assert!(Tongue::parse("de", "name = \"Deutsch\"\ncolour = 1\n").is_err());
        assert!(Tongue::parse("de", "name = \"Deutsch\"\nmonths = [\"Januar\"]\n").is_err());
        assert!(Tongue::parse("de", "[texts]\n").is_err());
    }

    #[test]
    fn a_language_is_named_by_letters_and_never_by_a_path() {
        assert_eq!(Language::named("ru"), Some(Language::Russian));
        assert_eq!(Language::named("../config"), None);
        assert_eq!(Language::named(""), None);
    }

    #[test]
    fn the_language_file_that_comes_with_tessera_gives_the_texts_of_the_table() {
        let file = include_str!("../../../languages/de.toml");
        let german = Tongue::parse("de", file).unwrap();
        for (english, _) in RUSSIAN {
            let given = german.file.texts.contains_key(*english);
            assert!(given, "`{english}` is not given");
        }
        for english in german.file.texts.keys() {
            let known = RUSSIAN.iter().any(|(text, _)| text == english);
            assert!(known, "`{english}` is no text of Tessera");
        }
    }

    #[test]
    fn the_table_gives_each_text_once() {
        for (index, (english, _)) in RUSSIAN.iter().enumerate() {
            let rest = &RUSSIAN[index + 1..];
            let again = rest.iter().any(|(other, _)| other == english);
            assert!(!again, "`{english}` is given twice");
        }
    }

    #[test]
    fn a_language_is_named_by_two_letters() {
        for language in Language::BUILTIN {
            let text = format!("language = \"{}\"", language.code());
            let read: toml::Table = toml::from_str(&text).unwrap();
            let parsed: Language = read["language"].clone().try_into().unwrap();
            assert_eq!(parsed, language);
        }
    }
}

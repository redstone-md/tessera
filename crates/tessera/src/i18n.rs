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
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::format::{Fixed, Item, StrftimeItems};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};
use gpui::{App, Global};
use serde::Deserialize;

use crate::config::config_dir;

/// The directory of the language files, within the configuration and
/// beside the program.
const DIRECTORY: &str = "languages";

/// Where language files are looked for: the configuration first, then
/// the directory the program lies in, where a download keeps them.
fn directories() -> Vec<PathBuf> {
    let beside = std::env::current_exe().ok();
    let beside = beside.as_deref().and_then(Path::parent).map(Path::to_owned);
    { config_dir().into_iter().chain(beside) }
        .map(|directory| directory.join(DIRECTORY))
        .collect()
}

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
        let file = format!("{code}.toml");
        let found = { directories().into_iter() }.find_map(|directory| {
            let path = directory.join(&file);
            let written = fs::metadata(&path).and_then(|file| file.modified()).ok()?;
            Some((path, written))
        });
        let (path, written) = found?;
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
        let entries = { directories().into_iter() }
            .filter_map(|directory| fs::read_dir(directory).ok())
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
        codes.dedup();
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
    // The search of the launcher.
    ("Wi-Fi", "Wi-Fi"),
    ("Display", "Дисплей"),
    ("Night light", "Ночной свет"),
    ("Sound", "Звук"),
    ("Power and battery", "Питание и батарея"),
    ("Storage", "Память устройства"),
    ("Bluetooth and devices", "Bluetooth и устройства"),
    ("Printers and scanners", "Принтеры и сканеры"),
    ("Mouse", "Мышь"),
    ("Network and internet", "Сеть и Интернет"),
    ("Personalization", "Персонализация"),
    ("Background", "Фон"),
    ("Colors", "Цвета"),
    ("Lock screen", "Экран блокировки"),
    ("Installed apps", "Установленные приложения"),
    ("Default apps", "Приложения по умолчанию"),
    ("Startup apps", "Автозагрузка"),
    ("Your account", "Учётная запись"),
    ("Sign-in options", "Варианты входа"),
    ("Date and time", "Дата и время"),
    ("Language and region", "Язык и регион"),
    ("Typing", "Ввод"),
    ("Accessibility", "Специальные возможности"),
    ("Privacy and security", "Конфиденциальность и защита"),
    ("Windows Update", "Центр обновления Windows"),
    ("About this computer", "О системе"),
    ("Nothing matches", "Ничего не найдено"),
    ("Search applications", "Поиск приложений"),
    // The Wi-Fi networks and the Bluetooth devices.
    ("Joining", "Подключение…"),
    ("Saved", "Сохранена"),
    (
        "Not saved; join it in the Windows settings first",
        "Не сохранена; сначала подключитесь в параметрах Windows",
    ),
    ("Open network", "Открытая сеть"),
    ("Asks for a password", "Нужен пароль"),
    ("Password", "Пароль"),
    ("Join", "Подключиться"),
    (
        "Right-click again to forget it",
        "Правый щелчок ещё раз — забыть сеть",
    ),
    (
        "A password has 8 to 63 Latin characters",
        "В пароле от 8 до 63 латинских символов",
    ),
    ("The network was not joined", "Подключиться не удалось"),
    (
        "The network was not joined; check the password",
        "Подключиться не удалось; проверьте пароль",
    ),
    ("Looking for networks", "Поиск сетей"),
    (
        "This machine has no Wi-Fi adapter",
        "В компьютере нет адаптера Wi-Fi",
    ),
    ("Wi-Fi is switched off", "Wi-Fi выключен"),
    ("No network is in reach", "Сетей поблизости нет"),
    ("Empty", "Пусто"),
    (
        "Tessera keeps no workspaces of its own",
        "Своих рабочих столов Tessera не ведёт",
    ),
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
    ("Extension", "Расширение"),
    ("Windows of every display", "Окна со всех экранов"),
    ("Appearance", "Оформление"),
    ("Brightness", "Яркость"),
    ("Command", "Команда"),
    ("Show desktop", "Показать рабочий стол"),
    ("Window title", "Заголовок окна"),
    ("Keyboard layout", "Раскладка"),
    ("Label", "Надпись"),
    ("Now playing", "Сейчас играет"),
    ("Power plan", "Схема питания"),
    ("Recycle Bin", "Корзина"),
    ("Updates", "Обновления"),
    ("Account", "Учётная запись"),
    ("Volume", "Громкость"),
    ("Quit", "Выйти"),
    ("Restart Tessera", "Перезапустить Tessera"),
    ("Updating", "Обновление"),
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
    ("Window switcher", "Переключатель окон"),
    ("Window previews", "Миниатюры окон"),
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
    // The file manager.
    ("File manager", "Файловый менеджер"),
    ("Open folders in Tessera", "Открывать папки в Tessera"),
    ("Filter", "Фильтр"),
    ("This PC", "Этот компьютер"),
    ("New folder", "Новая папка"),
    ("Paste", "Вставить"),
    ("Open in terminal", "Открыть в терминале"),
    ("Show hidden files", "Показывать скрытые файлы"),
    ("Properties", "Свойства"),
    ("Open", "Открыть"),
    ("Open in new window", "Открыть в новом окне"),
    ("Cut", "Вырезать"),
    ("Copy", "Копировать"),
    ("Copy path", "Копировать путь"),
    ("Rename", "Переименовать"),
    ("Delete", "Удалить"),
    ("Show in Explorer", "Показать в Проводнике"),
    ("Places", "Места"),
    ("Modified", "Изменён"),
    ("Type", "Тип"),
    ("Size", "Размер"),
    ("free of", "свободно из"),
    ("items", "элем."),
    ("selected", "выбрано"),
    ("Reading…", "Чтение…"),
    ("This folder cannot be read", "Эту папку нельзя прочитать"),
    ("This folder is empty", "Папка пуста"),
    // Settings: keys that name an action.
    ("Action", "Действие"),
    (
        "The action is written in the file by hand.",
        "Действие записывается в файл вручную.",
    ), // Tessera as the shell of the session (shell-host).
    ("Shell of the session", "Оболочка сеанса"),
    (
        "Tessera is the shell of this session.",
        "Tessera — оболочка этого сеанса.",
    ),
    (
        "Tessera runs beside Explorer.",
        "Tessera работает рядом с Проводником.",
    ),
    (
        "At the next sign-in Windows starts Tessera.",
        "При следующем входе Windows запустит Tessera.",
    ),
    (
        "At the next sign-in Windows starts Explorer.",
        "При следующем входе Windows запустит Проводник.",
    ),
    (
        "At the next sign-in Windows starts another program:",
        "При следующем входе Windows запустит другую программу:",
    ),
    (
        "A policy of this computer names the shell; only an administrator can change it.",
        "Оболочку задаёт политика этого компьютера; изменить её может только администратор.",
    ),
    (
        "Windows is told to read the shell of the computer. It is widely reported to read the one of the user all the same; that is not certain on this computer.",
        "Windows настроена читать оболочку компьютера. По многим сообщениям, она всё равно читает оболочку пользователя; на этом компьютере это не проверено.",
    ),
    (
        "A change takes effect at the next sign-in.",
        "Изменение вступит в силу при следующем входе.",
    ),
    ("Make Tessera the shell", "Сделать Tessera оболочкой"),
    (
        "Give the shell back to Explorer",
        "Вернуть оболочку Проводнику",
    ),
    (
        "Hand this session to Explorer now",
        "Передать этот сеанс Проводнику сейчас",
    ),
    (
        "Ctrl+Win+Alt+E starts Task Manager and hands the session to Explorer; Ctrl+Win+Alt+R starts Tessera again.",
        "Ctrl+Win+Alt+E запускает диспетчер задач и передаёт сеанс Проводнику; Ctrl+Win+Alt+R перезапускает Tessera.",
    ),
    // Startup applications (startup-apps).
    ("Registry, all users", "Реестр, все пользователи"),
    (
        "Registry, all users, 32-bit",
        "Реестр, все пользователи, 32 бита",
    ),
    ("Policy, all users", "Политика, все пользователи"),
    ("Registry, this user", "Реестр, этот пользователь"),
    ("Policy, this user", "Политика, этот пользователь"),
    (
        "Startup folder, all users",
        "Папка автозагрузки, все пользователи",
    ),
    (
        "Startup folder, this user",
        "Папка автозагрузки, этот пользователь",
    ),
    ("Store app", "Приложение из Store"),
    ("Set by policy", "Задано политикой"),
    (
        "Changing this for all users needs Tessera to run as administrator.",
        "Чтобы изменить это для всех пользователей, Tessera нужно запустить от имени администратора.",
    ),
    (
        "No program starts with Windows.",
        "Ни одна программа не запускается вместе с Windows.",
    ),
    (
        "Tessera starts the programs that are on when you sign in.",
        "Tessera запускает включённые программы при входе в систему.",
    ),
    (
        "Tessera starts none of them, as [startup] says run = false.",
        "Tessera не запускает ни одну из них: в [startup] указано run = false.",
    ),
    (
        "Explorer starts these programs when you sign in; Tessera starts them only as the shell.",
        "Эти программы при входе в систему запускает Проводник; Tessera запускает их, только когда она — оболочка.",
    ),
    // The file manager: tabs, panes, places, views, search and renaming.
    ("Open in new tab", "Открыть в новой вкладке"),
    ("Copy to the other pane", "Копировать в другую панель"),
    ("Move to the other pane", "Переместить в другую панель"),
    ("tabs", "вкладок"),
    ("Quick access", "Быстрый доступ"),
    (
        "Pin to Quick access",
        "Закрепить на панели быстрого доступа",
    ),
    (
        "Unpin from Quick access",
        "Открепить от панели быстрого доступа",
    ),
    ("Details", "Таблица"),
    ("List", "Список"),
    ("Tiles", "Плитка"),
    ("Large icons", "Крупные значки"),
    ("No groups", "Без группировки"),
    ("Group by type", "Группировать по типу"),
    ("Group by date", "Группировать по дате"),
    ("Folder", "Папка"),
    ("File", "Файл"),
    ("Today", "Сегодня"),
    ("Yesterday", "Вчера"),
    ("Earlier this week", "Ранее на этой неделе"),
    ("Last week", "На прошлой неделе"),
    ("Earlier this month", "Ранее в этом месяце"),
    ("Last month", "В прошлом месяце"),
    ("Earlier this year", "Ранее в этом году"),
    ("A long time ago", "Давно"),
    ("Unknown date", "Дата неизвестна"),
    ("Recent files", "Последние файлы"),
    ("Select all", "Выделить все"),
    ("Select by pattern", "Выделить по шаблону"),
    ("Invert selection", "Обратить выделение"),
    (
        "Patterns to select, as *.jpg; IMG_????.*",
        "Шаблоны для выделения, например *.jpg; IMG_????.*",
    ),
    ("Search results in", "Результаты поиска в"),
    ("Search results", "Результаты поиска"),
    ("Searching…", "Поиск…"),
    (
        "found by the Windows Search index",
        "найдено по индексу Windows Search",
    ),
    ("found by reading the folders", "найдено чтением папок"),
    ("the first shown", "показаны первые"),
    ("Open file location", "Расположение файла"),
    ("Nothing was found", "Ничего не найдено"),
    ("Rename all", "Переименовать все"),
    ("New name, # for the number", "Новое имя, # вместо номера"),
    ("Find", "Найти"),
    ("Replace with", "Заменить на"),
    ("No name changes", "Ни одно имя не меняется"),
    ("cannot be the name of a file", "не может быть именем файла"),
    ("would be the name of two files", "досталось бы двум файлам"),
    (
        "is the name of something else here",
        "уже носит здесь другой объект",
    ),
    // files-verbs-extras: the menu of the Recycle Bin.
    ("Restore all items", "Восстановить все объекты"),
    ("Empty the Recycle Bin", "Очистить корзину"),
    (
        "Move every item in the Recycle Bin back to the folder it was deleted from?",
        "Вернуть все объекты из корзины в папки, из которых они были удалены?",
    ),
    // The runners of the launcher, the runner and the Run dialog.
    ("Calculator", "Калькулятор"),
    ("Runs in a terminal", "Выполняется в терминале"),
    ("Opens the address", "Открывает адрес"),
    ("Opens the folder", "Открывает папку"),
    ("Runs the program", "Запускает программу"),
    ("Opens the file", "Открывает файл"),
    ("Copies the character", "Копирует символ"),
    ("Opened lately", "Недавно открытый"),
    ("Session and power", "Сеанс и питание"),
    ("Exchange rates are unavailable", "Курсы валют недоступны"),
    (
        "{} · rate of the European Central Bank of {}",
        "{} · курс Европейского центрального банка на {}",
    ),
    (
        "Exchange rates are being fetched…",
        "Курсы валют загружаются…",
    ),
    ("European Central Bank", "Европейский центральный банк"),
    ("Fetching them is switched off", "Их загрузка отключена"),
    ("They could not be fetched", "Их не удалось загрузить"),
    (
        "The European Central Bank gives no rate for this currency",
        "Европейский центральный банк не даёт курса этой валюты",
    ),
    ("Search {} for “{}”", "Найти в {} «{}»"),
    ("Search on the web", "Поиск в интернете"),
    ("Search the web for “{}”", "Найти в интернете «{}»"),
    ("Switch to the window", "Перейти к окну"),
    ("Ends the process {} at once", "Сразу завершает процесс {}"),
    (
        "Ends its {} processes at once",
        "Сразу завершает его процессы: {}",
    ),
    (
        "A program, a folder, a document or an address",
        "Программа, папка, документ или адрес",
    ),
    (
        "Windows cannot open “{}”.",
        "Windows не удаётся открыть «{}».",
    ),
    ("Run a command", "Выполнить команду"),
    (
        "Type the name of a program, a folder, a document or an address to open it. Ctrl+Shift+Enter runs it as an administrator.",
        "Введите имя программы, папки, документа или адрес, чтобы открыть их. Ctrl+Shift+Enter запускает от имени администратора.",
    ),
    ("As administrator", "От имени администратора"),
    ("Browse for a file…", "Выбрать файл…"),
    ("Search, calculate, run…", "Искать, считать, запускать…"),
    (
        "Press Enter again to confirm",
        "Нажмите Enter ещё раз для подтверждения",
    ),
    (
        "Applications, windows, commands, calculations and more",
        "Приложения, окна, команды, вычисления и не только",
    ),
    ("VPN", "VPN"),
    ("Taskbar", "Панель задач"),
    ("Fonts", "Шрифты"),
    ("Multitasking", "Многозадачность"),
    ("Touchpad", "Сенсорная панель"),
    ("Pen and Windows Ink", "Перо и Windows Ink"),
    ("AutoPlay", "Автозапуск"),
    ("USB", "USB"),
    ("Cameras", "Камеры"),
    ("Ethernet", "Ethernet"),
    ("Airplane mode settings", "Параметры режима «в самолёте»"),
    ("Proxy", "Прокси-сервер"),
    (
        "Advanced network settings",
        "Дополнительные сетевые параметры",
    ),
    ("Data usage", "Использование данных"),
    ("Graphics", "Графика"),
    ("Optional features", "Дополнительные компоненты"),
    ("Apps for websites", "Приложения для веб-сайтов"),
    ("Video playback", "Воспроизведение видео"),
    ("Email and accounts", "Электронная почта и учётные записи"),
    ("Family", "Семья"),
    ("Windows Backup", "Архивация Windows"),
    ("Other users", "Другие пользователи"),
    (
        "Access work or school",
        "Доступ к учётной записи места работы или учебного заведения",
    ),
    ("Game Bar", "Игровая панель"),
    ("Game Mode", "Игровой режим"),
    ("Captures", "Клипы"),
    ("Text size", "Размер текста"),
    ("Visual effects", "Визуальные эффекты"),
    ("Mouse pointer and touch", "Указатель мыши и сенсорный ввод"),
    ("Magnifier", "Экранная лупа"),
    ("Color filters", "Цветовые фильтры"),
    ("Narrator", "Экранный диктор"),
    ("Speech", "Речь"),
    ("Captions", "Субтитры"),
    ("Location permissions", "Разрешения на расположение"),
    ("Camera permissions", "Разрешения для камеры"),
    ("Microphone permissions", "Разрешения для микрофона"),
    ("Windows Security", "Безопасность Windows"),
    ("Find my device", "Поиск устройства"),
    ("For developers", "Для разработчиков"),
    ("Recovery", "Восстановление"),
    ("Activation", "Активация"),
    ("Troubleshoot", "Устранение неполадок"),
    ("Remote Desktop", "Удалённый рабочий стол"),
    ("Clipboard settings", "Параметры буфера обмена"),
    ("Nearby sharing", "Обмен с устройствами поблизости"),
    ("Region", "Регион"),
    ("Disks and volumes", "Диски и тома"),
    ("Control Panel", "Панель управления"),
    (
        "Manage devices and drivers",
        "Управление устройствами и драйверами",
    ),
    ("Events and logs of Windows", "События и журналы Windows"),
    (
        "Manage disks and partitions",
        "Управление дисками и разделами",
    ),
    ("Manage the computer", "Управление компьютером"),
    ("Network adapters", "Сетевые адаптеры"),
    (
        "Running programs and their load",
        "Запущенные программы и их нагрузка",
    ),
    ("Services", "Службы"),
    ("Registry Editor", "Редактор реестра"),
    ("System information", "Сведения о системе"),
    ("System configuration", "Конфигурация системы"),
    ("Task Scheduler", "Планировщик заданий"),
    ("Programs and Features", "Программы и компоненты"),
    ("User accounts", "Учётные записи пользователей"),
    ("Credential Manager", "Диспетчер учётных данных"),
    ("File Explorer options", "Параметры Проводника"),
    ("Environment variables", "Переменные среды"),
    (
        "Advanced system settings",
        "Дополнительные параметры системы",
    ),
    ("Windows Tools", "Инструменты Windows"),
    ("Windows Defender Firewall", "Брандмауэр Защитника Windows"),
    ("Resource Monitor", "Монитор ресурсов"),
    ("Character Map", "Таблица символов"),
    ("Lock the session", "Заблокировать сеанс"),
    (
        "Put the computer to sleep",
        "Перевести компьютер в спящий режим",
    ),
    ("Hibernate the computer", "Перевести компьютер в гибернацию"),
    ("Sign out of Windows", "Выйти из Windows"),
    ("Restart the computer", "Перезагрузить компьютер"),
    ("Shut down the computer", "Выключить компьютер"),
    ("Windows tool", "Средство Windows"),
    ("Length", "Длина"),
    ("Weight", "Масса"),
    ("Temperature", "Температура"),
    ("Capacity", "Объём"),
    ("Area", "Площадь"),
    ("Speed", "Скорость"),
    ("Data size", "Объём данных"),
    ("Duration", "Длительность"),
    // The dialog to shut down (winx-hotkeys).
    (
        "What should the computer do?",
        "Что должен сделать компьютер?",
    ),
    ("Switch user", "Сменить пользователя"),
    (
        "Shows the sign-in screen and keeps this session with its apps open.",
        "Показывает экран входа; этот сеанс и его приложения остаются открытыми.",
    ),
    ("Sign out", "Выйти из системы"),
    (
        "Closes all apps and ends the session.",
        "Закрывает все приложения и завершает сеанс.",
    ),
    ("Sleep", "Спящий режим"),
    (
        "Keeps the session in memory and puts the computer into a low-power state.",
        "Сохраняет сеанс в памяти и переводит компьютер в режим пониженного энергопотребления.",
    ),
    ("Hibernate", "Гибернация"),
    (
        "Saves the session to the disk and turns the computer off.",
        "Сохраняет сеанс на диск и выключает компьютер.",
    ),
    ("Update and shut down", "Обновить и завершить работу"),
    (
        "Installs the updates that wait, then turns the computer off.",
        "Устанавливает ожидающие обновления, затем выключает компьютер.",
    ),
    ("Shut down", "Завершение работы"),
    (
        "Closes all apps and turns the computer off.",
        "Закрывает все приложения и выключает компьютер.",
    ),
    ("Update and restart", "Обновить и перезагрузить"),
    (
        "Installs the updates that wait, then starts the computer again.",
        "Устанавливает ожидающие обновления, затем перезапускает компьютер.",
    ),
    ("Restart", "Перезагрузка"),
    (
        "Closes all apps, turns the computer off and starts it again.",
        "Закрывает все приложения, выключает компьютер и снова включает его.",
    ),
    ("OK", "ОК"),
    // How the displays show the desktop (winx-hotkeys).
    ("Project", "Проецирование"),
    ("PC screen only", "Только экран компьютера"),
    ("Duplicate", "Повторяющийся"),
    ("Extend", "Расширить"),
    ("Second screen only", "Только второй экран"),
    (
        "Connect to a wireless display",
        "Подключиться к беспроводному дисплею",
    ),
    (
        "Only one display is connected.",
        "Подключён только один дисплей.",
    ),
    // The keyboard layouts (winx-hotkeys).
    ("No window is typed into.", "Нет окна, в котором идёт ввод."),
    // The menu of Win+X (winx-hotkeys).
    ("Mobility Center", "Центр мобильности"),
    ("Power Options", "Управление электропитанием"),
    ("Event Viewer", "Просмотр событий"),
    ("System", "Система"),
    ("Device Manager", "Диспетчер устройств"),
    ("Network Connections", "Сетевые подключения"),
    ("Disk Management", "Управление дисками"),
    ("Computer Management", "Управление компьютером"),
    ("Terminal", "Терминал"),
    ("Terminal (Admin)", "Терминал (администратор)"),
    ("Windows PowerShell", "Windows PowerShell"),
    (
        "Windows PowerShell (Admin)",
        "Windows PowerShell (администратор)",
    ),
    ("Task Manager", "Диспетчер задач"),
    ("File Explorer", "Проводник"),
    ("Search", "Поиск"),
    ("Run", "Выполнить"),
    (
        "Shut down or sign out",
        "Завершение работы или выход из системы",
    ),
    // Settings: the button of the menu of Win+X (winx-hotkeys).
    ("Win+X menu", "Меню Win+X"),
    // Window snapping, rules, hot corners and the window menu.
    (
        "Choose a window for this space",
        "Выберите окно для этого места",
    ),
    ("Keep above other windows", "Держать поверх других окон"),
    (
        "Stop keeping above other windows",
        "Не держать поверх других окон",
    ),
    ("Window opacity", "Непрозрачность окна"),
    ("Move to the next display", "Переместить на следующий экран"),
    ("Snap to the left half", "Прикрепить к левой половине"),
    ("Snap to the right half", "Прикрепить к правой половине"),
    ("Restore the size of the window", "Восстановить размер окна"),
    ("Maximize the window", "Развернуть окно"),
    ("Minimize the window", "Свернуть окно"),
    ("Close the window", "Закрыть окно"),
    (
        "Create a rule for this window",
        "Создать правило для этого окна",
    ),
    ("Move to workspace", "Перенести на рабочий стол"),
    // Notifications: history, do not disturb and the bell.
    ("For 1 hour", "На 1 час"),
    ("Until morning", "До утра"),
    ("Until turned off", "Пока не выключено"),
    ("On until", "Включено до"),
    ("As scheduled, until", "По расписанию, до"),
    (
        "While an app fills the screen",
        "Пока приложение на весь экран",
    ),
    ("While a game runs", "Пока идёт игра"),
    ("While presenting", "Во время презентации"),
    ("While a display is duplicated", "Пока экран дублируется"),
    ("Unknown application", "Неизвестное приложение"),
    ("Banners are switched off", "Баннеры выключены"),
    // The history of the clipboard.
    ("Search the clipboard", "Поиск в буфере обмена"),
    ("Nothing was copied yet", "Ещё ничего не скопировано"),
    ("Clipboard history is off", "Журнал буфера обмена выключен"),
    ("Picture", "Изображение"),
    ("Pinned", "Закреплено"),
    // The quick settings: radios, hotspot, power mode and night light.
    ("On", "Вкл."),
    ("Unavailable", "Недоступно"),
    ("Bluetooth", "Bluetooth"),
    ("Airplane mode", "Режим «в самолёте»"),
    ("Mobile network", "Мобильная сеть"),
    ("FM radio", "FM-радио"),
    ("Radio", "Радиомодуль"),
    (
        "The radios cannot be read",
        "Не удаётся прочитать радиомодули",
    ),
    ("This machine has no radio", "В компьютере нет радиомодулей"),
    (
        "Switched off on the machine itself",
        "Выключен на самом компьютере",
    ),
    ("Mobile hotspot", "Мобильный хот-спот"),
    ("Devices", "Устройства"),
    ("Switching…", "Переключение…"),
    ("Any band", "Любой диапазон"),
    ("Network name", "Имя сети"),
    ("Devices on it", "Подключено устройств"),
    (
        "Scan with a phone to join",
        "Отсканируйте телефоном, чтобы подключиться",
    ),
    ("Mobile hotspot settings", "Параметры мобильного хот-спота"),
    (
        "There is no internet connection to share",
        "Нет подключения к интернету, которым можно поделиться",
    ),
    (
        "Windows does not say what the hotspot is",
        "Windows не сообщает состояние хот-спота",
    ),
    (
        "A group policy forbids the hotspot",
        "Групповая политика запрещает хот-спот",
    ),
    (
        "The Wi-Fi adapter cannot be a hotspot",
        "Адаптер Wi-Fi не может быть хот-спотом",
    ),
    (
        "The mobile operator forbids the hotspot",
        "Мобильный оператор запрещает хот-спот",
    ),
    (
        "This edition of Windows has no hotspot",
        "В этом выпуске Windows нет хот-спота",
    ),
    (
        "The application of the mobile operator is missing",
        "Не установлено приложение мобильного оператора",
    ),
    (
        "Windows does not allow the hotspot",
        "Windows не разрешает хот-спот",
    ),
    ("The hotspot is not available", "Хот-спот недоступен"),
    ("Power mode", "Режим питания"),
    ("Best power efficiency", "Максимальная экономия энергии"),
    ("Balanced", "Сбалансированный"),
    ("Best performance", "Максимальная производительность"),
    ("Efficiency", "Экономия"),
    ("Performance", "Производительность"),
    ("Set by the power plan", "Задаётся схемой питания"),
    (
        "This version of Windows has no power modes",
        "В этой версии Windows нет режимов питания",
    ),
    (
        "The power plan in use is not Balanced, so Windows offers no power mode",
        "Используется не сбалансированная схема питания, поэтому Windows не предлагает режимов",
    ),
    ("Battery saver is on", "Экономия заряда включена"),
    ("until", "до"),
    ("Night light settings", "Параметры ночного света"),
    (
        "Windows switches it by the schedule of its settings",
        "Windows включает его по расписанию из своих параметров",
    ),
    (
        "Tessera's own; no schedule",
        "Собственный ночной свет Tessera; без расписания",
    ),
    (
        "Tessera's own, from sunset to sunrise",
        "Собственный ночной свет Tessera, от заката до рассвета",
    ),
    (
        "Tessera's own; the schedule needs the place",
        "Собственный ночной свет Tessera; для расписания нужно указать место",
    ),
    (
        "Tessera's own, by the hours",
        "Собственный ночной свет Tessera, по часам",
    ),
    (
        "Windows holds no warmth of night light yet; set it once in its settings",
        "Windows ещё не хранит теплоту ночного света; задайте её один раз в параметрах",
    ),
    (
        "Windows keeps no state of night light yet; open it once in the settings",
        "Windows ещё не хранит состояние ночного света; откройте его один раз в параметрах",
    ),
    (
        "Windows keeps the state of night light in a way Tessera does not know",
        "Windows хранит состояние ночного света в незнакомом Tessera виде",
    ),
    // Quick settings: the power plans.
    (
        "Windows tells of no power plan",
        "Windows не сообщает ни одной схемы питания",
    ),
    // Quick settings: microphones, media sessions and flyouts.
    ("Input", "Ввод"),
    ("Sound settings", "Параметры звука"),
    ("Spatial sound", "Пространственный звук"),
    ("Microphone off", "Микрофон выключен"),
    ("Microphone on", "Микрофон включён"),
    ("Caps Lock on", "Caps Lock включён"),
    ("Caps Lock off", "Caps Lock выключен"),
    ("Num Lock on", "Num Lock включён"),
    ("Num Lock off", "Num Lock выключен"),
    ("Nothing is playing", "Ничего не воспроизводится"),
    ("Unnamed", "Без названия"),
    // Wallpaper: the widget and the theme of its colors.
    ("Wallpaper", "Обои"),
    ("Wallpaper colors", "Цвета обоев"),
    // Removable devices, the privacy indicator and battery warnings.
    ("Removable devices", "Съёмные устройства"),
    ("Eject", "Извлечь"),
    ("Ejecting…", "Извлечение…"),
    ("Safely remove", "Безопасное извлечение"),
    ("Nothing to take out", "Извлекать нечего"),
    ("Safe to remove", "Можно извлечь"),
    ("can now be taken out.", "теперь можно извлечь."),
    ("The device was not ejected", "Устройство не извлечено"),
    (
        "A program is using it; close its files and try again.",
        "Его использует программа; закройте её файлы и попробуйте снова.",
    ),
    ("It is in use by", "Его использует"),
    (
        "This account may not eject it.",
        "Этой учётной записи нельзя его извлечь.",
    ),
    (
        "Windows refused to eject it.",
        "Windows отказалась его извлечь.",
    ),
    ("A drive was plugged in", "Подключён диск"),
    ("A disc or a card was put in", "Вставлен диск или карта"),
    ("of the charge is left.", "заряда осталось."),
    ("The battery is low", "Батарея разряжается"),
    (
        "Plug the computer in soon.",
        "Скоро подключите компьютер к сети.",
    ),
    ("The battery is very low", "Батарея почти разряжена"),
    (
        "Plug the computer in now.",
        "Подключите компьютер к сети сейчас.",
    ),
    (
        "Plug the computer in now; it will soon go to sleep.",
        "Подключите компьютер к сети сейчас; скоро он перейдёт в спящий режим.",
    ),
    (
        "Plug the computer in now; it will soon hibernate.",
        "Подключите компьютер к сети сейчас; скоро он перейдёт в режим гибернации.",
    ),
    (
        "Plug the computer in now; it will soon shut down.",
        "Подключите компьютер к сети сейчас; скоро он выключится.",
    ),
    ("The battery is nearly empty", "Батарея почти пуста"),
    (
        "Save your work and plug the computer in.",
        "Сохраните работу и подключите компьютер к сети.",
    ),
    ("Microphone", "Микрофон"),
    ("Camera", "Камера"),
    ("Location", "Местоположение"),
    (
        "No application uses the microphone, the camera or the location",
        "Микрофон, камеру и местоположение не использует ни одно приложение",
    ),
    // The window switcher: its styles and filters.
    ("Workspace", "Рабочий стол"),
    ("Icon tiles", "Плитки значков"),
    ("Large pictures", "Большие изображения"),
    ("Compact list", "Компактный список"),
    ("Row of icons", "Ряд значков"),
    ("Which windows", "Какие окна"),
    ("This workspace", "Этот рабочий стол"),
    ("This display", "Этот дисплей"),
    ("All workspaces", "Все рабочие столы"),
    ("Minimized windows", "Свёрнутые окна"),
    ("In their place", "На своём месте"),
    ("After the others", "После остальных"),
    ("Left out", "Не показывать"),
    // integration: the verbs of the Shell in the menu of the file manager.
    ("New", "Новый"),
    ("Open with", "Открыть с помощью"),
    ("Choose another app", "Выбрать другое приложение"),
    ("Send to", "Отправить"),
    ("Show more options", "Показать дополнительные параметры"),
    // Shared by wave 2.
    ("Run as administrator", "Запуск от имени администратора"),
    ("Undo", "Отменить"),
    ("Redo", "Повторить"),
    ("Wallpaper settings", "Настройки обоев"),
    ("Display settings", "Параметры экрана"),
    ("Power settings", "Параметры питания"),
    ("Add widget", "Добавить виджет"),
    ("Add widgets", "Добавить виджеты"),
    ("Share", "Поделиться"),
    ("End task", "Снять задачу"),
    ("Kill process", "Завершить процесс"),
    ("Media", "Медиа"),
    ("Personalize", "Персонализация"),
    // Wave 2, jumplists-peek: the sections of a jump list.
    ("Frequent", "Часто используемые"),
    ("Recent", "Последние"),
    ("Tasks", "Задачи"),
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
    fn the_language_files_that_come_with_tessera_give_the_texts_of_the_table() {
        let files = [
            ("de", include_str!("../../../languages/de.toml")),
            ("es", include_str!("../../../languages/es.toml")),
            ("fr", include_str!("../../../languages/fr.toml")),
            ("uk", include_str!("../../../languages/uk.toml")),
        ];
        for (name, file) in files {
            let tongue = Tongue::parse(name, file).unwrap();
            for (english, _) in RUSSIAN {
                let given = tongue.file.texts.contains_key(*english);
                assert!(given, "`{english}` is not given in `{name}`");
            }
            for english in tongue.file.texts.keys() {
                let known = RUSSIAN.iter().any(|(text, _)| text == english);
                assert!(known, "`{english}` of `{name}` is no text of Tessera");
            }
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

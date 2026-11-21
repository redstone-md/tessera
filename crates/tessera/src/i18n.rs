// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The language Tessera speaks in its settings and its marketplace. Texts
//! are written in English where they are used; a table gives them in the
//! other languages, and a text it lacks stays English.

use gpui::{App, Global};
use serde::Deserialize;

/// A language of the user interface, named in the configuration by its
/// two letters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "ru")]
    Russian,
}

impl Global for Language {}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::Russian];

    /// The language in use.
    pub fn of(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// What the configuration calls it.
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
        }
    }

    /// What it calls itself.
    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Русский",
        }
    }

    /// `text` in this language, or as it is where the table lacks it.
    pub fn say(self, text: &'static str) -> &'static str {
        let table = match self {
            Self::English => return text,
            Self::Russian => RUSSIAN,
        };
        let found = table.iter().find(|(english, _)| *english == text);
        found.map_or(text, |&(_, said)| said)
    }
}

const RUSSIAN: &[(&str, &str)] = &[
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
        "Of the settings and the marketplace.",
        "Язык настроек и маркетплейса.",
    ),
    ("Windows key", "Клавиша Windows"),
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
    ("Gap", "Отступ"),
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
    fn the_table_gives_each_text_once() {
        for (index, (english, _)) in RUSSIAN.iter().enumerate() {
            let rest = &RUSSIAN[index + 1..];
            let again = rest.iter().any(|(other, _)| other == english);
            assert!(!again, "`{english}` is given twice");
        }
    }

    #[test]
    fn a_language_is_named_by_two_letters() {
        for language in Language::ALL {
            let text = format!("language = \"{}\"", language.code());
            let read: toml::Table = toml::from_str(&text).unwrap();
            let parsed: Language = read["language"].clone().try_into().unwrap();
            assert_eq!(parsed, language);
        }
    }
}

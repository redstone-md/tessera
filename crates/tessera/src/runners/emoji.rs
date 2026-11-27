// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Emoji and symbols: `:heart` offers ❤️ and the others whose names have
//! a word starting with what follows the colon, and choosing one copies
//! it. The names are English and Russian; in another language the
//! English ones are searched and shown.

use crate::i18n::Language;

use super::{Answer, AnswerAction, Query, Runner};

const ICON_EMOJI: &str = "\u{e76e}";

/// How sure the runner is of a search asked for with the colon, and how
/// many it shows.
const SCORE: u16 = 900;
const SHOWN: usize = 16;

/// How many letters after the colon the search waits for.
const LETTERS: usize = 2;

/// One emoji or symbol a line: the character, its English names and its
/// Russian names, the first of each the one it is shown with.
const TABLE: &str = "\
😀|grinning face,smile,happy|улыбка,радость
😃|smiling face with big eyes,smile|улыбка
😄|smiling face with smiling eyes,laugh|смех,улыбка
😁|beaming face,grin|ухмылка
😆|laughing,squinting|хохот,смех
😅|grinning with sweat,relief|облегчение,пот
🤣|rolling on the floor laughing,rofl|катаюсь от смеха
😂|tears of joy,lol,laugh|слёзы радости,смех
🙂|slightly smiling face|лёгкая улыбка
🙃|upside down face|перевёрнутое лицо
😉|winking face,wink|подмигивание
😊|smiling face with blush,blush|румянец,улыбка
😇|halo,angel,innocent|ангел,нимб
🥰|smiling face with hearts,love|влюблённость
😍|heart eyes,love|глаза-сердечки,любовь
🤩|star struck,excited|восторг,звёзды
😘|face blowing a kiss,kiss|воздушный поцелуй
😋|yum,delicious,tongue|вкусно
😛|tongue out|язык
😜|winking face with tongue,crazy|подмигивание с языком
🤪|zany face,crazy|чокнутый
🤔|thinking face,hmm|думаю,задумчивость
🤨|raised eyebrow,skeptical|бровь,сомнение
😐|neutral face|нейтральное лицо
😑|expressionless|без выражения
😶|no mouth,silent|молчание
🙄|rolling eyes|закатывает глаза
😏|smirk|ухмылка,самодовольство
😬|grimacing|гримаса
🤥|lying face,liar|ложь,враньё
😌|relieved|облегчение
😔|pensive,sad|грусть,задумчивость
😪|sleepy|сонный
😴|sleeping,zzz|сон,спит
😷|face with mask,sick|маска,болезнь
🤒|thermometer,fever,ill|температура,болезнь
🤢|nauseated|тошнота
🤮|vomiting|рвота
🥵|hot face|жарко
🥶|cold face,freezing|холодно,мороз
😵|dizzy face|головокружение
🤯|exploding head,mind blown|взрыв мозга
🥳|partying face,party,celebrate|праздник,вечеринка
😎|sunglasses,cool|очки,круто
🤓|nerd|ботаник
😕|confused|растерянность
😟|worried|тревога,беспокойство
🙁|slightly frowning face|недовольство
😮|open mouth,surprised|удивление
😲|astonished,shocked|изумление
😳|flushed,embarrassed|смущение
🥺|pleading face,puppy eyes|мольба,щенячьи глаза
😦|frowning with open mouth|испуг
😨|fearful|страх
😰|anxious with sweat|тревога
😢|crying face,tear|слеза,плач
😭|loudly crying,sob|рыдание,плач
😱|screaming in fear,scream|крик,ужас
😤|steam from nose,triumph|фырканье
😡|pouting face,angry,rage|злость,ярость
😠|angry face|сердитый
🤬|cursing,swearing|ругань
😈|smiling devil|чертёнок
💀|skull,dead|череп,смерть
💩|pile of poo,poop|какашка
🤡|clown|клоун
👻|ghost|призрак,привидение
👽|alien|инопланетянин
🤖|robot|робот
😺|grinning cat|улыбающийся кот
❤️|red heart,love|сердце,любовь
🧡|orange heart|оранжевое сердце
💛|yellow heart|жёлтое сердце
💚|green heart|зелёное сердце
💙|blue heart|синее сердце
💜|purple heart|фиолетовое сердце
🖤|black heart|чёрное сердце
🤍|white heart|белое сердце
💔|broken heart|разбитое сердце
💕|two hearts|два сердца
💖|sparkling heart|сверкающее сердце
💯|hundred points,100|сто баллов
💢|anger symbol|гнев
💥|collision,boom|бум,взрыв
💫|dizzy|звёздочки
💦|sweat droplets,water|капли
💬|speech balloon,chat|реплика,чат
💤|zzz,sleep|сон
👋|waving hand,hello,bye|привет,пока,машет рукой
🤚|raised back of hand|поднятая рука
✋|raised hand,stop|ладонь,стоп
🖖|vulcan salute|вулканский салют
👌|ok hand,okay|окей,хорошо
🤌|pinched fingers|щепоть
✌️|victory hand,peace|победа,мир
🤞|crossed fingers,luck|скрещённые пальцы,удача
🤟|love you gesture|люблю тебя
🤘|sign of the horns,rock|коза,рок
🤙|call me hand|позвони
👈|pointing left|влево
👉|pointing right|вправо
👆|pointing up|вверх
👇|pointing down|вниз
☝️|index pointing up|указательный палец
👍|thumbs up,like,yes|палец вверх,нравится,да
👎|thumbs down,dislike,no|палец вниз,не нравится
✊|raised fist|кулак
👊|oncoming fist,punch|удар кулаком
👏|clapping hands,applause|аплодисменты,хлопки
🙌|raising hands,hooray|ура
🤝|handshake,deal|рукопожатие,сделка
🙏|folded hands,please,thanks,pray|пожалуйста,спасибо,молитва
✍️|writing hand|пишет
💪|flexed biceps,strong|бицепс,сила
👀|eyes,look|глаза,смотрю
🧠|brain|мозг
👶|baby|младенец
👦|boy|мальчик
👧|girl|девочка
👨|man|мужчина
👩|woman|женщина
👴|old man|дедушка
👵|old woman|бабушка
🧑‍💻|technologist,programmer,developer|программист,разработчик
👮|police officer|полицейский
🎅|santa claus|дед мороз,санта
🤷|shrug|пожимаю плечами
🤦|facepalm|рукалицо
🙋|raising hand|поднимаю руку
🏃|running|бег
💃|dancing woman,dance|танец
🐶|dog face,puppy|собака,щенок
🐱|cat face,kitten|кошка,котёнок
🐭|mouse|мышь
🐹|hamster|хомяк
🐰|rabbit|кролик,заяц
🦊|fox|лиса
🐻|bear|медведь
🐼|panda|панда
🐨|koala|коала
🐯|tiger|тигр
🦁|lion|лев
🐮|cow|корова
🐷|pig|свинья
🐸|frog|лягушка
🐵|monkey|обезьяна
🐔|chicken|курица
🐧|penguin|пингвин
🐦|bird|птица
🦆|duck|утка
🦉|owl|сова
🐺|wolf|волк
🐴|horse|лошадь
🦄|unicorn|единорог
🐝|honeybee,bee|пчела
🐛|bug|жук,гусеница
🦋|butterfly|бабочка
🐌|snail|улитка
🐞|lady beetle,ladybug|божья коровка
🐢|turtle|черепаха
🐍|snake|змея
🐙|octopus|осьминог
🦀|crab,rust|краб
🐟|fish|рыба
🐬|dolphin|дельфин
🐳|whale|кит
🦈|shark|акула
🌸|cherry blossom,flower|сакура,цветок
🌹|rose|роза
🌻|sunflower|подсолнух
🌷|tulip|тюльпан
🌱|seedling,plant|росток
🌲|evergreen tree|ёлка
🌳|deciduous tree,tree|дерево
🌴|palm tree|пальма
🌵|cactus|кактус
🍀|four leaf clover,luck|клевер,удача
🍁|maple leaf|кленовый лист
🍄|mushroom|гриб
🍎|red apple,apple|яблоко
🍐|pear|груша
🍊|tangerine,orange|мандарин,апельсин
🍋|lemon|лимон
🍌|banana|банан
🍉|watermelon|арбуз
🍇|grapes|виноград
🍓|strawberry|клубника
🍒|cherries|вишня
🍑|peach|персик
🍍|pineapple|ананас
🥝|kiwi|киви
🍅|tomato|помидор
🥑|avocado|авокадо
🥕|carrot|морковь
🌽|corn|кукуруза
🥔|potato|картошка
🍞|bread|хлеб
🧀|cheese|сыр
🥚|egg|яйцо
🍳|cooking,fried egg|яичница
🥞|pancakes|блины
🥓|bacon|бекон
🍔|hamburger,burger|гамбургер,бургер
🍟|french fries|картошка фри
🍕|pizza|пицца
🌭|hot dog|хот-дог
🌮|taco|тако
🍣|sushi|суши
🍜|steaming bowl,noodles,ramen|лапша,рамен
🍝|spaghetti,pasta|спагетти,паста
🍪|cookie|печенье
🎂|birthday cake|торт,день рождения
🍰|shortcake,cake|пирожное
🍫|chocolate|шоколад
🍬|candy|конфета
🍩|doughnut|пончик
🍿|popcorn|попкорн
☕|hot beverage,coffee,tea|кофе,чай
🍵|teacup|чай
🍺|beer|пиво
🍷|wine|вино
🥂|clinking glasses,cheers|бокалы,тост
🍾|champagne,bottle|шампанское
🥤|cup with straw|напиток
🌍|globe europe africa,earth,world|земля,мир,глобус
🌎|globe americas|земля,америка
🌏|globe asia|земля,азия
🗺️|world map|карта мира
🏔️|snow capped mountain|гора
🏖️|beach|пляж
🏠|house,home|дом
🏢|office building|офис
🏥|hospital|больница
🏫|school|школа
⛪|church|церковь
🚗|automobile,car|машина,автомобиль
🚕|taxi|такси
🚌|bus|автобус
🚲|bicycle,bike|велосипед
🚀|rocket,launch|ракета,запуск
✈️|airplane,plane|самолёт
🚢|ship|корабль
🚂|locomotive,train|поезд,паровоз
⚓|anchor|якорь
⛽|fuel pump|заправка
🚦|traffic light|светофор
⏰|alarm clock|будильник
⌛|hourglass|песочные часы
⏳|hourglass flowing|время идёт
🌙|crescent moon,moon|луна,месяц
☀️|sun|солнце
⭐|star|звезда
🌟|glowing star|сияющая звезда
☁️|cloud|облако
⛅|sun behind cloud|переменная облачность
🌧️|cloud with rain,rain|дождь
⛈️|thunderstorm|гроза
❄️|snowflake,snow|снежинка,снег
☃️|snowman|снеговик
🌈|rainbow|радуга
☂️|umbrella|зонт
⚡|high voltage,lightning|молния,напряжение
🔥|fire,lit|огонь,пожар
💧|droplet|капля
🌊|water wave,sea|волна,море
🎃|jack o lantern,halloween|тыква,хэллоуин
🎄|christmas tree|рождественская ёлка,новый год
🎆|fireworks|фейерверк
✨|sparkles|блёстки,искры
🎈|balloon|воздушный шар
🎉|party popper,tada,congratulations|хлопушка,поздравляю
🎁|wrapped gift,present|подарок
🏆|trophy,winner|кубок,победитель
🥇|first place medal,gold|золото,первое место
⚽|soccer ball,football|футбол,мяч
🏀|basketball|баскетбол
🎾|tennis|теннис
🎮|video game,gaming|видеоигра,игры
🎲|game die,dice|кубик,кости
♟️|chess pawn,chess|шахматы
🎯|direct hit,target|мишень,в точку
🎵|musical note,music|нота,музыка
🎶|musical notes|ноты
🎸|guitar|гитара
🎹|piano,keyboard|пианино
🎤|microphone,karaoke|микрофон
🎧|headphone|наушники
🎬|clapper board,movie|кино,фильм
📷|camera,photo|фотоаппарат,фото
📹|video camera|видеокамера
📺|television,tv|телевизор
💻|laptop,computer|ноутбук,компьютер
🖥️|desktop computer|компьютер,монитор
⌨️|keyboard|клавиатура
🖱️|computer mouse|компьютерная мышь
🖨️|printer|принтер
📱|mobile phone,smartphone|телефон,смартфон
☎️|telephone|телефон
🔋|battery|батарея,аккумулятор
🔌|electric plug|вилка,розетка
💡|light bulb,idea|лампочка,идея
🔦|flashlight|фонарик
📚|books|книги
📖|open book,read|книга,чтение
📝|memo,note|заметка
✏️|pencil|карандаш
🖊️|pen|ручка
📎|paperclip|скрепка
📌|pushpin,pin|кнопка,закрепить
📅|calendar,date|календарь,дата
📈|chart increasing,growth|рост,график
📉|chart decreasing|падение,график
📊|bar chart|диаграмма
📁|file folder|папка
📄|page,document|страница,документ
📦|package,box|коробка,посылка
✉️|envelope,mail|конверт,письмо
📧|e-mail|почта
📢|loudspeaker|громкоговоритель
🔔|bell,notification|колокольчик,уведомление
🔕|bell with slash,mute|без звука
🔒|locked|замок,закрыто
🔓|unlocked|открыто
🔑|key|ключ
🔨|hammer|молоток
🔧|wrench|гаечный ключ
⚙️|gear,settings|шестерёнка,настройки
🧲|magnet|магнит
💊|pill,medicine|таблетка,лекарство
💉|syringe|шприц
🧬|dna|днк
🔬|microscope|микроскоп
🔭|telescope|телескоп
💰|money bag|мешок денег,деньги
💵|dollar banknote|доллары,деньги
💶|euro banknote|евро
💳|credit card|кредитная карта
💎|gem stone,diamond|бриллиант,алмаз
🛒|shopping cart|тележка,покупки
🚩|triangular flag|флажок
🏁|chequered flag,finish|финиш
🏳️‍🌈|rainbow flag,pride|радужный флаг
✅|check mark button,done|галочка,готово
✔️|check mark|галочка
❌|cross mark,no|крестик,нет
❓|question mark|вопрос
❗|exclamation mark|восклицательный знак
⚠️|warning|предупреждение,внимание
⛔|no entry|въезд запрещён
🚫|prohibited|запрещено
♻️|recycling|переработка
🆗|ok button|ок
🆕|new button|новое
🔴|red circle|красный круг
🟢|green circle|зелёный круг
🔵|blue circle|синий круг
⬛|black square|чёрный квадрат
⬜|white square|белый квадрат
➡️|right arrow|стрелка вправо
⬅️|left arrow|стрелка влево
⬆️|up arrow|стрелка вверх
⬇️|down arrow|стрелка вниз
🔄|counterclockwise arrows,refresh|обновить
→|arrow right symbol|стрелка вправо символ
←|arrow left symbol|стрелка влево символ
↑|arrow up symbol|стрелка вверх символ
↓|arrow down symbol|стрелка вниз символ
↔|left right arrow|двойная стрелка
⇒|double arrow right,implies|следует
©|copyright|копирайт,авторское право
®|registered|зарегистрировано
™|trade mark,trademark|товарный знак
°|degree|градус
±|plus minus|плюс минус
×|multiplication sign,times|умножение
÷|division sign|деление
≈|almost equal,approximately|приблизительно
≠|not equal|не равно
≤|less than or equal|меньше или равно
≥|greater than or equal|больше или равно
∞|infinity|бесконечность
√|square root|корень
∑|sum,sigma|сумма
π|pi|пи
µ|micro|микро
Ω|ohm,omega|ом,омега
α|alpha|альфа
β|beta|бета
γ|gamma|гамма
δ|delta|дельта
λ|lambda|лямбда
€|euro sign|евро знак
£|pound sign|фунт
¥|yen sign|иена
₽|ruble sign|рубль
₴|hryvnia sign|гривна
¢|cent sign|цент
§|section sign|параграф
¶|pilcrow,paragraph|абзац
•|bullet|маркер,точка
…|ellipsis|многоточие
—|em dash|длинное тире
–|en dash|короткое тире
«|left guillemet,quote|кавычки ёлочки
»|right guillemet|кавычки ёлочки закрывающие
„|low double quote|кавычки лапки
“|left double quote|кавычка
”|right double quote|кавычка закрывающая
✓|check symbol|галочка символ
✗|ballot x|крестик символ
★|black star symbol|звезда символ
☆|white star symbol|пустая звезда
♥|heart suit|черви
♦|diamond suit|бубны
♣|club suit|трефы
♠|spade suit|пики
⌘|command key|команда
⌥|option key|опция
⏎|return symbol,enter|ввод
⌫|erase to the left,backspace|удаление
";

/// An entry of the table.
struct Entry<'a> {
    character: &'a str,
    english: &'a str,
    russian: &'a str,
}

impl<'a> Entry<'a> {
    fn of(line: &'a str) -> Option<Self> {
        let mut parts = line.split('|');
        let (character, english, russian) = (parts.next()?, parts.next()?, parts.next()?);
        (!character.is_empty()).then_some(Self {
            character,
            english,
            russian,
        })
    }

    /// The names searched in `language`.
    fn names(&self, language: Language) -> impl Iterator<Item = &'a str> {
        let russian = match language {
            Language::Russian => self.russian,
            _ => "",
        };
        { self.english.split(',') }
            .chain(russian.split(','))
            .filter(|name| !name.is_empty())
    }

    /// The name shown in `language`.
    fn name(&self, language: Language) -> &'a str {
        let first = |names: &'a str| names.split(',').next().unwrap_or_default();
        match language {
            Language::Russian if !self.russian.is_empty() => first(self.russian),
            _ => first(self.english),
        }
    }
}

/// How well one of `names` answers `query`: 0 where a name starts with
/// it, 1 where a word of a name does.
fn word_rank<'a>(names: impl Iterator<Item = &'a str>, query: &str) -> Option<u8> {
    { names.map(str::to_lowercase) }
        .filter_map(|name| {
            if name.starts_with(query) {
                Some(0)
            } else if name.split([' ', '-']).any(|word| word.starts_with(query)) {
                Some(1)
            } else {
                None
            }
        })
        .min()
}

/// The entries that answer `query` in `language`, the best first, each
/// with its character and the name it is shown with.
fn matching(query: &str, language: Language) -> Vec<(&'static str, &'static str)> {
    let mut found: Vec<(u8, &str, &str)> = { TABLE.lines().filter_map(Entry::of) }
        .filter_map(|entry| {
            let rank = word_rank(entry.names(language), query)?;
            Some((rank, entry.character, entry.name(language)))
        })
        .collect();
    found.sort_by_key(|&(rank, ..)| rank);
    { found.into_iter() }
        .map(|(_, character, name)| (character, name))
        .collect()
}

pub struct Emoji;

impl Runner for Emoji {
    fn id(&self) -> &'static str {
        "emoji"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let Some(asked) = query.lower().strip_prefix(':').map(str::trim) else {
            return Vec::new();
        };
        if asked.chars().count() < LETTERS {
            return Vec::new();
        }
        { matching(asked, query.language()).into_iter().take(SHOWN) }
            .enumerate()
            .map(|(index, (character, name))| {
                let title = format!("{character}  {name}");
                let detail = query.say("Copies the character");
                // The order of the table holds among the equally good.
                let score = SCORE.saturating_sub(index as u16);
                let copy = AnswerAction::Copy(character.to_owned());
                Answer::new(title, detail, ICON_EMOJI, score, copy)
                    .copying(character)
                    .keyed(format!("emoji:{character}"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_of_the_table_is_an_entry_of_its_own() {
        let entries: Vec<Entry> = TABLE.lines().filter_map(Entry::of).collect();
        assert_eq!(entries.len(), TABLE.lines().count());
        let mut characters: Vec<&str> = entries.iter().map(|entry| entry.character).collect();
        characters.sort_unstable();
        characters.dedup();
        assert_eq!(characters.len(), entries.len());
        assert!(entries.iter().all(|entry| !entry.english.is_empty()));
    }

    #[test]
    fn an_emoji_is_found_by_a_word_of_its_name() {
        let hearts = matching("heart", Language::English);
        // "heart eyes" starts with the word, "red heart" only has it.
        assert_eq!(hearts[0].0, "😍");
        assert!(hearts.iter().any(|&(character, _)| character == "❤️"));
        assert!(hearts.iter().any(|&(character, _)| character == "💔"));
        let thumbs = matching("thumbs", Language::English);
        assert_eq!(thumbs[0], ("👍", "thumbs up"));
        let russian = matching("кот", Language::Russian);
        assert_eq!(russian[0], ("🐱", "кошка"));
        assert!(russian.contains(&("😺", "улыбающийся кот")));
        assert!(matching("кот", Language::English).is_empty());
        let euro = matching("eur", Language::English);
        assert!(euro.iter().any(|&(character, _)| character == "€"));
    }
}

// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What a notification of Windows says in full: the toast XML an
//! application posted, as Windows keeps it in its notification database
//! (`platform/win32/wpn.rs` reads it). It holds the texts, the pictures,
//! the progress bars, the header, the input fields, the buttons, the
//! sound and what a click starts, which the notification center of
//! Windows does not give out otherwise.
//!
//! This part only reads the XML; nothing here asks Windows anything. The
//! schema is Microsoft's toast content schema (`ToastGeneric`), with the
//! older templates read as far as they have texts and pictures.

use std::collections::HashMap;

use roxmltree::{Document, Node};

/// The most buttons a toast has, as Windows draws at most.
pub const MOST_BUTTONS: usize = 5;

/// The minutes a snooze lasts where the toast offers no choice.
pub const SNOOZE_MINUTES: u32 = 5;

/// How a click on a toast, or on one of its buttons, reaches its
/// application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Activation {
    /// The application is started or brought to the front with the
    /// arguments.
    #[default]
    Foreground,
    /// The application gets the arguments without coming to the front.
    Background,
    /// The arguments are an address (a URI) that is opened.
    Protocol,
    /// Windows itself does it: `snooze` or `dismiss`.
    System,
}

impl Activation {
    fn of(word: Option<&str>) -> Self {
        match word.map(str::to_ascii_lowercase).as_deref() {
            Some("background") => Self::Background,
            Some("protocol") => Self::Protocol,
            Some("system") => Self::System,
            _ => Self::Foreground,
        }
    }
}

/// What kind of toast it is, which decides how long its banner stays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scenario {
    #[default]
    Default,
    /// Stays until it is answered.
    Reminder,
    /// Stays until it is answered, and its sound repeats.
    Alarm,
    /// Stays until it is answered, as a call that rings.
    IncomingCall,
    /// Breaks through do not disturb where Windows lets it.
    Urgent,
}

impl Scenario {
    fn of(word: Option<&str>) -> Self {
        match word.map(str::to_ascii_lowercase).as_deref() {
            Some("reminder") => Self::Reminder,
            Some("alarm") => Self::Alarm,
            Some("incomingcall") => Self::IncomingCall,
            Some("urgent") => Self::Urgent,
            _ => Self::Default,
        }
    }

    /// Whether its banner stays until it is answered.
    pub fn stays(self) -> bool {
        matches!(self, Self::Reminder | Self::Alarm | Self::IncomingCall)
    }
}

/// A line of text, with the most lines it may take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastText {
    pub text: String,
    pub max_lines: Option<u8>,
}

/// A picture of a toast, by where it is to be read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastImage {
    /// As the toast names it: `file:///`, `ms-appx:///`,
    /// `ms-appdata:///local/`, `http(s)://` or a plain path.
    pub src: String,
    /// Drawn as a circle, as a picture of a person.
    pub circle: bool,
    /// What the picture shows, in words.
    pub alt: String,
}

/// Where a picture or a sound of a toast is to be read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file, by its path.
    File(String),
    /// A file in the install folder of the package of the application,
    /// by its path inside it.
    Package(String),
    /// A file in the `LocalState` folder of the package of the
    /// application, by its path inside it.
    PackageData(String),
    /// A file on the web.
    Web(String),
    /// A sound of the scheme of Windows, by the name of its event, as
    /// `Notification.Default`.
    SoundEvent(String),
}

impl Source {
    /// Where `src` says to read from; `None` for a kind Tessera does not
    /// read, such as `ms-appdata:///roaming/`.
    pub fn of(src: &str) -> Option<Self> {
        let src = src.trim();
        let lower = src.to_ascii_lowercase();
        let after = |prefix: &str| src[prefix.len()..].trim_start_matches('/').to_owned();
        if lower.starts_with("ms-appx:///") || lower.starts_with("ms-appx://") {
            let rest = src.split_once("://").map_or("", |(_, rest)| rest);
            // `ms-appx://package/path` names a package; Tessera reads the
            // application's own one.
            let rest = match lower.starts_with("ms-appx:///") {
                true => rest.trim_start_matches('/'),
                false => rest.split_once('/').map_or("", |(_, path)| path),
            };
            return (!rest.is_empty()).then(|| Self::Package(unescaped(rest)));
        }
        if lower.starts_with("ms-appdata:///local/") {
            let rest = after("ms-appdata:///local/");
            return (!rest.is_empty()).then(|| Self::PackageData(unescaped(&rest)));
        }
        if lower.starts_with("ms-winsoundevent:") {
            let name = &src["ms-winsoundevent:".len()..];
            return (!name.is_empty()).then(|| Self::SoundEvent(name.to_owned()));
        }
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Some(Self::Web(src.to_owned()));
        }
        if lower.starts_with("file:") {
            let rest = src["file:".len()..].trim_start_matches('/');
            return (!rest.is_empty()).then(|| Self::File(unescaped(rest).replace('/', "\\")));
        }
        // A plain path of the file system: a drive letter, or a share.
        let bytes = src.as_bytes();
        let drive = bytes.len() > 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic();
        (drive || src.starts_with("\\\\")).then(|| Self::File(src.to_owned()))
    }
}

/// `text` with its `%XX` escapes turned back into the letters they stand
/// for, as the paths of addresses are written.
fn unescaped(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        if bytes[at] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(at + 1).copied().and_then(hex),
                bytes.get(at + 2).copied().and_then(hex),
            )
        {
            out.push((high * 16 + low) as u8);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How far a progress bar is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressValue {
    /// It moves without telling how far.
    Indeterminate,
    /// This many thousandths are done.
    Permille(u16),
}

impl ProgressValue {
    fn of(text: &str) -> Self {
        let text = text.trim();
        if text.eq_ignore_ascii_case("indeterminate") {
            return Self::Indeterminate;
        }
        match text.parse::<f64>() {
            Ok(share) if share.is_finite() => {
                Self::Permille((share.clamp(0., 1.) * 1000.).round() as u16)
            }
            _ => Self::Indeterminate,
        }
    }

    /// The share done, from 0 to 1; `None` while it is not told.
    pub fn share(self) -> Option<f32> {
        match self {
            Self::Indeterminate => None,
            Self::Permille(permille) => Some(f32::from(permille) / 1000.),
        }
    }
}

/// A progress bar of a toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastProgress {
    /// What the bar is of, above it.
    pub title: Option<String>,
    pub value: ProgressValue,
    /// What is going on, under it at the left, as "Downloading…".
    pub status: String,
    /// What stands under it at the right in place of the percentage.
    pub value_text: Option<String>,
}

/// The header a toast is grouped under in the notification center.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastHeader {
    pub id: String,
    pub title: String,
    pub arguments: String,
    pub activation: Activation,
}

/// What an input field of a toast takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// A line of text, with what the empty field says.
    Text { placeholder: String },
    /// One of the choices, by their identifiers and what each says.
    Selection { choices: Vec<(String, String)> },
}

/// An input field of a toast, whose value goes with the button pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastInput {
    pub id: String,
    pub kind: InputKind,
    /// The label above it.
    pub title: Option<String>,
    /// What it holds at first: a text, or the identifier of a choice.
    pub default: Option<String>,
}

/// A button of a toast, or an entry of its menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastAction {
    /// What it says.
    pub content: String,
    /// What the application is given, or what is done for `System`.
    pub arguments: String,
    pub activation: Activation,
    /// A picture it shows, as for a button of a call.
    pub image: Option<String>,
    /// The input field it stands beside, as the send button of a reply.
    pub input_id: Option<String>,
    /// Whether it is in the menu of the notification rather than a button.
    pub context_menu: bool,
}

impl ToastAction {
    /// Whether it is Windows' snooze, which Tessera does.
    pub fn is_snooze(&self) -> bool {
        self.activation == Activation::System && self.arguments.eq_ignore_ascii_case("snooze")
    }

    /// Whether it is Windows' dismiss, which Tessera does.
    pub fn is_dismiss(&self) -> bool {
        self.activation == Activation::System && self.arguments.eq_ignore_ascii_case("dismiss")
    }
}

/// The sound of a toast.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToastAudio {
    /// The sound, as the toast names it; the default sound without one.
    pub src: Option<String>,
    /// Whether it plays again and again while the banner shows.
    pub looping: bool,
    /// Whether the toast is shown without a sound.
    pub silent: bool,
}

/// A toast as its application wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toast {
    /// What a click on the toast gives its application.
    pub launch: Option<String>,
    /// How a click on the toast reaches its application.
    pub activation: Activation,
    pub scenario: Scenario,
    /// Whether its banner stays longer (`duration="long"`).
    pub long: bool,
    /// The texts, the first as the title.
    pub texts: Vec<ToastText>,
    /// The small line of where it is from, as "Via SMS".
    pub attribution: Option<String>,
    /// The picture in place of the logo of the application.
    pub logo: Option<ToastImage>,
    /// The wide picture at the top.
    pub hero: Option<ToastImage>,
    /// The pictures in the content.
    pub images: Vec<ToastImage>,
    pub progress: Vec<ToastProgress>,
    pub header: Option<ToastHeader>,
    pub inputs: Vec<ToastInput>,
    /// The buttons (at most [`MOST_BUTTONS`]) and the entries of the menu,
    /// in the order the toast gives them.
    pub actions: Vec<ToastAction>,
    pub audio: ToastAudio,
}

impl Toast {
    /// Reads the toast XML `xml`. `{name}` in the texts and the progress
    /// bars stands for `data[name]` where the application gave a value in
    /// `data`, and for nothing otherwise. `None` for what is no toast.
    pub fn parse(xml: &str, data: &HashMap<String, String>) -> Option<Self> {
        let document = Document::parse(xml.trim_start_matches('\u{feff}')).ok()?;
        let root = document.root_element();
        if !named(root, "toast") {
            return None;
        }
        let bound = |text: &str| bind(text, data);
        let mut toast = Self {
            launch: attribute(root, "launch").map(str::to_owned),
            activation: Activation::of(attribute(root, "activationType")),
            scenario: Scenario::of(attribute(root, "scenario")),
            long: attribute(root, "duration").is_some_and(|word| word.eq_ignore_ascii_case("long")),
            ..Self::default()
        };
        if let Some(binding) = binding_of(root) {
            toast.read_content(binding, &bound);
        }
        for child in root.children().filter(Node::is_element) {
            if named(child, "header") {
                toast.header = Some(ToastHeader {
                    id: attribute(child, "id").unwrap_or_default().to_owned(),
                    title: bound(attribute(child, "title").unwrap_or_default()),
                    arguments: attribute(child, "arguments").unwrap_or_default().to_owned(),
                    activation: Activation::of(attribute(child, "activationType")),
                });
            } else if named(child, "actions") {
                toast.read_actions(child, &bound);
            } else if named(child, "audio") {
                toast.audio = ToastAudio {
                    src: attribute(child, "src").map(str::to_owned),
                    looping: truth(attribute(child, "loop")),
                    silent: truth(attribute(child, "silent")),
                };
            }
        }
        Some(toast)
    }

    /// Takes in the texts, pictures and progress bars of a binding, and of
    /// its groups and subgroups, which are drawn one after the other here.
    fn read_content(&mut self, parent: Node, bound: &impl Fn(&str) -> String) {
        for child in parent.children().filter(Node::is_element) {
            if named(child, "text") {
                let text = bound(&element_text(child));
                if attribute(child, "placement")
                    .is_some_and(|p| p.eq_ignore_ascii_case("attribution"))
                {
                    self.attribution = Some(text).filter(|text| !text.is_empty());
                } else {
                    let max_lines = attribute(child, "hint-maxLines").and_then(|n| n.parse().ok());
                    self.texts.push(ToastText { text, max_lines });
                }
            } else if named(child, "image") {
                let Some(src) = attribute(child, "src").filter(|src| !src.trim().is_empty()) else {
                    continue;
                };
                let image = ToastImage {
                    src: src.to_owned(),
                    circle: attribute(child, "hint-crop")
                        .is_some_and(|crop| crop.eq_ignore_ascii_case("circle")),
                    alt: attribute(child, "alt").unwrap_or_default().to_owned(),
                };
                match attribute(child, "placement")
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("applogooverride") => self.logo = Some(image),
                    Some("hero") => self.hero = Some(image),
                    _ => self.images.push(image),
                }
            } else if named(child, "progress") {
                let text = |name: &str| attribute(child, name).map(bound);
                self.progress.push(ToastProgress {
                    title: text("title").filter(|title| !title.is_empty()),
                    value: ProgressValue::of(&text("value").unwrap_or_default()),
                    status: text("status").unwrap_or_default(),
                    value_text: text("valueStringOverride").filter(|text| !text.is_empty()),
                });
            } else if named(child, "group") || named(child, "subgroup") {
                self.read_content(child, bound);
            }
        }
    }

    /// Takes in the input fields and the buttons of `<actions>`.
    fn read_actions(&mut self, actions: Node, bound: &impl Fn(&str) -> String) {
        for child in actions.children().filter(Node::is_element) {
            if named(child, "input") {
                let Some(id) = attribute(child, "id").filter(|id| !id.is_empty()) else {
                    continue;
                };
                let kind = match attribute(child, "type")
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("selection") => InputKind::Selection {
                        choices: { child.children().filter(|node| named(*node, "selection")) }
                            .filter_map(|choice| {
                                let id = attribute(choice, "id")?.to_owned();
                                let content = bound(attribute(choice, "content").unwrap_or(&id));
                                Some((id, content))
                            })
                            .collect(),
                    },
                    _ => InputKind::Text {
                        placeholder: bound(
                            attribute(child, "placeHolderContent").unwrap_or_default(),
                        ),
                    },
                };
                self.inputs.push(ToastInput {
                    id: id.to_owned(),
                    kind,
                    title: attribute(child, "title")
                        .map(bound)
                        .filter(|t| !t.is_empty()),
                    default: attribute(child, "defaultInput").map(str::to_owned),
                });
            } else if named(child, "action") {
                let context_menu = attribute(child, "placement")
                    .is_some_and(|place| place.eq_ignore_ascii_case("contextMenu"));
                let buttons = self.actions.iter().filter(|a| !a.context_menu).count();
                if !context_menu && buttons >= MOST_BUTTONS {
                    continue;
                }
                self.actions.push(ToastAction {
                    content: bound(attribute(child, "content").unwrap_or_default()),
                    arguments: attribute(child, "arguments").unwrap_or_default().to_owned(),
                    activation: Activation::of(attribute(child, "activationType")),
                    image: attribute(child, "imageUri").map(str::to_owned),
                    input_id: attribute(child, "hint-inputId").map(str::to_owned),
                    context_menu,
                });
            }
        }
        // The buttons Windows makes itself where the toast asks for them.
        let system = attribute(actions, "hint-systemCommands");
        if system.is_some_and(|commands| commands.eq_ignore_ascii_case("SnoozeAndDismiss")) {
            for (content, arguments) in [("", "snooze"), ("", "dismiss")] {
                self.actions.push(ToastAction {
                    content: content.to_owned(),
                    arguments: arguments.to_owned(),
                    activation: Activation::System,
                    image: None,
                    input_id: None,
                    context_menu: false,
                });
            }
        }
    }

    /// The input field `id`.
    pub fn input(&self, id: &str) -> Option<&ToastInput> {
        self.inputs.iter().find(|input| input.id == id)
    }

    /// The minutes a snooze of this toast lasts: the choice `chosen` of
    /// the field the snooze button stands beside, where it is a number of
    /// minutes, or else [`SNOOZE_MINUTES`].
    pub fn snooze_minutes(&self, chosen: Option<&str>) -> u32 {
        chosen
            .and_then(|minutes| minutes.trim().parse::<u32>().ok())
            .filter(|&minutes| minutes > 0)
            .unwrap_or(SNOOZE_MINUTES)
    }
}

/// The binding to read: the `ToastGeneric` one, or else the first.
fn binding_of<'a>(root: Node<'a, 'a>) -> Option<Node<'a, 'a>> {
    let visual = root.children().find(|node| named(*node, "visual"))?;
    let bindings: Vec<Node> = visual
        .children()
        .filter(|node| named(*node, "binding"))
        .collect();
    let generic = bindings.iter().copied().find(|binding| {
        attribute(*binding, "template").is_some_and(|t| t.eq_ignore_ascii_case("ToastGeneric"))
    });
    generic.or_else(|| bindings.first().copied())
}

/// Whether `node` is the element `name`, in any case.
fn named(node: Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name().eq_ignore_ascii_case(name)
}

/// The attribute `name` of `node`, its name in any case.
fn attribute<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name().eq_ignore_ascii_case(name))
        .map(|attribute| attribute.value())
}

/// The text inside `node`.
fn element_text(node: Node) -> String {
    let parts: Vec<&str> = node.descendants().filter_map(|node| node.text()).collect();
    parts.concat().trim().to_owned()
}

/// Whether an attribute says yes.
fn truth(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
}

/// `text` with every `{name}` replaced by `data[name]`, or by nothing
/// where `data` has no such value. A `{` without a closing `}` stays.
fn bind(text: &str, data: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                if let Some(value) = data.get(name) {
                    out.push_str(value);
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The values of the controls of a toast when one of its buttons is
/// pressed, by the identifiers of its input fields: what a text field
/// holds, or the identifier of the choice chosen.
pub type InputValues = Vec<(String, String)>;

/// The values `typed` (by field) gives, with the default of each field
/// the user did not touch.
pub fn input_values(toast: &Toast, typed: &HashMap<String, String>) -> InputValues {
    { toast.inputs.iter() }
        .filter_map(|input| {
            let value = typed
                .get(&input.id)
                .cloned()
                .or_else(|| input.default.clone());
            let value = value.or_else(|| match &input.kind {
                InputKind::Selection { choices } => choices.first().map(|(id, _)| id.clone()),
                InputKind::Text { .. } => None,
            })?;
            Some((input.id.clone(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> Toast {
        Toast::parse(xml, &HashMap::new()).expect("a toast")
    }

    impl Toast {
        /// The buttons, at most [`MOST_BUTTONS`], in their order.
        pub fn buttons(&self) -> impl Iterator<Item = &ToastAction> {
            self.actions.iter().filter(|action| !action.context_menu)
        }

        /// The entries of the menu of the notification.
        pub fn menu_entries(&self) -> impl Iterator<Item = &ToastAction> {
            self.actions.iter().filter(|action| action.context_menu)
        }

        /// The first text as the title and the others as the body, as the
        /// notification center says them.
        pub fn title_and_body(&self) -> (String, String) {
            let mut texts = self
                .texts
                .iter()
                .map(|text| text.text.trim())
                .filter(|t| !t.is_empty());
            let title = texts.next().unwrap_or_default().to_owned();
            let body = texts.collect::<Vec<_>>().join("\n");
            (title, body)
        }
    }

    const CHAT: &str = r#"<toast launch="action=openThread&amp;threadId=92187">
  <visual>
    <binding template="ToastGeneric">
      <text hint-maxLines="1">Andrew sent you a picture</text>
      <text>Check this out, The Enchantments in Washington!</text>
      <image src="https://example.org/andrew.jpg" placement="appLogoOverride" hint-crop="circle"/>
      <image src="ms-appx:///Assets/Enchantments.jpg"/>
      <text placement="attribution">Via SMS</text>
    </binding>
  </visual>
  <actions>
    <input id="textBox" type="text" placeHolderContent="Type a reply"/>
    <action content="Send" arguments="action=reply&amp;threadId=92187" activationType="background" imageUri="Assets/Reply.png" hint-inputId="textBox"/>
    <action content="Like" arguments="action=like&amp;threadId=92187" activationType="background"/>
    <action content="Mute this thread" arguments="action=mute" placement="contextMenu"/>
  </actions>
  <audio src="ms-winsoundevent:Notification.IM"/>
</toast>"#;

    #[test]
    fn a_chat_message_has_a_reply_field_and_buttons() {
        let toast = parse(CHAT);
        assert_eq!(
            toast.launch.as_deref(),
            Some("action=openThread&threadId=92187")
        );
        assert_eq!(toast.activation, Activation::Foreground);
        assert_eq!(
            toast.title_and_body(),
            (
                "Andrew sent you a picture".to_owned(),
                "Check this out, The Enchantments in Washington!".to_owned()
            )
        );
        assert_eq!(toast.texts[0].max_lines, Some(1));
        assert_eq!(toast.attribution.as_deref(), Some("Via SMS"));
        let logo = toast.logo.as_ref().unwrap();
        assert!(logo.circle);
        assert_eq!(
            Source::of(&logo.src),
            Some(Source::Web("https://example.org/andrew.jpg".to_owned()))
        );
        assert_eq!(toast.images.len(), 1);
        assert_eq!(
            Source::of(&toast.images[0].src),
            Some(Source::Package("Assets/Enchantments.jpg".to_owned()))
        );
        let input = toast.input("textBox").unwrap();
        assert_eq!(
            input.kind,
            InputKind::Text {
                placeholder: "Type a reply".to_owned()
            }
        );
        let buttons: Vec<_> = toast.buttons().collect();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].input_id.as_deref(), Some("textBox"));
        assert_eq!(buttons[0].activation, Activation::Background);
        let menu: Vec<_> = toast.menu_entries().map(|a| a.content.as_str()).collect();
        assert_eq!(menu, ["Mute this thread"]);
        assert_eq!(
            Source::of(toast.audio.src.as_deref().unwrap()),
            Some(Source::SoundEvent("Notification.IM".to_owned()))
        );
        let typed = HashMap::from([("textBox".to_owned(), "Lovely!".to_owned())]);
        assert_eq!(
            input_values(&toast, &typed),
            [("textBox".to_owned(), "Lovely!".to_owned())]
        );
        assert!(input_values(&toast, &HashMap::new()).is_empty());
    }

    #[test]
    fn a_mail_has_a_header_and_texts_of_groups() {
        let toast = parse(
            r#"<toast launch="mail:1234" activationType="protocol">
  <header id="inbox" title="Inbox (3)" arguments="folder=inbox"/>
  <visual>
    <binding template="ToastGeneric">
      <text>Ada Lovelace</text>
      <group>
        <subgroup><text hint-style="caption">Re: the engine</text><text>Tomorrow at ten?</text></subgroup>
        <subgroup><image src="C:\Mail\ada.png"/></subgroup>
      </group>
    </binding>
  </visual>
  <audio silent="true"/>
</toast>"#,
        );
        assert_eq!(toast.activation, Activation::Protocol);
        let header = toast.header.as_ref().unwrap();
        assert_eq!(
            (header.id.as_str(), header.title.as_str()),
            ("inbox", "Inbox (3)")
        );
        assert_eq!(toast.texts.len(), 3);
        assert_eq!(toast.title_and_body().1, "Re: the engine\nTomorrow at ten?");
        assert_eq!(
            Source::of(&toast.images[0].src),
            Some(Source::File(r"C:\Mail\ada.png".to_owned()))
        );
        assert!(toast.audio.silent && toast.audio.src.is_none());
    }

    #[test]
    fn a_download_has_a_progress_bar_with_bound_values() {
        let xml = r#"<toast>
  <visual>
    <binding template="ToastGeneric">
      <text>Downloading your weekly playlist...</text>
      <progress title="{progressTitle}" value="{progressValue}" valueStringOverride="{progressValueString}" status="{progressStatus}"/>
      <progress value="indeterminate" status="Waiting"/>
    </binding>
  </visual>
</toast>"#;
        let data = HashMap::from([
            ("progressTitle".to_owned(), "Weekly playlist".to_owned()),
            ("progressValue".to_owned(), "0.6".to_owned()),
            ("progressValueString".to_owned(), "15/26 songs".to_owned()),
            ("progressStatus".to_owned(), "Downloading...".to_owned()),
        ]);
        let toast = Toast::parse(xml, &data).unwrap();
        assert_eq!(
            toast.progress[0],
            ToastProgress {
                title: Some("Weekly playlist".to_owned()),
                value: ProgressValue::Permille(600),
                status: "Downloading...".to_owned(),
                value_text: Some("15/26 songs".to_owned()),
            }
        );
        assert_eq!(toast.progress[1].value, ProgressValue::Indeterminate);
        assert_eq!(toast.progress[0].value.share(), Some(0.6));
        // Without the data, a bound value is no value.
        let unbound = parse(xml);
        assert_eq!(unbound.progress[0].value, ProgressValue::Indeterminate);
        assert_eq!(unbound.progress[0].title, None);
    }

    #[test]
    fn a_reminder_stays_and_snoozes_for_the_chosen_minutes() {
        let toast = parse(
            r#"<toast scenario="reminder" duration="long">
  <visual><binding template="ToastGeneric"><text>Dentist</text><text>At 15:00</text></binding></visual>
  <actions>
    <input id="snoozeTime" type="selection" defaultInput="15">
      <selection id="5" content="5 minutes"/>
      <selection id="15" content="15 minutes"/>
      <selection id="60" content="1 hour"/>
    </input>
    <action activationType="system" arguments="snooze" hint-inputId="snoozeTime" content=""/>
    <action activationType="system" arguments="dismiss" content=""/>
  </actions>
</toast>"#,
        );
        assert!(toast.scenario.stays() && toast.long);
        let buttons: Vec<_> = toast.buttons().collect();
        assert!(buttons[0].is_snooze() && buttons[1].is_dismiss());
        let values = input_values(&toast, &HashMap::new());
        assert_eq!(values, [("snoozeTime".to_owned(), "15".to_owned())]);
        assert_eq!(toast.snooze_minutes(Some("15")), 15);
        assert_eq!(toast.snooze_minutes(Some("soon")), SNOOZE_MINUTES);
        let InputKind::Selection { choices } = &toast.inputs[0].kind else {
            panic!("a selection");
        };
        assert_eq!(choices.len(), 3);
        // The system buttons Windows adds where asked.
        let asked = parse(
            r#"<toast><visual><binding template="ToastGeneric"><text>Alarm</text></binding></visual><actions hint-systemCommands="SnoozeAndDismiss"/></toast>"#,
        );
        assert_eq!(
            asked
                .buttons()
                .filter(|b| b.activation == Activation::System)
                .count(),
            2
        );
    }

    #[test]
    fn more_than_five_buttons_are_left_out_and_what_is_no_toast_is_none() {
        let actions: String = (0..7)
            .map(|n| format!(r#"<action content="B{n}" arguments="{n}"/>"#))
            .collect();
        let toast = parse(&format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>Hi</text></binding></visual><actions>{actions}</actions></toast>"
        ));
        assert_eq!(toast.buttons().count(), MOST_BUTTONS);
        assert!(Toast::parse("<tile/>", &HashMap::new()).is_none());
        assert!(Toast::parse("not xml", &HashMap::new()).is_none());
        // An older template is read as far as it has texts.
        let old = parse(
            r#"<toast><visual><binding template="ToastText02"><text id="1">Title</text><text id="2">Text</text></binding></visual></toast>"#,
        );
        assert_eq!(
            old.title_and_body(),
            ("Title".to_owned(), "Text".to_owned())
        );
    }

    #[test]
    fn sources_are_told_apart() {
        assert_eq!(
            Source::of("file:///C:/Users/Ada/Pictures/a%20b.png"),
            Some(Source::File(r"C:\Users\Ada\Pictures\a b.png".to_owned()))
        );
        assert_eq!(
            Source::of("ms-appdata:///local/cache/x.png"),
            Some(Source::PackageData("cache/x.png".to_owned()))
        );
        assert_eq!(
            Source::of("ms-appx://Contoso.App/Assets/x.png"),
            Some(Source::Package("Assets/x.png".to_owned()))
        );
        assert_eq!(Source::of("ms-appdata:///roaming/x.png"), None);
        assert_eq!(Source::of("Assets/relative.png"), None);
        assert_eq!(
            Source::of(r"\\server\share\x.png"),
            Some(Source::File(r"\\server\share\x.png".to_owned()))
        );
    }

    #[test]
    fn bindings_take_their_values_or_nothing() {
        let data = HashMap::from([("a".to_owned(), "1".to_owned())]);
        assert_eq!(bind("x{a}y{b}z", &data), "x1yz");
        assert_eq!(bind("open {brace", &data), "open {brace");
        assert_eq!(bind("", &data), "");
    }
}

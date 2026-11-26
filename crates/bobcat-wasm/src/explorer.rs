//! `NativeModules.ExplorerModule`, the host module the Lynx Explorer homepage
//! (`packages/explorer-homepage`, `packages/explorer-lib`) calls, for the
//! GitHub Pages demo. Compiled into the browser build with the
//! `explorer-module` feature, and into this crate's native tests.
//!
//! The module runs inside `LynxView::pump` on the Render Worker, which has no
//! `document`, no `localStorage` and no template loader of the page's. So
//! [`ExplorerModule::invoke`] reads and checks each call's arguments here and
//! hands the page, through an [`ExplorerPage`], only what the page alone can
//! do: open a template, store a preference, and show a status text. The
//! methods that only log, log on the Render Worker's console.
//!
//! No method answers its caller: none of the Explorer's calls passes a
//! function argument, and `readFromLocalStorage` and `getSettingInfo` exist so
//! that the homepage's feature checks find them. A method called from
//! JavaScript returns `undefined`, which the homepage reads as no stored
//! value and no settings.

use bobcat_core::{ModuleCall, NativeModule};
use serde_json::Value;

pub(crate) const EXPLORER_MODULE_NAME: &str = "ExplorerModule";

const METHODS: [&str; 8] = [
    "openSchema",
    "openScan",
    "setThreadMode",
    "openDevtoolSwitchPage",
    "saveThemePreferences",
    "saveToLocalStorage",
    "readFromLocalStorage",
    "getSettingInfo",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExplorerNotice {
    EmptySchema,
    NoScanner,
}

impl ExplorerNotice {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::EmptySchema => "emptySchema",
            Self::NoScanner => "noScanner",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogLevel {
    Info,
    Warn,
}

pub(crate) trait ExplorerPage {
    fn open_schema(&self, schema: String);
    fn store_preference(&self, key: String, value: String);
    fn notice(&self, notice: ExplorerNotice);
    fn log(&self, level: LogLevel, message: &str);
}

#[derive(Debug)]
pub(crate) struct ExplorerModule<P> {
    page: P,
}

impl<P: ExplorerPage> ExplorerModule<P> {
    pub(crate) const fn new(page: P) -> Self {
        Self { page }
    }
}

impl<P: ExplorerPage> NativeModule for ExplorerModule<P> {
    fn name(&self) -> &str {
        EXPLORER_MODULE_NAME
    }

    fn methods(&self) -> Vec<String> {
        METHODS.iter().map(|&method| method.to_owned()).collect()
    }

    fn invoke(&self, call: ModuleCall) {
        let arguments = match serde_json::from_str::<Vec<Value>>(&call.arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                self.page.log(
                    LogLevel::Warn,
                    &format!(
                        "ExplorerModule.{}: the arguments are not a JSON array: {error}",
                        call.method
                    ),
                );
                return;
            }
        };
        match call.method.as_str() {
            "openSchema" => match arguments.first() {
                Some(Value::String(schema)) if !is_blank(schema) => {
                    self.page.open_schema(schema.clone());
                }
                _ => self.page.notice(ExplorerNotice::EmptySchema),
            },
            "openScan" => {
                self.page.notice(ExplorerNotice::NoScanner);
                self.page.log(
                    LogLevel::Warn,
                    "ExplorerModule.openScan: this demo has no camera scanner",
                );
            }
            "setThreadMode" => self.page.log(
                LogLevel::Info,
                "ExplorerModule.setThreadMode: Bobcat has one thread strategy",
            ),
            "openDevtoolSwitchPage" => self.page.log(
                LogLevel::Info,
                "ExplorerModule.openDevtoolSwitchPage: this demo ships no DevTool switches",
            ),
            "saveThemePreferences" | "saveToLocalStorage" => self
                .page
                .store_preference(js_string(arguments.first()), js_string(arguments.get(1))),
            _ => {}
        }
    }
}

fn is_blank(text: &str) -> bool {
    text.chars().all(|character| {
        (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
    })
}

fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                item => js_string(Some(item)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
        Some(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Seen {
        Open(String),
        Store(String, String),
        Notice(ExplorerNotice),
        Log(LogLevel, String),
    }

    #[derive(Debug, Default)]
    struct Recorder(RefCell<Vec<Seen>>);

    impl ExplorerPage for &Recorder {
        fn open_schema(&self, schema: String) {
            self.0.borrow_mut().push(Seen::Open(schema));
        }

        fn store_preference(&self, key: String, value: String) {
            self.0.borrow_mut().push(Seen::Store(key, value));
        }

        fn notice(&self, notice: ExplorerNotice) {
            self.0.borrow_mut().push(Seen::Notice(notice));
        }

        fn log(&self, level: LogLevel, message: &str) {
            self.0
                .borrow_mut()
                .push(Seen::Log(level, message.to_owned()));
        }
    }

    fn invoke(method: &str, arguments: &str) -> Vec<Seen> {
        let recorder = Recorder::default();
        ExplorerModule::new(&recorder).invoke(ModuleCall {
            method: method.to_owned(),
            arguments: arguments.to_owned(),
            callbacks: Vec::new(),
        });
        recorder.0.into_inner()
    }

    #[test]
    fn declares_the_explorer_methods() {
        let recorder = Recorder::default();
        let module = ExplorerModule::new(&recorder);
        assert_eq!(module.name(), "ExplorerModule");
        assert_eq!(
            module.methods(),
            [
                "openSchema",
                "openScan",
                "setThreadMode",
                "openDevtoolSwitchPage",
                "saveThemePreferences",
                "saveToLocalStorage",
                "readFromLocalStorage",
                "getSettingInfo",
            ]
        );
    }

    #[test]
    fn open_schema_passes_the_string_as_the_card_wrote_it() {
        assert_eq!(
            invoke(
                "openSchema",
                r#"["file://lynx?local://showcase/menu/main.lynx.bundle?x=1"]"#
            ),
            [Seen::Open(
                "file://lynx?local://showcase/menu/main.lynx.bundle?x=1".to_owned()
            )]
        );
        assert_eq!(
            invoke("openSchema", r#"[" card.web.bundle "]"#),
            [Seen::Open(" card.web.bundle ".to_owned())]
        );
    }

    #[test]
    fn open_schema_without_a_url_is_the_empty_schema_notice() {
        for arguments in ["[]", "[null]", "[42]", r#"[""]"#, "[\" \\n\\ufeff\"]"] {
            assert_eq!(
                invoke("openSchema", arguments),
                [Seen::Notice(ExplorerNotice::EmptySchema)],
                "{arguments}"
            );
        }
    }

    #[test]
    fn open_scan_is_the_no_scanner_notice_and_a_warning() {
        assert_eq!(
            invoke("openScan", "[]"),
            [
                Seen::Notice(ExplorerNotice::NoScanner),
                Seen::Log(
                    LogLevel::Warn,
                    "ExplorerModule.openScan: this demo has no camera scanner".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn methods_without_a_browser_answer_only_log() {
        assert_eq!(
            invoke("setThreadMode", "[1]"),
            [Seen::Log(
                LogLevel::Info,
                "ExplorerModule.setThreadMode: Bobcat has one thread strategy".to_owned()
            )]
        );
        assert_eq!(
            invoke("openDevtoolSwitchPage", "[]"),
            [Seen::Log(
                LogLevel::Info,
                "ExplorerModule.openDevtoolSwitchPage: this demo ships no DevTool switches"
                    .to_owned()
            )]
        );
        assert_eq!(invoke("readFromLocalStorage", r#"["recent"]"#), []);
        assert_eq!(invoke("getSettingInfo", "[]"), []);
    }

    #[test]
    fn preference_writes_carry_the_string_of_each_argument() {
        assert_eq!(
            invoke("saveThemePreferences", r#"["preferredTheme","Dark"]"#),
            [Seen::Store("preferredTheme".to_owned(), "Dark".to_owned())]
        );
        assert_eq!(
            invoke("saveToLocalStorage", r#"["recent","[]"]"#),
            [Seen::Store("recent".to_owned(), "[]".to_owned())]
        );
        let stored = |arguments| match invoke("saveToLocalStorage", arguments).pop() {
            Some(Seen::Store(key, value)) => (key, value),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            stored("[]"),
            ("undefined".to_owned(), "undefined".to_owned())
        );
        assert_eq!(stored("[1, true]"), ("1".to_owned(), "true".to_owned()));
        assert_eq!(stored("[null, 2.5]"), ("null".to_owned(), "2.5".to_owned()));
        assert_eq!(
            stored(r#"[["a", null, [1, 2]], {"b": 1}]"#),
            ("a,,1,2".to_owned(), "[object Object]".to_owned())
        );
    }

    #[test]
    fn notices_carry_the_protocol_names() {
        assert_eq!(ExplorerNotice::EmptySchema.name(), "emptySchema");
        assert_eq!(ExplorerNotice::NoScanner.name(), "noScanner");
    }

    #[test]
    fn arguments_that_are_not_an_array_are_a_warning() {
        let seen = invoke("saveToLocalStorage", "{}");
        assert!(
            matches!(
                seen.as_slice(),
                [Seen::Log(LogLevel::Warn, message)]
                    if message.starts_with("ExplorerModule.saveToLocalStorage: the arguments are not a JSON array")
            ),
            "{seen:?}"
        );
    }
}

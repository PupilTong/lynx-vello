//! Interactive console command parsing and input.

use std::io::{BufRead, IsTerminal, Write};
use std::num::NonZeroU32;
use std::ops::Range;
use std::path::PathBuf;

use bobcat_core::editing::TextInputState;

pub(crate) const COMMAND_HELP: &str = "\
Commands:
  continue | c             resume the frame clock
  pause | interrupt | p    pause the frame clock
  frame | f                render one frame
  screenshot [PATH]        write the current frame (default: bobcat-screenshot.png)
  set vsync FPS            set the headless frame rate (1..1000)
  show vsync               print the current headless frame rate
  tap X Y                  tap with a finger at viewport CSS px X,Y
  key KEY [TEXT]           press and release KEY (a W3C key value), typing TEXT
  compose TEXT [START END] set the input-method composition, selection in UTF-16 units
  commit TEXT              commit TEXT through the input method
  show text-input          print the focused editing host's text input state
  help | ?                 show this help
  quit | q                 exit";

#[derive(Debug)]
pub(crate) enum Command {
    Continue,
    Pause,
    Frame,
    Screenshot(PathBuf),
    SetVsync(NonZeroU32),
    ShowVsync,
    /// A finger's tap — down and up — at a viewport CSS px position.
    Tap(f32, f32),
    /// A key pressed and released: its W3C `key` value and the text it
    /// typed, which the `keydown` carries.
    Key {
        key: String,
        text: Option<String>,
    },
    /// An input-method composition, its selection in UTF-16 units relative
    /// to the start of the text.
    Compose {
        text: String,
        selection: Range<usize>,
    },
    /// An input-method commit.
    Commit(String),
    ShowTextInput,
    Help,
    Quit,
    Invalid(String),
}

pub(crate) fn parse(line: &str) -> Option<Command> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let (name, arguments) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(name, arguments)| (name, arguments.trim()));
    match name {
        "continue" | "c" | "run" | "r" => Some(no_arguments(arguments, Command::Continue, name)),
        "pause" | "interrupt" | "p" => Some(no_arguments(arguments, Command::Pause, name)),
        "frame" | "f" => Some(no_arguments(arguments, Command::Frame, name)),
        "screenshot" | "shot" => Some(parse_screenshot(arguments)),
        "set" => Some(parse_set(arguments)),
        "show" => Some(parse_show(arguments)),
        "tap" => Some(parse_tap(arguments)),
        "key" => Some(parse_key(arguments)),
        "compose" => Some(parse_compose(arguments)),
        "commit" => Some(parse_commit(arguments)),
        "help" | "h" | "?" => Some(no_arguments(arguments, Command::Help, name)),
        "quit" | "q" | "exit" => Some(no_arguments(arguments, Command::Quit, name)),
        _ => Some(Command::Invalid(format!(
            "unknown command `{name}`; enter `help` for the command list"
        ))),
    }
}

fn no_arguments(arguments: &str, command: Command, name: &str) -> Command {
    if arguments.is_empty() {
        command
    } else {
        Command::Invalid(format!("`{name}` does not take arguments"))
    }
}

fn parse_screenshot(arguments: &str) -> Command {
    if arguments.is_empty() {
        return Command::Screenshot(PathBuf::from("bobcat-screenshot.png"));
    }

    let path = if arguments.len() >= 2
        && ((arguments.starts_with('"') && arguments.ends_with('"'))
            || (arguments.starts_with('\'') && arguments.ends_with('\'')))
    {
        &arguments[1..arguments.len() - 1]
    } else {
        arguments
    };
    if path.is_empty() {
        Command::Invalid("`screenshot` needs a non-empty output path".to_owned())
    } else {
        Command::Screenshot(PathBuf::from(path))
    }
}

fn parse_set(arguments: &str) -> Command {
    let mut words = arguments.split_whitespace();
    let setting = words.next();
    let value = words.next();
    if words.next().is_some() || setting != Some("vsync") || value.is_none() {
        return Command::Invalid(
            "usage: set vsync FPS (where FPS is from 1 through 1000)".to_owned(),
        );
    }
    let value = value.expect("value was checked");
    let Some(rate) = value
        .parse::<u32>()
        .ok()
        .and_then(NonZeroU32::new)
        .filter(|rate| rate.get() <= 1_000)
    else {
        return Command::Invalid(format!(
            "vsync FPS must be an integer from 1 through 1000, got `{value}`"
        ));
    };
    Command::SetVsync(rate)
}

fn parse_show(arguments: &str) -> Command {
    match arguments {
        "vsync" => Command::ShowVsync,
        "text-input" => Command::ShowTextInput,
        _ => Command::Invalid("usage: show vsync | show text-input".to_owned()),
    }
}

/// The words of a command's arguments: split at white space, except inside
/// a pair of double quotes, which a word may use to hold spaces (`" "`) or
/// be empty (`""`). `None` for an unclosed quote.
fn words(arguments: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut rest = arguments.trim_start();
    while !rest.is_empty() {
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            words.push(quoted[..end].to_owned());
            rest = quoted[end + 1..].trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            words.push(rest[..end].to_owned());
            rest = rest[end..].trim_start();
        }
    }
    Some(words)
}

fn parse_tap(arguments: &str) -> Command {
    let usage = || Command::Invalid("usage: tap X Y (viewport CSS px)".to_owned());
    let mut numbers = arguments.split_whitespace().map(str::parse::<f32>);
    match (numbers.next(), numbers.next(), numbers.next()) {
        (Some(Ok(x)), Some(Ok(y)), None) if x.is_finite() && y.is_finite() => Command::Tap(x, y),
        _ => usage(),
    }
}

fn parse_key(arguments: &str) -> Command {
    match words(arguments).as_deref() {
        Some([key]) if !key.is_empty() => Command::Key {
            key: key.clone(),
            text: None,
        },
        Some([key, text]) if !key.is_empty() => Command::Key {
            key: key.clone(),
            text: Some(text.clone()),
        },
        _ => Command::Invalid("usage: key KEY [TEXT] (quote a space as \" \")".to_owned()),
    }
}

fn parse_compose(arguments: &str) -> Command {
    let usage = || Command::Invalid("usage: compose TEXT [START END]".to_owned());
    match words(arguments).as_deref() {
        Some([text]) => {
            let end = text.encode_utf16().count();
            Command::Compose {
                text: text.clone(),
                selection: end..end,
            }
        }
        Some([text, start, end]) => match (start.parse::<usize>(), end.parse::<usize>()) {
            (Ok(start), Ok(end)) if start <= end => Command::Compose {
                text: text.clone(),
                selection: start..end,
            },
            _ => usage(),
        },
        _ => usage(),
    }
}

fn parse_commit(arguments: &str) -> Command {
    match words(arguments).as_deref() {
        Some([text]) => Command::Commit(text.clone()),
        _ => Command::Invalid("usage: commit TEXT".to_owned()),
    }
}

/// One line describing a [`TextInputState`], as `show text-input` prints
/// it: the value and the UTF-16 selection and composition, and the caret in
/// viewport CSS px.
pub(crate) fn describe_text_input(state: Option<&TextInputState>) -> String {
    let Some(state) = state.filter(|state| state.host.is_some()) else {
        return "text-input: none".to_owned();
    };
    let composition = state.composition.as_ref().map_or_else(
        || "none".to_owned(),
        |range| format!("{}..{}", range.start, range.end),
    );
    format!(
        "text-input: value={:?} selection={}..{} composition={} caret={},{} {}x{}",
        state.value,
        state.selection.start,
        state.selection.end,
        composition,
        state.caret.origin.x,
        state.caret.origin.y,
        state.caret.size.width,
        state.caret.size.height,
    )
}

#[derive(Debug)]
pub(crate) struct Console {
    interactive: bool,
}

impl Console {
    pub(crate) fn start(
        send: impl Fn(Command) -> bool + Send + 'static,
    ) -> Result<Self, std::io::Error> {
        let interactive = std::io::stdin().is_terminal();
        std::thread::Builder::new()
            .name("bobcat-console".to_owned())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut input = stdin.lock();
                let mut line = String::new();
                loop {
                    line.clear();
                    match input.read_line(&mut line) {
                        Ok(0) => {
                            let _ = send(Command::Quit);
                            return;
                        }
                        Ok(_) => {
                            if let Some(command) = parse(&line)
                                && !send(command)
                            {
                                return;
                            }
                        }
                        Err(error) => {
                            if !send(Command::Invalid(format!(
                                "could not read the command stream: {error}"
                            ))) {
                                return;
                            }
                            let _ = send(Command::Quit);
                            return;
                        }
                    }
                }
            })
            .map(|_| Self { interactive })
    }

    pub(crate) fn prompt(&self) {
        if self.interactive {
            print!("(bobcat) ");
            let _ = std::io::stdout().flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, parse};

    #[test]
    fn screenshot_keeps_spaces_in_paths() {
        let Some(Command::Screenshot(path)) = parse("screenshot captures/frame one.png") else {
            panic!("expected screenshot command");
        };
        assert_eq!(path, std::path::Path::new("captures/frame one.png"));
    }

    #[test]
    fn parses_vsync_changes() {
        let Some(Command::SetVsync(rate)) = parse("set vsync 144") else {
            panic!("expected set-vsync command");
        };
        assert_eq!(rate.get(), 144);
        assert!(matches!(parse("set vsync 0"), Some(Command::Invalid(_))));
    }

    #[test]
    fn parses_the_editing_commands() {
        let Some(Command::Tap(x, y)) = parse("tap 20 12.5") else {
            panic!("expected tap command");
        };
        assert_eq!(
            (x.to_bits(), y.to_bits()),
            (20f32.to_bits(), 12.5f32.to_bits())
        );
        assert!(matches!(parse("tap 20"), Some(Command::Invalid(_))));
        assert!(matches!(
            parse("key a a"),
            Some(Command::Key { key, text: Some(text) }) if key == "a" && text == "a"
        ));
        assert!(matches!(
            parse("key Enter"),
            Some(Command::Key { key, text: None }) if key == "Enter"
        ));
        assert!(matches!(
            parse(r#"key " " " ""#),
            Some(Command::Key { key, text: Some(text) }) if key == " " && text == " "
        ));
        assert!(matches!(
            parse("compose にほ"),
            Some(Command::Compose { text, selection }) if text == "にほ" && selection == (2..2)
        ));
        assert!(matches!(
            parse("compose x 0 1"),
            Some(Command::Compose { text, selection }) if text == "x" && selection == (0..1)
        ));
        assert!(matches!(parse("compose x 1 0"), Some(Command::Invalid(_))));
        assert!(matches!(
            parse(r#"commit """#),
            Some(Command::Commit(text)) if text.is_empty()
        ));
        assert!(matches!(parse(r#"commit "a"#), Some(Command::Invalid(_))));
        assert!(matches!(
            parse("show text-input"),
            Some(Command::ShowTextInput)
        ));
    }

    #[test]
    fn empty_lines_do_nothing() {
        assert!(parse(" \t ").is_none());
    }
}

use std::io::{BufRead, IsTerminal, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;

pub(crate) const COMMAND_HELP: &str = "\
Commands:
  continue | c             resume the frame clock
  pause | interrupt | p    pause the frame clock
  frame | f                render one frame
  screenshot [PATH]        write the current frame (default: bobcat-screenshot.png)
  set vsync FPS            set the headless frame rate (1..1000)
  show vsync               print the current headless frame rate
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
    if arguments == "vsync" {
        Command::ShowVsync
    } else {
        Command::Invalid("usage: show vsync".to_owned())
    }
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
    fn empty_lines_do_nothing() {
        assert!(parse(" \t ").is_none());
    }
}

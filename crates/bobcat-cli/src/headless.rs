use std::path::Path;
use std::sync::mpsc;

use bobcat_core::renderer::HeadlessRenderer;

use crate::CliError;
use crate::args::Options;
use crate::command::{COMMAND_HELP, Command, Console};
use crate::page::Program;
use crate::screenshot::save_screenshot;

pub(crate) fn run(program: Program, options: &Options) -> Result<(), CliError> {
    let runtime = program.boot(
        options.viewport_width,
        options.viewport_height,
        options.device_pixel_ratio,
    )?;
    let mut renderer = HeadlessRenderer::new(runtime, options.vsync_hz)?;
    let (sender, receiver) = mpsc::channel();
    let console =
        Console::start(move |command| sender.send(command).is_ok()).map_err(CliError::Console)?;
    println!(
        "bobcat: headless renderer running at {} Hz; enter `help` for commands",
        renderer.vsync_hz().get()
    );
    console.prompt();

    loop {
        let Some(command) = renderer.wait(&receiver)? else {
            return Ok(());
        };
        match command {
            Command::Continue => {
                renderer.resume();
                println!("Continuing at {} Hz.", renderer.vsync_hz().get());
            }
            Command::Pause => {
                renderer.pause();
                println!("Frame clock paused.");
            }
            Command::Frame => {
                renderer.render_one_frame()?;
                println!("Rendered one frame.");
            }
            Command::Screenshot(path) => {
                // A screenshot failure must not tear down the session: report
                // it at the prompt like any other bad command and keep going.
                if let Err(error) = capture(&mut renderer, &path) {
                    eprintln!("bobcat: {error}");
                }
            }
            Command::SetVsync(rate) => {
                renderer.set_vsync_hz(rate);
                println!("Headless vsync is now {} Hz.", rate.get());
            }
            Command::ShowVsync => {
                println!("Headless vsync is {} Hz.", renderer.vsync_hz().get());
            }
            Command::Help => println!("{COMMAND_HELP}"),
            Command::Quit => return Ok(()),
            Command::Invalid(message) => eprintln!("bobcat: {message}"),
        }
        console.prompt();
    }
}

fn capture(renderer: &mut HeadlessRenderer, path: &Path) -> Result<(), CliError> {
    let frame = renderer.capture()?;
    save_screenshot(path, frame.size(), frame.pixels())
}

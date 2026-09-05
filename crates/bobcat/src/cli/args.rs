//! Command-line argument parsing and validation.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::num::NonZeroU32;

use url::Url;

use crate::cli::CliError;

pub(crate) const DEFAULT_VIEWPORT_WIDTH: f32 = 393.0;
pub(crate) const DEFAULT_VIEWPORT_HEIGHT: f32 = 727.0;
pub(crate) const DEFAULT_VSYNC_HZ: NonZeroU32 =
    NonZeroU32::new(60).expect("the default frame rate is non-zero");
pub(crate) const DEFAULT_DEVICE_PIXEL_RATIO: f32 = 1.0;
const MAX_VSYNC_HZ: u32 = 1_000;

#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub(crate) input: Url,
    pub(crate) headless: bool,
    pub(crate) vsync_hz: NonZeroU32,
    pub(crate) viewport_width: f32,
    pub(crate) viewport_height: f32,
    pub(crate) device_pixel_ratio: f32,
}

#[derive(Debug)]
pub(crate) enum Invocation {
    Run(Options),
    Help,
    Version,
}

pub(crate) fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Invocation, CliError> {
    let mut arguments = arguments.into_iter().collect::<VecDeque<_>>();
    let mut input = None;
    let mut headless = false;
    let mut vsync_hz = DEFAULT_VSYNC_HZ;
    let mut vsync_set = false;
    let mut viewport = (DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT);
    let mut viewport_set = false;
    let mut device_pixel_ratio = DEFAULT_DEVICE_PIXEL_RATIO;
    let mut dpr_set = false;

    while let Some(argument) = arguments.pop_front() {
        let Some(argument_text) = argument.to_str() else {
            return Err(CliError::arguments(
                "option names and input URLs must be valid UTF-8",
            ));
        };

        match argument_text {
            "-h" | "--help" => return Ok(Invocation::Help),
            "-V" | "--version" => return Ok(Invocation::Version),
            "--headless" => {
                if headless {
                    return Err(CliError::arguments("`--headless` was provided twice"));
                }
                headless = true;
            }
            "-i" | "--input" => {
                if input.is_some() {
                    return Err(CliError::arguments("the input URL was provided twice"));
                }
                let value = next_utf8(&mut arguments, argument_text)?;
                input = Some(parse_input_url(&value)?);
            }
            "--vsync" => {
                if vsync_set {
                    return Err(CliError::arguments("`--vsync` was provided twice"));
                }
                let value = next_utf8(&mut arguments, "--vsync")?;
                vsync_hz = parse_vsync(&value)?;
                vsync_set = true;
            }
            "--viewport" => {
                if viewport_set {
                    return Err(CliError::arguments("`--viewport` was provided twice"));
                }
                let value = next_utf8(&mut arguments, "--viewport")?;
                viewport = parse_viewport(&value)?;
                viewport_set = true;
            }
            "--dpr" => {
                if dpr_set {
                    return Err(CliError::arguments("`--dpr` was provided twice"));
                }
                let value = next_utf8(&mut arguments, "--dpr")?;
                device_pixel_ratio = parse_dpr(&value)?;
                dpr_set = true;
            }
            _ if argument_text.starts_with("--input=") => {
                if input.is_some() {
                    return Err(CliError::arguments("the input URL was provided twice"));
                }
                input = Some(parse_input_url(&argument_text["--input=".len()..])?);
            }
            _ if argument_text.starts_with("--vsync=") => {
                if vsync_set {
                    return Err(CliError::arguments("`--vsync` was provided twice"));
                }
                vsync_hz = parse_vsync(&argument_text["--vsync=".len()..])?;
                vsync_set = true;
            }
            _ if argument_text.starts_with("--viewport=") => {
                if viewport_set {
                    return Err(CliError::arguments("`--viewport` was provided twice"));
                }
                viewport = parse_viewport(&argument_text["--viewport=".len()..])?;
                viewport_set = true;
            }
            _ if argument_text.starts_with("--dpr=") => {
                if dpr_set {
                    return Err(CliError::arguments("`--dpr` was provided twice"));
                }
                device_pixel_ratio = parse_dpr(&argument_text["--dpr=".len()..])?;
                dpr_set = true;
            }
            _ => {
                return Err(CliError::arguments(format!(
                    "unrecognized argument `{argument_text}`"
                )));
            }
        }
    }

    finish(
        input,
        headless,
        vsync_hz,
        vsync_set,
        viewport,
        device_pixel_ratio,
        dpr_set,
    )
}

fn finish(
    input: Option<Url>,
    headless: bool,
    vsync_hz: NonZeroU32,
    vsync_set: bool,
    viewport: (f32, f32),
    device_pixel_ratio: f32,
    dpr_set: bool,
) -> Result<Invocation, CliError> {
    let input = input.ok_or_else(|| CliError::arguments("missing `-i <file:///...>`"))?;
    if vsync_set && !headless {
        return Err(CliError::arguments(
            "`--vsync` is available only with `--headless`",
        ));
    }
    if dpr_set && !headless {
        return Err(CliError::arguments(
            "`--dpr` is available only with `--headless`; headed mode uses the window's scale \
             factor",
        ));
    }
    Ok(Invocation::Run(Options {
        input,
        headless,
        vsync_hz,
        viewport_width: viewport.0,
        viewport_height: viewport.1,
        device_pixel_ratio,
    }))
}

fn next_utf8(arguments: &mut VecDeque<OsString>, option: &str) -> Result<String, CliError> {
    let value = arguments
        .pop_front()
        .ok_or_else(|| CliError::arguments(format!("`{option}` needs a value")))?;
    value
        .into_string()
        .map_err(|_| CliError::arguments(format!("`{option}` needs a valid UTF-8 value")))
}

fn parse_input_url(value: &str) -> Result<Url, CliError> {
    let url = Url::parse(value)
        .map_err(|error| CliError::arguments(format!("invalid input URL `{value}`: {error}")))?;
    if url.scheme() != "file"
        || url.host().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.to_file_path().is_err()
    {
        return Err(CliError::arguments(format!(
            "unsupported input URL `{value}`; only local `file:///` URLs are supported"
        )));
    }
    Ok(url)
}

fn parse_vsync(value: &str) -> Result<NonZeroU32, CliError> {
    let parsed = value
        .parse::<u32>()
        .ok()
        .and_then(NonZeroU32::new)
        .filter(|value| value.get() <= MAX_VSYNC_HZ)
        .ok_or_else(|| {
            CliError::arguments(format!(
                "`--vsync` must be an integer from 1 through {MAX_VSYNC_HZ}, got `{value}`"
            ))
        })?;
    Ok(parsed)
}

fn parse_viewport(value: &str) -> Result<(f32, f32), CliError> {
    let (width, height) = value
        .split_once(['x', 'X', '\u{d7}'])
        .ok_or_else(|| CliError::arguments("`--viewport` must have the form WIDTHxHEIGHT"))?;
    let width = parse_dimension(width, "width")?;
    let height = parse_dimension(height, "height")?;
    Ok((width, height))
}

fn parse_dpr(value: &str) -> Result<f32, CliError> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| CliError::arguments(format!("`--dpr` is not a number: `{value}`")))?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(CliError::arguments(format!(
            "`--dpr` must be finite and greater than zero, got `{value}`"
        )));
    }
    Ok(parsed)
}

fn parse_dimension(value: &str, name: &str) -> Result<f32, CliError> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| CliError::arguments(format!("viewport {name} is not a number: `{value}`")))?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(CliError::arguments(format!(
            "viewport {name} must be finite and greater than zero, got `{value}`"
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{Invocation, parse};

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_a_headless_file_invocation() {
        let Invocation::Run(options) = parse(args(&[
            "-i",
            "file:///tmp/card.web.bundle",
            "--headless",
            "--vsync",
            "120",
            "--viewport=800x600",
        ]))
        .unwrap() else {
            panic!("expected a runnable invocation");
        };

        assert!(options.headless);
        assert_eq!(options.vsync_hz.get(), 120);
        assert_eq!(options.viewport_width.to_bits(), 800.0_f32.to_bits());
        assert_eq!(options.viewport_height.to_bits(), 600.0_f32.to_bits());
    }

    #[test]
    fn accepts_equivalent_local_file_url_spellings() {
        for url in [
            "FILE:///tmp/card.web.bundle",
            "file:/tmp/card.web.bundle",
            "file://localhost/tmp/card.web.bundle",
        ] {
            assert!(
                matches!(
                    parse(args(&["-i", url, "--headless"])),
                    Ok(Invocation::Run(_))
                ),
                "{url}"
            );
        }
    }

    #[test]
    fn parses_a_headless_device_pixel_ratio() {
        let Invocation::Run(options) = parse(args(&[
            "-i",
            "file:///tmp/card.web.bundle",
            "--headless",
            "--dpr",
            "2",
        ]))
        .unwrap() else {
            panic!("expected a runnable invocation");
        };
        assert_eq!(options.device_pixel_ratio.to_bits(), 2.0_f32.to_bits());
    }

    #[test]
    fn headed_dpr_is_not_silently_ignored() {
        let error = parse(args(&["-i", "file:///tmp/card.web.bundle", "--dpr", "2"])).unwrap_err();
        assert!(error.to_string().contains("--headless"));
    }

    #[test]
    fn rejects_a_duplicate_viewport() {
        let error = parse(args(&[
            "-i",
            "file:///tmp/card.web.bundle",
            "--headless",
            "--viewport",
            "1x1",
            "--viewport=800x600",
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("--viewport"));
    }

    #[test]
    fn rejects_non_file_and_remote_file_urls() {
        for url in [
            "https://example.com/card.web.bundle",
            "file://server/card.web.bundle",
            "relative.web.bundle",
        ] {
            let error = parse(args(&["-i", url, "--headless"])).unwrap_err();
            assert!(error.to_string().contains("URL"), "{url}: {error}");
        }
    }

    #[test]
    fn headed_vsync_is_not_silently_ignored() {
        let error = parse(args(&[
            "-i",
            "file:///tmp/card.web.bundle",
            "--vsync",
            "30",
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("--headless"));
    }

    #[test]
    fn rejects_the_removed_screenshot_startup_option() {
        let error = parse(args(&[
            "-i",
            "file:///tmp/card.web.bundle",
            "--headless",
            "--screenshot",
            "capture.png",
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("unrecognized argument"));
    }
}

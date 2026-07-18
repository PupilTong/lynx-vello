use std::error::Error;
use std::ffi::OsString;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os();
    let program = arguments
        .next()
        .unwrap_or_else(|| OsString::from("convert"));
    let input = arguments.next().map(PathBuf::from);
    let output = arguments.next().map(PathBuf::from);
    if input.is_none() || output.is_none() || arguments.next().is_some() {
        return Err(format!(
            "usage: {} <input.lynx.bundle> <output.web.bundle>",
            PathBuf::from(program).display()
        )
        .into());
    }
    let input = input.expect("checked above");
    let output = output.expect("checked above");
    let native = std::fs::read(input)?;
    let web = lynx_template_converter::convert(&native)?;
    std::fs::write(output, web)?;
    Ok(())
}

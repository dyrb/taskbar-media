#![windows_subsystem = "windows"]

fn main() {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args().skip(1);
        let duration = match args.next().as_deref() {
            None => {
                taskbar_media_band::host::enable(taskbar_media_band::host::Registration::automatic()?)?;
                return Ok(());
            }
            Some("--window") => None,
            Some("--seconds") => {
                let seconds: u64 = args.next().ok_or("--seconds needs a value")?.parse()?;
                if !(1..=86400).contains(&seconds) {
                    return Err("seconds must be 1..86400".into());
                }
                Some(std::time::Duration::from_secs(seconds))
            }
            _ => return Err("use taskbar-media-ui [--window | --seconds N]".into()),
        };
        if args.next().is_some() {
            return Err("unexpected argument".into());
        }
        Ok(taskbar_media_band::ui::run_for(duration)?)
    })();
    if let Err(error) = result {
        taskbar_media_band::ui::report_startup_error(error.as_ref());
        std::process::exit(2);
    }
}

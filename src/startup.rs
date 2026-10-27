use anyhow::Result;
use std::path::Path;

pub fn install(exe: &Path, config: Option<&Path>, mode: &str) -> Result<String> {
    platform::install(exe, config, mode)
}

pub fn uninstall() -> Result<String> {
    platform::uninstall()
}

#[cfg(any(windows, target_os = "macos"))]
const NAME: &str = "monctl";

// The entry is keyed by NAME on both platforms (registry value name on Windows,
// plist basename on macOS), so uninstall works with any app_path.
#[cfg(any(windows, target_os = "macos"))]
fn launcher(app_path: &str, args: &[String]) -> Result<auto_launch::AutoLaunch> {
    use anyhow::Context;
    auto_launch::AutoLaunchBuilder::new()
        .set_app_name(NAME)
        .set_app_path(app_path)
        // macOS: a LaunchAgent, because AppleScript login items cannot carry
        // our --config/mode args
        .set_use_launch_agent(true)
        .set_args(args)
        .build()
        .context("building the startup entry")
}

#[cfg(windows)]
mod platform {
    use anyhow::{Context, Result};
    use std::path::Path;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};

    use super::NAME;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    // Task Manager records each entry's enabled state here; auto-launch's
    // disable leaves it behind, and it outlives the Run value, so a row
    // lingers in the Startup tab unless it is cleared too.
    const APPROVED_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    pub fn install(exe: &Path, config: Option<&Path>, mode: &str) -> Result<String> {
        super::launcher(&quoted(exe), &args(config, mode))?
            .enable()
            .context("writing startup value")?;
        Ok(format!(r"HKCU\{RUN_KEY}\{NAME}"))
    }

    pub fn uninstall() -> Result<String> {
        let exe = std::env::current_exe().context("finding the monctl executable")?;
        match super::launcher(&quoted(&exe), &[])?.disable() {
            Ok(()) => {}
            Err(auto_launch::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::Error::new(e).context("deleting startup value")),
        }
        if let Ok(approved) =
            RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(APPROVED_KEY, KEY_WRITE)
        {
            let _ = approved.delete_value(NAME);
        }
        Ok(format!(r"HKCU\{RUN_KEY}\{NAME}"))
    }

    // auto-launch writes the Run value as app_path and args joined with plain
    // spaces, so each path must carry its own quotes to survive spaces.
    fn quoted(path: &Path) -> String {
        format!("\"{}\"", path.display())
    }

    fn args(config: Option<&Path>, mode: &str) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(c) = config {
            v.push("--config".to_string());
            v.push(quoted(c));
        }
        v.push(mode.to_string());
        v
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn run_value_quotes_paths() {
            // joined the same way auto-launch builds the Run value
            let value = format!(
                "{} {}",
                quoted(Path::new(r"C:\a b\monctl.exe")),
                args(Some(Path::new(r"C:\u\config.toml")), "watch").join(" ")
            );
            assert_eq!(
                value,
                r#""C:\a b\monctl.exe" --config "C:\u\config.toml" watch"#
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    use super::NAME;

    // Where auto-launch puts its launch agent; reported so main can print the
    // `launchctl load` hint.
    fn plist_path() -> Result<PathBuf> {
        let home = std::env::home_dir().context("could not find the home directory")?;
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{NAME}.plist")))
    }

    pub fn install(exe: &Path, config: Option<&Path>, mode: &str) -> Result<String> {
        let mut args = Vec::new();
        if let Some(c) = config {
            args.push("--config".to_string());
            args.push(c.display().to_string());
        }
        args.push(mode.to_string());
        super::launcher(&exe.display().to_string(), &args)?
            .enable()
            .context("writing the launch agent")?;
        Ok(plist_path()?.display().to_string())
    }

    pub fn uninstall() -> Result<String> {
        let exe = std::env::current_exe().context("finding the monctl executable")?;
        super::launcher(&exe.display().to_string(), &[])?
            .disable()
            .context("removing the launch agent")?;
        Ok(plist_path()?.display().to_string())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use anyhow::{Result, bail};
    use std::path::Path;

    pub fn install(_exe: &Path, _config: Option<&Path>, _mode: &str) -> Result<String> {
        bail!("`monctl startup` only works on Windows and macOS");
    }

    pub fn uninstall() -> Result<String> {
        bail!("`monctl startup` only works on Windows and macOS");
    }
}

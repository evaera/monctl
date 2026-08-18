use anyhow::Result;

/// Wake this machine's display so the monitor, when it switches here, finds a
/// live signal instead of a sleeping output and doesn't bounce back.
#[cfg(windows)]
pub fn wake() -> Result<()> {
    use windows_sys::Win32::System::Power::{
        ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
    };

    fn mouse_move(dx: i32, dy: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    // Resetting the idle timer alone won't relight a panel that is already
    // off; a synthetic one-pixel mouse nudge (and back) counts as user input
    // and does.
    unsafe { SetThreadExecutionState(ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED) };
    let jiggle = [mouse_move(0, 1), mouse_move(0, -1)];
    let sent = unsafe {
        SendInput(
            jiggle.len() as u32,
            jiggle.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != jiggle.len() as u32 {
        anyhow::bail!("SendInput delivered {sent}/{} events", jiggle.len());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn wake() -> Result<()> {
    use anyhow::Context;
    // declares user activity to the power manager, which lights the display
    let status = std::process::Command::new("caffeinate")
        .args(["-u", "-t", "1"])
        .status()
        .context("running caffeinate -u")?;
    if !status.success() {
        anyhow::bail!("caffeinate exited with {status}");
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn wake() -> Result<()> {
    anyhow::bail!("screen wake is only implemented on Windows and macOS")
}

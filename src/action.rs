use crate::config::{Action, Config};
use crate::monitor;
use anyhow::{Context, Result};
use std::time::{Duration, Instant};

pub fn run(config: &Config, model: Option<&str>, actions: &[Action]) -> Result<()> {
    run_while(config, model, actions, || true).map(|_| ())
}

/// Stop an enabling sequence when its hold ends. The caller runs `off` to
/// restore any actions already applied. Disabling sequences are unconditional.
pub fn run_while(
    config: &Config,
    model: Option<&str>,
    actions: &[Action],
    mut keep_running: impl FnMut() -> bool,
) -> Result<bool> {
    for (index, action) in actions.iter().enumerate() {
        if !keep_running() {
            return Ok(false);
        }
        let completed = execute(config, model, action, &mut keep_running)
            .with_context(|| format!("action {} of {}", index + 1, actions.len()))?;
        if !completed {
            return Ok(false);
        }
    }
    Ok(true)
}

fn execute(
    config: &Config,
    model: Option<&str>,
    action: &Action,
    keep_running: &mut dyn FnMut() -> bool,
) -> Result<bool> {
    match action {
        Action::Msi(step) => {
            return crate::vendor::msi::set_while(&step.msi.command, step.msi.value, keep_running);
        }
        Action::WindowsHdr(step) => crate::hdr::set(model, step.windows_hdr)?,
        Action::Wait(step) => {
            let start = Instant::now();
            let duration = Duration::from_millis(step.wait_ms);
            while start.elapsed() < duration {
                if !keep_running() {
                    return Ok(false);
                }
                std::thread::sleep(
                    (duration.saturating_sub(start.elapsed())).min(Duration::from_millis(10)),
                );
            }
        }
        Action::Input(step) => {
            let value = config.resolve_input(&step.input)?;
            let mut display = monitor::open(model)?;
            if !keep_running() {
                return Ok(false);
            }
            display.set_feature(0x60, value)?;
        }
        Action::Ddc(step) => {
            let mut display = monitor::open(model)?;
            if !keep_running() {
                return Ok(false);
            }
            display.set_feature(step.ddc.feature, step.ddc.value)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputStep, WaitStep};

    #[test]
    fn canceled_hold_skips_remaining_actions() {
        let actions = [
            Action::Wait(WaitStep { wait_ms: 1000 }),
            Action::Input(InputStep {
                input: "invalid-input".into(),
            }),
        ];
        let mut checks = 0;
        let completed = run_while(&Config::default(), None, &actions, || {
            checks += 1;
            checks < 3
        })
        .unwrap();
        assert!(!completed);
        assert_eq!(checks, 3);
    }

    #[test]
    fn inactive_hold_does_not_touch_hardware() {
        let actions = [Action::Input(InputStep {
            input: "invalid-input".into(),
        })];
        assert!(!run_while(&Config::default(), None, &actions, || false).unwrap());
        assert!(run(&Config::default(), None, &actions).is_err());
    }
}

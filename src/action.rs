use crate::config::{Action, Config};
use crate::monitor;
use anyhow::{Context, Result};
use std::time::Duration;

pub fn run(config: &Config, model: Option<&str>, actions: &[Action]) -> Result<()> {
    for (index, action) in actions.iter().enumerate() {
        execute(config, model, action)
            .with_context(|| format!("action {} of {}", index + 1, actions.len()))?;
    }
    Ok(())
}

fn execute(config: &Config, model: Option<&str>, action: &Action) -> Result<()> {
    match action {
        Action::Msi(step) => crate::vendor::msi::set(&step.msi.command, step.msi.value),
        Action::WindowsHdr(step) => crate::hdr::set(model, step.windows_hdr),
        Action::Wait(step) => {
            std::thread::sleep(Duration::from_millis(step.wait_ms));
            Ok(())
        }
        Action::Input(step) => {
            let value = config.resolve_input(&step.input)?;
            let mut display = monitor::open(model)?;
            display.set_feature(0x60, value)
        }
        Action::Ddc(step) => {
            let mut display = monitor::open(model)?;
            display.set_feature(step.ddc.feature, step.ddc.value)
        }
    }
}

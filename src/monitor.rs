use anyhow::{Context, Result, bail};
use ddc_hi::{Ddc, Display, DisplayInfo};
use std::time::Duration;

const INPUT_VCP: u8 = 0x60; // Input Source

pub trait InputSource {
    fn get_input(&mut self) -> Result<u16>;
    fn set_input(&mut self, value: u16) -> Result<()>;
}

pub struct Monitor {
    display: Display,
    pub label: String,
}

impl InputSource for Monitor {
    fn get_input(&mut self) -> Result<u16> {
        self.display
            .handle
            .get_vcp_feature(INPUT_VCP)
            .map(|v| v.value())
            .with_context(|| format!("reading input from {}", self.label))
    }

    fn set_input(&mut self, value: u16) -> Result<()> {
        // QD-OLED can drop DDC after sleep; retry once
        if self
            .display
            .handle
            .set_vcp_feature(INPUT_VCP, value)
            .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
        self.display
            .handle
            .set_vcp_feature(INPUT_VCP, value)
            .with_context(|| format!("setting input {value} on {}", self.label))
    }
}

impl Monitor {
    pub fn set_feature(&mut self, feature: u8, value: u16) -> Result<()> {
        if self.display.handle.set_vcp_feature(feature, value).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
        self.display
            .handle
            .set_vcp_feature(feature, value)
            .with_context(|| format!("setting VCP 0x{feature:02X} to {value} on {}", self.label))
    }
}

pub fn open(model: Option<&str>) -> Result<Monitor> {
    let display = select(model)?;
    let label = describe(&display.info);
    Ok(Monitor { display, label })
}

fn select(model: Option<&str>) -> Result<Display> {
    let mut displays = Display::enumerate();
    if displays.is_empty() {
        bail!("no DDC/CI displays found. check the cable and that DDC/CI is on in the OSD.");
    }

    let Some(model) = model else {
        return Ok(displays.swap_remove(0));
    };
    let needle = model.to_lowercase();

    if let Some(i) = matching(&displays, &needle) {
        return Ok(displays.swap_remove(i));
    }
    // some backends fill model_name only after this (slow)
    for d in &mut displays {
        if d.info.model_name.is_none() {
            let _ = d.update_capabilities();
        }
    }
    if let Some(i) = matching(&displays, &needle) {
        return Ok(displays.swap_remove(i));
    }
    if displays.len() == 1 {
        eprintln!(
            "monkey: nothing matched '{model}', using the only display ({}).",
            describe(&displays[0].info)
        );
        return Ok(displays.swap_remove(0));
    }
    bail!("no display matched '{model}'. run `monkey list` and set `model` in monkey.toml.");
}

fn matching(displays: &[Display], needle: &str) -> Option<usize> {
    displays.iter().position(|d| {
        d.info
            .model_name
            .as_deref()
            .is_some_and(|m| m.to_lowercase().contains(needle))
    })
}

fn describe(info: &DisplayInfo) -> String {
    info.model_name
        .clone()
        .unwrap_or_else(|| format!("{}:{}", info.backend, info.id))
}

pub struct DisplayReport {
    pub label: String,
    pub backend: String,
    pub id: String,
    pub serial: Option<String>,
    pub input: Result<u16, String>,
}

pub fn report_all() -> Result<Vec<DisplayReport>> {
    let mut displays = Display::enumerate();
    if displays.is_empty() {
        bail!("no DDC/CI displays found.");
    }
    Ok(displays
        .iter_mut()
        .map(|d| DisplayReport {
            label: describe(&d.info),
            backend: d.info.backend.to_string(),
            id: d.info.id.clone(),
            serial: d
                .info
                .serial_number
                .clone()
                .or_else(|| d.info.serial.map(|s| s.to_string())),
            input: d
                .handle
                .get_vcp_feature(INPUT_VCP)
                .map(|v| v.value())
                .map_err(|e| format!("{e:#}")),
        })
        .collect())
}

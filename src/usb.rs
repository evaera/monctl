use crate::config::Config;
use crate::monitor::{self, InputSource};
use anyhow::{Context, Result, bail};
use nusb::MaybeFuture;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(500);

/// Print connected USB devices, for finding a device's VID:PID.
pub fn list() -> Result<()> {
    for d in nusb::list_devices().wait().context("listing USB devices")? {
        let name = d
            .product_string()
            .or(d.manufacturer_string())
            .unwrap_or("?");
        println!("{:04x}:{:04x}  {name}", d.vendor_id(), d.product_id());
    }
    Ok(())
}

/// Watch the configured USB device and switch the monitor on connect/disconnect.
pub fn watch(config: &Config, model: Option<&str>) -> Result<()> {
    watch_with_pause(config, model, Arc::new(AtomicBool::new(false)))
}

pub fn watch_with_pause(
    config: &Config,
    model: Option<&str>,
    paused: Arc<AtomicBool>,
) -> Result<()> {
    let usb = config.usb.as_ref().context(
        "no [usb] section in config.toml (device = \"3297:1977\", on_connect/on_disconnect)",
    )?;
    let (vid, pid) = usb.ids()?;
    let on_connect = usb
        .on_connect
        .as_deref()
        .map(|s| config.resolve_input(s))
        .transpose()?;
    let on_disconnect = usb
        .on_disconnect
        .as_deref()
        .map(|s| config.resolve_input(s))
        .transpose()?;
    if on_connect.is_none()
        && on_disconnect.is_none()
        && !usb.wake_on_connect
        && !usb.wake_on_disconnect
    {
        bail!("[usb] needs on_connect/on_disconnect and/or wake_on_connect/wake_on_disconnect");
    }
    let wake_settle = Duration::from_millis(usb.wake_settle_ms);

    let mut present = device_present(vid, pid)?;
    let mut was_paused = paused.load(Ordering::SeqCst);
    let model = model.map(str::to_owned);
    println!(
        "watching USB {} ({}), monitor follows on connect/disconnect. ctrl+c to quit.",
        usb.device,
        if present { "connected" } else { "disconnected" }
    );

    loop {
        std::thread::sleep(POLL);
        let now = match device_present(vid, pid) {
            Ok(p) => p,
            Err(_) => continue, // transient enumeration hiccup; try again next tick
        };
        let is_paused = paused.load(Ordering::SeqCst);
        if is_paused {
            if !was_paused {
                println!("USB input switching paused by an active mode");
            }
            was_paused = true;
            present = now;
            continue;
        }
        let resumed = was_paused;
        if resumed {
            println!("USB input switching resumed; synchronizing current device state");
            was_paused = false;
        }
        if now == present && !resumed {
            continue;
        }
        present = now;
        let event = if resumed {
            "resume"
        } else if now {
            "connected"
        } else {
            "disconnected"
        };
        // wake first, so the input switch (ours or the other machine's) finds
        // this output already live
        let wake = if now {
            usb.wake_on_connect
        } else {
            usb.wake_on_disconnect
        };
        if wake {
            match crate::wake::wake() {
                Ok(()) => std::thread::sleep(wake_settle),
                Err(e) => eprintln!("USB {event}: screen wake failed: {e:#}"),
            }
        }
        let target = if now { on_connect } else { on_disconnect };
        let Some(value) = target else { continue };
        match switch(model.as_deref(), value) {
            Ok(label) => println!("USB {event} -> input {value} on {label}"),
            Err(e) => eprintln!("USB {event}: switch to {value} failed: {e:#}"),
        }
    }
}

fn switch(model: Option<&str>, value: u16) -> Result<String> {
    let mut mon = monitor::open(model)?;
    mon.set_input(value)?;
    Ok(mon.label)
}

fn device_present(vid: u16, pid: u16) -> Result<bool> {
    Ok(nusb::list_devices()
        .wait()
        .context("listing USB devices")?
        .any(|d| d.vendor_id() == vid && d.product_id() == pid))
}

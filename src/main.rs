// Build on the Windows GUI subsystem so a logon-launched daemon never allocates a
// console window. `attach_parent_console` reattaches to a terminal for CLI use.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod action;
mod cli;
mod config;
mod hdr;
#[cfg(windows)]
mod hold_hotkey;
mod hotkey;
mod monitor;
mod startup;
mod usb;
mod vendor;
mod wake;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{Cli, Command, ModeState, MsiAction, VendorCommand};
use config::Config;
use monitor::InputSource;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    // GUI-subsystem builds have no console of their own; if we were launched from a
    // terminal, borrow its console so output (and clap's help/errors) is visible.
    #[cfg(windows)]
    attach_parent_console();

    // clap handles --help/--version and usage errors itself
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("monctl: {e:#}");
            ExitCode::FAILURE
        }
    }
}

// Reattach stdio to the launching terminal's console, if there is one. When
// started from the registry at logon there is no parent console and this is a
// harmless no-op, leaving the process windowless.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

fn run(cli: Cli) -> Result<()> {
    let Cli {
        model,
        config,
        command,
    } = cli;

    let cfg = config::load(config.as_deref())?;
    let model = model.as_deref().or(cfg.model.as_deref());

    match command {
        Command::Read => cmd_read(&cfg, model),
        Command::List => cmd_list(&cfg),
        Command::Set { input } => cmd_set(&cfg, model, &input),
        Command::Toggle { input_a, input_b } => cmd_toggle(&cfg, model, &input_a, &input_b),
        Command::Mode { name, state } => cmd_mode(&cfg, model, &name, state),
        Command::Hdr { state } => cmd_hdr(model, state),
        Command::Vendor { vendor } => match vendor {
            VendorCommand::Msi { action } => cmd_msi(action),
        },
        Command::Watch => hotkey::watch(&cfg, model),
        Command::Usb => usb::list(),
        Command::Startup { remove } => cmd_startup(&cfg, config.as_deref(), remove),
        Command::Switch(args) => match args.as_slice() {
            [input] => cmd_set(&cfg, model, input),
            _ => bail!("expected one input, e.g. `monctl usbc`"),
        },
    }
}

fn cmd_hdr(model: Option<&str>, state: Option<ModeState>) -> Result<()> {
    if let Some(state) = state {
        hdr::set(model, state == ModeState::On)?;
    }
    let enabled = hdr::get(model)?;
    println!("Windows HDR: {}", if enabled { "on" } else { "off" });
    Ok(())
}

fn cmd_msi(action: MsiAction) -> Result<()> {
    use vendor::msi;
    match action {
        MsiAction::List => {
            for setting in msi::SETTINGS {
                if setting.writable {
                    println!("{}", setting.name);
                } else {
                    println!("{}  (read-only)", setting.name);
                }
            }
            Ok(())
        }
        MsiAction::Probe => msi::probe(),
        MsiAction::Get { setting } => {
            let value = msi::get(&setting)?;
            println!("{setting}: {value}");
            Ok(())
        }
        MsiAction::Set { setting, value } => {
            msi::set(&setting, value)?;
            // sets are not acknowledged, so show the read-back when we can
            match msi::get(&setting) {
                Ok(now) => println!("{setting} set to {value} (reads back {now})"),
                Err(_) => println!("{setting} set to {value} (no read-back)"),
            }
            Ok(())
        }
    }
}

fn cmd_mode(cfg: &Config, model: Option<&str>, name: &str, state: ModeState) -> Result<()> {
    let Some((canonical_name, mode)) = cfg.mode(name) else {
        let known = cfg.modes.keys().cloned().collect::<Vec<_>>().join(", ");
        bail!("unknown mode '{name}'. known: {known}");
    };
    let actions = match state {
        ModeState::On => &mode.on,
        ModeState::Off => &mode.off,
    };
    action::run(cfg, model, actions)?;
    println!(
        "mode {canonical_name} {}",
        match state {
            ModeState::On => "on",
            ModeState::Off => "off",
        }
    );
    Ok(())
}

fn cmd_startup(cfg: &Config, config_path: Option<&Path>, remove: bool) -> Result<()> {
    if remove {
        println!("removed startup entry ({})", startup::uninstall()?);
        return Ok(());
    }
    if cfg.hotkeys.is_empty() && cfg.usb.is_none() && cfg.hold_hotkeys.is_empty() {
        eprintln!(
            "monctl: warning: no [hotkeys], [usb], or [[hold_hotkeys]] in config, so the daemon exits at once; add one and re-run"
        );
    }
    let exe = std::env::current_exe().context("finding the monctl executable")?;
    let config_abs = config::resolve_path(config_path)
        .map(std::path::absolute)
        .transpose()
        .context("resolving the config path")?;
    let at = startup::install(&exe, config_abs.as_deref(), "watch")?;
    println!("registered `monctl watch` at login ({at})");
    println!("takes effect at your next login");
    #[cfg(target_os = "macos")]
    println!("start it now with: launchctl load \"{at}\"");
    Ok(())
}

fn cmd_read(cfg: &Config, model: Option<&str>) -> Result<()> {
    let mut mon = monitor::open(model)?;
    let value = mon.get_input()?;
    match cfg.name_for_value(value) {
        Some(name) => println!("current input: {value} ({name}) on {}", mon.label),
        None => println!("current input: {value} on {}", mon.label),
    }
    Ok(())
}

fn cmd_list(cfg: &Config) -> Result<()> {
    let reports = monitor::report_all()?;
    for (i, r) in reports.iter().enumerate() {
        let input = match &r.input {
            Ok(v) => match cfg.name_for_value(*v) {
                Some(name) => format!("{v} ({name})"),
                None => v.to_string(),
            },
            Err(e) => format!("unavailable ({e})"),
        };
        println!("[{i}] {}", r.label);
        println!("    backend: {}   id: {}", r.backend, r.id);
        if let Some(serial) = &r.serial {
            println!("    serial:  {serial}");
        }
        println!("    input (0x60): {input}");
    }
    Ok(())
}

fn cmd_set(cfg: &Config, model: Option<&str>, target: &str) -> Result<()> {
    let value = cfg.resolve_input(target)?;
    let mut mon = monitor::open(model)?;
    mon.set_input(value)?;
    println!("switched {} to {target} ({value})", mon.label);
    Ok(())
}

fn cmd_toggle(cfg: &Config, model: Option<&str>, input_a: &str, input_b: &str) -> Result<()> {
    let a = cfg.resolve_input(input_a)?;
    let b = cfg.resolve_input(input_b)?;
    let mut mon = monitor::open(model)?;
    let current = mon.get_input()?;
    // Flip between the pair; when on neither, pick the first.
    let target = if current == a { b } else { a };
    mon.set_input(target)?;
    let name = cfg.name_for_value(target).unwrap_or("?");
    println!("toggled {} from {current} to {target} ({name})", mon.label);
    Ok(())
}

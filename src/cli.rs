use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug, PartialEq, Eq)]
#[command(
    name = "monkey",
    version,
    about = "Switch a monitor's input over DDC/CI",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Match the display by EDID model substring
    #[arg(short, long, global = true, value_name = "SUBSTR")]
    pub model: Option<String>,

    /// Path to a monkey.toml
    #[arg(short, long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the current input
    Read,
    /// List displays and their inputs
    #[command(alias = "ls")]
    List,
    /// Switch input by name or number (or just `monkey <input>`)
    Set {
        #[arg(value_name = "INPUT")]
        input: String,
    },
    /// Toggle between the two `toggle` inputs
    Toggle,
    /// Apply one side of a configured mode
    Mode {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_enum)]
        state: ModeState,
    },
    /// Read or change Windows HDR for the selected display
    Hdr {
        #[arg(value_enum)]
        state: Option<ModeState>,
    },
    /// Vendor-specific monitor control beyond DDC/CI
    Vendor {
        #[command(subcommand)]
        vendor: VendorCommand,
    },
    /// Daemon: global hotkeys and the [usb] watcher (alias: listen)
    #[command(alias = "listen")]
    Watch,
    /// List connected USB devices (to find a VID:PID)
    Usb,
    /// Run `monkey listen` at login (--remove to undo)
    Startup {
        /// Remove the startup entry instead of adding it
        #[arg(long)]
        remove: bool,
    },
    // bare `monkey <input>`, shorthand for `set <input>`
    #[command(external_subcommand)]
    Switch(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ModeState {
    On,
    Off,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum VendorCommand {
    /// MSI gaming monitors, over the scaler's USB HID protocol
    Msi {
        #[command(subcommand)]
        action: MsiAction,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum MsiAction {
    /// List the known settings
    #[command(alias = "ls")]
    List,
    /// List MSI HID devices and probe which one is the monitor
    Probe,
    /// Read a setting, e.g. `monkey msi get pip_pbp`
    Get {
        #[arg(value_name = "SETTING")]
        setting: String,
    },
    /// Write a setting, e.g. `monkey msi set pip_pbp 2`
    Set {
        #[arg(value_name = "SETTING")]
        setting: String,
        #[arg(value_name = "VALUE")]
        value: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_defs_are_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_global_flag_and_subcommand() {
        let cli = Cli::try_parse_from(["monkey", "-m", "Dell", "set", "16"]).unwrap();
        assert_eq!(cli.model.as_deref(), Some("Dell"));
        assert_eq!(cli.command, Command::Set { input: "16".into() });
    }

    #[test]
    fn bare_input_is_shorthand_for_set() {
        let cli = Cli::try_parse_from(["monkey", "usbc"]).unwrap();
        assert_eq!(cli.command, Command::Switch(vec!["usbc".into()]));
    }

    #[test]
    fn parses_mode_state() {
        let cli = Cli::try_parse_from(["monkey", "mode", "pbp", "off"]).unwrap();
        assert_eq!(
            cli.command,
            Command::Mode {
                name: "pbp".into(),
                state: ModeState::Off,
            }
        );
    }

    #[test]
    fn listen_is_an_alias_for_watch() {
        assert_eq!(
            Cli::try_parse_from(["monkey", "listen"]).unwrap().command,
            Command::Watch
        );
    }

    #[test]
    fn parses_vendor_msi_actions() {
        assert_eq!(
            Cli::try_parse_from(["monkey", "vendor", "msi", "get", "dsc"])
                .unwrap()
                .command,
            Command::Vendor {
                vendor: VendorCommand::Msi {
                    action: MsiAction::Get {
                        setting: "dsc".into()
                    }
                }
            }
        );
        assert_eq!(
            Cli::try_parse_from(["monkey", "vendor", "msi", "set", "pip_pbp", "2"])
                .unwrap()
                .command,
            Command::Vendor {
                vendor: VendorCommand::Msi {
                    action: MsiAction::Set {
                        setting: "pip_pbp".into(),
                        value: 2,
                    }
                }
            }
        );
    }

    #[test]
    fn parses_hdr_read_and_write() {
        assert_eq!(
            Cli::try_parse_from(["monkey", "hdr"]).unwrap().command,
            Command::Hdr { state: None }
        );
        assert_eq!(
            Cli::try_parse_from(["monkey", "hdr", "on"])
                .unwrap()
                .command,
            Command::Hdr {
                state: Some(ModeState::On),
            }
        );
    }
}

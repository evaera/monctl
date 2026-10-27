use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_inputs")]
    pub inputs: BTreeMap<String, u16>,
    #[serde(default)]
    pub hotkeys: BTreeMap<String, HotkeyTarget>,
    #[serde(default)]
    pub hold_hotkeys: Vec<HoldHotkey>,
    /// Stateful monitor modes with explicit on/off actions.
    #[serde(default)]
    pub modes: BTreeMap<String, ModeToggle>,
    #[serde(default)]
    pub usb: Option<UsbWatch>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyTarget {
    Input(String),
    Mode(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldHotkey {
    pub keys: Vec<String>,
    #[serde(default)]
    pub enable_on: Option<String>,
    #[serde(default)]
    pub disable_on: Option<String>,
    /// Exact foreground executable basename, matched case-insensitively.
    pub process: String,
    pub mode: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateKey {
    Key(i32),
    WheelRight,
    WheelLeft,
}

pub fn gate_key(key: &str) -> Result<GateKey> {
    match key.to_ascii_lowercase().as_str() {
        "wheel_right" => Ok(GateKey::WheelRight),
        "wheel_left" => Ok(GateKey::WheelLeft),
        _ => hold_key(key).map(GateKey::Key),
    }
}

/// Windows virtual-key values. Kept platform-independent for config validation.
pub fn hold_key(key: &str) -> Result<i32> {
    let key = key.to_ascii_lowercase();
    let vk = match key.as_str() {
        "left_mouse" => 0x01,
        "right_mouse" => 0x02,
        "middle_mouse" => 0x04,
        "mouse4" => 0x05,
        "mouse5" => 0x06,
        "space" => 0x20,
        "tab" => 0x09,
        "enter" => 0x0d,
        "escape" => 0x1b,
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
            i32::from(key.as_bytes()[0].to_ascii_uppercase())
        }
        _ => {
            if let Some(n) = key.strip_prefix('f').and_then(|s| s.parse::<i32>().ok())
                && (1..=24).contains(&n)
            {
                return Ok(0x70 + n - 1);
            }
            bail!(
                "unsupported hold key '{key}': use a letter, digit, F1-F24, space, tab, enter, escape, or a mouse button"
            );
        }
    };
    Ok(vk)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeToggle {
    #[serde(default)]
    pub pause_usb: bool,
    pub on: Vec<Action>,
    pub off: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Action {
    Msi(MsiStep),
    WindowsHdr(WindowsHdrStep),
    Wait(WaitStep),
    Input(InputStep),
    Ddc(DdcStep),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsHdrStep {
    pub windows_hdr: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsiStep {
    pub msi: MsiCommand,
}

/// An OSD setting written straight to the monitor over USB HID; names come
/// from [`crate::vendor::msi::SETTINGS`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsiCommand {
    pub command: String,
    pub value: u16,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitStep {
    pub wait_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputStep {
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DdcStep {
    pub ddc: DdcCommand,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DdcCommand {
    pub feature: u8,
    pub value: u16,
}

/// `monctl watch`: switch input when a USB device connects/disconnects.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbWatch {
    /// USB vendor:product in hex, e.g. "3297:1977".
    pub device: String,
    #[serde(default)]
    pub on_connect: Option<String>,
    #[serde(default)]
    pub on_disconnect: Option<String>,
    /// Wake this machine's screen on the event that hands it the monitor, so
    /// the monitor finds a live signal instead of a sleeping output.
    #[serde(default)]
    pub wake_on_connect: bool,
    #[serde(default)]
    pub wake_on_disconnect: bool,
    /// How long to let the panel light up after a wake before switching input.
    #[serde(default = "default_wake_settle_ms")]
    pub wake_settle_ms: u64,
}

impl UsbWatch {
    pub fn ids(&self) -> Result<(u16, u16)> {
        let (v, p) = self.device.split_once(':').with_context(|| {
            format!(
                "`device` must be VID:PID, e.g. 3297:1977 (got '{}')",
                self.device
            )
        })?;
        let vid =
            u16::from_str_radix(v.trim(), 16).with_context(|| format!("bad vendor id '{v}'"))?;
        let pid =
            u16::from_str_radix(p.trim(), 16).with_context(|| format!("bad product id '{p}'"))?;
        Ok((vid, pid))
    }
}

fn default_wake_settle_ms() -> u64 {
    500
}

fn default_inputs() -> BTreeMap<String, u16> {
    BTreeMap::from([
        ("dp".to_string(), 15),
        ("usbc".to_string(), 16),
        ("hdmi1".to_string(), 17),
        ("hdmi2".to_string(), 18),
    ])
}

impl Default for Config {
    fn default() -> Self {
        Config {
            model: Some("MPG".to_string()),
            inputs: default_inputs(),
            hotkeys: BTreeMap::new(),
            hold_hotkeys: Vec::new(),
            modes: BTreeMap::new(),
            usb: None,
        }
    }
}

impl Config {
    // name (case-insensitive) or a raw number
    pub fn resolve_input(&self, target: &str) -> Result<u16> {
        let t = target.trim();
        if let Ok(v) = t.parse::<u16>() {
            return Ok(v);
        }
        for (name, v) in &self.inputs {
            if name.eq_ignore_ascii_case(t) {
                return Ok(*v);
            }
        }
        let known = self.inputs.keys().cloned().collect::<Vec<_>>().join(", ");
        bail!("unknown input '{target}'. known: {known} (or pass a number)");
    }

    pub fn name_for_value(&self, value: u16) -> Option<&str> {
        self.inputs
            .iter()
            .find(|(_, v)| **v == value)
            .map(|(name, _)| name.as_str())
    }

    pub fn mode(&self, target: &str) -> Option<(&str, &ModeToggle)> {
        self.modes
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(target.trim()))
            .map(|(name, mode)| (name.as_str(), mode))
    }

    fn validate(&self) -> Result<()> {
        for hold in &self.hold_hotkeys {
            if hold.keys.is_empty() {
                bail!("[[hold_hotkeys]] needs at least one key");
            }
            for key in &hold.keys {
                hold_key(key)?;
            }
            match (&hold.enable_on, &hold.disable_on) {
                (Some(on), Some(off)) => {
                    if gate_key(on)? == gate_key(off)? {
                        bail!("hold hotkey enable_on/disable_on keys must differ");
                    }
                }
                (None, None) => {}
                _ => bail!("hold hotkey enable_on and disable_on must be supplied together"),
            }
            let (name, _) = self
                .mode(&hold.mode)
                .with_context(|| format!("hold hotkey references unknown mode '{}'", hold.mode))?;
            if self
                .hotkeys
                .values()
                .any(|target| matches!(target, HotkeyTarget::Mode(target) if target.trim().eq_ignore_ascii_case(name)))
            {
                bail!("mode '{name}' cannot be both held and toggled by [hotkeys]");
            }
            if !hold.process.to_ascii_lowercase().ends_with(".exe")
                || hold.process.len() <= 4
                || hold.process.contains(['/', '\\', ':'])
            {
                bail!("[[hold_hotkeys]] process must be an executable basename, e.g. game.exe");
            }
        }
        for (name, mode) in &self.modes {
            if mode.on.is_empty() || mode.off.is_empty() {
                bail!("mode '{name}' needs non-empty `on` and `off` action lists");
            }
            for (side, actions) in [("on", &mode.on), ("off", &mode.off)] {
                for action in actions {
                    match action {
                        Action::Msi(step) => {
                            let setting = crate::vendor::msi::resolve(&step.msi.command)
                                .with_context(|| format!("mode '{name}' `{side}` MSI command"))?;
                            if !setting.writable {
                                bail!(
                                    "mode '{name}' `{side}` MSI setting '{}' is read-only",
                                    setting.name
                                );
                            }
                        }
                        Action::Wait(_) | Action::Ddc(_) | Action::WindowsHdr(_) => {}
                        Action::Input(step) => {
                            self.resolve_input(&step.input)
                                .with_context(|| format!("mode '{name}' `{side}` input"))?;
                        }
                    }
                }
            }
        }
        for (combo, target) in &self.hotkeys {
            match target {
                HotkeyTarget::Input(input) => {
                    self.resolve_input(input)
                        .with_context(|| format!("hotkey '{combo}'"))?;
                }
                HotkeyTarget::Mode(mode) => {
                    self.mode(mode).with_context(|| {
                        format!("hotkey '{combo}' references unknown mode '{mode}'")
                    })?;
                }
            }
        }
        if let Some(usb) = &self.usb {
            usb.ids().context("in [usb]")?;
            for name in [&usb.on_connect, &usb.on_disconnect].into_iter().flatten() {
                self.resolve_input(name)
                    .with_context(|| format!("[usb] references '{name}'"))?;
            }
        }
        Ok(())
    }
}

pub fn load(explicit: Option<&Path>) -> Result<Config> {
    let Some(path) = resolve_path(explicit) else {
        return Ok(Config::default());
    };
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let config: Config =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    config
        .validate()
        .with_context(|| format!("in {}", path.display()))?;
    Ok(config)
}

pub fn resolve_path(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve_path_with(
        explicit,
        std::env::var_os("MONCTL_CONFIG").as_deref(),
        dirs::config_dir().as_deref(),
    )
}

fn resolve_path_with(
    explicit: Option<&Path>,
    env: Option<&OsStr>,
    user_config_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    if let Some(env) = env.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(env));
    }
    user_config_dir
        .map(|dir| dir.join("monctl").join("config.toml"))
        .filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_from(src: &str) -> Config {
        let c: Config = toml::from_str(src).expect("parse");
        c.validate().expect("validate");
        c
    }

    #[test]
    fn resolve() {
        let c = Config::default();
        assert_eq!(c.resolve_input("usbc").unwrap(), 16);
        assert_eq!(c.resolve_input("USBC").unwrap(), 16);
        assert_eq!(c.resolve_input(" 16 ").unwrap(), 16);
        assert!(c.resolve_input("hdmi9").is_err());
    }

    #[test]
    fn custom_config() {
        let c = cfg_from("model = \"Dell\"\n\n[inputs]\nl = 17\nr = 18\n");
        assert_eq!(c.model.as_deref(), Some("Dell"));
        assert_eq!(c.resolve_input("l").unwrap(), 17);
        assert_eq!(c.resolve_input("r").unwrap(), 18);
    }

    #[test]
    fn hold_hotkey_config() {
        let mode = "[modes.zoom]\non = [{msi = { command = 'optix_scope', value = 1 }}]\noff = [{msi = { command = 'optix_scope', value = 0 }}]\n";
        let binding = "[[hold_hotkeys]]\nkeys = ['right_mouse', 'f12']\nprocess = 'game.exe'\nmode = 'ZOOM'\n";
        let c = cfg_from(&format!("{mode}{binding}"));
        assert_eq!(c.hold_hotkeys[0].keys, ["right_mouse", "f12"]);
        assert_eq!(c.hold_hotkeys[0].process, "game.exe");
        assert!(c.hold_hotkeys[0].enable_on.is_none());
        assert!(c.hold_hotkeys[0].disable_on.is_none());
        let selected = cfg_from(&format!(
            "{mode}{binding}enable_on = 'wheel_left'\ndisable_on = 'wheel_right'\n{binding}"
        ));
        // Multiple bindings may hold the same mode, including different processes.
        assert_eq!(selected.hold_hotkeys.len(), 2);
        assert_eq!(
            selected.hold_hotkeys[0].enable_on.as_deref(),
            Some("wheel_left")
        );
        assert!(selected.hold_hotkeys[1].enable_on.is_none());
        for invalid_selection in [
            "enable_on = 'wheel_left'",
            "disable_on = 'wheel_right'",
            "enable_on = 'invalid'\ndisable_on = 'wheel_left'",
            "enable_on = 'wheel_left'\ndisable_on = 'invalid'",
            "enable_on = 'f11'\ndisable_on = 'F11'",
        ] {
            let config: Config =
                toml::from_str(&format!("{mode}{binding}{invalid_selection}")).unwrap();
            assert!(config.validate().is_err(), "accepted {invalid_selection}");
        }
        assert_eq!(gate_key("WHEEL_RIGHT").unwrap(), GateKey::WheelRight);
        assert_eq!(gate_key("wheel_left").unwrap(), GateKey::WheelLeft);
        assert_eq!(gate_key("f11").unwrap(), GateKey::Key(0x7a));
        let conflict: Config = toml::from_str(&format!(
            "{mode}{binding}[hotkeys]\n'ctrl+alt+z' = {{ mode = 'zoom' }}\n"
        ))
        .unwrap();
        assert!(conflict.validate().is_err());
        assert!(toml::from_str::<Config>(&format!("{mode}{binding}[hold_hotkeys.gate]\nkeys = ['right_mouse']\non = 'wheel_left'\noff = 'wheel_right'\n")).is_err());
        for invalid in [
            "keys = []\nprocess = 'game.exe'\nmode = 'zoom'",
            "keys = ['f12']\nprocess = ''\nmode = 'zoom'",
            "keys = ['f12']\nprocess = 'C:/game.exe'\nmode = 'zoom'",
            "keys = ['f12']\nprocess = '.exe'\nmode = 'zoom'",
            "keys = ['f25']\nprocess = 'game.exe'\nmode = 'zoom'",
            "keys = ['f12']\nprocess = 'game.exe'\nmode = 'missing'",
        ] {
            let config: Config =
                toml::from_str(&format!("{mode}[[hold_hotkeys]]\n{invalid}")).unwrap();
            assert!(config.validate().is_err(), "accepted {invalid}");
        }
        assert_eq!(hold_key("F12").unwrap(), 0x7b);
        assert_eq!(hold_key("f1").unwrap(), 0x70);
        assert_eq!(hold_key("f24").unwrap(), 0x87);
        assert_eq!(hold_key("right_mouse").unwrap(), 2);
        assert_eq!(hold_key("a").unwrap(), 0x41);
        assert!(hold_key("ctrl+f12").is_err());
    }

    #[test]
    fn mode_actions_and_named_msi_commands() {
        let c = cfg_from(
            r#"
[hotkeys]
"ctrl+alt+p" = { mode = "pbp" }

[modes.pbp]
pause_usb = true
on = [
  { msi = { command = "adaptive-sync", value = 0 } },
  { wait_ms = 500 },
  { ddc = { feature = 96, value = 15 } },
]
off = [
  { msi = { command = "pip_pbp", value = 0 } },
  { input = "dp" },
]
"#,
        );
        let mode = &c.modes["pbp"];
        assert!(mode.pause_usb);
        let Action::Msi(step) = &mode.on[0] else {
            panic!("expected MSI action");
        };
        assert_eq!(step.msi.command, "adaptive-sync");
        assert_eq!(step.msi.value, 0);
    }

    #[test]
    fn rejects_unknown_and_read_only_msi_settings() {
        let bad = toml::from_str::<Config>(
            "[modes.m]\non = [{ msi = { command = \"pbp\", value = 1 } }]\noff = [{ wait_ms = 1 }]\n",
        )
        .unwrap();
        assert!(bad.validate().is_err());
        let read_only = toml::from_str::<Config>(
            "[modes.m]\non = [{ msi = { command = \"model\", value = 1 } }]\noff = [{ wait_ms = 1 }]\n",
        )
        .unwrap();
        assert!(read_only.validate().is_err());
    }

    #[test]
    fn rejects_unknown_hotkey_target_and_bad_mode() {
        assert!(
            toml::from_str::<Config>("[hotkeys]\n\"ctrl+alt+p\" = { mode = \"missing\" }\n")
                .unwrap()
                .validate()
                .is_err()
        );
        assert!(
            toml::from_str::<Config>("[modes.pbp]\non = []\noff = [{ wait_ms = 1 }]\n")
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn usb_wake() {
        let c = cfg_from(
            "[usb]\ndevice = \"3297:1977\"\non_connect = \"dp\"\nwake_on_connect = true\n",
        );
        let usb = c.usb.unwrap();
        assert!(usb.wake_on_connect);
        assert!(!usb.wake_on_disconnect);
        assert_eq!(usb.wake_settle_ms, 500);

        let c = cfg_from("[usb]\ndevice = \"1:2\"\non_connect = \"dp\"\nwake_settle_ms = 1200\n");
        assert_eq!(c.usb.unwrap().wake_settle_ms, 1200);
    }

    #[test]
    fn rejects_bad_config() {
        assert!(toml::from_str::<Config>("notathing = 1\n").is_err());
        assert!(toml::from_str::<Config>("toggle = [\"dp\", \"usbc\"]\n").is_err());
    }

    #[test]
    fn hotkeys_have_explicit_targets() {
        // Identical input and mode names are unambiguous with explicit targets.
        let c = cfg_from(
            r#"
[hotkeys]
"ctrl+alt+1" = { input = "DP" }
"ctrl+alt+2" = { mode = " dp " }
"ctrl+alt+3" = { input = "16" }
[modes.dp]
on = [{ input = "dp" }]
off = [{ input = "usbc" }]
"#,
        );
        assert_eq!(c.hotkeys["ctrl+alt+1"], HotkeyTarget::Input("DP".into()));
        assert_eq!(c.hotkeys["ctrl+alt+2"], HotkeyTarget::Mode(" dp ".into()));
        for target in [
            "'dp'",
            "{}",
            "{ input = 'dp', mode = 'dp' }",
            "{ input = 'dp', typo = true }",
            "{ command = 'dp' }",
        ] {
            assert!(
                toml::from_str::<Config>(&format!("[hotkeys]\n'ctrl+alt+1' = {target}\n")).is_err(),
                "accepted {target}"
            );
        }
        let invalid: Config =
            toml::from_str("[hotkeys]\n'ctrl+alt+1' = { input = 'missing' }\n").unwrap();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn config_path_precedence() {
        let base = std::env::temp_dir().join(format!("monctl-test-{}", std::process::id()));
        let app_dir = base.join("monctl");
        std::fs::create_dir_all(&app_dir).unwrap();
        let cfg = app_dir.join("config.toml");
        std::fs::write(&cfg, "model = \"X\"\n").unwrap();
        let explicit = Path::new("explicit.toml");
        let env = OsStr::new("env.toml");
        assert_eq!(
            resolve_path_with(Some(explicit), Some(env), Some(&base)),
            Some(explicit.to_path_buf())
        );
        assert_eq!(
            resolve_path_with(None, Some(env), Some(&base)),
            Some(PathBuf::from(env))
        );
        assert_eq!(
            resolve_path_with(None, None, Some(&base)),
            Some(cfg.clone())
        );
        assert_eq!(
            resolve_path_with(None, Some(OsStr::new("")), Some(&base)),
            Some(cfg.clone())
        );
        assert_eq!(resolve_path_with(None, None, None), None);
        std::fs::remove_file(cfg).unwrap();
        assert_eq!(resolve_path_with(None, None, Some(&base)), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}

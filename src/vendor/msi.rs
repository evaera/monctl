//! Direct USB-HID control of MSI gaming monitors, replacing the Gaming
//! Intelligence app. The monitor's scaler speaks an ASCII protocol over
//! 64-byte HID reports: sets are `5b` + 5-char code + 3-digit value + CR,
//! gets are `58` + code + CR, and every reply echoes `5b` + code with the
//! value in the last three characters. Command codes recovered from
//! MonitorControlLibrary.dll (Gaming Intelligence 0.1.4.x); the same protocol
//! is implemented by msigd and msi-monitor-ctrl for older models.

use anyhow::{Context, Result, bail};

/// One OSD setting addressable over the monitor's USB HID interface.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub name: &'static str,
    /// ASCII payload between the `5b`/`58` prefix and the value digits.
    /// MSI extends past '9' with the following ASCII characters, so codes
    /// like `002D` (DSC) and `006=` (PBP layout) are literal.
    code: &'static [u8; 5],
    pub writable: bool,
}

const fn rw(name: &'static str, code: &'static [u8; 5]) -> Setting {
    Setting {
        name,
        code,
        writable: true,
    }
}

const fn ro(name: &'static str, code: &'static [u8; 5]) -> Setting {
    Setting {
        name,
        code,
        writable: false,
    }
}

/// `pip_pbp`: 0 = off, 1 = PIP, 2 = PBP. Sources use MSI's internal input
/// order: HDMI1=0, HDMI2=1, DP=2, Type-C=3.
pub const SETTINGS: &[Setting] = &[
    rw("adaptive_sync", b"00280"),
    rw("dsc", b"002D0"),
    rw("pip_pbp", b"00600"),
    rw("pbp_layout", b"006=0"),
    rw("pbp_source", b"00610"),
    rw("pbp_source_1", b"00611"),
    rw("pbp_source_2", b"00612"),
    rw("pip_source", b"00620"),
    rw("pip_size", b"00630"),
    rw("pip_loc", b"00640"),
    rw("display_switch", b"00650"),
    rw("audio_switch", b"00660"),
    rw("input", b"00500"),
    rw("auto_scan", b"00510"),
    rw("kvm", b"008>0"),
    rw("monitor_hdr", b"004:0"),
    rw("brightness", b"00400"),
    ro("model", b"00140"),
    ro("uart_version", b"00150"),
    ro("cur_mode", b"00120"),
    ro("freesync_status", b"00160"),
    ro("hdr_status", b"00190"),
];

pub fn resolve(name: &str) -> Result<&'static Setting> {
    let normalized = name.trim().to_ascii_lowercase().replace('-', "_");
    let normalized = match normalized.as_str() {
        "pbp_mode" => "pbp_layout",
        "input_source" => "input",
        other => other,
    };
    SETTINGS
        .iter()
        .find(|s| s.name == normalized)
        .with_context(|| {
            let known = SETTINGS
                .iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown MSI setting '{name}'; known: {known}")
        })
}

/// `5b` + code + value (decimal, right-aligned over the trailing "000") + CR.
fn set_frame(setting: &Setting, value: u16) -> Result<[u8; 11]> {
    if !setting.writable {
        bail!("MSI setting '{}' is read-only", setting.name);
    }
    if value > 999 {
        bail!("MSI value {value} out of range (0-999)");
    }
    let mut frame = *b"5b00000000\r";
    frame[2..7].copy_from_slice(setting.code);
    let digits = [value / 100 % 10, value / 10 % 10, value % 10];
    for (i, d) in digits.iter().enumerate() {
        frame[7 + i] = b'0' + *d as u8;
    }
    Ok(frame)
}

fn get_frame(setting: &Setting) -> [u8; 8] {
    let mut frame = *b"5800000\r";
    frame[2..7].copy_from_slice(setting.code);
    frame
}

/// A reply is valid when it echoes `5b` + the setting's code. Value bytes are
/// positional `'0' + n` where n may exceed 9: model 124 arrives as the single
/// byte 0xAC in the last position, not as three digits.
fn parse_reply(setting: &Setting, frame: &[u8]) -> Option<u16> {
    if frame.len() < 10 || &frame[..2] != b"5b" || &frame[2..7] != setting.code {
        return None;
    }
    let mut value = 0u32;
    for c in &frame[7..10] {
        if *c < b'0' {
            return None;
        }
        value = value * 10 + (*c - b'0') as u32;
    }
    u16::try_from(value).ok()
}

#[cfg(windows)]
pub use platform::{get, probe, set};

#[cfg(windows)]
mod platform {
    use super::{Setting, get_frame, parse_reply, resolve, set_frame};
    use anyhow::{Context, Result, bail};
    use hidapi::{HidApi, HidDevice};
    use std::time::{Duration, Instant};

    const MSI_VID: u16 = 0x1462;
    const REPORT_LEN: usize = 64;
    const READ_TIMEOUT_MS: i32 = 3000;

    /// Sets are fire-and-forget: the scaler echoes gets but not sets, so
    /// (like Gaming Intelligence) success here means "delivered", not
    /// "applied". Read the setting back to confirm when that matters.
    pub fn set(name: &str, value: u16) -> Result<()> {
        let setting = resolve(name)?;
        let frame = set_frame(setting, value)?;
        let device = open_monitor()?;
        send_only(&device, &frame, setting)
    }

    pub fn get(name: &str) -> Result<u16> {
        let setting = resolve(name)?;
        let frame = get_frame(setting);
        let device = open_monitor()?;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(100));
            }
            if let Some(value) = transact(&device, &frame, setting)? {
                return Ok(value);
            }
        }
        bail!("the monitor did not answer MSI get {}", setting.name)
    }

    /// List MSI HID interfaces and which one answers as a monitor.
    pub fn probe() -> Result<()> {
        let api = HidApi::new().context("initializing HID")?;
        let mut found = false;
        for info in api.device_list().filter(|d| d.vendor_id() == MSI_VID) {
            found = true;
            let label = format!(
                "{:04x}:{:04x} usage {:04x}:{:04x} {}",
                info.vendor_id(),
                info.product_id(),
                info.usage_page(),
                info.usage(),
                info.product_string().unwrap_or("?"),
            );
            match info.open_device(&api) {
                Ok(device) => {
                    let model = resolve("model").expect("model is a known setting");
                    match transact(&device, &get_frame(model), model) {
                        Ok(Some(value)) => println!("{label}  -> monitor, model index {value}"),
                        Ok(None) => println!("{label}  -> no monitor reply"),
                        Err(e) => println!("{label}  -> {e:#}"),
                    }
                }
                Err(e) => println!("{label}  -> open failed: {e}"),
            }
        }
        if !found {
            bail!("no MSI (vid 1462) HID devices; is the monitor's USB upstream cable connected?");
        }
        Ok(())
    }

    /// The monitor is whichever MSI HID interface answers a get-model probe.
    fn open_monitor() -> Result<HidDevice> {
        let api = HidApi::new().context("initializing HID")?;
        let model = resolve("model").expect("model is a known setting");
        let mut last_err = None;
        let mut candidates = 0;
        for info in api.device_list().filter(|d| d.vendor_id() == MSI_VID) {
            candidates += 1;
            let device = match info.open_device(&api) {
                Ok(d) => d,
                Err(e) => {
                    last_err = Some(anyhow::anyhow!("{e}"));
                    continue;
                }
            };
            match transact(&device, &get_frame(model), model) {
                Ok(Some(_)) => return Ok(device),
                Ok(None) => {}
                Err(e) => last_err = Some(e),
            }
        }
        if candidates == 0 {
            bail!("no MSI (vid 1462) HID devices; is the monitor's USB upstream cable connected?");
        }
        match last_err {
            Some(e) => Err(e.context("no MSI HID device answered as a monitor")),
            None => bail!("no MSI HID device answered as a monitor"),
        }
    }

    fn drain(device: &HidDevice) {
        // The monitor also emits unsolicited reports (OSD micro-key events);
        // clear them so a later read sees only our reply.
        let mut scratch = [0u8; REPORT_LEN];
        for _ in 0..10 {
            match device.read_timeout(&mut scratch, 5) {
                Ok(n) if n > 0 => continue,
                _ => break,
            }
        }
    }

    fn write_frame(device: &HidDevice, payload: &[u8], setting: &Setting) -> Result<()> {
        // Report id 1, then the ASCII frame, zero-padded to the report's
        // exact size (1 id byte + 63 data bytes; anything else is rejected).
        let mut report = [0u8; REPORT_LEN];
        report[0] = 0x01;
        report[1..1 + payload.len()].copy_from_slice(payload);
        let written = device
            .write(&report)
            .with_context(|| format!("writing MSI command to {}", setting.name))?;
        if std::env::var_os("MONKEY_MSI_DEBUG").is_some() {
            eprintln!("msi debug: wrote {written} bytes: {:02x?}", &report[..16]);
        }
        Ok(())
    }

    fn send_only(device: &HidDevice, payload: &[u8], setting: &Setting) -> Result<()> {
        let _guard = MonitorMutex::acquire();
        drain(device);
        write_frame(device, payload, setting)
    }

    /// One command/reply exchange. Returns the reply value, or None when the
    /// device answered with something that is not an echo of this command.
    fn transact(device: &HidDevice, payload: &[u8], setting: &Setting) -> Result<Option<u16>> {
        let _guard = MonitorMutex::acquire();
        drain(device);
        write_frame(device, payload, setting)?;
        let debug = std::env::var_os("MONKEY_MSI_DEBUG").is_some();

        let deadline = Instant::now() + Duration::from_millis(READ_TIMEOUT_MS as u64);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let mut buf = [0u8; REPORT_LEN];
            let n = device
                .read_timeout(&mut buf, remaining.as_millis() as i32)
                .with_context(|| format!("reading MSI reply for {}", setting.name))?;
            if n == 0 {
                if debug {
                    eprintln!("msi debug: read timed out");
                }
                return Ok(None);
            }
            if debug {
                eprintln!("msi debug: read {n} bytes: {:02x?}", &buf[..n.min(16)]);
            }
            // The reply may or may not carry the report id byte in front.
            let frame = if buf[0] == b'5' {
                &buf[..n]
            } else {
                &buf[1..n]
            };
            if let Some(value) = parse_reply(setting, frame) {
                return Ok(Some(value));
            }
            // not ours (e.g. a key event that raced the drain); keep reading
        }
    }

    /// Gaming Intelligence and MonitorMicroKeyDetector serialize access to the
    /// scaler with this global mutex-as-flag: holding it means "bus busy", so
    /// take it for the duration of each exchange to avoid interleaving with
    /// them. Best-effort: after a bounded wait we proceed anyway rather than
    /// hang monkey on a wedged MSI process.
    struct MonitorMutex(Option<isize>);

    impl MonitorMutex {
        fn acquire() -> Self {
            use windows_sys::Win32::Foundation::{
                ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
            };
            use windows_sys::Win32::System::Threading::CreateMutexW;

            let name: Vec<u16> = "Global\\MSIMonitorMacroKeyDetectorRunningFlag"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let handle: HANDLE = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
                let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE && !already {
                    return MonitorMutex(Some(handle as isize));
                }
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
                }
                if Instant::now() >= deadline {
                    return MonitorMutex(None);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for MonitorMutex {
        fn drop(&mut self) {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Threading::ReleaseMutex;
            if let Some(handle) = self.0.take() {
                unsafe {
                    ReleaseMutex(handle as _);
                    CloseHandle(handle as _);
                }
            }
        }
    }
}

#[cfg(not(windows))]
pub fn set(_name: &str, _value: u16) -> Result<()> {
    bail!("MSI monitor control is only available on Windows")
}

#[cfg(not(windows))]
pub fn get(_name: &str) -> Result<u16> {
    bail!("MSI monitor control is only available on Windows")
}

#[cfg(not(windows))]
pub fn probe() -> Result<()> {
    bail!("MSI monitor control is only available on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_names_and_aliases() {
        assert_eq!(resolve("dsc").unwrap().name, "dsc");
        assert_eq!(resolve("Adaptive-Sync").unwrap().name, "adaptive_sync");
        assert_eq!(resolve("pbp_mode").unwrap().name, "pbp_layout");
        assert_eq!(resolve("input_source").unwrap().name, "input");
        assert!(resolve("pbp").is_err());
        assert!(resolve("nope").is_err());
    }

    #[test]
    fn set_frames_match_gaming_intelligence() {
        // Byte-for-byte against MonitorControlLibrary's SetCmdDictionary.
        let f = set_frame(resolve("adaptive_sync").unwrap(), 0).unwrap();
        assert_eq!(&f, b"5b00280000\r");
        let f = set_frame(resolve("dsc").unwrap(), 1).unwrap();
        assert_eq!(&f, b"5b002D0001\r");
        let f = set_frame(resolve("pip_pbp").unwrap(), 2).unwrap();
        assert_eq!(&f, b"5b00600002\r");
        let f = set_frame(resolve("pbp_layout").unwrap(), 1).unwrap();
        assert_eq!(&f, b"5b006=0001\r");
        // multi-digit values right-align like GI's brightness encoding
        let f = set_frame(resolve("brightness").unwrap(), 69).unwrap();
        assert_eq!(&f, b"5b00400069\r");
    }

    #[test]
    fn get_frames_and_read_only() {
        assert_eq!(&get_frame(resolve("model").unwrap()), b"5800140\r");
        assert_eq!(&get_frame(resolve("input").unwrap()), b"5800500\r");
        assert!(set_frame(resolve("model").unwrap(), 1).is_err());
        assert!(set_frame(resolve("dsc").unwrap(), 1000).is_err());
    }

    #[test]
    fn parses_replies() {
        let s = resolve("model").unwrap();
        assert_eq!(parse_reply(s, b"5b00140124\r"), Some(124));
        // the 491CQPX really answers model 124 as a single '0'+124 byte
        assert_eq!(parse_reply(s, b"5b0014000\xac\r"), Some(124));
        assert_eq!(parse_reply(s, b"5b00150124\r"), None); // wrong code
        assert_eq!(parse_reply(s, b"5800140124\r"), None); // wrong prefix
        assert_eq!(parse_reply(s, b"5b0014012\x1f\r"), None); // below '0'
        let s = resolve("input").unwrap();
        assert_eq!(parse_reply(s, b"5b00500002\r"), Some(2));
    }
}

use crate::config::{Config, HotkeyTarget};
use crate::monitor::{self, InputSource};
use anyhow::{Context, Result, anyhow, bail};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct Binding {
    combo: String,
    target: BindingTarget,
}

enum BindingTarget {
    Input { name: String, value: u16 },
    Mode { name: String },
}

pub fn watch(config: &Config, model: Option<&str>) -> Result<()> {
    #[cfg(windows)]
    let has_holds = !config.hold_hotkeys.is_empty();
    #[cfg(not(windows))]
    let has_holds = false;
    #[cfg(not(windows))]
    if !config.hold_hotkeys.is_empty() {
        eprintln!("[[hold_hotkeys]] is Windows-only; ignoring it on this platform");
    }
    if config.hotkeys.is_empty() && !has_holds {
        // no hotkeys to bind: the daemon is just the USB watcher
        if config.usb.is_some() {
            return crate::usb::watch(config, model);
        }
        bail!("nothing to run: config.toml has no [hotkeys], [usb], or supported [[hold_hotkeys]]");
    }

    let manager = GlobalHotKeyManager::new().context("starting the hotkey manager")?;
    let mut bindings: HashMap<u32, Binding> = HashMap::new();
    for (combo, target) in &config.hotkeys {
        let hotkey = HotKey::from_str(combo).map_err(|e| anyhow!("bad hotkey '{combo}': {e}"))?;
        let binding_target = match target {
            HotkeyTarget::Mode(mode) => {
                let (name, _) = config
                    .mode(mode)
                    .with_context(|| format!("unknown mode '{mode}'"))?;
                BindingTarget::Mode {
                    name: name.to_string(),
                }
            }
            HotkeyTarget::Input(input) => BindingTarget::Input {
                name: input.clone(),
                value: config
                    .resolve_input(input)
                    .with_context(|| format!("hotkey '{combo}'"))?,
            },
        };
        manager
            .register(hotkey)
            .with_context(|| format!("registering '{combo}' (already taken?)"))?;
        match &binding_target {
            BindingTarget::Input { name, value } => {
                println!("{combo:<16} -> {name} ({value})")
            }
            BindingTarget::Mode { name } => println!("{combo:<16} -> mode {name}"),
        }
        bindings.insert(
            hotkey.id(),
            Binding {
                combo: combo.clone(),
                target: binding_target,
            },
        );
    }
    println!("listening for {} hotkey(s), ctrl+c to quit", bindings.len());

    // One daemon handles every trigger. If a USB device is also configured,
    // follow it on a background thread while this thread runs the hotkey loop,
    // which must own the main thread for the platform event pump.
    let initially_held = |name: &str| {
        has_holds
            && config
                .hold_hotkeys
                .iter()
                .any(|hold| hold.mode.trim().eq_ignore_ascii_case(name))
    };
    let initial_states: BTreeMap<String, bool> = config
        .modes
        .iter()
        .map(|(name, mode)| (name.clone(), mode.pause_usb && initially_held(name)))
        .collect();
    // Hold modes restore `off` at startup. Pause before even starting USB
    // watching, and retain each mode's pause until that restoration completes.
    let usb_paused = Arc::new(AtomicBool::new(
        initial_states.values().any(|active| *active),
    ));
    if config.usb.is_some() {
        let cfg = config.clone();
        let model = model.map(str::to_owned);
        let paused = usb_paused.clone();
        std::thread::spawn(move || {
            if let Err(e) = crate::usb::watch_with_pause(&cfg, model.as_deref(), paused) {
                eprintln!("usb watch stopped: {e:#}");
            }
        });
    }

    // owned so the closure can outlive `listen` (macOS keeps it in a callback)
    let model = model.map(str::to_owned);
    let config = config.clone();
    let mode_states = Arc::new(Mutex::new(initial_states));
    #[cfg(windows)]
    let _holds = if has_holds {
        Some(crate::hold_hotkey::Watcher::start(
            &config,
            model.as_deref(),
            mode_states.clone(),
            usb_paused.clone(),
        )?)
    } else {
        None
    };
    run(move |id| {
        let Some(b) = bindings.get(&id) else { return };
        match &b.target {
            BindingTarget::Input { name, value } => {
                // re-pick the display; it may have dropped after sleep
                let result = monitor::open(model.as_deref()).and_then(|mut m| {
                    m.set_input(*value)?;
                    Ok(m.label)
                });
                match result {
                    Ok(label) => println!("{} -> {} ({}) on {label}", b.combo, name, value),
                    Err(e) => eprintln!("{} -> {} failed: {e:#}", b.combo, name),
                }
            }
            BindingTarget::Mode { name } => {
                let mode = &config.modes[name];
                let turning_on = !mode_states
                    .lock()
                    .expect("mode state poisoned")
                    .get(name)
                    .copied()
                    .unwrap_or(false);

                // Pause before the first enabling command, and leave the watcher
                // paused through the complete disabling/restoration sequence.
                if turning_on && mode.pause_usb {
                    let mut states = mode_states.lock().expect("mode state poisoned");
                    states.insert(name.clone(), true);
                    usb_paused.store(true, Ordering::SeqCst);
                }
                let actions = if turning_on { &mode.on } else { &mode.off };
                let result = crate::action::run(&config, model.as_deref(), actions);
                match result {
                    Ok(()) => {
                        let mut states = mode_states.lock().expect("mode state poisoned");
                        states.insert(name.clone(), turning_on);
                        let pause = config.modes.iter().any(|(mode_name, definition)| {
                            definition.pause_usb && states.get(mode_name).copied().unwrap_or(false)
                        });
                        usb_paused.store(pause, Ordering::SeqCst);
                        println!(
                            "{} -> mode {} {}",
                            b.combo,
                            name,
                            if turning_on { "on" } else { "off" }
                        );
                    }
                    Err(e) => {
                        let mut states = mode_states.lock().expect("mode state poisoned");
                        if turning_on {
                            states.insert(name.clone(), false);
                        }
                        let pause = config.modes.iter().any(|(mode_name, definition)| {
                            definition.pause_usb && states.get(mode_name).copied().unwrap_or(false)
                        });
                        usb_paused.store(pause, Ordering::SeqCst);
                        eprintln!(
                            "{} -> mode {} {} failed: {e:#}",
                            b.combo,
                            name,
                            if turning_on { "on" } else { "off" }
                        );
                    }
                }
            }
        }
    })
}

#[cfg(windows)]
fn run<F: Fn(u32) + Send + Sync + 'static>(dispatch: F) -> Result<()> {
    // pump this thread's messages so global-hotkey's hidden window proc runs
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG};

    let events = GlobalHotKeyEvent::receiver();
    let mut msg: MSG = unsafe { core::mem::zeroed() };
    loop {
        match unsafe { GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) } {
            0 => return Ok(()),
            -1 => bail!("GetMessageW failed"),
            _ => unsafe {
                DispatchMessageW(&msg);
            },
        }
        while let Ok(e) = events.try_recv() {
            if e.state == HotKeyState::Pressed {
                dispatch(e.id);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn run<F: Fn(u32) + Send + Sync + 'static>(dispatch: F) -> Result<()> {
    // Carbon hotkeys reach the application event target, which only dispatches
    // under the app event loop, so a bare CFRunLoop never delivers them. Become a
    // UI-agent process (no Dock icon), then run that loop and take events via the
    // crate's callback (which the run loop, not our channel, drives).
    #[repr(C)]
    struct ProcessSerialNumber {
        high: u32,
        low: u32,
    }
    const K_CURRENT_PROCESS: u32 = 2;
    const TO_UI_ELEMENT: u32 = 4;
    unsafe extern "C" {
        fn TransformProcessType(psn: *const ProcessSerialNumber, kind: u32) -> i32;
        fn RunApplicationEventLoop();
    }

    let psn = ProcessSerialNumber {
        high: 0,
        low: K_CURRENT_PROCESS,
    };
    unsafe { TransformProcessType(&psn, TO_UI_ELEMENT) };

    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            dispatch(e.id);
        }
    }));
    unsafe { RunApplicationEventLoop() };
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run<F: Fn(u32) + Send + Sync + 'static>(_dispatch: F) -> Result<()> {
    bail!("`monctl listen` only works on Windows and macOS");
}

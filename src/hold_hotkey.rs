//! Windows-only, non-consuming, foreground-scoped hold bindings for modes.
use crate::config::{Config, GateKey, gate_key, hold_key};
use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetWindowThreadProcessId, HHOOK, MSG,
    MSLLHOOKSTRUCT, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_NOREMOVE, PM_REMOVE,
    PeekMessageW, PostThreadMessageW, QS_ALLINPUT, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_MOUSE_LL, WM_MOUSEHWHEEL, WM_QUIT,
};

static WATCH_THREAD: AtomicU32 = AtomicU32::new(0);

// Let the message loop unwind normally, so workers apply their off actions.
unsafe extern "system" fn console_control(event: u32) -> i32 {
    if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        let thread = WATCH_THREAD.load(Ordering::SeqCst);
        if thread != 0 {
            return unsafe { PostThreadMessageW(thread, WM_QUIT, 0, 0) };
        }
    }
    0
}

pub struct Watcher {
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl Watcher {
    pub fn start(
        config: &Config,
        model: Option<&str>,
        states: Arc<Mutex<BTreeMap<String, bool>>>,
        paused: Arc<AtomicBool>,
    ) -> Result<Self> {
        // Ensure this thread has a message queue before Ctrl+C can post to it.
        let mut msg: MSG = unsafe { core::mem::zeroed() };
        unsafe { PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
        WATCH_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        let mut watcher = Self {
            stop: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
        };
        if unsafe { SetConsoleCtrlHandler(Some(console_control), 1) } == 0 {
            return Err(std::io::Error::last_os_error())
                .context("registering hold hotkey shutdown handler");
        }
        let mut modes: BTreeMap<String, Vec<HoldInput>> = BTreeMap::new();
        for binding in &config.hold_hotkeys {
            let name = config
                .mode(&binding.mode)
                .context("unknown hold mode")?
                .0
                .to_owned();
            let input = HoldInput {
                process: binding.process.clone(),
                keys: binding
                    .keys
                    .iter()
                    .map(|key| hold_key(key))
                    .collect::<Result<_>>()?,
                gate: binding
                    .enable_on
                    .as_ref()
                    .map(|on| -> Result<GateInput> {
                        Ok(GateInput {
                            on: gate_key(on)?,
                            off: gate_key(
                                binding
                                    .disable_on
                                    .as_deref()
                                    .context("missing disable_on")?,
                            )?,
                            enabled: false,
                            on_down: false,
                            off_down: false,
                        })
                    })
                    .transpose()?,
            };
            println!(
                "hold {:?} -> mode {} while {} is focused; enable_on: {:?}, disable_on: {:?}",
                binding.keys, name, binding.process, binding.enable_on, binding.disable_on
            );
            modes.entry(name).or_default().push(input);
        }
        for (name, mut inputs) in modes {
            // Track input separately so USB waits cannot delay selector changes.
            let active = Arc::new(AtomicBool::new(false));
            let sampled_active = active.clone();
            let input_stop = watcher.stop.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
            watcher.workers.push(
                std::thread::Builder::new()
                    .name(format!("hold-input-{name}"))
                    .spawn(move || {
                        let hook = if inputs
                            .iter()
                            .any(|input| input.gate.as_ref().is_some_and(GateInput::uses_wheel))
                        {
                            WheelHook::install().map(Some)
                        } else {
                            Ok(None)
                        };
                        let _hook = match hook {
                            Ok(hook) => hook,
                            Err(e) => {
                                let _ = ready_tx.send(Err(e));
                                return;
                            }
                        };
                        let _ = ready_tx.send(Ok(()));
                        let mut next_sample = Instant::now();
                        while !input_stop.load(Ordering::SeqCst) {
                            let events = wheel_events();
                            if !events.is_empty() || Instant::now() >= next_sample {
                                // Sample every binding even when an earlier one
                                // is active, so its selectors keep tracking input.
                                let desired = inputs
                                    .iter_mut()
                                    .map(|input| input.sample(&events))
                                    .fold(false, |any, active| any | active);
                                sampled_active.store(desired, Ordering::SeqCst);
                                next_sample = Instant::now() + Duration::from_millis(10);
                            }
                            // Low-level hooks run synchronously on this thread.
                            // Wake to forward mouse events immediately, while
                            // limiting ordinary key/focus sampling to 100 Hz.
                            let timeout = next_sample
                                .saturating_duration_since(Instant::now())
                                .as_millis()
                                .max(1) as u32;
                            let result = unsafe {
                                MsgWaitForMultipleObjectsEx(
                                    0,
                                    core::ptr::null(),
                                    timeout,
                                    QS_ALLINPUT,
                                    MWMO_INPUTAVAILABLE,
                                )
                            };
                            if result == WAIT_FAILED {
                                eprintln!(
                                    "hold input wait failed: {}",
                                    std::io::Error::last_os_error()
                                );
                                break;
                            }
                        }
                        sampled_active.store(false, Ordering::SeqCst);
                    })
                    .context("starting hold input sampler")?,
            );
            ready_rx
                .recv()
                .context("hold input sampler exited during startup")??;
            let config = config.clone();
            let model = model.map(str::to_owned);
            let stop = watcher.stop.clone();
            let states = states.clone();
            let paused = paused.clone();
            watcher.workers.push(
                std::thread::Builder::new()
                    .name(format!("hold-{name}"))
                    .spawn(move || {
                        watch(
                            &config,
                            model.as_deref(),
                            &name,
                            &active,
                            &stop,
                            &states,
                            &paused,
                        )
                    })
                    .context("starting hold hotkey watcher")?,
            );
        }
        Ok(watcher)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        WATCH_THREAD.store(0, Ordering::SeqCst);
        unsafe { SetConsoleCtrlHandler(Some(console_control), 0) };
    }
}

fn foreground_window(expected: &str) -> Option<isize> {
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut path = [0u16; 32768];
        let mut len = path.len() as u32;
        let success = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if success == 0 || GetForegroundWindow() != window {
            return None;
        }
        String::from_utf16_lossy(&path[..len as usize])
            .rsplit('\\')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(expected))
            .then_some(window as isize)
    }
}

#[derive(Default)]
struct HoldInput {
    process: String,
    keys: Vec<i32>,
    gate: Option<GateInput>,
}

impl HoldInput {
    fn desired(&self, held: bool, focused: bool) -> bool {
        focused && held && self.gate.as_ref().is_none_or(|gate| gate.enabled)
    }

    fn sample(&mut self, events: &[WheelEvent]) -> bool {
        // Only the high bit is reliable. Polling does not reserve or swallow
        // the key, and also catches releases after focus changes.
        let any_held = |keys: &[i32]| keys.iter().any(|key| unsafe { GetAsyncKeyState(*key) < 0 });
        let held = any_held(&self.keys);
        let window = foreground_window(&self.process);
        if let Some(gate) = &mut self.gate {
            for event in events {
                gate.select(event.key, window == Some(event.window));
            }
            // Keyboard/button selectors are edge-triggered. Holding an on key
            // must not undo a later off event. Off wins simultaneous presses.
            let down = |key| match key {
                GateKey::Key(vk) => unsafe { GetAsyncKeyState(vk) < 0 },
                _ => false,
            };
            let on_down = down(gate.on);
            let off_down = down(gate.off);
            if on_down && !gate.on_down {
                gate.select(gate.on, window.is_some());
            }
            if off_down && !gate.off_down {
                gate.select(gate.off, window.is_some());
            }
            gate.on_down = on_down;
            gate.off_down = off_down;
        }
        self.desired(held, window.is_some())
    }
}

struct GateInput {
    on: GateKey,
    off: GateKey,
    enabled: bool,
    on_down: bool,
    off_down: bool,
}

impl GateInput {
    fn select(&mut self, key: GateKey, focused: bool) {
        if focused {
            if key == self.on {
                self.enabled = true;
            }
            if key == self.off {
                self.enabled = false;
            }
        }
    }

    fn uses_wheel(&self) -> bool {
        !matches!(self.on, GateKey::Key(_)) || !matches!(self.off, GateKey::Key(_))
    }
}

struct WheelEvent {
    key: GateKey,
    window: isize,
}

thread_local! {
    static WHEEL_EVENTS: RefCell<Vec<WheelEvent>> = const { RefCell::new(Vec::new()) };
}

fn wheel_direction(data: u32) -> Option<GateKey> {
    match ((data >> 16) as i16).cmp(&0) {
        std::cmp::Ordering::Greater => Some(GateKey::WheelRight),
        std::cmp::Ordering::Less => Some(GateKey::WheelLeft),
        std::cmp::Ordering::Equal => None,
    }
}

unsafe extern "system" fn mouse_hook(code: i32, message: usize, data: isize) -> isize {
    if code >= 0 && message == WM_MOUSEHWHEEL as usize {
        let event = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
        if let Some(key) = wheel_direction(event.mouseData) {
            let window = unsafe { GetForegroundWindow() } as isize;
            WHEEL_EVENTS.with(|events| events.borrow_mut().push(WheelEvent { key, window }));
        }
    }
    // Observe only; scrolling and mouse buttons still reach the game.
    unsafe { CallNextHookEx(core::ptr::null_mut(), code, message, data) }
}

struct WheelHook(HHOOK);

impl WheelHook {
    fn install() -> Result<Self> {
        let hook = unsafe {
            SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_hook),
                GetModuleHandleW(core::ptr::null()),
                0,
            )
        };
        if hook.is_null() {
            return Err(std::io::Error::last_os_error())
                .context("installing horizontal wheel hook");
        }
        Ok(Self(hook))
    }
}

impl Drop for WheelHook {
    fn drop(&mut self) {
        unsafe { UnhookWindowsHookEx(self.0) };
    }
}

fn wheel_events() -> Vec<WheelEvent> {
    let mut msg: MSG = unsafe { core::mem::zeroed() };
    while unsafe { PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        unsafe { DispatchMessageW(&msg) };
    }
    WHEEL_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

fn watch(
    config: &Config,
    model: Option<&str>,
    name: &str,
    active: &AtomicBool,
    stop: &AtomicBool,
    states: &Mutex<BTreeMap<String, bool>>,
    paused: &AtomicBool,
) {
    let mode = &config.modes[name];
    let publish = |enabled| {
        let mut states = states.lock().expect("mode state poisoned");
        states.insert(name.to_owned(), enabled);
        paused.store(
            config
                .modes
                .iter()
                .any(|(name, mode)| mode.pause_usb && states.get(name).copied().unwrap_or(false)),
            Ordering::SeqCst,
        );
    };
    let mut applied = None;
    let mut retry_at = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        if Instant::now() >= retry_at {
            // Unknown/partially-applied state must be restored before another
            // activation. This also initializes the configured mode to off.
            let desired = applied.is_some() && active.load(Ordering::SeqCst);
            if applied != Some(desired) {
                if desired || (applied.is_none() && mode.pause_usb) {
                    publish(true); // pause before activation or uncertain restoration
                }
                let actions = if desired { &mode.on } else { &mode.off };
                let result = crate::action::run_while(config, model, actions, || {
                    !desired || (!stop.load(Ordering::SeqCst) && active.load(Ordering::SeqCst))
                });
                match result {
                    Ok(true) => {
                        applied = Some(desired);
                        publish(desired);
                    }
                    Ok(false) => applied = None, // hold ended during on; run off next
                    Err(e) => {
                        eprintln!(
                            "hold mode {} {} failed: {e:#}",
                            name,
                            if desired { "on" } else { "off" }
                        );
                        applied = None;
                        retry_at = Instant::now() + Duration::from_secs(1);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if applied != Some(false) {
        match crate::action::run(config, model, &mode.off) {
            Ok(()) => publish(false),
            Err(e) => eprintln!("restoring hold mode {} at shutdown failed: {e:#}", name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GateInput, GateKey, HoldInput, wheel_direction};

    fn input() -> HoldInput {
        HoldInput {
            process: "game.exe".into(),
            keys: vec![2],
            gate: Some(GateInput {
                on: GateKey::WheelRight,
                off: GateKey::WheelLeft,
                enabled: false,
                on_down: false,
                off_down: false,
            }),
        }
    }

    #[test]
    fn scroll_changes_scope_during_right_click_hold() {
        let mut input = input();
        assert!(!input.desired(true, true)); // disabled at startup
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelRight, true);
        assert!(input.desired(true, true)); // no new mouse press needed
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelLeft, true);
        assert!(!input.desired(true, true)); // still holding right click
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelRight, true);
        assert!(input.desired(true, true));
    }

    #[test]
    fn selection_persists_between_mouse_holds() {
        let mut input = input();
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelRight, true);
        assert!(!input.desired(false, true)); // scrolling alone is not a hold
        assert!(input.desired(true, true));
        assert!(!input.desired(false, true));
        assert!(input.desired(true, true)); // next aim uses last selection
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelLeft, true);
        assert!(!input.desired(false, true));
        assert!(!input.desired(true, true));
    }

    #[test]
    fn f11_is_independent_and_never_latches() {
        let f11 = HoldInput::default();
        let mut input = input();
        assert!(f11.desired(true, true));
        assert!(!input.desired(true, true));
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelRight, true);
        assert!(input.desired(true, true));
        assert!(!f11.desired(false, true)); // F11 release leaves mouse binding active
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelLeft, true);
        assert!(f11.desired(true, true)); // disabling mouse binding leaves F11 active
        assert!(!input.desired(true, true));
        assert!(!f11.desired(false, true)); // no latch after release
        assert!(!f11.desired(true, false)); // no activation outside game
        assert!(!input.desired(false, true));
    }

    #[test]
    fn focus_limits_activation_and_selector_events() {
        let mut input = input();
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelRight, false);
        assert!(!input.desired(true, true)); // outside scroll ignored
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelRight, true);
        assert!(!input.desired(true, false)); // no scope outside game
        input
            .gate
            .as_mut()
            .unwrap()
            .select(GateKey::WheelLeft, false);
        assert!(input.desired(true, true)); // last in-game selection retained
    }

    #[test]
    fn horizontal_wheel_direction_uses_signed_high_word() {
        assert_eq!(wheel_direction(120 << 16), Some(GateKey::WheelRight));
        assert_eq!(
            wheel_direction(((-120i16) as u16 as u32) << 16),
            Some(GateKey::WheelLeft)
        );
        assert_eq!(wheel_direction(1 << 16), Some(GateKey::WheelRight));
        assert_eq!(wheel_direction(0), None);
    }
}

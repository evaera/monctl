# monkey
(mon-key)

Switches an external monitor's active input over DDC/CI (VCP `0x60`). One small
binary for Windows and macOS, instead of flaky vendor OSD utilities. The
motivating case is two machines sharing one monitor, one on DisplayPort and one
on USB-C.

## build

```
cargo build --release
```

## use

```
monkey <input>        switch by name or raw value, e.g. `monkey usbc`, `monkey 16`
monkey toggle         flip between the two `toggle` inputs
monkey mode <n> on|off  apply one side of a configured stateful mode
monkey hdr [on|off]    read or change Windows HDR for the selected display
monkey read           current input value
monkey list           every display and its current input
monkey vendor msi ... vendor-specific control; see `vendors` below
monkey watch          daemon: global hotkeys plus the [usb] watcher (alias: listen)
monkey usb            list connected USB devices (to find a VID:PID)
monkey startup        run the daemon at login (--remove to undo)
```

`-m/--model <substr>` picks the display by EDID model name; `-c/--config <path>`
points at a specific file. Exit codes: 0 ok, 1 DDC/config failure, 2 bad args.

`startup` registers `watch` at login: an `HKCU\...\Run` entry on Windows, a
LaunchAgent on macOS, pinned to the config in use, windowless on Windows.

## config

`monkey.toml` is found by walking up from the working directory; `--config` and
`$MONKEY_CONFIG` override.

```toml
model = "MPG"            # EDID model substring used to pick the display
toggle = ["dp", "usbc"]  # what `toggle` flips between

[inputs]                 # name -> VCP 0x60 value; find them with `monkey list`
dp = 15
usbc = 16

[hotkeys]                # combo -> input, for `monkey watch`
"ctrl+alt+1" = "dp"
"ctrl+alt+2" = "usbc"

# Hotkeys can also target a named mode. Actions run in order; MSI actions
# talk to the monitor's scaler over its USB upstream cable. Raw DDC features
# and input actions make the same mechanism useful with other monitor brands.
"ctrl+alt+p" = "pbp"

[modes.pbp]
pause_usb = true         # ignore [usb] changes while this mode is on
on = [
  { msi = { command = "adaptive_sync", value = 0 } },
  { wait_ms = 3000 },
  { msi = { command = "dsc", value = 0 } },
  { wait_ms = 3000 },
  { msi = { command = "pip_pbp", value = 2 } },
]
off = [
  { msi = { command = "pip_pbp", value = 0 } },
  { wait_ms = 3000 },
  { msi = { command = "dsc", value = 1 } },
  { wait_ms = 3000 },
  { msi = { command = "adaptive_sync", value = 1 } },
]

[usb]                    # follow a USB device across machines
device = "3297:1977"     # VID:PID, from `monkey usb`
on_connect = "dp"        # switch here when it appears
on_disconnect = "usbc"   # hand off when it leaves
wake_on_connect = true   # wake this machine's screen first
# wake_on_disconnect = false
# wake_settle_ms = 500   # panel light-up time before the switch
```

The same panel can appear once per backend in `list` (a winapi "Generic PnP
Monitor" and an nvapi/macos entry with the real name); `model` picks which one
gets driven, so if a switch does not take, try the other name.

## two machines

Most monitors only accept DDC on the input currently on screen, so a machine
that is not showing cannot pull the monitor to itself. Bind each machine to
switch *away*: the DisplayPort machine runs `usbc`, the USB-C machine runs
`dp`. Point `[usb]` at the keyboard on a KM switch and the video follows the
keyboard. `wake_on_connect` wakes the gaining machine's screen first; asleep,
it offers no signal and the monitor bounces straight back.

## modes

Mode actions may be `{ msi = { command = "dsc", value = 0 } }`,
`{ windows_hdr = false }`,
`{ ddc = { feature = 96, value = 15 } }`, `{ input = "dp" }`, or
`{ wait_ms = 500 }`. Vendor actions like `msi` are described under
`vendors`.

`pause_usb` pauses the `[usb]` watcher before the first `on` action, keeps it
paused for the complete `off` sequence, then resynchronizes the current USB
state.

## vendors

`monkey vendor <name> ...` holds vendor-specific control that DDC/CI cannot
reach; each vendor also contributes a mode action of the same name. MSI is
the first; adding another means one module under `src/vendor/`, a
`VendorCommand` variant, and (for modes) a config action.

### msi

Speaks the scaler's own protocol over the monitor's USB upstream cable (HID,
vid 1462), the same wire format Gaming Intelligence uses, so GI does not need
to be running or installed.

```
monkey vendor msi list           the known settings
monkey vendor msi probe          find the monitor's USB control interface
monkey vendor msi get pip_pbp    read a setting
monkey vendor msi set pip_pbp 2  write one
```

Settings include `pip_pbp` (0 off, 1 PIP, 2 PBP), `pbp_layout`,
`pbp_source_1`, `pbp_source_2`, `dsc`, `adaptive_sync`, `input`, `kvm`, and
`monitor_hdr`. Sets are fire-and-forget (the scaler only acknowledges
reads), so `set` prints the read-back value as confirmation. PBP source
values use MSI's input order (HDMI1=0, HDMI2=1, DP=2, Type-C=3), not the DDC
`[inputs]` numbers.

## license

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).

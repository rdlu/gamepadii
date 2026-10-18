# Milestone 8b: a GP2040-CE control channel

Status: v1 implemented in the firmware fork, branch `gamepadii/control-channel`
(2026-10-17). Decisions from issue #2 are folded in (§9). Deviations from the
draft are listed in §3.6, "Changes during implementation". v2 (§3.5) is next.

Firmware read at `v0.7.12` (the S16's build, `0014e4a`), at our branch
`gamepadii/freeze-investigation` (v0.7.12 + freeze fixes) and at `main`
(`8013b41`). TinyUSB is the speedypotato fork that the v0.7.12 submodule pins
(`9865cba`). The kernel code is read from `drivers/usb/core/devio.c` in Linux
master (7.2 series).

## 1. Goal

While the pad is in a gamepad mode (XInput, Keyboard, HID, Switch, PS4/5 and
the rest), gamepadii should be able to:

- read status
- switch the active profile
- set the input mode
- reboot into gamepad or web-config (not BOOTSEL; decided in #2, §9)

It should do all of this with no button press. That removes the guided
"press S2+B3+B4" step from `mode`, `webconfig` and `reboot`.

Hard constraints:

- **No descriptor changes in any mode.** No new interface, no MS OS
  descriptors and no new strings. Consoles must see exactly what they see
  today.
- Any vendor request that isn't ours behaves **exactly as it does today**.
  It goes to the active driver unchanged, which STALLs it, ACKs it, or
  answers it as before (§2.3).
- The host must not have to detach or claim any kernel driver: xpad,
  hid-generic, hid-sony/hid-playstation or hid-nintendo stay bound and the
  pad keeps working during a request.

## 2. Findings

### 2.1 Where vendor requests go (TinyUSB)

In `lib/tinyusb/src/device/usbd.c`, `process_control_request()` sends
**every** request of type VENDOR to `tud_vendor_control_xfer_cb`. This
happens before the switch on recipient, so a vendor request goes there
whether its recipient is the device, an interface or an endpoint. The same
function is installed as the completion callback, so it is also called for
`CONTROL_STAGE_DATA` and `CONTROL_STAGE_ACK`.

```c
if ( p_request->bmRequestType_bit.type == TUSB_REQ_TYPE_VENDOR ) {
  usbd_control_set_complete_callback(tud_vendor_control_xfer_cb);
  return tud_vendor_control_xfer_cb(rhport, CONTROL_STAGE_SETUP, p_request);
}
```

GP2040's `src/usbdriver.cpp:66` forwards the call to
`DriverManager::getDriver()->vendor_control_xfer_cb()`. A return of `false`
makes TinyUSB STALL EP0. The hook goes in front of that forward, so a request
that isn't ours reaches the driver byte for byte as it does today.

The control endpoint, from `headers/tusb_config.h` and `usbd_control.c`:

- `CFG_TUD_ENDPOINT0_SIZE` is 64 and every driver's `bMaxPacketSize0` is 64.
  The board is full speed: the RP2040 has no high-speed PHY.
- TinyUSB's own EP0 bounce buffer `_usbd_ctrl_buf` holds 64 bytes. With
  `tud_control_xfer()`, a longer transfer works packet by packet from a
  caller-owned buffer, which must stay valid until the transfer completes.
  v1 keeps every request and response at **64 bytes or less** (one packet),
  with static buffers. v1.4 adds the first longer transfers: READ_SLOT and
  WRITE_SLOT move one slot (§3.10) in one static buffer, packet by packet.
- For an OUT request with data, the callback gets `CONTROL_STAGE_DATA` once
  all the data has arrived. If it returns `false` there, the status stage
  STALLs, so a bad payload is reported to the host as an error.

Threading: `tud_task()` runs on core0, from the main loop (`gp2040.cpp:338`)
and in web-config from `rndis_task()`. Vendor callbacks therefore run on core0,
on the same thread as `gamepad->hotkey()` and `checkSaveRebootState()`. The
web-config HTTP handlers already raise `GPStorageSaveEvent` and
`GPRestartEvent` from this same context, and `EventManager::triggerEvent` is
synchronous. The pattern is established.

### 2.2 Linux: no claim, no detach

`drivers/usb/core/devio.c` handles both paths: `do_proc_control()` (ioctl
`USBDEVFS_CONTROL`) and `proc_do_submiturb()` (`USBDEVFS_URB_TYPE_CONTROL`,
which libusb uses). Each calls `check_ctrlrecip()` first:

```c
static int check_ctrlrecip(struct usb_dev_state *ps, unsigned int requesttype,
                           unsigned int request, unsigned int index)
{
        ...
        if (USB_TYPE_VENDOR == (USB_TYPE_MASK & requesttype))
                return 0;
        ...
        switch (requesttype & USB_RECIP_MASK) {
        case USB_RECIP_ENDPOINT: ... checkintf(ps, ret) ...
        case USB_RECIP_INTERFACE: ret = checkintf(ps, index);
```

For a **vendor** request it returns 0 before any interface check, whatever
the recipient. `checkintf()` is what would implicitly claim the interface (and
fail with `-EBUSY` when xpad or usbhid owns it), and it is never reached, so no
"did not claim interface" warning appears either.

A device-recipient vendor request therefore needs only an open fd on
`/dev/bus/usb/BBB/DDD`. The kernel driver stays bound, and the kernel
serialises our URB on ep0 with its own.

There is one requirement. `usbdev_ioctl` → `usbdev_do_ioctl` returns `-EPERM`
unless the file was opened with `FMODE_WRITE`, so the node has to be opened
`O_RDWR`. On this machine the nodes are `crw-rw-r-- root root`
(`50-udev-default.rules`), so a normal user needs a `uaccess` rule (§5.3).
Steam's `60-steam-input.rules` only grants usb-node access for 28de (Valve),
so it doesn't cover us.

### 2.3 What every driver does with vendor requests today

| Driver (mode) | `vendor_control_xfer_cb` today | Notes |
|---|---|---|
| XInput (0) | **Accepts every vendor request** when the auth driver is `available()`. That is the default: `xinputAuthType` defaults to `NONE`, and `XInputAuth::available()` is true for anything but `USB`, so XSM3 runs. An unknown IN gets a 0-byte data stage; an unknown OUT is ACKed. | Handles XSM3 bRequest `0x81–0x84, 0x86, 0x87` (the 360 console sends these as `0xC1/0x41`, interface-recipient, wIndex `0x0103`). xpad's probe sends `0xC1 0x01 wValue 0x0100` and today gets a 0-byte reply. |
| XBOne (5) | **Accepts every vendor request**. An IN gets `wLength` bytes of an *uninitialised* 255-byte stack buffer, except MS OS 1.0 `0xC0 0x20 wIndex 4` (Extended Compat ID). | String `0xEE` "MSFT100" sets vendor code `0x20`. |
| Switch 2 Pro (main only) | Handles bRequest `0x02, 0x03` (IN) and `0x04` (OUT); STALLs anything else. | Not in v0.7.12. |
| Xbox OG (12) | Forwards to `xid_control_xfer_cb`, which handles `0xC1 0x06 / 0x01` (XID descriptor and capabilities) and STALLs anything else. | Interface recipient. |
| PS3, PS4/PS5, Switch, Switch Pro, Keyboard, HID, MD-Mini, NeoGeo, PCE, Egret, Astro, PSClassic, SInput/P5General (main), Net (web-config) | `return false` → STALL. | PS4/PS5 auth uses **HID class** GET/SET_REPORT (feature reports) and Switch Pro uses interrupt-endpoint reports. Neither is vendor type, so neither reaches this callback. |

So "unknown vendor requests STALL" is only true for most modes. The real rule
is: **anything that doesn't match our request takes the existing path,
unchanged.** XInput and XBOne carry on ACKing whatever they ACK today.

This matters for detection on the host side. On stock firmware in XInput or
XBOne mode, a request that looks like ours does *not* STALL: it gets 0 bytes
or garbage. The HELLO reply therefore has to prove itself, with a magic value
and a nonce echo (§3.3).

Bytes already in use that we avoid:

- bRequest `0x01–0x06` (XID, xpad, Switch 2 Pro, and the codes TinyUSB's
  WebUSB/MS OS 2.0 examples use)
- `0x20` (MS OS 1.0 vendor code)
- `0x81–0x87` (XSM3)

### 2.4 Firmware mechanics we reuse

- **Profile switch** (`gamepad.cpp`, `HOTKEY_LOAD_PROFILE_n`):
  `Storage::setProfile(n)` checks that the profile exists and is enabled, then
  sets `config.gamepadOptions.profileNumber` in RAM. The main loop's
  `GP2040::getReinitGamepad()` sees the change on its next pass and does the
  rest:
  - de-initialises the standard GPIO
  - calls `setFunctionalPinMappings()` and re-initialises the GPIO
  - calls `gamepad->reinit()` and `addons.ReinitializeAddons()`
  - fires `GPProfileChangeEvent` (the OLED shows it)

  The hotkey then fires `GPStorageSaveEvent(true)`, so **the hotkey path writes
  flash on every switch.** The live switch itself needs no flash write, which
  makes a RAM-only switch possible (§3.4). `main` is identical.
- **Saves:** `GPStorageSaveEvent(force, restartAfterSave)` →
  `GP2040::handleStorageSave` only sets flags. `checkSaveRebootState()` at the
  end of the loop runs `Storage::save(force)`, which goes to
  `ConfigUtils::save`. That function asserts core0, encodes nanopb into
  `EEPROM.writeCache` and writes flash. `force=true` skips the "PS4/PS5 with a
  USB auth dongle" guard, as the hotkeys and web-config do.
- **Reboot:** `GPRestartEvent(mode)` → `handleSystemReboot` sets
  `rebootRequested` and `rebootMode`. `checkSaveRebootState()` arms a 500 ms
  timer and then calls `System::reboot(mode)`. That function writes the mode
  into `watchdog_hw->scratch[5]` (GAMEPAD `0x43d566cd`, WEBCONFIG `0xe77784a5`,
  USB/BOOTSEL `0xf737e4e1`) and resets through the watchdog.
  `System::takeBootMode()` reads it back on the next boot.
  - BootMode **GAMEPAD** skips the held-button scan entirely (`getBootAction`
    returns `NONE`).
  - DEFAULT runs the scan, and on testboot builds goes to web-config.
  - A save event resets `rebootMode` to DEFAULT and overwrites
    `rebootRequested`. A restart event raised *after* the save event, in the
    same callback, wins.
- **Forced setup locks** (`forcedSetupOptions.mode`):
  - `LOCK_MODE_SWITCH` (1)
  - `LOCK_WEB_CONFIG` (2)
  - `LOCK_BOTH` (3)

  The channel honours them (§3.4).

## 3. Wire protocol v1

### 3.1 The setup packet

| Field | IN (read) | OUT (command) |
|---|---|---|
| `bmRequestType` | `0xC0` (device-to-host, vendor, device) | `0x40` (host-to-device, vendor, device) |
| `bRequest` | `0x47` (`'G'`) | `0x47` |
| `wValue` | per-command argument (nonce, string id, offset) | `0` |
| `wIndex` | `0x50nn`: high byte `0x50` (`'P'`) is the magic, low byte `nn` is the command | same |
| `wLength` | at most 64; the host asks for 64 and the reply is short | payload length, 8–64 |

The firmware **matches** a request only when all of these hold:

- the channel is enabled
- `bmRequestType ∈ {0xC0, 0x40}`
- `bRequest == 0x47`
- `wIndex >> 8 == 0x50`
- the direction bit agrees with the command's direction

Anything else goes to the driver unchanged. A matched request that is
malformed (an unknown command, the wrong length, a bad token) STALLs. That is
safe, because the only devices that produce such a request are our own host
or a fuzzer. The match is stateless, so the DATA and ACK stage callbacks
re-run it on the stored request and route to the same handler.

Multi-byte fields are little-endian. Strings are UTF-8 with no NUL.

### 3.2 Commands

| Cmd | Dir | Name | wValue | Payload or reply | Since |
|---|---|---|---|---|---|
| `0x01` | IN | HELLO | host nonce (u16) | reply: 20 bytes (§3.3) | v1 |
| `0x02` | IN | STATUS | 0 | reply: 16 bytes (§3.3) | v1 |
| `0x03` | IN | STRING | string id | reply: raw string, ≤ 63 bytes | v1 |
| `0x04` | IN | LED_STATUS | 0 | reply: 12 bytes (§3.7); v0.7.12 line only | v1.1 |
| `0x05` | IN | LIGHTS_STATUS | 0 | reply: 16 bytes (§3.8); main line | v1.2 |
| `0x06` | IN | READ_SLOT | profile (1–6) | reply: one slot, 20 + 10 × pin count bytes (§3.10); `wLength` 512 | v1.4 |
| `0x07` | IN | READ_LIGHTS | lights profile (0–3) | reply: a lights profile; host side not done | v1.5 |
| `0x10` | OUT | SET_PROFILE | 0 | header + `u8 profile (1–6)`, `u8 persist (0/1)` | v1 |
| `0x11` | OUT | REBOOT | 0 | header + `u8 boot_mode` (0 gamepad, 1 web-config; the `/api/reboot` numbering, but 2 = BOOTSEL is refused with UNSUPPORTED) | v1 |
| `0x12` | OUT | SET_INPUT_MODE | 0 | header + `u8 input_mode` (the `InputMode` enum), `u8 reserved` | v1 |
| `0x13` | OUT | SET_LED | 0 | header + 8 bytes: mask, values, persist (§3.7); v0.7.12 line only | v1.1 |
| `0x14` | OUT | SET_LIGHTS | 0 | header + 4 bytes: mask, profile, brightness, persist (§3.8); main line | v1.2 |
| `0x15` | OUT | WRITE_SLOT | 0 | header + one slot (§3.10) | v1.4 |
| `0x16` | OUT | WRITE_LIGHTS | 0 | header + a lights profile; host side not done | v1.5 |
| `0x20` | OUT | CFG_SELECT | 0 | header + `u8 section`, `u8 op (read/write)` | v2 |
| `0x21` | IN | CFG_READ | byte offset | reply: up to 64 bytes of the encoded section | v2 |
| `0x22` | OUT | CFG_WRITE | byte offset | header + chunk | v2 |
| `0x23` | OUT | CFG_COMMIT | 0 | header + `u16 size`, `u32 crc32` | v2 |

Every OUT payload starts with an **8-byte header**:

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | session token (from HELLO) |
| 4 | u16 | sequence number (chosen by the host, echoed in STATUS) |
| 6 | u16 | reserved, 0 |

STRING ids:

| Id | Value |
|---|---|
| 1 | `GP2040VERSION` (for example `v0.7.12-5-g85ca0f6`) |
| 2 | `GP2040_BOARDCONFIG` (`Haute42COSMOX`) |
| 3 | `BOARD_CONFIG_LABEL` (`Haute42 COSMOX`) |
| 4 | `GP2040CONFIG` (the build type) |
| 5 | v1.3: the unique ID of the flash chip, 16 upper-case hex digits (§3.9). Older firmware STALLs it |
| `0x11–0x16` | label of profile 1–6 |
| `0x20 + n` | v1.1: the firmware's name for animation effect `n` (`EFFECT_STATIC_COLOR`, ...); `n` ≥ the effect count STALLs |
| `0x30 + n` | v1.2: reserved for the name of lights profile `n`. Main has no name for a lights profile, so these STALL |

### 3.3 Replies

**HELLO** (20 bytes):

| Offset | Type | Field |
|---|---|---|
| 0 | `[u8;4]` | magic `"GPii"` (`47 50 69 69`) |
| 4 | u16 | echo of the host nonce (`wValue`) |
| 6 | u8 | protocol major (1); a host refuses a major it doesn't know |
| 7 | u8 | protocol minor (0); fields only ever get added |
| 8 | u32 | capability bitmap (below) |
| 12 | u32 | session token: random per boot (`time_us_32()` mixed with the unique board id) |
| 16 | u32 | bitmap of `InputMode` values 0–31 this build can boot |

The magic plus the nonce echo is the handshake. On stock XInput firmware a
HELLO gets a 0-byte reply, and on stock XBOne it gets uninitialised stack
bytes. Neither can carry the magic *and* echo a fresh random nonce, so the
host never mistakes them for the channel. The token makes an OUT command
require a prior, successful HELLO: something that sends random vendor
requests would have to guess 32 bits on top of the 16-bit `bRequest`/`wIndex`
magic.

Capability bits:

| Bit | Capability |
|---|---|
| 0 | STATUS |
| 1 | STRING |
| 2 | SET_PROFILE |
| 3 | REBOOT (gamepad / web-config) |
| 4 | REBOOT to BOOTSEL (reserved; never set, see §9) |
| 5 | SET_INPUT_MODE |
| 6 | CFG_READ |
| 7 | CFG_WRITE |
| 8 | LEDS: LED_STATUS and SET_LED (v1.1, §3.7; the v0.7.12 line only) |
| 9 | LIGHTS: LIGHTS_STATUS and SET_LIGHTS (v1.2, §3.8; the main line) |
| 10 | SLOTS: READ_SLOT and WRITE_SLOT (v1.4, §3.10) |
| 11 | LIGHTS_PROFILES: READ_LIGHTS and WRITE_LIGHTS (v1.5; set only when the LEDs run) |

Bits 12–31 are reserved.

**STATUS** (16 bytes):

| Offset | Type | Field |
|---|---|---|
| 0 | u8 | protocol major |
| 1 | u8 | input mode (`DriverManager::getInputMode()`; 255 means web-config) |
| 2 | u8 | active profile (`profileNumber`, 1–6) |
| 3 | u8 | enabled-profile bitmap (bit 0 is profile 1, always set) |
| 4 | u8 | `forcedSetupOptions.mode` |
| 5 | u8 | flags: bit 0 set when the profile change is not yet applied (`profileNumber != lastReinitProfileNumber`); bit 1 set once a REBOOT or SET_INPUT_MODE has been accepted and the reboot is pending (§3.6) |
| 6 | u16 | sequence number of the last OUT command |
| 8 | u8 | command id of the last OUT command |
| 9 | u8 | result of the last OUT command (below) |
| 10 | u16 | reserved |
| 12 | u32 | uptime in ms (`to_ms_since_boot`) |

Result codes:

| Code | Meaning |
|---|---|
| 0 | OK |
| 1 | BAD_TOKEN |
| 2 | BAD_LENGTH |
| 3 | BAD_ARG |
| 4 | LOCKED (forced setup) |
| 5 | UNSUPPORTED (mode not in this build) |
| 6 | PROFILE_DISABLED |
| 7 | BUSY |

A failed OUT command STALLs. The host then reads STATUS to learn why, and the
sequence number tells it which command the result belongs to.

### 3.4 Command semantics

Each OUT command is validated *and* applied in its `CONTROL_STAGE_DATA`
callback, as XInput's auth path does. A `false` return there STALLs the
status stage. Nothing slow runs in the callback: no flash writes and no GPIO
work. It only sets RAM state and raises events, which the main loop acts on.

- **SET_PROFILE:** calls `Storage::setProfile(n)`. If that returns `false`,
  the result is PROFILE_DISABLED and the request STALLs. With `persist=1` it
  also raises `GPStorageSaveEvent(true)`, which matches what the hotkey does.
  - With `persist=0` nothing is written to flash, and the pad goes back to
    the stored profile on its next boot. Use this for quick per-game
    switching and to save flash wear.
  - Caveat: the next save of any kind persists it anyway. Turbo, LED and
    hotkey saves all call `save()` on the whole config.
  - The reinit happens on the next main-loop pass (well under 1 ms at
    1000 Hz polling). STATUS flag bit 0 shows that it is still pending.
- **REBOOT:** maps 0, 1 and 2 to `System::BootMode::GAMEPAD`, `WEBCONFIG`
  and `USB`, then raises `GPRestartEvent`.
  - If the mode is web-config and `forcedSetupMode` is 2 or 3, the result is
    LOCKED. That lock exists to keep people out of web-config, and the
    channel must not be a way around it.
  - BOOTSEL (2) is refused with UNSUPPORTED, and capability bit 4 is never
    set (decision 2 in #2). gamepadii reaches BOOTSEL through REBOOT(1) and
    then `/api/reboot {"bootMode":2}`.
  - The reboot happens at least 500 ms after the status stage, so the host
    always sees its ACK before the device drops off the bus.
- **SET_INPUT_MODE:**
  - The mode must be in the HELLO mode bitmap and must not be
    `INPUT_MODE_CONFIG` (255); otherwise the result is UNSUPPORTED.
  - If `forcedSetupMode` is 1 or 3, the result is LOCKED. The lock
    disables mode switching at boot, so the channel obeys it too.
  - It sets `gamepadOptions.inputMode`, then raises
    `GPStorageSaveEvent(true)` followed by `GPRestartEvent(GAMEPAD)`. The
    restart comes second because the save handler resets `rebootMode` to
    DEFAULT. `checkSaveRebootState()` saves on the same pass and reboots
    500 ms later.
  - GAMEPAD boot mode skips the button scan, so a held button can't override
    the new mode, and a testboot build doesn't fall into web-config.
  - A driver can't be swapped live, so this command always reboots.

### 3.5 v2: config sections as nanopb protobuf (sketch)

This lets `profile apply` and `backup` work with no web-config at all.

- The section ids are field numbers of the top-level `Config` message: for
  example `gamepadOptions`, `gpioMappings`, `profileOptions`,
  `keyboardMapping`, `hotkeyOptions`.
- **Read:** CFG_SELECT encodes that sub-message into a static buffer (a few
  KB; nanopb is fast enough to do this in the callback). The host then pulls
  it with CFG_READ, using `wValue` as the offset.
- **Write:** CFG_SELECT, then CFG_WRITE chunks, then CFG_COMMIT with the size
  and a CRC32. The main loop does the rest, so the firmware needs a
  `ControlChannel::process()` call in `GP2040::run()`:
  1. decode into a scratch struct
  2. validate
  3. swap it into `Storage::config`
  4. raise `GPStorageSaveEvent(true)`

  The host polls STATUS for the result.
- A whole-section replace needs the same care as the `setGamepadOptions`
  hazard: the host sends complete sections, read-modify-write.
- RAM: on the RP2040 a 4 KB buffer is affordable in gamepad mode. Measure it
  with `getMemoryReport`.
- The `.proto` files must match between host and firmware. The host ships the
  firmware's `proto/` for each supported version and checks the version
  string first.

### 3.6 Changes during implementation

None of these changes the layout of a setup packet, a payload or a reply in
§3.1–§3.3. They pin down behaviour the draft left open, plus one added flag
bit. A host written against the draft keeps working.

1. **BOOTSEL removed** (decision in #2). REBOOT(2) is refused with
   UNSUPPORTED and capability bit 4 is never set. v1 capabilities on the fork
   are `0x0000002F` (STATUS, STRING, SET_PROFILE, REBOOT, SET_INPUT_MODE).
2. **New STATUS flag, bit 1: reboot pending.** Set once a REBOOT or
   SET_INPUT_MODE has been accepted. From then on every OUT command fails
   with **BUSY** (7) until the device reboots, so a second command can't race
   the pending save or reboot (a save event would otherwise reset the reboot
   mode to DEFAULT, §2.4).
3. **HELLO mode bitmap** on v0.7.12 is `0x0000FFFF` (InputMode 0–15; every
   mode `DriverManager::setup()` knows). `INPUT_MODE_CONFIG` (255) is never in
   it.
4. **Session token:** made on the first HELLO after boot (`time_us_32()`
   XOR an FNV-1a hash of the board's unique id, through the murmur3
   finaliser) and then stays the same for the whole boot, so several host
   processes can share it. It is never 0.
5. **Match rule, exactly:** `bRequest == 0x47`, `wIndex >> 8 == 0x50`,
   `bmRequestType` is exactly `0xC0` or `0x40`, and, *for a known command*,
   the direction agrees. A known command in the wrong direction (for
   example HELLO sent as OUT) is not ours and goes to the driver. An
   *unknown* command with our magic (including the v2 ids `0x20–0x23`) is
   ours and STALLs.
6. **Lengths:**
   - any matched request with `wLength > 64` STALLs in SETUP;
   - IN replies are truncated to `wLength` (normal USB);
   - an OUT payload must be at least 8 bytes (the header), else it STALLs in
     SETUP; it must also cover the command's arguments (SET_PROFILE 10,
     REBOOT 9, SET_INPUT_MODE 10), else BAD_LENGTH; **bytes past the
     arguments are ignored**, so later minors can append fields.
7. **What STATUS records for a failed OUT:**
   - rejected in SETUP (`wLength` < 8 or > 64, or an unknown OUT command):
     `last_cmd` = the command byte, `last_seq` = 0 (the payload was never
     read), `last_result` = BAD_LENGTH, or UNSUPPORTED for an unknown command;
   - rejected in DATA: `last_seq` from the payload, and the checks run in
     this order: token (BAD_TOKEN), pending reboot (BUSY), argument length
     (BAD_LENGTH), argument values (BAD_ARG / LOCKED / UNSUPPORTED /
     PROFILE_DISABLED).
   IN commands never touch these fields.
8. **Argument checks:** SET_PROFILE needs profile 1–6 and persist 0/1
   (else BAD_ARG), then `Storage::setProfile()` (false → PROFILE_DISABLED).
   SET_INPUT_MODE's `reserved` byte is ignored; a mode ≥ 32 or not in the
   bitmap is UNSUPPORTED. REBOOT's mode > 2 is BAD_ARG.
9. **STRING:** an unknown id STALLs. An empty value (an unnamed profile, or
   profile `n` beyond `gpioMappingsSets_count`) is a 0-byte reply. Values are
   cut at 63 bytes; profile labels are at most 16.
10. **Web-config mode** (decision in #2): the channel answers there too;
    STATUS byte 1 is 255. SET_PROFILE changes the RAM profile there as well,
    and SET_INPUT_MODE from web-config saves and reboots to the new gamepad
    mode.
11. **CMake:** `-DGP2040_CONTROL_CHANNEL=ON` adds `src/controlchannel.cpp`
    and the compile definition to the target only. With it off, the build is
    byte-identical to the freeze-fix build apart from the embedded version
    string (checked: 22 differing bytes, all in the two version strings;
    identical size and identical `tud_vendor_control_xfer_cb`).

### 3.7 v1.1: LED commands

Issue #19. Additive only: a v1 host keeps working, and a v1.1 host talks to
a v1.0 pad by its capabilities.

- **HELLO:** the protocol minor is **1**. Capability **bit 8, LEDS**, covers
  LED_STATUS and SET_LED. The firmware sets it only once the LED addon
  (AnimationStation) has run its setup: the LEDs are configured
  (`ledOptions.dataPin >= 0`) on this build and board. The S16 advertises
  `0x0000012F`.
- **STRING `0x20 + n`:** the firmware's name for animation effect `n`
  (`EFFECT_STATIC_COLOR`, ...). An `n` at or past the effect count STALLs.
  The host keeps its own table from `proto/enums.proto` and asks only for
  effects past it.
- **Without bit 8**, LED_STATUS STALLs like any unknown IN. STATUS's
  last-command fields are not touched, the §3.6 item 7 rule for IN
  commands. SET_LED is an unknown OUT command: UNSUPPORTED, `last_seq` 0.
  The host relies on the capability bit and never sends either without it.

**LED_STATUS** (`0x04`, IN, `wValue` 0), a 12-byte reply:

| Offset | Type | Field |
|---|---|---|
| 0 | u8 | flags: bit 0 the LEDs run (always set in a reply), bit 1 `hasCustomTheme`, bit 2 unsaved (below) |
| 1 | u8 | animation effect (`baseAnimationIndex`, the `AnimationEffects` enum) |
| 2 | u8 | effects this build cycles through: 4, plus 1 with a custom theme (0 static colour, 1 rainbow, 2 chase, 3 static theme, 4 custom theme). The host checks `animation < count` |
| 3 | u8 | brightness level (`AnimationOptions.brightness`, 0 to the steps) |
| 4 | u8 | brightness steps (`ledOptions.brightnessSteps`, clamped to 255) |
| 5 | u8 | brightness maximum (`ledOptions.brightnessMaximum`, clamped to 255) |
| 6 | u8 | `staticColorIndex` |
| 7 | u8 | `buttonColorIndex` |
| 8 | u8 | `themeIndex` |
| 9 | u8 | colours (`AnimationColor`) the colour indexes take: 16 |
| 10 | u8 | static themes `themeIndex` takes: 20 |
| 11 | u8 | reserved, 0 |

Flag bit 2 is set when the effective values differ from a snapshot. The
firmware takes that snapshot at LED setup and again on every storage save
of any kind: hotkeys, web-config and the channel all count. LED_STATUS
shows the last accepted SET_LED even before the LED core has applied it,
which takes about 10 ms on core 1.

**SET_LED** (`0x13`, OUT): the 8-byte header, then 8 bytes.

| Offset | Type | Field |
|---|---|---|
| 0 | u8 | mask: bit 0 animation, bit 1 brightness, bit 2 static colour, bit 3 button colour, bit 4 theme; bits 5–7 must be 0 |
| 1 | u8 | animation |
| 2 | u8 | brightness |
| 3 | u8 | `staticColorIndex` |
| 4 | u8 | `buttonColorIndex` |
| 5 | u8 | `themeIndex` |
| 6 | u8 | persist: 0 RAM only, 1 save to flash |
| 7 | u8 | reserved (ignored) |

- **Length:** the arguments must be all 8 bytes (16 with the header), else
  BAD_LENGTH. Bytes past them are ignored.
- **Checks, in order:** token (BAD_TOKEN), pending reboot (BUSY), length
  (BAD_LENGTH), capability (UNSUPPORTED), values. Mask bits 5–7, or a
  persist byte above 1, are BAD_ARG. So is any masked value out of range:
  each index below its count from LED_STATUS, and brightness at most the
  steps.
- **All or nothing:** a refused SET_LED changes nothing. Fields outside
  the mask are left as they are.
- **No-ops and merging:** mask 0 with persist 1 saves the current state;
  mask 0 with persist 0 is a no-op (OK). Two SET_LEDs that arrive before
  the LED core applies the first are merged (the masks are OR'd).
- **How it applies:** the control callback flags the request, and the core
  that owns AnimationStation applies it, as the LED hotkey handlers do.
  LED state is never touched from the USB IRQ context. There is no hotkey
  cooldown, and it works in web-config mode too.
- **RAM only (persist 0):** the change is lost on restart, unless something
  else saves the config first (any later save writes `AnimationOptions`
  from RAM). This is the SET_PROFILE caveat; flag bit 2 tells the host.
- **Persist 1:** the normal (non-forced) storage save, with no reboot. The
  firmware skips that save in PS4/PS5 mode while the dongle authentication
  needs the flash, so flag bit 2 stays set; the host notes an unconfirmed
  save rather than failing.

Not in v1.1, maybe in v1.2: the custom theme's colours. 18 normal and 18
pressed RGB values are 108 bytes, more than one setup's payload, so it
would need a chunked SET_THEME. Web-config's `setCustomTheme` covers it
for now.

Host side: `channel::Channel::led_status` / `set_led`, `leds.rs` and
`flow::leds` / `leds_set` (`gamepadii leds`, `leds set`, `leds
brightness`). `channel/sim.rs` models the checks, the RAM and saved state,
the effect count and the names (`SimChannel::with_leds`); it applies a
SET_LED at once, so there is nothing to merge.

### 3.8 v1.2: lights on main

Issue #21. Additive. The fork's main line (`gamepadii/main`) has the
protocol minor **2**. Main replaced the AnimationStation settings of
v0.7.12 (§3.7) with lights profiles. Thus, bit 8, LED_STATUS and SET_LED
are **not on main**: bit 8 is never set, LED_STATUS STALLs, and SET_LED is
an unknown OUT command (UNSUPPORTED, `last_seq` 0). A host checks bit 9
for main, and keeps the v1.1 commands for pads that set bit 8.

On main, the HELLO mode bitmap is `0x000BFFFF`: `InputMode` 0 to 17, and
19 (P5General 16, SInput 17, Switch 2 Pro 19; 18 does not exist). Tests
must not use a fixed mode as "not in this build": they take one that is
not in the bitmap.

- **Capability bit 9, LIGHTS:** set only after `NeoPicoLEDAddon` has run
  its setup (`ledOptions.dataPin` is a valid pin on this board). The S16 on
  main gives `0x0000022F`.
- **Without bit 9**, LIGHTS_STATUS STALLs and does not change the
  last-command fields of STATUS (§3.6 item 7). SET_LIGHTS fails with
  UNSUPPORTED after the token, BUSY and length checks.

**LIGHTS_STATUS** (`0x05`, IN, `wValue` 0), a 16-byte reply:

| Offset | Type | Field |
|---|---|---|
| 0 | u8 | flags: bit 0 the lights run (always set in a reply), bit 1 unsaved (below), bit 2 a web-config preview runs, bit 3 (v1.5) a change waits for core 1; bits 4–7 are 0 |
| 1 | i8 | lights profile: -1 off, or 0–3 (`AnimationStation::GetSavedMode()`) |
| 2 | u8 | enabled profiles: bit `n` is `animationOptions.profiles[n].bEnabled` |
| 3 | u8 | profile count: 4 |
| 4 | u8 | brightness level (`animationOptions.brightness`) |
| 5 | u8 | brightness steps (10 on main) |
| 6 | u8 | brightness maximum (`ledOptions.brightnessMaximum`, 0–255) |
| 7 | u8 | effect when not pressed, of the profile in byte 1 (`AnimationNonPressedEffects`); 255 when off |
| 8 | u8 | effect when pressed (`AnimationPressedEffects`); 255 when off |
| 9 | u8 | case effect (`AnimationNonPressedEffects`); 255 when off |
| 10 | u8 | cycle time when not pressed (0–9); 255 when off |
| 11 | u8 | cycle time when pressed; 255 when off |
| 12 | u8 | case cycle time; 255 when off |
| 13 | u8 | reserved, 0 |
| 14 | u16 | idle timeout in seconds (0: never) |

Bytes 1 and 4 show the last accepted SET_LIGHTS, also before core 1
applies it (about 10 ms). Flag bit 1 is set when the profile or the
brightness is different from a snapshot. The firmware takes the snapshot
at LED setup and at each save of the settings. The effects and the cycle
times are for the profile in byte 1, and v1.2 cannot change them.

**SET_LIGHTS** (`0x14`, OUT): the 8-byte header, then 4 bytes.

| Offset | Type | Field |
|---|---|---|
| 0 | u8 | mask: bit 0 profile, bit 1 brightness; bits 2–7 must be 0 |
| 1 | i8 | profile: -1 off, or 0–3 |
| 2 | u8 | brightness level |
| 3 | u8 | persist: 0 RAM only, 1 save to flash |

- **Length:** the arguments are 4 bytes (12 with the header), else
  BAD_LENGTH. The firmware ignores more bytes.
- **Checks, in this order:** token (BAD_TOKEN), pending reboot (BUSY),
  length (BAD_LENGTH), capability (UNSUPPORTED), mask bits 2–7 or persist
  above 1 (BAD_ARG), a web-config preview that runs (BUSY), values
  (BAD_ARG). A profile must be -1 or an **enabled** 0–3. A brightness must
  be at most the steps.
- **All or nothing:** a refused SET_LIGHTS changes nothing. Fields that are
  not in the mask do not change.
- **Mask 0:** with persist 1 it saves the current state; with persist 0 it
  does nothing (OK). The firmware merges requests that come before core 1
  applies the first one.
- **RAM only (persist 0):** the change is lost at a restart, unless
  something else saves the settings first. Flag bit 1 tells the host.
- **Persist 1:** a normal save, with no reboot. As in §3.7, the firmware
  does not save in PS4/PS5 mode while the dongle authentication uses the
  flash.
- **Not in v1.2:** a change to the effects, colours or cycle times of a
  profile, or to the light layout. These need the v2 config transfer
  (§3.5). Until then, web-config `getAnimationProtoOptions` and
  `setAnimationProtoOptions` do it. They are not on the allowlist of
  gamepadii yet.

Host side: `channel::Channel::lights_status` / `set_lights`,
`leds::over_channel`, `leds::plan_lights` and `flow::leds_set`. With
bit 9, `gamepadii leds` reads LIGHTS_STATUS, and `leds set
--lights-profile N|off --brightness N|up|down` sends SET_LIGHTS. The host
refuses the v0.7.12 options (`--animation`, the colours, `--theme`) on
main, and `--lights-profile` on the v0.7.12 line, before it sends a
command. While a web-config preview runs, the host sends nothing and asks
the user to stop the preview. `channel/sim.rs` models main with
`SimChannel::with_lights`: the checks in the order above, the RAM and
saved state, and the effects of each profile from the mock's
`getAnimationProtoOptions`.

### 3.9 v1.3: the board ID

Additive. The protocol minor is **3**. STRING id **5** gives the unique ID
of the flash chip as 16 upper-case hex digits
(`pico_get_unique_board_id_string`). It does not change when the firmware
or the settings change. The firmware has it in every mode, and in
web-config.

- **Older firmware STALLs STRING 5.** The host sends it only when HELLO
  gives major 1, minor 3 or higher, and the STRING capability.
- **It is stable, but not always unique.** The S16 gives `433030383835362E`,
  which is the ASCII text `C008856.`. This flash chip gives a fixed value,
  so other pads with the same chip can give the same ID. picotool reads the
  same value (`flash id: 0x433030383835362E`).
- **The USB serials come from it.** XInput sends `0` and bytes 5 to 7 in
  hex (the S16: `035362E`). Xbox One sends `'A' + byte % 26` for each of
  the 8 bytes, then `8ABCDEFGH`. Web-config sends `123456` on all pads, so
  it is not a key.

Host side: `Channel::board_id`. gamepadii keys the device cache by the
board config and the board ID (`Haute42COSMOX-433030383835362E.json`). A
cache by serial from an older firmware still matches, because gamepadii
makes the serial from the ID (`cache::PadId::same`), and the first read
with the ID replaces that file. When two connected pads give the same ID,
`status` tells them apart by USB port, and shows a warning. Before 1.3,
the key stays the USB serial.

### 3.10 v1.4: profile slots

Additive. The protocol minor is **4**, and capability **bit 10, SLOTS**,
covers READ_SLOT and WRITE_SLOT. The fork's commit is cb01f703. A host
sends them only when HELLO gives major 1, minor 4 or higher, and bit 10.
They let a host read and change the pin mappings of a profile with no
reboot, for example a per-game profile.

A **slot** (READ_SLOT reply, WRITE_SLOT argument after the header), in
little-endian:

| Offset | Type | Field |
|---|---|---|
| 0 | u8 | profile |
| 1 | u8 | flags: bit 0 enabled, bit 1 active (selected), bit 2 save to flash (WRITE_SLOT only) |
| 2 | u8 | pin count (`NUM_BANK0_GPIOS`: 30 on RP2040, 48 on RP2350) |
| 3 | u8 | bytes for each pin: 10 |
| 4 | 16 bytes | label, UTF-8, padded with NUL; no NUL is necessary |
| 20 | pins | for each GPIO from 0: `i16` action (`GpioAction`), `u32` `customButtonMask`, `u32` `customDpadMask` |

**READ_SLOT** (`0x06`, IN, `wValue` the profile). The host does not know
the pin count before the reply, so the firmware does not limit `wLength`
for this command; a host asks for 512 bytes. Profiles 0 and 7 or more
STALL. A slot that was never written gives zeros after the head. The
active bit comes from `profileNumber` in RAM.

**WRITE_SLOT** (`0x15`, OUT). The arguments must be exactly 20 + 10 × pin
count bytes (328 with the header on RP2040), else BAD_LENGTH. A transfer
longer than 8 + 20 + 10 × 48 bytes STALLs in SETUP, with BAD_LENGTH and
`last_seq` 0.

- **Checks, in this order:** token, BUSY, length; then BAD_ARG for a
  profile that is not 2–6, an unknown flag bit, a pin count that is not
  `NUM_BANK0_GPIOS`, bytes for each pin that are not 10, or an action that
  is not -10 to 138; then PROFILE_DISABLED for a slot without "enabled"
  that has "active", or that is the active profile now.
- **All or nothing:** a refused WRITE_SLOT changes nothing.
- **Profile 1's fixed pins:** a pin that profile 1 gives as RESERVED (-5)
  or ASSIGNED_TO_ADDON (0) keeps profile 1's action, with masks 0. In a
  slot, these two actions mean "the pin of profile 1".
- **The direction of a pin** is not on the wire. WRITE_SLOT keeps it.
- **Configs from before profile 6** have only 4 extra slots (the S16). A
  WRITE_SLOT to profile 6 adds the missing slots, empty and disabled.
- **Apply:** when the profile is active after the write, the firmware
  forces the reinit. STATUS flag bit 0 (PROFILE_PENDING) is set until it is
  done, 9 to 12 ms on the S16. A host reads STATUS until bit 0 is clear,
  `last_command` is `0x15`, and `last_seq` is its sequence number.
- **RAM only** without bit 2: a restart gives the saved profiles again.
  With bit 2, the normal storage save.

Host side: `Channel::read_slot` / `write_slot`, the module `slots`
(action names, the slot file, `plan`, `expected`), and `flow::profile_show`
/ `profile_write` (`gamepadii profile show [PROFILE]`, `profile write FILE
--slot N [--select] [--save] [--label TEXT]`). The JSON of `profile show`
is a slot file. A slot file can leave out pins: they get 0, so they follow
profile 1. Before a write, gamepadii checks what the firmware checks, and
backs up the slot in the slot file format
(`<time>-<board>-profile<N>.json`), so a write of the backup undoes the
change. After the write, it waits for STATUS and reads the slot back. A
saved write also updates the device cache. `channel/sim.rs` models the
slots with `SimChannel::with_slots`.

## 4. Firmware implementation

The footprint is deliberately small:

- **New files:** `headers/controlchannel.h` and `src/controlchannel.cpp`,
  roughly 250 lines for v1.
- **`src/usbdriver.cpp`, three lines:**
  ```cpp
  bool tud_vendor_control_xfer_cb(uint8_t rhport, uint8_t stage, tusb_control_request_t const *request) {
  #if GP2040_CONTROL_CHANNEL
      if (ControlChannel::matches(request)) return ControlChannel::handle(rhport, stage, request);
  #endif
      return DriverManager::getInstance().getDriver()->vendor_control_xfer_cb(rhport, stage, request);
  }
  ```
- **`CMakeLists.txt`:** `option(GP2040_CONTROL_CHANNEL ... OFF)`, which adds
  the source file and the compile definition. The pattern is the same as our
  `GAMEPADII_TEST_BOOT_WEBCONFIG`. Our builds pass `-DGP2040_CONTROL_CHANNEL=ON`
  through `CMAKE_EXTRA_ARGS`.
- **v2 only:** a `ControlChannel::process()` call in the main loop.

How it's switched on:

1. **Fork (v1):** compile time only. With the option off, the binary is
   byte-for-byte stock in this path. With it on, the channel is always live.
   The fork adds **no** `.proto` fields. The config is persisted in flash, so
   a field number we picked could collide with one upstream assigns later,
   and the old data would then be read as something else.
2. **Runtime flag (not planned):** an upstream version would need a runtime
   flag in `config.proto`, default off. We do not send the channel to
   upstream now (§7), so the fork has the compile-time option only.

Other rules for the implementation:

- Reply buffers are static (`tud_control_xfer` keeps the pointer until the
  transfer completes).
- IN replies are built in `CONTROL_STAGE_SETUP`; OUT commands act in
  `CONTROL_STAGE_DATA`. Other stages return `true`.
- `wLength > 64` on a matched request STALLs in SETUP.
- Only events are raised; nothing calls `Storage::save()` directly. Flash
  writes stay in `checkSaveRebootState()`, on core0, at the end of the loop.

## 5. Host side (gamepadii)

### 5.1 Transport: raw usbfs, behind a trait

| Option | Dependencies | Device-level control with a kernel driver bound | Portability |
|---|---|---|---|
| raw usbfs ioctl (`USBDEVFS_CONTROL`) via `libc` | `libc` only (tiny, ubiquitous) | yes (§2.2) | Linux only |
| `rusb` | libusb-1.0 C library (system copy or vendored build) | yes on Linux (libusb submits a control URB, no claim needed) | Linux, macOS, Windows (needs WinUSB/UsbDk; §6) |
| `nusb` | pure Rust | expected yes on Linux; its API for device-level control on Windows is still to be checked | Linux, macOS, Windows |

**Decision: raw usbfs through `libc` for v1**, in
`gamepadii-core/src/transport/usbctl.rs` behind `#[cfg(target_os = "linux")]`:

- open `/dev/bus/usb/{busnum:03}/{devnum:03}` with `O_RDWR | O_CLOEXEC`,
  taking `busnum` and `devnum` from the `Device` that discovery already builds
- issue `ioctl(fd, USBDEVFS_CONTROL, &usbdevfs_ctrltransfer)`
- compute the request number as `_IOWR('U', 0, struct usbdevfs_ctrltransfer)`
  from `size_of` (24 bytes on 64-bit), not as a hard-coded constant

`libc` gives the correctly typed `ioctl` for both glibc and musl: they
disagree on the request argument's type (`unsigned long` versus `int`), and
that is the reason not to declare it by hand. This is one `unsafe` block of
about 80 lines, and it brings in no C library.

Errno mapping:

| errno | Meaning | gamepadii error |
|---|---|---|
| `EPIPE` | STALL | channel present but the command was rejected; read STATUS for the reason |
| `EACCES` | no permission on the node | needs the udev rule (§5.3), exit 5 with a hint |
| `ENODEV`, `ESHUTDOWN`, `EPROTO` | the device dropped off the bus (expected after REBOOT) | — |
| `ETIMEDOUT` | no reply within the timeout (1000 ms) | transport error (exit 4) |

A `ControlPipe` trait (`control_in(cmd, value, len)`,
`control_out(cmd, payload)`) gets a `sim::SimPad` implementation, so flow
tests run without hardware and without real sleeps, as they do now. Revisit
`nusb` when the GUI goes cross-platform.

### 5.2 Detection and fallback

1. Discovery finds the pad by VID:PID in sysfs, as it does today.
2. gamepadii sends HELLO with a random nonce and a 200 ms timeout. It treats
   the channel as present only when the reply has the right magic, echoes the
   nonce, and carries a major version it knows. Anything else means "no
   channel":
   - a STALL
   - a short or 0-byte reply (stock XInput)
   - garbage (stock XBOne)
   - `EACCES`; when the pad is *known* to support the channel (from the
     cache), gamepadii also prints the udev hint
3. The device cache records `control_channel: {major, minor, caps}` per board
   config (`control-channel.json`). `status --json` shows it, `null` when unknown.
4. Each flow uses the channel when it can and falls back otherwise:
   - `mode X`: SET_INPUT_MODE, then wait for re-enumeration (the devnum
     changes), then verify with STATUS
   - `webconfig` and `reboot …`: REBOOT
   - new `profile switch N [--persist]`: SET_PROFILE
   - `info` and `backup` (anything that needs the HTTP API, until v2):
     REBOOT(1), the reads, then `/api/reboot {"bootMode":0}` and a check
     that the pad returns in its mode (`--stay` stays in web-config); on a
     failure the pad is still sent back, over REBOOT(0) when HTTP is down
   - otherwise, the existing guided web-config flows

   Every STATUS also updates the device cache's running profile, so the
   key hints in the guided flows follow a RAM-only `profile switch`.

   Each command's JSON says which transport it used (`"via": "control" | "webconfig"`).
5. The channel is also live in web-config mode (NetDriver), so HELLO is a
   uniform probe in every mode. HTTP stays the path for anything the channel
   lacks.

Writes follow the hardware rules in `CLAUDE.md`: SET_INPUT_MODE and
SET_PROFILE with `persist=1` write flash. The live tests for them are gated on
`GAMEPADII_LIVE_WRITE=1`, and REBOOT on `GAMEPADII_LIVE_REBOOT=1`.

### 5.3 udev rule

The rule is shipped as `70-gamepadii.rules`. It has to sort before
`73-seat-late.rules`, where `uaccess` is applied. `gamepadii udev-rule` prints
it, and the AUR package installs it. It has one line per VID:PID in
`mode.rs`, excluding web-config, which needs no rule:

```
SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="045e", ATTR{idProduct}=="028e", TAG+="uaccess"
SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="cafe", ATTR{idProduct}=="0001", TAG+="uaccess"
# … 10c4:82c0, 0f0d:0092, 1532:0401, and the rest of the table
```

The catch: these VID:PIDs are borrowed. The rule also gives the seat user
write access to the usbfs node of a *real* Xbox 360 pad, a real Hori pad, and
so on. That is the same trade-off Steam makes for hidraw, and it's acceptable,
but the docs should say so.

`usbOverrideID` lets users set custom IDs, so the rule file carries a comment
on how to add a line. Matching on the manufacturer string ("Open Stick
Community") doesn't work: `usbDesc*` overrides it and vendor builds change it.

### 5.4 Safety with other devices

GP2040-CE uses the USB ids of real controllers. A real Xbox 360 pad, and an
8BitDo pad in XInput mode, are also `045e:028e`. Thus, a device that has
one of our ids can be any device. These rules keep gamepadii away from
them:

1. **No OUT request without a valid HELLO.** A `Channel` exists only after
   a HELLO reply with the magic `GPii`, the echo of our nonce, and a major
   that we know. A STALL, an empty reply, garbage, a wrong nonce, a wrong
   magic, a newer major, or no reply: no command follows. The tests in
   `crates/gamepadii-core/tests/safety.rs` run every command on such a
   device, and check that it gets only HELLO.
2. **One HELLO for each connection.** When a device answers HELLO, and the
   answer is not the channel (`Probe::Stock`), gamepadii records that
   connection (bus, port and device number) in
   `$XDG_CACHE_HOME/gamepadii/no-channel.json`. It does not send HELLO
   again to that connection, for example from a waybar module that runs
   `status` every few seconds. A replug or a reboot makes a new
   connection, and gamepadii asks again. No reply (a timeout), or no
   permission, is not recorded: the pad can be still starting.
3. **A narrow udev rule (§5.3).** Each line names the exact VID:PID that
   the firmware uses. Only TinyUSB's example vendor (`cafe`) is a full
   vendor. The rule never matches a full vendor such as Sony (`054c`).
   A test checks this.
4. **Not as root.** As root, gamepadii does not use the channel: root
   gets past the limit of the udev rule, to every pad. `Usbfs::open`
   refuses, and the commands use web-config or button presses.
5. **A late ACL.** Right after a pad connects, udev can apply the
   `uaccess` ACL a moment after it makes the node. Then the first open
   gives `EACCES`. When the node is less than 5 s old and the udev rule is
   installed, gamepadii tries again for up to 1 s
   (`Usbfs::open_new`, `channel::wait_for_acl`). Then it shows the udev
   hint.

## 6. Windows and macOS (later, notes only)

- **Windows:** user mode can only send a control transfer through a handle
  from the device's function driver, which here is xusb22 or HID. WinUSB
  needs its own interface, bound through MS OS 2.0 descriptors (a BOS
  descriptor with a vendor code and `CompatibleID "WINUSB"`), and that is a
  descriptor change. We rule it out for console-facing modes, and XBOne
  already uses MS OS 1.0 for its own purposes. The options are:
  - libusb with the **UsbDk** filter driver, which can reach any device
    without replacing its driver. It is an install-time burden on the user.
  - A future opt-in "PC mode" variant that adds a WinUSB interface, kept
    separate from the console modes.
  - Web-config, as the fallback.
- **macOS:** `IOUSBHostDevice` / `DeviceRequest` on a device interface may
  need `USBDeviceOpen`, which fails when a kernel driver holds the device.
  This is unverified, and it is part of why `nusb` gets evaluated later.

## 7. Compatibility with upstream

We do not send the control channel to upstream (decided 2026-10-18). The
first reports were closed, and upstream work is paused. These rules apply:

1. gamepadii operates with stock GP2040-CE. The basic functions (status,
   info, backup, mode change, restart) use web-config and button presses.
2. The advanced functions use our fork: the control channel, `getConfig`,
   and the LED commands. gamepadii finds the fork with a capability check
   and does not use these functions on stock firmware.
3. The fork stays close to upstream. The patch touches one hook in
   `usbdriver.cpp` and adds one file, so we can move it to new upstream code
   easily (#21).
4. The fork adds no `.proto` fields (§4), so its saved settings stay
   compatible with stock firmware. You can install stock firmware again
   without a settings reset.

The old plan (an upstream RFC, then a PR) is in the git history of this
file.

## 8. Test plan (S16)

The firmware branch is `gamepadii/control-channel`, based on
`gamepadii/freeze-investigation`. Builds go through the existing script:

```sh
CMAKE_EXTRA_ARGS="-DGP2040_CONTROL_CHANNEL=ON -DGAMEPADII_TEST_BOOT_WEBCONFIG=ON" \
  ./build.sh gamepadii/control-channel cc-testboot        # unattended loop
CMAKE_EXTRA_ARGS="-DGP2040_CONTROL_CHANNEL=ON" \
  ./build.sh gamepadii/control-channel cc                 # release-style
```

`flash-test.sh <uf2> <label>` takes the backup, flashes and comes back in
web-config, because the testboot build boots DEFAULT into web-config. From
there, REBOOT(0) boots straight into gamepad mode, since GAMEPAD boot mode
skips the testboot redirect. That gives an unattended loop.

A small `cc-probe` (a gamepadii subcommand or a test binary) drives the
requests.

1. **Read-only, every mode the S16 uses** (XInput, Keyboard, HID, Switch,
   PS4): send HELLO, STATUS and all STRING ids, and check each of these:
   - the kernel driver stays bound (`/sys/bus/usb/devices/<dev>:1.0/driver`
     is unchanged)
   - `evtest` keeps reporting presses during a 10 000-request loop
   - no new `dmesg` lines
   - the input report rate stays at 1000 Hz (`evhz` or a usbmon capture)
2. **No regression for foreign requests.** Use usbmon to compare the
   stock-v0.7.12, freeze-fix and cc builds:
   - xpad's probe (`0xC1 0x01 0x0100`) still gets a 0-byte reply in XInput
   - `0xC0 0x47` with a *wrong* wIndex magic gets the same reply as on stock
     in every mode: a STALL in most, a ZLP in XInput, garbage in XBOne
   - with the channel compiled out, the build behaves identically to the
     freeze-fix build
3. **Fallback detection:** on the stock and freeze-fix builds, HELLO reports
   "no channel" in XInput (ZLP), Keyboard (STALL) and XBOne (garbage).
4. **Negative OUT cases:** a wrong token, `wLength` > 64, a bad
   profile/mode/boot-mode argument, and a disabled profile. Each must STALL
   and show the right result in STATUS.
5. **Reboots** (`GAMEPADII_LIVE_REBOOT=1`):
   - REBOOT(1) brings up `cafe:4028`
   - REBOOT(2) brings up `2e8a:0003`; recover by replugging, or let
     flash-test.sh reflash
   - REBOOT(0) comes back in the stored mode
   - each time, the host must see the ACK before the device disappears
6. **RAM profile switch:** SET_PROFILE(n, persist=0) switches the GPIO map
   (shown on `evtest` and the OLED), and a replug brings back the old
   profile. This writes no flash, but it does change live state, so ask
   first.
7. **Writes** (`GAMEPADII_LIVE_WRITE=1`, with the user's OK and a backup;
   flash-test.sh takes one):
   - SET_INPUT_MODE keyboard brings up CAFE:0001, then XInput again
   - SET_PROFILE(n, persist=1) followed by a reboot leaves STATUS showing `n`
   - restore the original mode and profile afterwards
8. **Stress during a save:** request STATUS in a loop while a persist save
   runs. Expect a few ms of NAK and no timeouts at 1000 ms.
9. **Console smoke test, optional:** if a Switch or PS4/5 is on hand, plug
   in the `cc` build. The pad must work as before; with no descriptor change
   nothing is expected to differ.

### 8.1 Results, v1 on the S16 (2026-10-17)

Builds: `cc-testboot` (v0.7.12-7-g1008647-testboot) for the loop, then `cc`
(v0.7.12-6-gf507f0b, the release-style build). Client:
`GP2040-CE-builds/tools/ccprobe.py` (raw `USBDEVFS_CONTROL`). Logs are in
`GP2040-CE-builds/results/cc/`.

| Check | Web-config (rndis_host) | XInput (xpad) | Keyboard (usbhid) |
|---|---|---|---|
| HELLO: magic, nonce echo, 1.0, caps `0x2F`, modes `0xFFFF` | ok | ok | ok |
| STATUS (`input_mode` 255 / 0 / 3, profile 2, enabled `0x1F`) | ok | ok | ok |
| STRING 1–4 and `0x11–0x16` (`Default`, `SilkSong`, `Celeste`, `DKC`, `TGX`, empty) | ok | ok | ok |
| 2000 to 5000 × (HELLO + STATUS + STRING): errors, driver still bound | 0, yes | 0, yes | 0, yes |
| Foreign requests, same result as stock (below) | yes | yes | yes |
| Negative cases (`ccprobe.py negative`): 16 STALL, with the expected result and seq in STATUS; a HELLO with wLength 4 returns the first 4 bytes | ok | ok | ok |
| SET_PROFILE RAM-only (2→1→3→2) | — | ok, applied before the next STATUS (flag 0 already) | ok |
| RAM-only switch, then REBOOT(0), comes back on the stored profile | — | ok (profile 2) | — |
| SET_PROFILE persist=1 (2→1, reboot, 1→2, reboot) | — | ok | — |
| OUT command after an accepted REBOOT gets BUSY (STATUS flag bit 1) | — | ok | — |
| SET_INPUT_MODE: XInput→Keyboard (`cafe:0001`) →XInput, stored profile kept | — | ok | ok |
| REBOOT(0) from web-config gives a gamepad mode; REBOOT(1) from a gamepad mode gives `cafe:4028` | ok | ok | — |
| STATUS polled during a persist save | — | 200 polls, max 7 ms, no errors | — |

Foreign requests (`ccprobe.py foreign`): the xpad probe `C1 01 0100`, a wrong
magic (`C0 47` with wIndex `0x5101` or `0x0001`), an interface-recipient
`C1 47 5001`, `C0 46 5001`, `C0 01`, XSM3 `C1 81 wIndex 0103`, MS OS `C0 20 0004`,
a known OUT id sent as IN (`C0 47 5010`), a known IN id sent as OUT
(`40 47 5001`), and `41 47 5010`.

- **XInput:** the results match stock freeze-fix-rc exactly. Every IN gets 0
  bytes except XSM3 `0x81`, which returns the same 29-byte serial on both
  builds; the OUTs are ACKed.
- **Web-config:** everything STALLs, on stock and on cc alike.
- **Keyboard:** everything STALLs, which is what KeyboardDriver does on stock
  too. No stock capture was taken in Keyboard mode, because that would have
  needed another mode switch.
- **On stock freeze-fix-rc:** HELLO gets a 0-byte reply in XInput and a STALL
  in web-config, so detection reports "no channel" in both.

Not covered:
- **usbmon:** it needs root on this machine, so the comparisons above are
  ioctl outcomes, not packet captures.
- **evtest and input report rate:** not checked. There was nobody to press
  buttons, and xpad sends no events without input. The driver stayed bound,
  and the kernel log gained no new lines during the loops.
- **LOCKED path:** not exercised, because setting `forcedSetupMode` would be a
  settings write.
- **SET_INPUT_MODE sent from web-config:** not exercised.
- **HID, Switch, PS4, XBOne modes:** not exercised.

The pre-existing xpad line "unable to receive magic message: -121" appears at
every XInput enumeration, on stock as well. It is xpad's probe getting a
0-byte reply, and the channel doesn't change it.

`gamepadii info` and `gamepadii backup` from XInput (the `cc` build,
2026-10-17): REBOOT(1), the reads, `/api/reboot {"bootMode":0}`, back in
XInput (confirmed by STATUS) in 4 to 6 s. After a RAM-only switch to
profile 3, the restart put it back on the saved profile 2, as expected.
`backup --stay` left it in web-config, and `webconfig --exit` brought it
back.

With the option off, the `cc-off` build is byte-identical to freeze-fix-rc
except for the version string (§3.6).

## 9. Decisions (issue #2, 2026-10-17)

1. **Gate:** compile time only in our fork (`GP2040_CONTROL_CHANNEL`).
   There is no runtime flag (§4). Updated 2026-10-18: no upstream proposal
   (§7).
2. **No REBOOT to BOOTSEL** over the channel. Flashing is occasional;
   gamepadii goes REBOOT(1) → web-config → `/api/reboot {"bootMode":2}`.
3. **Honour `forcedSetupMode`** for REBOOT(web-config) and SET_INPUT_MODE
   (LOCKED).
4. **`profile switch` is RAM-only by default**; `--save` sends persist=1.
5. **The channel answers in web-config mode too**, for uniform detection.
6. **v2** (config sections over the channel, §3.5) comes next, before
   milestone 7 (PICOBOOT, #7). Not in v1.
7. **The udev rule is opt-in**, through `gamepadii setup udev`, which explains
   what it grants (the borrowed VID:PIDs include real Xbox 360 and Hori pads).

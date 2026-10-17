# gamepadii

Rust CLI (then ratatui TUI, then Tauri GUI) for GP2040-CE gamepads; the
first target is a Haute42 S16. Decisions, firmware facts, API hazards and
the architecture are in `docs/DESIGN.md`. Roadmap and design work are
tracked in GitHub issues (epic #1, https://github.com/rdlu/gamepadii/issues/1,
and its sub-issues); keep the issue bodies current when decisions change.

## Commands

```sh
cargo build
cargo test --workspace                       # insta snapshots: cargo insta review
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -- [args]
```

All three of test, fmt --check and clippy must pass before a commit
(`mise run check`). `mise tasks` lists the rest: `test-live`, `review`,
`status`, `install`, `outdated`, `update`. CI is `.github/workflows/ci.yml`;
every action is pinned to a commit SHA with its version in a comment.

## Conventions

- Workspace: `crates/gamepadii-core` is the library, `crates/gamepadii` the
  binary (CLI, later TUI). The core never prints: commands return plain
  serde data and the front ends render it (`output.rs`).
- Layering: `discovery` → `transport` / `channel` → `client` → `commands` /
  `flow` → `output`. The guided flows (`flow.rs`) take an `Env`: a `Scan`
  (sysfs, or `sim::SimPad` in tests), a `Clock` (`wait::ManualClock` in
  tests, so no real sleeps), an `open` closure for the web-config client, a
  `control` closure for the USB control channel (`SimPad::open_control` in
  tests, usbfs live), and report through a `Progress` (the CLI's spinner on
  stderr). In a gamepad mode the flows try the channel first and fall back
  to the guided button presses. Re-enumeration is detected by
  polling sysfs (devnum change), not a udev monitor.
- No async runtime: std threads + channels.
- sysfs readers take a `Root` (`/` or a fixture dir) and never hard-code
  `/sys`, so tests run against fixture trees under `tests/fixtures/`. The
  usbfs transport opens `/dev/bus/usb/...` under the same `Root`, so a
  fixture `--sysroot` can never reach a real device; end-to-end tests of
  the channel use the hidden `--mock-channel stock|channel|no-access`.
  Redact serials in captured fixtures.
- Output contract: `--format human|json|waybar` (`--json` is an alias).
  JSON has `"schema": 1`, snake_case keys, `null` for unknowns (never
  omitted); a failure under `--json` still prints `{"schema": 1, "error":
  {...}}` on stdout, and waybar always gets one valid line. Exit codes are
  stable (`gamepadii_core::error::exit`): 0 ok, 1 check failed, 2 device
  not found, 3 usage, 4 transport, 5 needs user action.
- Errors: the hand-written `Error` enum in `gamepadii-core/src/error.rs`, no
  `anyhow`/`thiserror`.
- VID:PID detection is a guess for borrowed ids: report a confidence
  (`confirmed` / `likely`), never claim certainty.
- Hotkeys and boot buttons are logical; anything telling the user what to
  press names the physical keys too (`buttons.rs`, from the device cache).
- Tests that run the binary point `XDG_CACHE_HOME` and `XDG_DATA_HOME` at
  temporary directories (`exit.rs`'s `gamepadii_in`): commands write the
  device cache and backups there.

## Web-config API hazards

The client must guard against these (details in `docs/DESIGN.md`):

- `setGamepadOptions` zeroes every key it isn't sent: always read-modify-write
  the full object (`api::set_gamepad_options`, which also turns the hex USB
  ids into integers, refuses a body missing a setter key, and verifies by
  reading back). The mock models the setter, so tests catch partial bodies.
  It is the only write so far; any new `set*` gets the same treatment.
- The server ignores the HTTP method: never GET a `set*` endpoint (it replays
  the previous POST body).
- Serialise requests (one at a time); POST bodies are capped at 16 KB.
- lwIP's httpd matches `Content-Length:` case-sensitively; requests must use
  that exact casing (we use our own HTTP/1.0 client for this).
- `resetSettings` is a destructive GET: never call it.
- Never call `/api/getConfig` on v0.7.12: on the S16 (Haute42COSMOX, build
  0014e4a) it hangs the device's network stack until replugged. Back up via
  the per-section `get*` endpoints.
- Never call `/api/getI2CPeripheralMap` either: it runs a live I2C bus scan
  and hangs the S16 the same way. The client reads only from an explicit
  allowlist of endpoints verified safe (the list is in `docs/DESIGN.md`), never
  "anything named `get*`".

## Hardware

Agents may read from the S16 and reboot it between modes; never write
settings to the device without the user's explicit OK, and take a backup
first. That includes `gamepadii mode <MODE>` with the pad in web-config,
which writes `inputMode`. Live tests that reboot or write skip unless
`GAMEPADII_LIVE_REBOOT=1` / `GAMEPADII_LIVE_WRITE=1` is set; the control
channel ones also need `GAMEPADII_LIVE_CHANNEL=1`. Over the channel,
SET_INPUT_MODE and `profile switch --save` write flash, and even a RAM-only
`profile switch` changes the live mapping: treat all three as writes.
The one exception, allowed by the user: after a web-config round trip
(`info`/`backup` over the channel), gamepadii itself switches back, RAM only,
to a profile the restart dropped; running `info`/`backup` needs no extra OK
for that.
Never run `gamepadii setup udev --install` for the user; it goes through
pkexec and is theirs to approve.

## Git

Until TUI work starts, commit straight to `main` (the MVP and the firmware
spikes). From the first TUI change on: feature branches (`feat/…`, `fix/…`,
`docs/…`), an open PR per change, Conventional Commits with scopes, and `main`
protected by a ruleset. Never commit to `main` directly after that point.

## References

- `~/Projects/GP2040-CE`: upstream firmware (API in `src/webconfig.cpp`,
  schema in `proto/`, VID:PIDs in `headers/drivers/*/*Descriptors.h`).
- `~/Projects/gp2040ce-binary-tools`: PICOBOOT config dump and edit.
- Sibling projects for conventions: `~/Projects/plugii` (scaffolding, CI,
  fixtures, TUI) and `~/Projects/sofleii/sofleii` (layering, output
  contract, errors).

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
`status`, `install`, `outdated`, `update`, `release-preview`. CI is
`.github/workflows/ci.yml`; every action is pinned to a commit SHA with its
version in a comment. Dependabot (`.github/dependabot.yml`) keeps the crates
and the actions current.

Releases: release-plz keeps a release PR open; a merge of it makes the tag
and the GitHub release (`docs/RELEASE.md`, `release-plz.toml`). The commit
types set the version and the CHANGELOG, so use them correctly. Do not merge
a release PR without the user's OK: it publishes a version.

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
- Tests that run the binary point `XDG_CACHE_HOME`, `XDG_DATA_HOME` and
  `XDG_CONFIG_HOME` at temporary directories (`exit.rs`'s `gamepadii_in`):
  commands write the device cache and backups there, and read the profile
  library there.

## Writing

All text uses ASD-STE100 Simplified Technical English (STE): issues, PRs,
commit messages, docs, code comments, CLI messages, and messages to the user.

- Write short sentences: 20 words or fewer for instructions, 25 or fewer for
  descriptions. Put one instruction in each sentence.
- Use the active voice. Use each word with one meaning only.
- Technical words that are not in STE must be in `docs/GLOSSARY.md`, with
  one meaning. If you use a new one, add it in the same change.
- Every issue and every PR starts with the **intent**: what the user wants
  to do, and why the change is necessary. Write it so that a person who does
  not know the code understands it. Then give the details. The templates are
  in `.github/`.
- Keep it short. Give the steps that show the problem, the result you
  expect, and the result you get. Put long technical analysis in a
  linked file, not in the issue body.
- Text for third-party projects (for example GP2040-CE) is written or
  approved by the user, one item at a time.

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
- Never call `/api/getConfig` on stock v0.7.12: on the S16 (Haute42COSMOX,
  build 0014e4a) it hangs the device's network stack until replugged. Back
  up via the per-section `get*` endpoints. It is allowed on fork builds with
  the fixes, only through `api::fork` with a `ForkFixes` from the capability
  check (`Client::fork_fixes`; the rule is in `docs/DESIGN.md`, "Fork
  firmware"); `backup` adds it as `full_config` there.
- Never call `/api/getI2CPeripheralMap` on stock either: it runs a live I2C
  bus scan and hangs the S16 the same way (fork builds: same check). The
  client reads only from an explicit allowlist of endpoints verified safe
  (the list is in `docs/DESIGN.md`), never "anything named `get*`".

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

`main` is protected by a ruleset: no direct pushes, no force-pushes, and no
deletion. Every change goes through a PR, and CI must pass (clippy, rustfmt,
test, MSRV).

- One branch per change: `feat/…`, `fix/…`, `docs/…`, `chore/…`, `refactor/…`.
  Parallel agents each work in their own git worktree under
  `.claude/worktrees/`.
- Conventional Commits with scopes (`feat(cli): …`, `fix(core): …`).
- Keep the PR description current as the branch changes, and link the issue
  it closes (`Closes #N`).
- Merge once CI is green. Squash-merge small PRs; use a merge commit when the
  individual commits matter.

## References

- `~/Projects/GP2040-CE`: upstream firmware (API in `src/webconfig.cpp`,
  schema in `proto/`, VID:PIDs in `headers/drivers/*/*Descriptors.h`).
- `~/Projects/gp2040ce-binary-tools`: PICOBOOT config dump and edit.
- Sibling projects for conventions: `~/Projects/plugii` (scaffolding, CI,
  fixtures, TUI) and `~/Projects/sofleii/sofleii` (layering, output
  contract, errors).

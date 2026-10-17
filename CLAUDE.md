# gamepadii

Rust CLI (then ratatui TUI, then Tauri GUI) for GP2040-CE gamepads; the
first target is a Haute42 S16. The agreed plan, with decisions, firmware
facts and milestones, is in `docs/PLAN.md`: it is the source of truth.

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
- Layering: `discovery` → `transport` → `client` → `commands` → `output`.
- No async runtime: std threads + channels.
- sysfs readers take a `Root` (`/` or a fixture dir) and never hard-code
  `/sys`, so tests run against fixture trees under `tests/fixtures/`.
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

## Web-config API hazards

The client must guard against these (details in `docs/PLAN.md`):

- `setGamepadOptions` zeroes every key it isn't sent: always read-modify-write
  the full object.
- The server ignores the HTTP method: never GET a `set*` endpoint (it replays
  the previous POST body).
- Serialise requests (one at a time); POST bodies are capped at 16 KB.
- `resetSettings` is a destructive GET: never call it.

## Hardware

Agents may read from the S16 and reboot it between modes; never write
settings to the device without the user's explicit OK, and take a backup
first.

## Git

The MVP (through milestone 3) lands on `main`. After that: feature branches
(`feat/…`, `fix/…`, `docs/…`), an open PR per change, Conventional Commits
with scopes, `main` protected — never commit to `main` directly.

## References

- `~/Projects/GP2040-CE`: upstream firmware (API in `src/webconfig.cpp`,
  schema in `proto/`, VID:PIDs in `headers/drivers/*/*Descriptors.h`).
- `~/Projects/gp2040ce-binary-tools`: PICOBOOT config dump and edit.
- Sibling projects for conventions: `~/Projects/plugii` (scaffolding, CI,
  fixtures, TUI) and `~/Projects/sofleii/sofleii` (layering, output
  contract, errors).

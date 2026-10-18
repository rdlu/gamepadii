# s16-main

**Synthetic.** The S16 as it would answer on stock upstream GP2040-CE main
(8013b41, unreleased, `v0.7.12-119-g8013b41`), for the mock web-config
server. No pad has run main yet: every file here was built from the stock
v0.7.12 capture (`../s16-v0.7.12`) reshaped to what main's getters write
(`src/webconfig.cpp` at 8013b41). Use it for shape, not values; replace
the files with real captures once the S16 runs main.

It is an overlay: `BASE` names the directory every endpoint not here comes
from, and `REMOVED` lists the endpoints main no longer has (`getCustomTheme`,
`getHETriggerCalibration`, `getHETriggerOptions`), which the mock answers with
a 404 although the base has them.

- `getFirmwareVersion.json`: main's version string and commit.
- `getGamepadOptions.json`: the capture plus main's 15 `switch2Pro*` keys
  after `ps4ControllerIDMode`, in the getter's order (values made up).
- `getDisplayOptions.json`: the capture plus `displayContrast` (255, the
  `DISPLAY_CONTRAST` default) after `inputHistoryRow`.
- `getLedOptions.json`: main's short form (no LED layout, button map,
  player-LED indexes or case RGB), `brightnessMaximum` as a percentage (50).
- `getButtonLayouts.json`: the capture without `ledLayout`, which main
  dropped.
- `getAnimationProtoOptions.json`: four LED profiles with the effects of
  main's board defaults (rainbow synced, static, rainbow rotate, chase
  horizontal ping-pong with burst); colours, timings and brightness made up.
- `getLightsDataOptions.json`: the Haute42COSMOX T16 light layout
  (`LIGHT_DATA_DEFAULT` in main's `configs/Haute42COSMOX/BoardConfig.h`).
- `getBootModeOptions.json`: the getter's shape with nothing mapped.
- `getBoardDefinition.json`: the RP2040's pins, with the actions of the
  capture's base profile as `usedPins`.
- `getHETriggerCalibrations.json`: 32 unconfigured triggers.

Inherited unchanged although main's getter differs: `getAddonsOptions` (main
adds analog calibration, ADS1115 and profile-slider keys and reshapes the
HE trigger mux pins). gamepadii only stores it verbatim in backups.

The mock hangs on `getConfig` and `getI2CPeripheralMap` for this build, as
stock main still does on `getConfig`.

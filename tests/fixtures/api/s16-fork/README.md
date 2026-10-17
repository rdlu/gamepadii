# s16-fork

The S16 on our GP2040-CE fork (`rdlu/GP2040-CE`, the `cc` build,
v0.7.12-6-gf507f0b), for the mock web-config server. It is an overlay: `BASE`
names the directory every endpoint not here comes from (`../s16-v0.7.12`, the
real stock capture).

- `getFirmwareVersion.json`: the fork build's version string and commit.
- `getConfig.json`: **synthetic**. Built from the stock capture's per-section
  shapes (gamepad, hotkey, keyboard, display, LED, addon, peripheral and pin
  options, the profiles) in the layout `getConfig` uses, plus an
  `animationOptions` object (`baseAnimationIndex` 2, `brightness` 3, colours
  made up). It is not a capture of the pad: use it for shape, not values.
- `getI2CPeripheralMap.json`: the OLED at `0x3C` on `i2c0`, as the fixed
  builds answered in their stress test.

On stock firmware (a bare release tag such as `v0.7.12`) the mock hangs on
`getConfig` and `getI2CPeripheralMap` instead, as the real pad does, and logs
the request, so tests can check that stock is never asked.

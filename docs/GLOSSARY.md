# Glossary

gamepadii text uses ASD-STE100 Simplified Technical English (STE). This
glossary gives the technical names and technical verbs that are not in the
STE dictionary. Each term has one meaning in this project. Use the term only
with that meaning.

If you write a technical word that is not in STE and not in this list, add
it here in the same change.

## Rules for entries

- Give the term, its category (technical name, technical verb, or name of a
  thing on the device), and one meaning.
- Write the meaning in STE.
- If an STE word can replace the term, write "Use instead: ..." and do not
  use the term.

## Technical names

| Term | Meaning |
|---|---|
| allowlist | The list of web-config endpoints that gamepadii can read safely. gamepadii does not send a request to an endpoint that is not on this list. |
| backup | A file that contains a copy of the settings of the pad. gamepadii writes it before it changes the settings. |
| board ID | The unique ID of the flash chip of the pad: 16 hex digits. The control channel gives it from protocol 1.3. It does not change, but pads with the same flash chip can have the same board ID. |
| board config | The name of the hardware definition that the firmware was built for. The S16 uses `Haute42COSMOX`. |
| boot mode | The mode that the pad starts in: gamepad mode, web-config, or BOOTSEL. |
| BOOTSEL | A mode of the RP2040 chip. In this mode, the computer shows the pad as a USB drive, and you can install firmware. |
| build | One firmware file (UF2) made from one commit, with one set of options. |
| cache | Files on the computer where gamepadii keeps data about each pad (for example, the profile names), so that it can show this data when the pad is in a gamepad mode. |
| capability | A function that the firmware tells the host it has. The HELLO reply gives the capabilities as bits. |
| channel | Short name for the control channel. |
| CLI | The command-line interface: you type a command, for example `gamepadii status`, and gamepadii writes the result. |
| commit | One recorded change in git. A short hash (for example `0a23ea3`) identifies it. |
| config, config sector | The settings of the pad. The firmware keeps them in a 32 KB area of flash memory at address `0x101F8000`. |
| control channel | The USB connection that our firmware adds. gamepadii uses it to read the status of the pad and to change the profile, the input mode, and the LEDs, without a button press. |
| custom theme | The LED colors that the user sets for each button, one color when the button is released and one color when it is pressed. |
| endpoint | One address of the web-config server, for example `/api/getGamepadOptions`. |
| firmware line | A family of firmware versions: the v0.7.12 release, or the upstream main branch. The two lines have different settings and endpoints. |
| firmware | The software that operates on the pad. |
| fixture | A file with recorded data from a real pad. The tests use fixtures in place of the pad. |
| fork | Our copy of the GP2040-CE firmware (`rdlu/GP2040-CE`), with our fixes and the control channel. |
| gamepad mode | A mode in which the pad operates as a game controller (for example XInput or Switch). |
| GP2040-CE | The open-source firmware that the S16 uses. |
| GPIO, pin | A connection on the RP2040 chip. Each button connects to one pin. |
| hash | A short code that identifies one commit. |
| HELLO | The first request on the control channel. The reply tells the host that the firmware has the control channel, and which capabilities it has. |
| host | The computer that the pad is connected to. |
| hotkey | A set of buttons that you push together to do an action on the pad, for example to change the LED brightness. |
| input mode | The type of controller that the pad shows to the host: XInput, Switch, PS4, keyboard, and others. |
| KDL | A text format for configuration files. gamepadii will keep its profile library in KDL files. |
| LED | A light under a button. The S16 has one LED for each button. |
| lights profile | On main firmware, a set of LED effects: one when a button is released, one when it is pressed, and one for the case. The pad has 4, numbered from 0. The LEDs can also be off. |
| main (upstream main) | The newest code of GP2040-CE. It is not a release. |
| OLED | The small display on the pad. It shows the buttons and the active profile. |
| PR (pull request) | A request to merge a branch into `main` on GitHub. |
| profile | A set of button mappings on the pad. The pad has up to 6 profiles. |
| profile slot | One of the 6 places on the pad for a profile. Profile 1 is the board's own mapping. gamepadii can write slots 2 to 6. |
| RAM only | A change that the pad keeps until it restarts. The pad does not write it to flash memory. |
| release | A firmware version that the GP2040-CE project published with a version number, for example v0.7.12. |
| S16 | The Haute42 S16, the first pad that gamepadii supports. |
| slot file | A JSON file with the pin mappings and the label of one profile. `gamepadii profile show --json` makes one, and `gamepadii profile write` reads one. |
| simulated pad | A model of the S16 in the code. Tests and demos use it in place of a real pad. |
| snapshot | A recorded output of gamepadii that a test compares with the new output. |
| STALL | The USB reply that tells the host that the firmware did not accept a request. |
| stock | Firmware from the GP2040-CE project, without our changes. |
| test-boot build | A build for tests only. It starts in web-config after each installation, so that a test can continue without a person. |
| TUI | The terminal user interface: a full-screen view in the terminal that you operate with keys. |
| UF2 | The file format for firmware. You copy a UF2 file to the pad in BOOTSEL mode. |
| upstream | The GP2040-CE project and its maintainers. |
| VID:PID | Two numbers that a USB device sends to identify itself (vendor and product). |
| waybar | A status bar for Linux desktops. `gamepadii status --format waybar` gives one line for it. |
| web-config | A mode in which the pad operates as a small web server. You use a web browser to change the settings. |
| worktree | A second working folder for one git repository. Each agent uses its own worktree. |

## Technical verbs

| Term | Meaning |
|---|---|
| build | To make a firmware file or a program from source code. |
| commit | To record a change in git. |
| flash | To write firmware or data to the flash memory of the pad. |
| merge | To add the changes of one branch to another branch. |
| push | To send commits from the computer to GitHub. |
| reboot | To restart the pad. Use instead: restart (STE approved), when you can. |

## Words not to use

| Do not use | Use instead |
|---|---|
| hang, freeze | stop (to operate): "the pad stops answering" |
| leverage, utilize | use |
| leverless | (keep only as the product type; explain it once: "a controller with buttons in place of a joystick") |
| round trip | go to web-config and back |
| spike | test |

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
| ADC | The part of the RP2040 chip that reads an analog value, for example the position of a stick. |
| alias | A second name for a physical button in a board file, from a label on the case. On the S16, `back` is an alias of `share`. A profile can use the name or the alias. |
| allowlist | The list of web-config endpoints that gamepadii can read safely. gamepadii does not send a request to an endpoint that is not on this list. |
| backup | A file that contains a copy of the settings of the pad. gamepadii writes it before it changes the settings. |
| base profile, base | A profile in the library that another profile starts from. The other profile gets each part that it does not give from its base. A base can have its own base. |
| board file | A KDL file that gamepadii ships for one pad model. It gives the name of each physical button and its GPIO, so that a profile names buttons and not GPIOs. |
| board ID | The unique ID of the flash chip of the pad: 16 hex digits. The control channel gives it from protocol 1.3. It does not change, but pads with the same flash chip can have the same board ID. |
| board config | The name of the hardware definition that the firmware was built for. The S16 uses `Haute42COSMOX`. |
| boot number | The number of a boot in the log of the pad. It is 0 when a new log starts, and it goes up by 1 at each restart that keeps the log. |
| boot mode | The mode that the pad starts in: gamepad mode, web-config, or BOOTSEL. |
| BOOTSEL | A mode of the RP2040 chip. In this mode, the computer shows the pad as a USB drive, and you can install firmware. |
| build | One firmware file (UF2) made from one commit, with one set of options. |
| cache | Files on the computer where gamepadii keeps data about each pad (for example, the profile names), so that it can show this data when the pad is in a gamepad mode. |
| capability | A function that the firmware tells the host it has. The HELLO reply gives the capabilities as bits. |
| channel | Short name for the control channel. |
| chunk | One part of the config blob that gamepadii sends with one WRITE_CONFIG: at most 316 bytes. |
| CLI | The command-line interface: you type a command, for example `gamepadii status`, and gamepadii writes the result. |
| colours file | A JSON file with the custom colours. `gamepadii leds colors --json` makes one, and `gamepadii leds colors write` reads one. |
| commit | One recorded change in git. A short hash (for example `0a23ea3`) identifies it. |
| completion | The words that the shell shows when you push Tab: the commands, the options and their values. `gamepadii completions` gives the script for each shell. |
| config blob | The saved config of the pad as one block of bytes: a `Config` message in protobuf, as the firmware keeps it in flash. Control channel 1.6 reads and writes it. A backup keeps it in hex. |
| config, config sector | The settings of the pad. The firmware keeps them in a 32 KB area of flash memory at address `0x101F8000`. |
| control channel | The USB connection that our firmware adds. gamepadii uses it to read the status of the pad and to change the profile, the input mode, and the LEDs, without a button press. |
| CRC-32 | A 32-bit number that the pad and gamepadii calculate from a block of bytes (the zlib method). When one byte changes, the number changes. |
| custom colour | On main firmware, one of 16 RGB colours that the user sets. A lights profile uses custom colour N as colour number 14 + N. All the lights profiles share them. |
| custom theme | The LED colors that the user sets for each button, one color when the button is released and one color when it is pressed. |
| D-pad | The four direction buttons: up, down, left and right. |
| debounce | A check that a button change stays for some reads, so that a short electrical change (a bounce) does not count as a press. |
| diff | The list of the differences between two versions of a thing. `gamepadii library diff` shows the differences between a library profile and a slot of the pad. |
| Dependabot | The GitHub service that opens PRs to update the crates and the GitHub Actions. |
| effect | The LED animation of a lights profile, for example `static-color` or `rainbow-synced`. A lights profile has one when a button is released, one when it is pressed, and one for the case. |
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
| hex | A number or data written with the digits 0–9 and the letters a–f, two for each byte. |
| host | The computer that the pad is connected to. |
| hotkey | A set of buttons that you push together to do an action on the pad, for example to change the LED brightness. |
| input mode | The type of controller that the pad shows to the host: XInput, Switch, PS4, keyboard, and others. |
| input snapshot | The reply to READ_INPUT (control channel 1.7): the pins that are pressed at that time, and the gamepad state. |
| KDL | A text format for configuration files. gamepadii keeps its profile library in KDL 2.0 files. |
| label | The name of a profile that the pad keeps and shows, at most 16 bytes. |
| LED | A light under a button. The S16 has one LED for each button. |
| library, profile library | The profiles that gamepadii keeps on the computer, in KDL files in `~/.config/gamepadii/profiles/`, and the example profiles that it ships. The library can have more profiles than the pad. |
| lights file | A JSON file with the effects and colours of one lights profile. `gamepadii leds profile show --json` makes one, and `gamepadii leds profile write` reads one. |
| lights profile | On main firmware, a set of LED effects: one when a button is released, one when it is pressed, and one for the case. The pad has 4, numbered from 0. The LEDs can also be off. |
| log | On our firmware from control channel v1.8, the list of the last 64 events of the pad (boots, panics, commands, saves). `gamepadii log` shows it. |
| main (upstream main) | The newest code of GP2040-CE. It is not a release. |
| name system | The names of the logical buttons on one console: `gp2040` (B1, S2), `xbox` (A, Start), `playstation` (Cross, Options) or `switch` (B, Plus). A library file gives its name system with a `names` node. |
| OLED | The small display on the pad. It shows the buttons and the active profile. |
| panic | A firmware failure that stops the firmware. Our firmware writes the message to the log and then restarts the pad. |
| pin scan | A temporary state that PIN_SCAN starts (control channel 1.7). Each pin with no action and no function becomes an input, so that gamepadii can read a button on it. It changes no setting. |
| PAT (personal access token) | A secret text that gives a program access to GitHub with the rights of a user. The release workflow uses one. |
| PR (pull request) | A request to merge a branch into `main` on GitHub. |
| profile | A set of button mappings on the pad. The pad has up to 6 profiles. |
| profile slot | One of the 6 places on the pad for a profile. Profile 1 is the board's own mapping. gamepadii can write slots 2 to 6. |
| protobuf | A binary format for structured data. The firmware keeps its settings in this format. |
| pull-up | A resistor in the chip that keeps an input pin high when nothing pushes it low. A pushed button makes the pin low. |
| RAM only | A change that the pad keeps until it restarts. The pad does not write it to flash memory. |
| release | A version that a project published with a version number: a GP2040-CE firmware version (v0.7.12) or a gamepadii version (v0.1.0). |
| release PR | The PR that release-plz keeps open. It has the next gamepadii version and its CHANGELOG section. A merge of it makes the release. |
| release-plz | The tool that makes the release PR, and the tag and the GitHub release after the merge. |
| RGB | A colour as three values: red, green and blue, 0 to 255 each. gamepadii writes it in hex, as `#rrggbb`. |
| S16 | The Haute42 S16, the first pad that gamepadii supports. |
| sequence number | The number of one record in the log of the pad. The pad counts the records from 0, from the start of the log. |
| slot file | A JSON file with the pin mappings and the label of one profile. `gamepadii profile show --json` makes one, and `gamepadii profile write` reads one. |
| simulated pad | A model of the S16 in the code. Tests and demos use it in place of a real pad. |
| snapshot | A recorded output of gamepadii that a test compares with the new output. |
| SOCD | The rule that the firmware applies when two opposite directions are pressed at the same time (for example, left and right). |
| STALL | The USB reply that tells the host that the firmware did not accept a request. |
| stick | An analog control of a gamepad, with an X and a Y axis. The S16 has no stick: the firmware gives the axes at the centre. |
| stock | Firmware from the GP2040-CE project, without our changes. |
| tag | A git name for one commit. A gamepadii release has the tag `vX.Y.Z`. |
| test-boot build | A build for tests only. It starts in web-config after each installation, so that a test can continue without a person. |
| theme | In the profile library, a `theme` node: the colours and effects of one lights profile, by button name. gamepadii makes a theme file from it. Not the same as a custom theme. |
| theme file | A JSON file with custom colours and the fields of one lights profile. `gamepadii leds theme show --json` makes one, and `gamepadii leds theme apply` reads one. |
| trigger | An analog control of a gamepad, L2 or R2, with a value from 0 to 255. |
| TUI | The terminal user interface: a full-screen view in the terminal that you operate with keys. |
| UF2 | The file format for firmware. You copy a UF2 file to the pad in BOOTSEL mode. |
| upload | The chunks that gamepadii sent to the free space of the pad, before COMMIT_CONFIG. A save on the pad or a restart removes it. |
| upstream | The GP2040-CE project and its maintainers. |
| VID:PID | Two numbers that a USB device sends to identify itself (vendor and product). |
| watchdog | A timer in the RP2040 chip that restarts it. The firmware uses it for a soft reboot. A watchdog reset keeps the RAM, thus the log stays. |
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
| parse | To read a text file and find its parts (for example, the profiles in a KDL file). A file that "does not parse" has an error in its syntax. |
| pull | To read the profiles of the pad and write them as KDL (`gamepadii library pull`). |
| push | To send commits from the computer to GitHub. |
| reboot | To restart the pad. Use instead: restart (STE approved), when you can. |
| restore | To put back a state from before: the settings of a backup (`gamepadii restore`), or the profile that a restart removed. |
| ship | To include a file in gamepadii, so that each installation has it (for example, the board files and the example profiles). |

## Words not to use

| Do not use | Use instead |
|---|---|
| hang, freeze | stop (to operate): "the pad stops answering" |
| leverage, utilize | use |
| leverless | (keep only as the product type; explain it once: "a controller with buttons in place of a joystick") |
| round trip | go to web-config and back |
| spike | test |

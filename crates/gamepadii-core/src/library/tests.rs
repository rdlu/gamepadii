use std::collections::BTreeMap;

use super::*;
use crate::channel::sim::SimChannel;
use crate::channel::{Slot, SlotPin};
use crate::slots::{self, SlotWrite};
use crate::transport::mock::Mock;

const S16: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/api/s16-v0.7.12"
);

fn file(name: &str) -> Origin {
    Origin::File(PathBuf::from(name))
}

/// A library of `files` and no examples.
fn lib(files: &[(&str, &str)]) -> Library {
    Library::from_sources(
        files
            .iter()
            .map(|(n, t)| (file(n), t.to_string()))
            .collect(),
        false,
    )
}

/// The one error of a file that does not parse.
fn parse_error(text: &str) -> String {
    parse::profiles(file("a.kdl"), text)
        .unwrap_err()
        .to_string()
}

/// The error of resolving `name`.
fn resolve_error(files: &[(&str, &str)], name: &str) -> String {
    let l = lib(files);
    assert_eq!(l.errors, [], "the files parse");
    let e = l.resolve(name).unwrap_err().to_string();
    e.strip_prefix("usage: ").unwrap_or(&e).to_string()
}

/// The slots of the S16 fixture: profile 1, then the enabled ones.
fn s16_slots() -> Vec<Slot> {
    let sim = SimChannel::from_mock(&Mock::new(S16), None);
    (1..=6).map(|n| sim.slot(n)).filter(Slot::enabled).collect()
}

const BASE: &str = r#"
profile "Base" {
    board "haute42-s16"
    pins {
        up Up
        k1 B1
    }
}
"#;

#[test]
fn a_profile_parses() {
    let text = r#"
// A comment.
profile "Hades II" {
    base "Base"
    label "Hades"
    input-mode switch-pro
    pins {
        default none
        up B2; k1 "BUTTON_PRESS_B1"
        k2 5
        turbo fn
        k4 custom-button-combo button-mask=0x3 dpad-mask=1
    }
}
"#;
    let [d] = parse::profiles(file("a.kdl"), text)
        .unwrap()
        .try_into()
        .unwrap();
    let pin = |action| Pin {
        action,
        ..Pin::default()
    };
    assert_eq!(
        d.profile,
        Profile {
            name: "Hades II".into(),
            base: Some("Base".into()),
            board: None,
            label: Some("Hades".into()),
            input_mode: Some(InputMode::SwitchPro),
            default: Some(pin(-10)),
            pins: vec![
                ("up".into(), pin(6)),
                ("k1".into(), pin(5)),
                ("k2".into(), pin(5)),
                ("turbo".into(), pin(19)),
                (
                    "k4".into(),
                    Pin {
                        action: 40,
                        button_mask: 3,
                        dpad_mask: 1,
                    }
                ),
            ],
        }
    );
    assert_eq!(d.at.node, Place { line: 3, column: 1 });
    assert_eq!(d.at.base, Some(Place { line: 4, column: 5 }));
    assert_eq!(d.at.default, Some(Place { line: 8, column: 9 }));
    assert_eq!(
        d.at.pins[1],
        Place {
            line: 9,
            column: 16
        }
    );
}

#[test]
fn bad_files_name_the_line_and_column() {
    let cases = [
        (
            "profile \"A\" {\n    pins {\n        up sparkle\n",
            "a.kdl:2:10: KDL: No closing '}' for child block",
        ),
        (
            "profile \"A\" {\n    pins {\n        up sparkle\n    }\n}\n",
            "a.kdl:3:12: unknown action \"sparkle\"",
        ),
        (
            "profile \"A\" {\n    pins { up B1; up B2; }\n}\n",
            "a.kdl:2:19: button up is twice in pins",
        ),
        (
            "profile \"A\" {\n    input-mode xinput\n    input-mode keyboard\n}\n",
            "a.kdl:3:5: input-mode conflict: the profile gives two input modes",
        ),
        (
            "profile \"A\" {\n    input-mode xinput\n    input-mode xinput\n}\n",
            "a.kdl:3:5: input-mode is twice in the profile",
        ),
        (
            "profile \"A\" { input-mode webconfig; }\n",
            "a.kdl:1:15: \"webconfig\" is not an input mode",
        ),
        (
            "profile \"A\" { pins { up 139; }; }\n",
            "a.kdl:1:25: action 139: not a GpioAction",
        ),
        (
            "profile \"A\" { pins { up B1 x=1; }; }\n",
            "a.kdl:1:28: unknown property \"x\"",
        ),
        (
            "profile \"A\" { colour red; }\n",
            "a.kdl:1:15: unknown node \"colour\" in a profile",
        ),
        (
            "keymap \"kb-main\" { Up KeyW; }\n",
            "a.kdl:1:1: gamepadii does not read keymap nodes yet",
        ),
        ("profile { }\n", "a.kdl:1:1: profile: give one name"),
        (
            "profile \"A\" { base; }\n",
            "a.kdl:1:15: base: give one value",
        ),
        // KDL 1.0 is not KDL 2.0: `true` is `#true` now.
        ("profile \"A\" { label true; }\n", "a.kdl:1:"),
    ];
    for (text, want) in cases {
        let got = parse_error(text);
        assert!(got.starts_with(want), "{text:?}:\n got {got}\nwant {want}");
    }
    // The column counts characters, not bytes.
    let got = parse_error("profile \"Ü\" { pins { up sparkle; }; }\n");
    assert!(got.starts_with("a.kdl:1:25:"), "{got}");
}

#[test]
fn resolving_checks_bases_buttons_and_labels() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "profile \"A\" {\n    base \"Nope\"\n}\n",
            "A",
            "b.kdl:2:5: base profile \"Nope\" is not in the library",
        ),
        (
            "profile \"A\" { base \"B\"; }\nprofile \"B\" {\n  base \"A\"\n}\n",
            "A",
            "b.kdl:3:3: a loop of bases: A -> B -> A",
        ),
        (
            "profile \"A\" { base \"A\"; }\n",
            "A",
            "b.kdl:1:15: a loop of bases: A -> A",
        ),
        (
            "profile \"A\" {\n  base \"Base\"\n  pins { thumb B1; }\n}\n",
            "A",
            "b.kdl:3:10: unknown button \"thumb\": haute42-s16 has left, down",
        ),
        (
            "profile \"A\" { pins { up B1; }; }\n",
            "A",
            "b.kdl:1:1: profile \"A\" has no board",
        ),
        (
            "profile \"A\" { board \"pico\"; }\n",
            "A",
            "b.kdl:1:15: no board file \"pico\" (known: haute42-s16)",
        ),
        (
            "profile \"A\" {\n  base \"K\"\n  input-mode xinput\n}\n\
             profile \"K\" { base \"Base\"; input-mode keyboard; }\n",
            "A",
            "b.kdl:3:3: input-mode conflict: a base needs keyboard, this profile xinput",
        ),
        (
            "profile \"A long name for a pad\" { base \"Base\"; }\n",
            "a long name for a pad",
            "b.kdl:1:1: the name (the label when there is no label) \"A long name for a pad\" \
             is 21 bytes; the pad keeps 16",
        ),
        (
            "profile \"A\" {\n  base \"Base\"\n  label \"A long label for a pad\"\n}\n",
            "A",
            "b.kdl:3:3: the label \"A long label for a pad\" is 22 bytes",
        ),
    ];
    for (text, name, want) in cases {
        let got = resolve_error(&[("a.kdl", BASE), ("b.kdl", text)], name);
        assert!(got.starts_with(want), "{text:?}:\n got {got}\nwant {want}");
    }
    let got = resolve_error(&[("a.kdl", BASE)], "Other");
    assert!(got.contains("no profile \"Other\" in the library"), "{got}");
    // An error is a usage error: exit 3.
    let e = lib(&[("a.kdl", "profile \"A\" { }\n")])
        .resolve("A")
        .unwrap_err();
    assert_eq!(e.exit_code(), crate::error::exit::USAGE);
}

#[test]
fn a_profile_takes_what_it_leaves_out_from_its_base() {
    let text = r#"
profile "Game" {
    base "Base"
    input-mode keyboard
    pins { up B2; }
}
profile "Game 2" {
    base "Game"
    label "G2"
    pins { k1 L1; }
}
profile "Quiet" {
    base "Game 2"
    pins {
        default none
        k1 B3
    }
}
"#;
    let l = lib(&[("a.kdl", BASE), ("b.kdl", text)]);
    let r = l.resolve("game 2").unwrap();
    assert_eq!(r.name, "Game 2");
    assert_eq!(r.bases, ["Game", "Base"]);
    assert_eq!(r.label, "G2");
    assert_eq!(r.input_mode, Some(InputMode::Keyboard));
    assert_eq!(
        (r.board.as_str(), r.firmware_board.as_str()),
        ("haute42-s16", "Haute42COSMOX")
    );
    let pin = |r: &Resolved, b: &str| {
        let p = r.pins.iter().find(|p| p.button == b).unwrap();
        (p.gpio, p.action_name.clone(), p.from.clone())
    };
    let s = |v: &str| Some(v.to_string());
    assert_eq!(pin(&r, "up"), (2, s("B2"), s("Game")));
    assert_eq!(pin(&r, "k1"), (6, s("L1"), s("Game 2")));
    assert_eq!(pin(&r, "k2"), (7, None, None), "not named: profile 1");
    assert_eq!(r.pins.len(), 21, "every button of the board");
    // The label of a base does not carry over.
    assert_eq!(l.resolve("Quiet").unwrap().label, "Quiet");
    // The slot file has the buttons with an action, by GPIO.
    let f = r.slot_file();
    assert_eq!(f.board.as_deref(), Some("Haute42COSMOX"));
    assert_eq!(f.label.as_deref(), Some("G2"));
    let gpios: Vec<(u8, i16)> = f.pins.iter().map(|p| (p.gpio, p.action.0)).collect();
    assert_eq!(gpios, [(2, 6), (6, 9)]);
    // `default` gives every other button of the board.
    let q = l.resolve("Quiet").unwrap();
    assert_eq!(pin(&q, "k2"), (7, s("none"), s("Quiet")));
    assert_eq!(pin(&q, "up"), (2, s("B2"), s("Game")));
    assert_eq!(q.slot_file().pins.len(), 21);
}

#[test]
fn profiles_survive_a_round_trip() {
    let text = r#"
profile "Hades \"II\"" {
    base "Base"
    label "Hades\\2"
    input-mode switch2-pro
    pins {
        default assigned-to-addon
        up B2
        k4 custom-button-combo button-mask=0x3 dpad-mask=0x1
    }
}
profile "Empty" { }
"#;
    let mut all = vec![parse::profiles(file("a.kdl"), text).unwrap()];
    for (name, text) in EXAMPLES {
        all.push(parse::profiles(Origin::Shipped(name), text).unwrap());
    }
    for defined in all {
        let profiles: Vec<Profile> = defined.into_iter().map(|d| d.profile).collect();
        let text = write::file(&["A header.".into()], &profiles);
        let again: Vec<Profile> = parse::profiles(file("b.kdl"), &text)
            .unwrap_or_else(|e| panic!("{e}\n{text}"))
            .into_iter()
            .map(|d| d.profile)
            .collect();
        assert_eq!(again, profiles, "{text}");
        assert_eq!(write::file(&["A header.".into()], &again), text);
    }
}

#[test]
fn the_shipped_files_pass_the_checks() {
    let l = Library::from_sources(Vec::new(), true);
    let report = l.list();
    assert_eq!(report.errors, []);
    assert!(report.profiles.iter().all(|p| p.ok && p.shipped));
    let names: Vec<&str> = report.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Default",
            "SilkSong",
            "Celeste",
            "DKC",
            "TGX",
            "My Game",
            "My Game Alt"
        ]
    );
}

#[test]
fn a_user_profile_hides_a_shipped_one_and_names_are_unique() {
    let mine = "profile \"Celeste\" { base \"Default\"; pins { up B2; }; }\n";
    let l = Library::from_sources(
        vec![
            (file("mine.kdl"), mine.into()),
            (file("again.kdl"), mine.into()),
            (file("broken.kdl"), "profile \"X\" {".into()),
        ],
        true,
    );
    let r = l.resolve("Celeste").unwrap();
    assert_eq!(r.file, file("mine.kdl"));
    assert_eq!(r.pins[4].action_name.as_deref(), Some("B2"));
    let report = l.list();
    let errors: Vec<String> = report.errors.iter().map(|e| e.to_string()).collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors[0].starts_with("again.kdl:1:1: profile \"Celeste\" is also in mine.kdl"),
        "{errors:?}"
    );
    assert!(errors[1].starts_with("broken.kdl:1:"), "{errors:?}");
    assert_eq!(
        report
            .profiles
            .iter()
            .filter(|p| p.name == "Celeste")
            .count(),
        1
    );
}

#[test]
fn the_s16_board_file_matches_the_fixtures() {
    let board = board::named("haute42-s16").unwrap();
    assert_eq!(board.firmware_board, "Haute42COSMOX");
    let gpio = |name: &str| board.button(name).unwrap().gpio;
    assert_eq!(
        [
            gpio("up"),
            gpio("m1"),
            gpio("down"),
            gpio("left"),
            gpio("right"),
            gpio("turbo")
        ],
        [2, 27, 3, 5, 4, 14]
    );
    let base = &s16_slots()[0];
    assert_eq!(base.label, "Default");
    // Profile 1 has a button press on each button of the board, but turbo:
    // the owner gave GPIO 14 the menu toggle.
    for b in &board.buttons {
        let action = base.pins[usize::from(b.gpio)].action;
        match b.name.as_str() {
            "turbo" => assert_eq!(action, 78, "MENU_NAVIGATION_TOGGLE"),
            _ => assert!(
                crate::buttons::Button::from_action(action.into()).is_some(),
                "{}: GPIO {} has action {action}",
                b.name,
                b.gpio
            ),
        }
    }
    // And each GPIO of profile 1 with an action has a button.
    for (gpio, p) in (0u8..).zip(&base.pins) {
        if !matches!(p.action, -10 | -5 | 0) {
            assert!(board.by_gpio(gpio).is_some(), "GPIO {gpio}: no button");
        }
    }
    assert!(board::for_firmware("Haute42COSMOX").is_some());
    assert!(board::for_firmware("Pico").is_none());
}

#[test]
fn bad_board_files_are_refused() {
    let cases = [
        (
            "board \"x\" { firmware-board \"X\"; button a gpio=1; button b gpio=1; }",
            "1:50: GPIO 1 has two buttons",
        ),
        (
            "board \"x\" { firmware-board \"X\"; button a gpio=1; button a gpio=2; }",
            "1:50: button a is twice",
        ),
        (
            "board \"x\" { button a gpio=48; }",
            "1:22: gpio: a number from 0 to 47",
        ),
        (
            "board \"x\" { button default gpio=4; }",
            "1:13: \"default\" is not a button name",
        ),
        ("board \"x\" { button a; }", "1:13: button a has no gpio"),
        (
            "board \"x\" { button a gpio=1; }",
            "1:1: the board has no firmware-board",
        ),
        (
            "profile \"x\" { }",
            "1:1: a board file has exactly one board node",
        ),
    ];
    for (text, want) in cases {
        let got = Board::parse(file("x.kdl"), text).unwrap_err().to_string();
        assert!(
            got.starts_with(&format!("x.kdl:{want}")),
            "{text}:\n got {got}"
        );
    }
}

#[test]
fn a_pull_gives_the_same_mapping_back() {
    let slots = s16_slots();
    let board = board::named("haute42-s16").unwrap();
    let (profiles, skipped) = pull::profiles(&board, &slots);
    assert_eq!(skipped, []);
    let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Default", "SilkSong", "Celeste", "DKC", "TGX"]);
    let text = pull::text(&board, &profiles, &skipped);
    let l = lib(&[("pull.kdl", &text)]);
    assert_eq!(l.errors, []);
    // Each pulled profile, written back as a slot file, gives the pad the
    // mapping of its slot.
    let base = &slots[0];
    for slot in &slots {
        let r = l.resolve(&slot.label).unwrap();
        let f = r.slot_file();
        let planned = slots::plan(
            &f,
            &SlotWrite {
                slot: 6,
                ..SlotWrite::default()
            },
            30,
            2,
            Some("Haute42COSMOX"),
        )
        .unwrap();
        let runs = |s: &[SlotPin]| -> BTreeMap<usize, SlotPin> {
            s.iter()
                .enumerate()
                .map(|(i, p)| match slots::follows_profile_1(p.action) {
                    true => (i, base.pins[i]),
                    false => (i, *p),
                })
                .collect()
        };
        assert_eq!(runs(&planned.pins), runs(&slot.pins), "{}", slot.label);
        assert_eq!(planned.label, slot.label);
    }
}

#[test]
fn a_pull_keeps_names_unique_and_tells_what_it_skips() {
    let board = board::named("haute42-s16").unwrap();
    let mut slots = s16_slots();
    slots[2].label = "SilkSong".into();
    slots[3].label = String::new();
    slots[1].pins[15].action = 32;
    for s in &mut slots {
        s.pins[22].action = 5;
    }
    let (profiles, skipped) = pull::profiles(&board, &slots);
    let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["Default", "SilkSong", "SilkSong (3)", "Profile 4", "TGX"]
    );
    assert_eq!(profiles[2].label.as_deref(), Some("SilkSong"));
    assert_eq!(profiles[3].label, None);
    let got: Vec<(u8, u8, Option<&str>)> = skipped
        .iter()
        .map(|s| (s.profile, s.gpio, s.action_name.as_deref()))
        .collect();
    assert_eq!(got, [(1, 22, Some("B1")), (2, 15, Some("turbo"))]);
    let text = pull::text(&board, &profiles, &skipped);
    assert!(
        text.contains("// Not pulled: profile 2, GPIO 15 has turbo;"),
        "{text}"
    );
    let l = lib(&[("pull.kdl", &text)]);
    assert_eq!(l.list().errors, []);
    assert_eq!(l.resolve("SilkSong (3)").unwrap().label, "SilkSong");
    assert_eq!(l.resolve("Profile 4").unwrap().label, "Profile 4");
}

#[test]
fn save_pull_never_replaces_a_file_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let report = || PullReport {
        via: crate::commands::Route::Control,
        host: "node".into(),
        board: "haute42-s16".into(),
        firmware_board: "Haute42COSMOX".into(),
        profiles: vec!["Default".into()],
        slots: vec![1],
        skipped: Vec::new(),
        kdl: "profile \"Default\" { board \"haute42-s16\"; }\n".into(),
        file: None,
    };
    let lib_dir = dir.path().join("profiles");
    let mut r = report();
    save_pull(&mut r, Some(&lib_dir), Path::new("pad.kdl"), false).unwrap();
    assert_eq!(r.file, Some(lib_dir.join("pad.kdl")));
    let e = save_pull(&mut report(), Some(&lib_dir), Path::new("pad.kdl"), false).unwrap_err();
    assert!(e.to_string().contains("--force replaces it"), "{e}");
    save_pull(&mut report(), Some(&lib_dir), Path::new("pad.kdl"), true).unwrap();
    // The same names in another file of the library.
    let e = save_pull(&mut report(), Some(&lib_dir), Path::new("other.kdl"), false).unwrap_err();
    assert!(e.to_string().contains("\"Default\" is in"), "{e}");
    let e = save_pull(&mut report(), Some(&lib_dir), Path::new("pad.json"), false).unwrap_err();
    assert!(e.to_string().contains("only .kdl files"), "{e}");
    let e = save_pull(&mut report(), None, Path::new("pad.kdl"), false).unwrap_err();
    assert!(e.to_string().contains("give a path"), "{e}");
    // A path with a directory goes there.
    let elsewhere = dir.path().join("elsewhere/pad.kdl");
    save_pull(&mut report(), Some(&lib_dir), &elsewhere, false).unwrap();
    assert!(elsewhere.is_file());
    let l = Library::load(Some(&lib_dir)).unwrap();
    assert_eq!(l.errors, []);
    assert_eq!(
        l.resolve("default").unwrap().file,
        Origin::File(lib_dir.join("pad.kdl"))
    );
}

#[test]
fn a_missing_library_directory_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let l = Library::load(Some(&dir.path().join("none"))).unwrap();
    assert!(l.profiles.iter().all(|d| d.origin.shipped()));
    assert_eq!(l.errors, []);
}

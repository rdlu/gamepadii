use std::collections::BTreeMap;

use super::*;
use crate::buttons::NameSystem;
use crate::channel::sim::SimChannel;
use crate::channel::{Rgb, Slot, SlotPin};
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
            names: NameSystem::Gp2040,
            theme: None,
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
            "My Game Alt",
            "My Game Xbox"
        ]
    );
    let themes: Vec<&str> = report.themes.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(themes, ["purple-green", "snes"]);
    assert!(report.themes.iter().all(|t| t.ok && t.shipped));
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
        errors[0].starts_with("again.kdl:1:1: \"Celeste\" is also a profile in mine.kdl"),
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
    // The 4 small buttons, from left to right, and what profile 1 gives
    // them: the PlayStation labels on the case are the names, and the
    // labels under them are aliases.
    let small: Vec<(u8, i16, &[String])> = ["touchpad", "home", "share", "options"]
        .iter()
        .map(|n| {
            let b = board.button(n).unwrap();
            (
                b.gpio,
                base.pins[usize::from(b.gpio)].action,
                &b.aliases[..],
            )
        })
        .collect();
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        small,
        [
            (21, 16, &s(&["minus"])[..]),
            (20, 15, &[][..]),
            (16, 13, &s(&["back"])[..]),
            (17, 14, &s(&["start"])[..]),
        ],
        "A2, A1, S1, S2"
    );
    assert_eq!(board.button("back").unwrap().name, "share");
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
        (
            "board \"x\" { firmware-board \"X\"; button a gpio=1; button b gpio=2 alias=a; }",
            "1:50: \"a\" is a name of button a already",
        ),
        (
            "board \"x\" { firmware-board \"X\"; button a gpio=1 alias=c; button c gpio=2; }",
            "1:58: \"c\" is a name of button a already",
        ),
        (
            "board \"x\" { firmware-board \"X\"; button a gpio=1 alias=c; button b gpio=2 alias=c; }",
            "1:58: \"c\" is a name of button a already",
        ),
        (
            "board \"x\" { button a gpio=1 alias=a; }",
            "1:29: alias \"a\": not a new name",
        ),
        (
            "board \"x\" { button a gpio=1 alias=default; }",
            "1:29: alias \"default\": not a new name",
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
        names: NameSystem::Gp2040,
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

#[test]
fn aliases_and_renamed_buttons() {
    let text = "profile \"A\" {\n  base \"Base\"\n  pins { back S2; start S1; minus Fn; }\n}\n";
    let l = lib(&[("a.kdl", BASE), ("b.kdl", text)]);
    let r = l.resolve("A").unwrap();
    let action = |b: &str| {
        let p = r.pins.iter().find(|p| p.button == b).unwrap();
        (p.gpio, p.action_name.clone())
    };
    let s = |v: &str| Some(v.to_string());
    assert_eq!(action("share"), (16, s("S2")), "back is share");
    assert_eq!(action("options"), (17, s("S1")));
    assert_eq!(action("touchpad"), (21, s("Fn")));
    // Button names and aliases are case-insensitive, as action names are.
    let caps = "profile \"U\" {\n  base \"Base\"\n  pins { Touchpad B1; BACK B2; }\n}\n";
    let l = lib(&[("a.kdl", BASE), ("b.kdl", caps)]);
    let r = l.resolve("U").unwrap();
    let action = |b: &str| {
        r.pins
            .iter()
            .find(|p| p.button == b)
            .unwrap()
            .action_name
            .clone()
    };
    assert_eq!(action("touchpad"), s("B1"));
    assert_eq!(action("share"), s("B2"));
    // A child that names the button by its name replaces the alias.
    let child = "profile \"C\" { base \"A\"; pins { share B1; }; }\n";
    let l = lib(&[("a.kdl", BASE), ("b.kdl", text), ("c.kdl", child)]);
    let r = l.resolve("C").unwrap();
    let share = r.pins.iter().find(|p| p.button == "share").unwrap();
    assert_eq!(share.action_name.as_deref(), Some("B1"));
    for (pins, want) in [
        (
            "share S1; back S2;",
            "b.kdl:3:20: button share is twice in pins: share and back are names of one button",
        ),
        (
            "fn3 S1;",
            "b.kdl:3:10: button \"fn3\" is now \"share\" on haute42-s16",
        ),
        ("fn1 A2;", "b.kdl:3:10: button \"fn1\" is now \"touchpad\""),
        ("FN2 A1;", "b.kdl:3:10: button \"FN2\" is now \"home\""),
        (
            "share S1; Share S2;",
            "b.kdl:3:20: button share is twice in pins",
        ),
        ("fn4 S2;", "b.kdl:3:10: button \"fn4\" is now \"options\""),
    ] {
        let text = format!("profile \"A\" {{\n  base \"Base\"\n  pins {{ {pins} }}\n}}\n");
        let got = resolve_error(&[("a.kdl", BASE), ("b.kdl", &text)], "A");
        assert!(got.starts_with(want), "{pins}:\n got {got}\nwant {want}");
    }
}

#[test]
fn each_name_system_reads_its_own_names() {
    let pin = |files: &[(&str, &str)], name: &str, button: &str| {
        let r = lib(files).resolve(name).unwrap();
        r.pins
            .iter()
            .find(|p| p.button == button)
            .unwrap()
            .action_name
            .clone()
            .unwrap()
    };
    for (system, action, want) in [
        ("xbox", "A", "B1"),
        ("switch", "A", "B2"),
        ("switch", "a", "B2"),
        ("playstation", "Cross", "B1"),
        ("ps", "triangle", "B4"),
        ("xbox", "view", "S1"),
        ("xbox", "Menu", "S2"),
        ("playstation", "Create", "S1"),
        ("switch", "ZL", "L2"),
        ("switch", "Capture", "A2"),
        ("gp2040", "B2", "B2"),
        // GP2040 names and the other actions are valid in each system.
        ("xbox", "B1", "B1"),
        ("switch", "L1", "L1"),
        ("playstation", "R3", "R3"),
        ("switch", "Up", "Up"),
        ("switch", "none", "none"),
        ("xbox", "Fn", "Fn"),
        ("switch", "menu-navigation-toggle", "menu-navigation-toggle"),
        ("xbox", "BUTTON_PRESS_B2", "B2"),
        ("xbox", "6", "B2"),
    ] {
        let text = format!(
            "names {system}\nprofile \"A\" {{ base \"Base\"; pins {{ up {action}; }}; }}\n"
        );
        assert_eq!(
            pin(&[("a.kdl", BASE), ("b.kdl", &text)], "A", "up"),
            want,
            "{system} {action}"
        );
    }
    // A profile's names node replaces the file's, and a base and a child
    // can use two systems: each one in its own.
    let text = "names xbox\n\
        profile \"X\" { base \"Base\"; pins { up A; k2 A; }; }\n\
        profile \"S\" { base \"X\"; names switch; pins { k2 A; }; }\n";
    let files = [("a.kdl", BASE), ("b.kdl", text)];
    assert_eq!(pin(&files, "S", "up"), "B1", "from X, in Xbox names");
    assert_eq!(pin(&files, "S", "k2"), "B2", "in Switch names");
    let l = lib(&files);
    assert_eq!(l.resolve("S").unwrap().names, NameSystem::Switch);
    assert_eq!(l.resolve("X").unwrap().names, NameSystem::Xbox);
    assert_eq!(l.resolve("Base").unwrap().names, NameSystem::Gp2040);
    // The names node can come after the pins, and after the profiles.
    let text = "profile \"A\" { base \"Base\"; pins { up A; }; names switch; }\n";
    assert_eq!(pin(&[("a.kdl", BASE), ("b.kdl", text)], "A", "up"), "B2");
    let text = "profile \"A\" { base \"Base\"; pins { up A; }; }\nnames xbox\n";
    assert_eq!(pin(&[("a.kdl", BASE), ("b.kdl", text)], "A", "up"), "B1");
}

#[test]
fn a_name_of_another_system_is_an_error_that_lists_the_names() {
    let cases = [
        (
            "profile \"A\" { pins { up A; }; }\n",
            "a.kdl:1:25: unknown action \"A\": the GP2040 names are Up, Down, Left, Right, B1,",
        ),
        (
            "names xbox\nprofile \"A\" { pins { up Cross; }; }\n",
            "a.kdl:2:25: unknown action \"Cross\": the xbox names are A (B1), B (B2), X (B3), \
             Y (B4), LB (L1), RB (R1), LT (L2), RT (R2), Back/View (S1), Start/Menu (S2), \
             Guide (A1), LS (L3), RS (R3); the GP2040 names",
        ),
        (
            "names switch\nprofile \"A\" { pins { up LB; }; }\n",
            "a.kdl:2:25: unknown action \"LB\": the switch names are B (B1), A (B2)",
        ),
        (
            "names playstation\nprofile \"A\" { pins { up Plus; }; }\n",
            "a.kdl:2:25: unknown action \"Plus\": the playstation names are Cross (B1)",
        ),
        (
            "names snes\n",
            "a.kdl:1:1: \"snes\" is not a name system (gp2040, xbox, playstation, switch)",
        ),
        (
            "names xbox\nnames xbox\n",
            "a.kdl:2:1: names is twice in the file",
        ),
        (
            "profile \"A\" { names xbox; names ps; }\n",
            "a.kdl:1:27: names is twice in the profile",
        ),
    ];
    for (text, want) in cases {
        let got = parse_error(text);
        assert!(got.starts_with(want), "{text:?}:\n got {got}\nwant {want}");
    }
}

#[test]
fn a_profile_resolves_to_one_slot_file_in_each_name_system() {
    let board = board::named("haute42-s16").unwrap();
    let slots = s16_slots();
    let (pulled, skipped) = pull::profiles(&board, &slots);
    let files = |names: NameSystem| {
        let mut profiles = pulled.clone();
        for p in &mut profiles {
            p.names = names;
        }
        pull::text(&board, &profiles, &skipped)
    };
    let gp2040 = lib(&[("pull.kdl", &files(NameSystem::Gp2040))]);
    for names in NameSystem::ALL {
        let text = files(*names);
        if *names != NameSystem::Gp2040 {
            assert!(
                text.contains(&format!("\nnames {}\n", names.key())),
                "{text}"
            );
        }
        // The KDL round trip keeps each profile and its text.
        let again: Vec<Profile> = parse::profiles(file("pull.kdl"), &text)
            .unwrap_or_else(|e| panic!("{e}\n{text}"))
            .into_iter()
            .map(|d| d.profile)
            .collect();
        assert!(again.iter().all(|p| p.names == *names));
        let mut want = pulled.clone();
        for p in &mut want {
            p.names = *names;
        }
        assert_eq!(again, want, "{names}");
        let l = lib(&[("pull.kdl", &text)]);
        assert_eq!(l.list().errors, [], "{names}");
        for p in &pulled {
            assert_eq!(
                l.resolve(&p.name).unwrap().slot_file(),
                gp2040.resolve(&p.name).unwrap().slot_file(),
                "{names}: {}",
                p.name
            );
        }
    }
    // Xbox names in the KDL: `k1 A` for B1.
    let text = files(NameSystem::Xbox);
    assert!(text.contains("        k1 A\n"), "{text}");
    assert!(
        text.contains("        touchpad A2\n"),
        "Xbox has no A2: {text}"
    );
    let text = files(NameSystem::Switch);
    assert!(text.contains("        k1 B\n        k2 A\n"), "{text}");
    // A file with two systems gives each profile its own names node.
    let mut mixed = pulled.clone();
    mixed[1].names = NameSystem::Playstation;
    let text = write::file(&[], &mixed);
    assert!(!text.starts_with("names"), "{text}");
    assert!(text.contains("    names playstation\n"), "{text}");
    let again: Vec<Profile> = parse::profiles(file("m.kdl"), &text)
        .unwrap()
        .into_iter()
        .map(|d| d.profile)
        .collect();
    assert_eq!(again, mixed);
    // The shipped example in Xbox names is "My Game" with another label.
    let shipped = Library::from_sources(Vec::new(), true);
    let x = shipped.resolve("My Game Xbox").unwrap().slot_file();
    let g = shipped.resolve("My Game").unwrap().slot_file();
    assert_eq!(x.pins, g.pins);
}

const THEMES: &str = r##"
theme "t" {
    board "haute42-s16"
    effect idle="static-color" pressed=jigglestatic
    idle "#111111"
    pressed "red"
    button k1 idle="#222222" pressed="#111111"
    button back pressed=white
    case "#333333"
    lights idle_cycle_time=3 case_in_pressed=#true idle_special_color="#00ff00"
}
"##;

fn theme_error(text: &str) -> String {
    parse::file(file("t.kdl"), text).unwrap_err().to_string()
}

#[test]
fn a_theme_parses_and_gives_a_theme_file() {
    let l = lib(&[("t.kdl", THEMES)]);
    assert_eq!(l.errors, []);
    let t = l.resolve_theme("T").unwrap();
    // Each #rrggbb in a custom colour, in the order of first use.
    let rgb = Rgb::from_u32;
    assert_eq!(
        t.colors,
        Some(vec![rgb(0x111111), rgb(0x222222), rgb(0x333333)])
    );
    assert_eq!(t.idle.as_ref().map(|c| c.number), Some(14));
    assert_eq!(
        t.pressed.as_ref().map(|c| c.number),
        Some(2),
        "red is built in"
    );
    // The buttons by GPIO, in the order of the board file.
    let buttons: Vec<(&str, u8)> = t
        .buttons
        .iter()
        .map(|b| (b.button.as_str(), b.gpio))
        .collect();
    assert_eq!(buttons, [("k1", 6), ("share", 16)]);
    let f = &t.theme_file;
    assert_eq!(f.board.as_deref(), Some("Haute42COSMOX"));
    // The theme file is what `leds theme apply` takes.
    let json = serde_json::to_string(f).unwrap();
    let again = crate::colors::ThemeFile::parse(&json).unwrap();
    assert_eq!(&again, f);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let lights = &v["lights"];
    let idle = lights["idle_colors"].as_array().unwrap();
    let pressed = lights["pressed_colors"].as_array().unwrap();
    assert_eq!((idle.len(), pressed.len()), (30, 30));
    assert_eq!((idle[6].as_u64(), idle[7].as_u64()), (Some(15), Some(14)));
    assert_eq!(idle[29], 14, "a GPIO with no button gets the colour of all");
    assert_eq!(
        (pressed[6].as_u64(), pressed[16].as_u64()),
        (Some(14), Some(1))
    );
    assert_eq!(pressed[0], 2);
    assert_eq!(lights["case_colors"], serde_json::json!(vec![16; 32]));
    assert_eq!(lights["idle_effect"], 0);
    assert_eq!(lights["pressed_effect"], 2);
    assert_eq!(lights["idle_cycle_time"], 3);
    assert_eq!(lights["case_in_pressed"], true);
    assert_eq!(lights["idle_special_color"], "#00ff00");
    assert_eq!(
        v["colors"],
        serde_json::json!(["#111111", "#222222", "#333333"])
    );
    // The shipped themes too.
    let shipped = Library::from_sources(Vec::new(), true);
    let pg = shipped.resolve_theme("purple-green").unwrap();
    assert_eq!(pg.colors, Some(vec![rgb(0x8000ff), rgb(0x00ff60)]));
    let lights = pg.theme_file.lights.unwrap();
    assert_eq!(lights.values[&5], crate::pb::Value::Bytes(vec![14; 30]));
    assert_eq!(lights.values[&6], crate::pb::Value::Bytes(vec![15; 30]));
    let snes = shipped.resolve_theme("snes").unwrap();
    assert_eq!(snes.colors.as_ref().map(Vec::len), Some(5));
}

#[test]
fn a_theme_without_rrggbb_keeps_the_custom_colours() {
    let text = "theme \"t\" { idle \"custom 3\"; pressed blue; }\n";
    let t = lib(&[("t.kdl", text)]).resolve_theme("t").unwrap();
    assert_eq!(t.colors, None);
    assert_eq!(t.theme_file.colors, None);
    assert_eq!(t.board, None);
    let idle = &t.theme_file.lights.as_ref().unwrap().values[&5];
    assert_eq!(*idle, crate::pb::Value::Bytes(vec![17; 30]));
}

#[test]
fn a_theme_has_at_most_16_rrggbb_colours() {
    let mut text = String::from("theme \"t\" {\n    board \"haute42-s16\"\n");
    let board = board::named("haute42-s16").unwrap();
    for (i, b) in board.buttons.iter().take(16).enumerate() {
        text.push_str(&format!("    button {} idle=\"#0000{i:02x}\"\n", b.name));
    }
    // The same colour again needs no new custom colour.
    text.push_str("    idle \"#00000f\"\n");
    let ok = format!("{text}}}\n");
    let t = lib(&[("t.kdl", &ok)]).resolve_theme("t").unwrap();
    assert_eq!(t.colors.unwrap().len(), 16);
    let bad = format!("{text}    pressed \"#ffffff\"\n}}\n");
    let got = theme_error(&bad);
    assert!(
        got.starts_with(
            "t.kdl:20:13: #ffffff is colour 17 of the theme in #rrggbb: the pad keeps at most 16"
        ),
        "{got}"
    );
}

#[test]
fn bad_themes_name_the_line_and_column() {
    let cases = [
        (
            "theme \"t\" { colour red; }\n",
            "t.kdl:1:13: unknown node \"colour\" in a theme",
        ),
        (
            "theme \"t\" { idle sparkle; }\n",
            "t.kdl:1:18: unknown colour \"sparkle\": give black, white",
        ),
        (
            "theme \"t\" { idle \"#12345\"; }\n",
            "t.kdl:1:18: \"#12345\" is not a colour: give #rrggbb",
        ),
        (
            "theme \"t\" { idle 3; }\n",
            "t.kdl:1:18: a colour is a name",
        ),
        (
            "theme \"t\" { idle red blue; }\n",
            "t.kdl:1:13: idle: give one value",
        ),
        (
            "theme \"t\" { idle red; idle blue; }\n",
            "t.kdl:1:23: idle is twice in the theme",
        ),
        (
            "theme \"t\" {\n  idle \"custom 1\"\n  pressed \"#ffffff\"\n}\n",
            "t.kdl:2:8: a theme with #rrggbb colours sets the custom colours itself",
        ),
        (
            "theme \"t\" { button k1; }\n",
            "t.kdl:1:13: button k1: give idle=COLOUR",
        ),
        (
            "theme \"t\" { button k1 glow=red; }\n",
            "t.kdl:1:23: unknown property \"glow\"",
        ),
        (
            "theme \"t\" { button k1 idle=red; button k1 idle=blue; }\n",
            "t.kdl:1:33: button k1 is twice in the theme",
        ),
        (
            "theme \"t\" { effect idle=sparkle; }\n",
            "t.kdl:1:20: idle_effect: no effect sparkle",
        ),
        (
            "theme \"t\" { effect idle=40; }\n",
            "t.kdl:1:20: idle_effect: the effects are 0 to",
        ),
        (
            "theme \"t\" { effect glow=1; }\n",
            "t.kdl:1:20: unknown property \"glow\"",
        ),
        (
            "theme \"t\" { lights idle_cycle_time=12; }\n",
            "t.kdl:1:20: idle_cycle_time: the cycle times are 0 to 9",
        ),
        (
            "theme \"t\" { lights sparkle=1; }\n",
            "t.kdl:1:20: unknown lights field \"sparkle\"",
        ),
        (
            "theme \"t\" { lights idle_colors=1; }\n",
            "t.kdl:1:20: idle_colors: a theme gives it with idle and button",
        ),
        (
            "theme \"t\" { lights idle_effect=1; }\n",
            "t.kdl:1:20: idle_effect: a theme gives it with effect idle=",
        ),
        ("theme \"t\" { }\n", "t.kdl:1:1: theme \"t\" sets nothing"),
        ("theme { idle red; }\n", "t.kdl:1:1: theme: give one name"),
    ];
    for (text, want) in cases {
        let got = theme_error(text);
        assert!(got.starts_with(want), "{text:?}:\n got {got}\nwant {want}");
    }
}

#[test]
fn themes_resolve_with_their_board_and_with_their_profile() {
    let theme = "theme \"lit\" {\n  idle red\n  button k1 idle=blue\n}\n";
    // No board: the button nodes need one.
    let l = lib(&[("t.kdl", theme)]);
    let e = l.resolve_theme("lit").unwrap_err().to_string();
    assert!(
        e.contains("t.kdl:3:3: theme \"lit\" has button nodes but no board"),
        "{e}"
    );
    // A profile with a board gives it.
    let profile = "profile \"A\" {\n  base \"Base\"\n  theme \"lit\"\n}\n\
                   profile \"B\" { base \"A\"; }\n";
    let l = lib(&[("a.kdl", BASE), ("t.kdl", theme), ("p.kdl", profile)]);
    let a = l.resolve("A").unwrap();
    assert_eq!(a.theme.as_deref(), Some("lit"));
    let t = l.profile_theme(&a).unwrap().unwrap();
    assert_eq!(t.buttons[0].gpio, 6);
    // The base gives the theme.
    let b = l.resolve("B").unwrap();
    assert_eq!(b.theme.as_deref(), Some("lit"));
    assert!(
        l.profile_theme(&l.resolve("Base").unwrap())
            .unwrap()
            .is_none()
    );
    let a_profile = "profile \"A\" {\n  base \"Base\"\n  theme \"lit\"\n}\n";
    for (theme, profile, want) in [
        (
            theme,
            "profile \"A\" {\n  base \"Base\"\n  theme \"dark\"\n}\n",
            "p.kdl:3:3: theme \"dark\" is not in the library",
        ),
        (
            "theme \"lit\" {\n  board \"pico\"\n  idle red\n}\n",
            a_profile,
            "t.kdl:2:3: no board file \"pico\"",
        ),
        (
            "theme \"lit\" {\n  button k9 idle=red\n}\n",
            a_profile,
            "t.kdl:2:3: unknown button \"k9\": haute42-s16 has",
        ),
        (
            "theme \"lit\" {\n  button fn2 idle=red\n}\n",
            a_profile,
            "t.kdl:2:3: button \"fn2\" is now \"home\"",
        ),
        (
            "theme \"lit\" {\n  button share idle=red\n  button back pressed=red\n}\n",
            a_profile,
            "t.kdl:3:3: button share is twice in the theme: share names it too",
        ),
    ] {
        let got = resolve_error(
            &[("a.kdl", BASE), ("t.kdl", theme), ("p.kdl", profile)],
            "A",
        );
        assert!(got.starts_with(want), "{theme:?}:\n got {got}\nwant {want}");
    }
    // `library list` gives a theme that does not resolve.
    let l = lib(&[("t.kdl", "theme \"lit\" { board \"pico\"; idle red; }\n")]);
    let report = l.list();
    assert!(!report.themes[0].ok);
    assert!(
        report.errors[0].to_string().contains("no board file"),
        "{:?}",
        report.errors
    );
}

#[test]
fn profiles_and_themes_share_the_names() {
    let l = lib(&[
        ("a.kdl", BASE),
        (
            "t.kdl",
            "theme \"base\" { idle red; }\ntheme \"x\" { idle red; }\n",
        ),
        ("u.kdl", "theme \"X\" { idle blue; }\n"),
    ]);
    let errors: Vec<String> = l.list().errors.iter().map(|e| e.to_string()).collect();
    assert_eq!(
        errors,
        [
            "t.kdl:1:1: \"base\" is also a profile in a.kdl; names must be unique",
            "u.kdl:1:1: \"X\" is also a theme in t.kdl; names must be unique",
        ]
    );
    assert!(l.is_theme("X") && !l.is_theme("Base"));
    let e = l.resolve("x").unwrap_err().to_string();
    assert!(
        e.contains("no profile \"x\" in the library (\"x\" is a theme)"),
        "{e}"
    );
    let e = l.resolve_theme("base").unwrap_err().to_string();
    assert!(
        e.contains("no theme \"base\" in the library (\"Base\" is a profile)"),
        "{e}"
    );
    // A user theme hides a shipped one.
    let l = Library::from_sources(
        vec![(file("mine.kdl"), "theme \"snes\" { idle red; }\n".into())],
        true,
    );
    assert_eq!(l.list().errors, []);
    assert_eq!(l.resolve_theme("snes").unwrap().file, file("mine.kdl"));
}

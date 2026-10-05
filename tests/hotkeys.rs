use quick_panel::{hotkeys::*, model::Settings};
use std::{cell::RefCell, collections::HashMap};

#[test]
fn defaults_register_only_the_panel_and_disabled_keys_are_not_claimed() {
    let mut bindings = HotkeyBindings::default();
    let registry = RefCell::new(HashMap::new());
    bindings
        .replace(
            settings_hotkeys(&Settings::default()).unwrap(),
            |id, key| {
                registry.borrow_mut().insert(id, key);
                Ok(())
            },
            |id| {
                registry.borrow_mut().remove(&id);
            },
        )
        .unwrap();
    let entries: Vec<_> = registry.borrow().values().copied().collect();
    assert_eq!(
        entries,
        vec![Hotkey {
            modifiers: 3,
            key: 32
        }]
    );
    let id = *registry.borrow().keys().next().unwrap();
    assert_eq!(bindings.action_for(id), Some(0));
    bindings
        .replace(
            [None, None, None],
            |_, _| panic!("disabled keys must not be registered"),
            |id| {
                registry.borrow_mut().remove(&id);
            },
        )
        .unwrap();
    assert!(registry.borrow().is_empty());
    assert_eq!(
        bindings.action_for(id),
        None,
        "queued old hotkey must be ignored"
    );
}

#[test]
fn settings_accept_empty_disabled_keys_and_preserve_custom_bindings() {
    assert_eq!(parse_hotkey("  ").unwrap(), (0, 0));
    assert_eq!(format_hotkey(0, 0), "");
    assert_eq!(parse_hotkey(" ctrl + shift + p ").unwrap(), (6, 80));
    assert_eq!(format_hotkey(6, 80), "Ctrl+Shift+P");
    assert_eq!(parse_hotkey("Alt+F12").unwrap(), (1, 123));
    let settings = Settings {
        hotkey_modifiers: 6,
        hotkey_key: 80,
        clipboard_hotkey_modifiers: 0,
        clipboard_hotkey_key: 0,
        ..Settings::default()
    };
    let restored: Settings =
        serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        settings_hotkeys(&restored).unwrap(),
        [
            Some(Hotkey {
                modifiers: 6,
                key: 80
            }),
            None,
            None
        ]
    );
}

#[test]
fn invalid_reserved_and_duplicate_active_keys_are_rejected() {
    for value in [
        "Ctrl+",
        "V",
        "Ctrl+A+B",
        "Ctrl+Space+V",
        "Alt+F25",
        "Ctrl++V",
    ] {
        assert!(
            parse_hotkey(value).is_err(),
            "accepted invalid binding {value}"
        );
    }
    let reserved = Settings {
        hotkey_modifiers: 8,
        hotkey_key: 86,
        ..Settings::default()
    };
    assert!(settings_hotkeys(&reserved).is_err());
    let duplicate = Settings {
        clipboard_hotkey_modifiers: 3,
        clipboard_hotkey_key: 32,
        ..Settings::default()
    };
    assert!(settings_hotkeys(&duplicate).is_err());
    let both_off = Settings {
        hotkey_modifiers: 0,
        hotkey_key: 0,
        ..Settings::default()
    };
    assert_eq!(settings_hotkeys(&both_off).unwrap(), [None, None, None]);
}

#[test]
fn conflicting_replacement_rolls_back_staged_keys_and_keeps_old_bindings() {
    let mut bindings = HotkeyBindings::default();
    let registry = RefCell::new(HashMap::new());
    let original = [
        Some(Hotkey {
            modifiers: 3,
            key: 32,
        }),
        Some(Hotkey {
            modifiers: 3,
            key: 86,
        }),
        None,
    ];
    bindings
        .replace(
            original,
            |id, key| {
                registry.borrow_mut().insert(id, key);
                Ok(())
            },
            |id| {
                registry.borrow_mut().remove(&id);
            },
        )
        .unwrap();
    let before = registry.borrow().clone();
    let result = bindings.replace(
        [
            Some(Hotkey {
                modifiers: 6,
                key: 80,
            }),
            Some(Hotkey {
                modifiers: 6,
                key: 81,
            }),
            None,
        ],
        |id, key| {
            if key.key == 81 {
                Err("occupied".into())
            } else {
                registry.borrow_mut().insert(id, key);
                Ok(())
            }
        },
        |id| {
            registry.borrow_mut().remove(&id);
        },
    );
    assert!(result.is_err());
    assert_eq!(*registry.borrow(), before);
    for (id, key) in before {
        assert_eq!(
            bindings.action_for(id),
            Some(if key.key == 32 { 0 } else { 1 })
        );
    }
    bindings
        .replace(
            [original[1], original[0], None],
            |_, _| panic!("swapped owned keys should be reused"),
            |_| panic!("reused keys should stay registered"),
        )
        .unwrap();
    for (&id, key) in registry.borrow().iter() {
        assert_eq!(
            bindings.action_for(id),
            Some(if key.key == 86 { 0 } else { 1 })
        );
    }
}

#[test]
fn third_hotkey_can_swap_all_actions_and_conflict_rolls_back_every_binding() {
    let keys = [
        Some(Hotkey {
            modifiers: 3,
            key: 32,
        }),
        Some(Hotkey {
            modifiers: 3,
            key: 86,
        }),
        Some(Hotkey {
            modifiers: 6,
            key: 83,
        }),
    ];
    let mut bindings = HotkeyBindings::default();
    let registry = RefCell::new(HashMap::new());
    bindings
        .replace(
            keys,
            |id, key| {
                registry.borrow_mut().insert(id, key);
                Ok(())
            },
            |id| {
                registry.borrow_mut().remove(&id);
            },
        )
        .unwrap();
    bindings
        .replace(
            [keys[2], keys[0], keys[1]],
            |_, _| panic!("owned keys reused"),
            |_| panic!("owned keys retained"),
        )
        .unwrap();
    let before = registry.borrow().clone();
    let result = bindings.replace(
        [
            Some(Hotkey {
                modifiers: 6,
                key: 80,
            }),
            Some(Hotkey {
                modifiers: 6,
                key: 81,
            }),
            Some(Hotkey {
                modifiers: 6,
                key: 82,
            }),
        ],
        |id, key| {
            if key.key == 82 {
                Err("third key occupied".into())
            } else {
                registry.borrow_mut().insert(id, key);
                Ok(())
            }
        },
        |id| {
            registry.borrow_mut().remove(&id);
        },
    );
    assert!(result.is_err());
    assert_eq!(*registry.borrow(), before);
    for (id, key) in before {
        assert_eq!(
            bindings.action_for(id),
            Some(if key.key == 83 {
                0
            } else if key.key == 32 {
                1
            } else {
                2
            })
        );
    }
    bindings
        .replace(
            [None, None, keys[1]],
            |_, _| panic!("existing key retained"),
            |id| {
                registry.borrow_mut().remove(&id);
            },
        )
        .unwrap();
    let id = *registry.borrow().keys().next().unwrap();
    assert_eq!(registry.borrow().len(), 1);
    assert_eq!(bindings.action_for(id), Some(2));
    assert!(
        bindings
            .replace(
                [keys[1], None, keys[1]],
                |_, _| panic!("duplicate rejected before registration"),
                |_| panic!("original retained")
            )
            .is_err()
    );
}

#[test]
fn screenshot_defaults_disabled_and_settings_reject_any_of_three_duplicate_keys() {
    let old: Settings = serde_json::from_str(r#"{"hotkey_modifiers":6,"hotkey_key":80}"#).unwrap();
    assert_eq!(old.screenshot_hotkey_key, 0);
    assert_eq!(old.screenshot_hotkey_modifiers, 0);
    let settings = Settings {
        screenshot_hotkey_modifiers: 6,
        screenshot_hotkey_key: 83,
        ..old
    };
    let restored: Settings =
        serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        settings_hotkeys(&restored).unwrap()[2],
        Some(Hotkey {
            modifiers: 6,
            key: 83
        })
    );
    for key in [80, 86] {
        let settings = Settings {
            clipboard_hotkey_modifiers: 6,
            clipboard_hotkey_key: 86,
            screenshot_hotkey_key: key,
            ..settings.clone()
        };
        assert!(settings_hotkeys(&settings).is_err());
    }
}

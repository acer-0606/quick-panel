//! Portable validation and transactional replacement of Windows hotkey bindings.
use crate::model::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

fn binding(modifiers: u32, key: u32) -> Result<Option<Hotkey>, String> {
    if (modifiers, key) == (0, 0) {
        return Ok(None);
    }
    if modifiers == 0
        || modifiers & !15 != 0
        || !(key == 32
            || (48..=57).contains(&key)
            || (65..=90).contains(&key)
            || (112..=135).contains(&key))
    {
        return Err("快捷键无效；请输入 Ctrl/Alt/Shift/Win + 字母、数字、Space 或 F1–F24".into());
    }
    if (modifiers, key) == (8, 86) {
        return Err("Win+V 留给系统剪贴板，请选择其他组合".into());
    }
    Ok(Some(Hotkey { modifiers, key }))
}

pub fn settings_hotkeys(settings: &Settings) -> Result<[Option<Hotkey>; 2], String> {
    let keys = [
        binding(settings.hotkey_modifiers, settings.hotkey_key)?,
        binding(
            settings.clipboard_hotkey_modifiers,
            settings.clipboard_hotkey_key,
        )?,
    ];
    if keys[0].is_some() && keys[0] == keys[1] {
        return Err("两组已启用的快捷键须不同".into());
    }
    Ok(keys)
}

pub fn parse_hotkey(value: &str) -> Result<(u32, u32), String> {
    if value.trim().is_empty() {
        return Ok((0, 0));
    }
    let invalid =
        || "快捷键无效；请输入 Ctrl/Alt/Shift/Win + 字母、数字、Space 或 F1–F24".to_owned();
    let mut modifiers = 0;
    let mut key = None;
    for part in value.split('+').map(str::trim) {
        match part.to_ascii_uppercase().as_str() {
            "CTRL" => modifiers |= 2,
            "ALT" => modifiers |= 1,
            "SHIFT" => modifiers |= 4,
            "WIN" => modifiers |= 8,
            "SPACE" => {
                if key.replace(32).is_some() {
                    return Err(invalid());
                }
            }
            text => {
                let code = if text.len() == 1 && text.as_bytes()[0].is_ascii_alphanumeric() {
                    text.as_bytes()[0] as u32
                } else if let Some(number) = text
                    .strip_prefix('F')
                    .and_then(|number| number.parse::<u32>().ok())
                    .filter(|number| (1..=24).contains(number))
                {
                    111 + number
                } else {
                    return Err(invalid());
                };
                if key.replace(code).is_some() {
                    return Err(invalid());
                }
            }
        }
    }
    let key = key.ok_or_else(invalid)?;
    binding(modifiers, key)?;
    Ok((modifiers, key))
}

pub fn format_hotkey(modifiers: u32, key: u32) -> String {
    if key == 0 {
        return String::new();
    }
    let mut parts = Vec::new();
    for (bit, name) in [(2, "Ctrl"), (1, "Alt"), (4, "Shift"), (8, "Win")] {
        if modifiers & bit != 0 {
            parts.push(name.to_owned());
        }
    }
    parts.push(if key == 32 {
        "Space".into()
    } else if (112..=135).contains(&key) {
        format!("F{}", key - 111)
    } else {
        char::from_u32(key).unwrap_or('?').to_string()
    });
    parts.join("+")
}

#[derive(Default)]
pub struct HotkeyBindings {
    active: [Option<(i32, Hotkey)>; 2],
}
impl HotkeyBindings {
    pub fn replace(
        &mut self,
        next: [Option<Hotkey>; 2],
        mut register: impl FnMut(i32, Hotkey) -> Result<(), String>,
        mut unregister: impl FnMut(i32),
    ) -> Result<(), String> {
        for key in next.iter().flatten() {
            binding(key.modifiers, key.key)?;
        }
        if next[0].is_some() && next[0] == next[1] {
            return Err("两组已启用的快捷键须不同".into());
        }
        let mut fresh =
            (1..=8).filter(|id| !self.active.iter().flatten().any(|(old, _)| old == id));
        let mut staged = Vec::new();
        let mut replacement = [None, None];
        for (index, desired) in next.into_iter().enumerate() {
            let Some(key) = desired else {
                continue;
            };
            if let Some(&previous) = self.active.iter().flatten().find(|(_, old)| *old == key) {
                replacement[index] = Some(previous);
                continue;
            }
            let id = fresh.next().expect("two staged bindings fit eight IDs");
            if let Err(error) = register(id, key) {
                for id in staged {
                    unregister(id);
                }
                return Err(error);
            }
            staged.push(id);
            replacement[index] = Some((id, key));
        }
        for &(id, _) in self.active.iter().flatten() {
            if !replacement
                .iter()
                .flatten()
                .any(|(next_id, _)| *next_id == id)
            {
                unregister(id);
            }
        }
        self.active = replacement;
        Ok(())
    }
    pub fn action_for(&self, id: i32) -> Option<usize> {
        self.active
            .iter()
            .position(|binding| binding.is_some_and(|(active_id, _)| active_id == id))
    }
    pub fn clear(&mut self, mut unregister: impl FnMut(i32)) {
        for &(id, _) in self.active.iter().flatten() {
            unregister(id);
        }
        self.active = [None, None];
    }
}

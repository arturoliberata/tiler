//! Parse "alt+shift+h" into Win32 RegisterHotKey modifiers and a virtual-key code.
//! The constants mirror the Win32 values so the core crate stays platform-free.

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

pub fn parse(s: &str) -> Result<Hotkey, String> {
    let mut mods = 0;
    let mut vk = None;
    for part in s.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "alt" => mods |= MOD_ALT,
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" => mods |= MOD_WIN,
            key => {
                if vk.is_some() {
                    return Err(format!("'{s}': more than one non-modifier key"));
                }
                vk = Some(key_code(key).ok_or_else(|| format!("'{s}': unknown key '{key}'"))?);
            }
        }
    }
    let vk = vk.ok_or_else(|| format!("'{s}': no key given"))?;
    if mods == 0 {
        return Err(format!(
            "'{s}': a global hotkey needs at least one modifier"
        ));
    }
    Ok(Hotkey {
        mods: mods | MOD_NOREPEAT,
        vk,
    })
}

fn key_code(k: &str) -> Option<u32> {
    let b = k.as_bytes();
    if b.len() == 1 {
        let c = b[0].to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as u32); // 'A'..'Z' and '0'..'9' map directly
        }
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    Some(match k {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "comma" | "," => 0xBC,
        "period" | "." => 0xBE,
        "minus" | "-" => 0xBD,
        "equals" | "plus" | "=" => 0xBB,
        "semicolon" | ";" => 0xBA,
        "slash" | "/" => 0xBF,
        "backslash" | "\\" => 0xDC,
        "lbracket" | "[" => 0xDB,
        "rbracket" | "]" => 0xDD,
        "quote" | "'" => 0xDE,
        "backtick" | "`" => 0xC0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_keys() {
        assert_eq!(
            parse("alt+h").unwrap(),
            Hotkey {
                mods: MOD_ALT | MOD_NOREPEAT,
                vk: 0x48
            }
        );
        assert_eq!(
            parse("Alt+Shift+1").unwrap(),
            Hotkey {
                mods: MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
                vk: 0x31
            }
        );
        assert_eq!(parse("ctrl+alt+f12").unwrap().vk, 0x7B);
        assert_eq!(parse("win+space").unwrap().vk, 0x20);
    }

    #[test]
    fn rejects_bad_hotkeys() {
        assert!(parse("h").is_err());
        assert!(parse("alt+h+j").is_err());
        assert!(parse("alt+banana").is_err());
        assert!(parse("alt+shift").is_err());
    }
}

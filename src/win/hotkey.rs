//! Global hotkey parsing ("Ctrl+Alt+D", "Win+Shift+Space", "F7"…).

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};

pub fn parse(s: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let mut mods = HOT_KEY_MODIFIERS(0);
    let mut key: Option<u32> = None;
    for part in s.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" | "meta" => mods |= MOD_WIN,
            "" => return None,
            k => {
                if key.is_some() {
                    return None;
                }
                key = Some(vk_of(k)?);
            }
        }
    }
    Some((mods | MOD_NOREPEAT, key?))
}

fn vk_of(k: &str) -> Option<u32> {
    let b = k.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_alphanumeric()) {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x70 + n - 1);
    }
    Some(match k {
        "space" => 0x20,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "`" | "backquote" | "grave" => 0xC0,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        _ => return None,
    })
}

pub fn register(hwnd: HWND, id: i32, spec: &str) -> bool {
    let Some((mods, vk)) = parse(spec) else {
        crate::log!("hotkey: cannot parse '{spec}'");
        return false;
    };
    unsafe { RegisterHotKey(Some(hwnd), id, mods, vk).is_ok() }
}

pub fn unregister(hwnd: HWND, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_specs() {
        assert_eq!(parse("Ctrl+Alt+D"), Some((MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'D' as u32)));
        assert_eq!(parse("win + shift + space"), Some((MOD_WIN | MOD_SHIFT | MOD_NOREPEAT, 0x20)));
        assert_eq!(parse("F7"), Some((MOD_NOREPEAT, 0x76)));
        assert_eq!(parse("Ctrl+"), None);
        assert_eq!(parse("Ctrl+A+B"), None);
        assert_eq!(parse("Hyper+X"), None);
    }
}

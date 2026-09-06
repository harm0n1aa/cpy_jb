//! Map typed characters onto US-HID keys the device already injects.
//!
//! The installed tweak types ASCII via HID and only inserts non-ASCII through a
//! SpringBoard broadcast that never reaches the focused app. With the Windows
//! layout on English, WM_CHAR is ASCII so HID works — and a Russian iOS keyboard
//! shows ЙЦУКЕН. After Alt+Shift, WM_CHAR is Cyrillic (or never arrives), HID
//! never fires, and nothing appears.
//!
//! On Windows we therefore type from physical QWERTY keys, ignoring the OS
//! layout. Translating leftover ЙЦУКЕН WM_CHAR (Linux, or if WM_CHAR still
//! arrives) back to the same US keys covers the other path.

use minifb::Key;

/// Convert a character from the Windows Russian (ЙЦУКЕН) layout into the US
/// key the tweak can HID-type. `None` means send the character unchanged.
pub fn to_hid_typeable(c: char) -> Option<char> {
    if c.is_ascii() {
        return Some(c);
    }
    let upper = c.is_uppercase();
    let base = match c.to_lowercase().next()? {
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'х' => '[',
        'ъ' => ']',
        'ф' => 'a',
        'ы' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'ж' => ';',
        'э' => '\'',
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        'б' => ',',
        'ю' => '.',
        'ё' => '`',
        _ => return None,
    };
    Some(if upper { us_shift(base) } else { base })
}

fn us_shift(c: char) -> char {
    match c {
        '`' => '~',
        '[' => '{',
        ']' => '}',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '-' => '_',
        '=' => '+',
        '\\' => '|',
        c if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
        _ => c,
    }
}

pub fn to_hid_typeable_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            continue;
        }
        match to_hid_typeable(c) {
            Some(mapped) => out.push(mapped),
            None => out.push(c),
        }
    }
    out
}

fn letter(lower: char, shift: bool, caps: bool) -> char {
    if shift ^ caps {
        lower.to_ascii_uppercase()
    } else {
        lower
    }
}

/// US-QWERTY character for a physical key, independent of the Windows layout.
/// `None` for keys that are not typed as text (modifiers, arrows, Enter, …).
pub fn hid_char_from_key(key: Key, shift: bool, caps: bool) -> Option<char> {
    Some(match key {
        Key::A => letter('a', shift, caps),
        Key::B => letter('b', shift, caps),
        Key::C => letter('c', shift, caps),
        Key::D => letter('d', shift, caps),
        Key::E => letter('e', shift, caps),
        Key::F => letter('f', shift, caps),
        Key::G => letter('g', shift, caps),
        Key::H => letter('h', shift, caps),
        Key::I => letter('i', shift, caps),
        Key::J => letter('j', shift, caps),
        Key::K => letter('k', shift, caps),
        Key::L => letter('l', shift, caps),
        Key::M => letter('m', shift, caps),
        Key::N => letter('n', shift, caps),
        Key::O => letter('o', shift, caps),
        Key::P => letter('p', shift, caps),
        Key::Q => letter('q', shift, caps),
        Key::R => letter('r', shift, caps),
        Key::S => letter('s', shift, caps),
        Key::T => letter('t', shift, caps),
        Key::U => letter('u', shift, caps),
        Key::V => letter('v', shift, caps),
        Key::W => letter('w', shift, caps),
        Key::X => letter('x', shift, caps),
        Key::Y => letter('y', shift, caps),
        Key::Z => letter('z', shift, caps),
        Key::Key1 => {
            if shift {
                '!'
            } else {
                '1'
            }
        }
        Key::Key2 => {
            if shift {
                '@'
            } else {
                '2'
            }
        }
        Key::Key3 => {
            if shift {
                '#'
            } else {
                '3'
            }
        }
        Key::Key4 => {
            if shift {
                '$'
            } else {
                '4'
            }
        }
        Key::Key5 => {
            if shift {
                '%'
            } else {
                '5'
            }
        }
        Key::Key6 => {
            if shift {
                '^'
            } else {
                '6'
            }
        }
        Key::Key7 => {
            if shift {
                '&'
            } else {
                '7'
            }
        }
        Key::Key8 => {
            if shift {
                '*'
            } else {
                '8'
            }
        }
        Key::Key9 => {
            if shift {
                '('
            } else {
                '9'
            }
        }
        Key::Key0 => {
            if shift {
                ')'
            } else {
                '0'
            }
        }
        Key::Space => ' ',
        Key::Minus => {
            if shift {
                '_'
            } else {
                '-'
            }
        }
        Key::Equal => {
            if shift {
                '+'
            } else {
                '='
            }
        }
        Key::LeftBracket => {
            if shift {
                '{'
            } else {
                '['
            }
        }
        Key::RightBracket => {
            if shift {
                '}'
            } else {
                ']'
            }
        }
        Key::Backslash => {
            if shift {
                '|'
            } else {
                '\\'
            }
        }
        Key::Semicolon => {
            if shift {
                ':'
            } else {
                ';'
            }
        }
        Key::Apostrophe => {
            if shift {
                '"'
            } else {
                '\''
            }
        }
        Key::Backquote => {
            if shift {
                '~'
            } else {
                '`'
            }
        }
        Key::Comma => {
            if shift {
                '<'
            } else {
                ','
            }
        }
        Key::Period => {
            if shift {
                '>'
            } else {
                '.'
            }
        }
        Key::Slash => {
            if shift {
                '?'
            } else {
                '/'
            }
        }
        Key::NumPad0 => '0',
        Key::NumPad1 => '1',
        Key::NumPad2 => '2',
        Key::NumPad3 => '3',
        Key::NumPad4 => '4',
        Key::NumPad5 => '5',
        Key::NumPad6 => '6',
        Key::NumPad7 => '7',
        Key::NumPad8 => '8',
        Key::NumPad9 => '9',
        Key::NumPadDot => '.',
        Key::NumPadSlash => '/',
        Key::NumPadAsterisk => '*',
        Key::NumPadMinus => '-',
        Key::NumPadPlus => '+',
        _ => return None,
    })
}

#[cfg(windows)]
mod win_keys {
    const VK_MENU: i32 = 0x12;
    const VK_CAPITAL: i32 = 0x14;
    const LANG_RUSSIAN: u32 = 0x19;
    const LANG_UKRAINIAN: u32 = 0x22;
    const LANG_BELARUSIAN: u32 = 0x23;
    const LANG_BULGARIAN: u32 = 0x02;

    extern "system" {
        fn GetKeyState(n_virt_key: i32) -> i16;
        fn GetKeyboardLayout(id_thread: u32) -> isize;
    }

    pub fn alt_down() -> bool {
        unsafe { GetKeyState(VK_MENU) as u16 & 0x8000 != 0 }
    }

    pub fn caps_on() -> bool {
        unsafe { GetKeyState(VK_CAPITAL) & 1 != 0 }
    }

    /// True when the focused Windows layout types Cyrillic (Alt+Shift to RU).
    pub fn layout_is_cyrillic() -> bool {
        let hkl = unsafe { GetKeyboardLayout(0) } as u32;
        let primary = hkl & 0x3ff;
        matches!(
            primary,
            LANG_RUSSIAN | LANG_UKRAINIAN | LANG_BELARUSIAN | LANG_BULGARIAN
        )
    }
}

#[cfg(windows)]
pub use win_keys::{alt_down, caps_on, layout_is_cyrillic};

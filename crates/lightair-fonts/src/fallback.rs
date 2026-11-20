//! The families to try when the chosen one lacks a character: Inter for Latin, Greek and
//! Cyrillic (bundled), the script's Noto family (downloaded when first needed), and Noto Color
//! Emoji for emoji (bundled) (docs/plans/fonts.md §3, §5).

use crate::bundled;

/// The Noto family that covers a character's script, when Inter doesn't.
pub fn noto_for(c: char) -> Option<&'static str> {
    let u = c as u32;
    Some(match u {
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => "Noto Sans Hebrew",
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => "Noto Sans Arabic",
        0x0900..=0x097F => "Noto Sans Devanagari",
        0x0980..=0x09FF => "Noto Sans Bengali",
        0x0A00..=0x0A7F => "Noto Sans Gurmukhi",
        0x0A80..=0x0AFF => "Noto Sans Gujarati",
        0x0B80..=0x0BFF => "Noto Sans Tamil",
        0x0C00..=0x0C7F => "Noto Sans Telugu",
        0x0C80..=0x0CFF => "Noto Sans Kannada",
        0x0D00..=0x0D7F => "Noto Sans Malayalam",
        0x0D80..=0x0DFF => "Noto Sans Sinhala",
        0x0E00..=0x0E7F => "Noto Sans Thai",
        0x0E80..=0x0EFF => "Noto Sans Lao",
        0x1000..=0x109F => "Noto Sans Myanmar",
        0x10A0..=0x10FF => "Noto Sans Georgian",
        0x0530..=0x058F => "Noto Sans Armenian",
        0x1200..=0x139F => "Noto Sans Ethiopic",
        0x1780..=0x17FF => "Noto Sans Khmer",
        0x3040..=0x30FF | 0x31F0..=0x31FF => "Noto Sans JP",
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => "Noto Sans KR",
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F => "Noto Sans SC",
        _ if is_emoji(c) => bundled::EMOJI,
        _ => return None,
    })
}

/// Emoji and pictographs (approximate, by block; good enough to choose a font).
pub fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F300..=0x1F5FF | 0x1F600..=0x1F64F | 0x1F680..=0x1F6FF | 0x1F900..=0x1F9FF
        | 0x1FA70..=0x1FAFF | 0x2600..=0x26FF | 0x2700..=0x27BF | 0x1F1E6..=0x1F1FF)
}

/// The fallback families `text` needs, in order, without the chosen family: Inter, then each
/// script's Noto family as it first appears, then emoji. Chinese characters next to kana are
/// Japanese, next to Hangul Korean.
pub fn families_for(text: &str) -> Vec<&'static str> {
    let has_kana = text.chars().any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x31F0..=0x31FF));
    let has_hangul = text.chars().any(|c| matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF));
    let mut out = vec![bundled::SANS];
    for c in text.chars() {
        let Some(mut family) = noto_for(c) else { continue };
        if family == "Noto Sans SC" && has_kana {
            family = "Noto Sans JP";
        } else if family == "Noto Sans SC" && has_hangul {
            family = "Noto Sans KR";
        }
        if !out.contains(&family) {
            out.push(family);
        }
    }
    // emoji always last
    if let Some(i) = out.iter().position(|f| *f == bundled::EMOJI) {
        out.remove(i);
        out.push(bundled::EMOJI);
    }
    out
}

/// The whole chain for text drawn in `chosen`.
pub fn chain(chosen: &str, text: &str) -> Vec<String> {
    let mut out = vec![chosen.to_string()];
    for f in families_for(text) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(f)) {
            out.push(f.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_choose_their_noto_family() {
        assert_eq!(families_for("Hello"), ["Inter"]);
        assert_eq!(families_for("Привет"), ["Inter"]);
        assert_eq!(families_for("渋谷へ行こう"), ["Inter", "Noto Sans JP"]);
        assert_eq!(families_for("中文"), ["Inter", "Noto Sans SC"]);
        assert_eq!(families_for("한국어 漢字"), ["Inter", "Noto Sans KR"]);
        assert_eq!(families_for("🔥 مرحبا"), ["Inter", "Noto Sans Arabic", "Noto Color Emoji"]);
    }

    #[test]
    fn the_chain_starts_with_the_chosen_family() {
        assert_eq!(chain("Bebas Neue", "GG 🎉"), ["Bebas Neue", "Inter", "Noto Color Emoji"]);
        assert_eq!(chain("Inter", "hi"), ["Inter"]);
    }
}

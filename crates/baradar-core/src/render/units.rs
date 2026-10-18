//! Unit conversions (port of `web/src/render/units.ts`).
//!
//! 180 dpi models (PT-P710BT, PT-E720BT) → 7.0866 dots/mm; 360 dpi models (PT-P910BT) →
//! 14.173 dots/mm. Callers pass the print area's dpi.

pub const MM_PER_INCH: f64 = 25.4;

/// `Math.round(mm·dpi/25.4)`. JavaScript rounds halves towards +∞, so this does too.
pub fn mm_to_dots(mm: f64, dpi: u16) -> i64 {
    js_round(mm * f64::from(dpi) / MM_PER_INCH)
}

pub fn dots_to_mm(dots: f64, dpi: u16) -> f64 {
    dots * MM_PER_INCH / f64::from(dpi)
}

/// Points → crisp-plane pixels: `pt·dpi·f/72` (1 pt = 2.5 dots at 180 dpi).
pub fn pt_to_px(pt: f64, dpi: u16, factor: u32) -> f64 {
    pt * f64::from(dpi) * f64::from(factor) / 72.0
}

/// JavaScript's `Math.round`: halves round towards +∞ (`-2.5` → `-2`), unlike `f64::round`.
pub fn js_round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// CSS colour names a tape/ink can report or a user is likely to type.
const NAMED: &[(&str, u32)] = &[
    ("black", 0x000000),
    ("white", 0xffffff),
    ("red", 0xff0000),
    ("green", 0x008000),
    ("lime", 0x00ff00),
    ("blue", 0x0000ff),
    ("navy", 0x000080),
    ("yellow", 0xffff00),
    ("gold", 0xffd700),
    ("orange", 0xffa500),
    ("pink", 0xffc0cb),
    ("purple", 0x800080),
    ("violet", 0xee82ee),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("silver", 0xc0c0c0),
    ("brown", 0xa52a2a),
    ("beige", 0xf5f5dc),
    ("cyan", 0x00ffff),
    ("magenta", 0xff00ff),
    ("transparent", 0xffffff),
];

/// `0xRRGGBB` from a CSS colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()`
/// (comma or space syntax, alpha ignored) or a common colour name. Black on parse failure.
pub fn css_to_rgb(css: &str) -> u32 {
    let s = css.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#')
        && matches!(hex.len(), 3 | 4 | 6 | 8)
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let full: String = if hex.len() <= 4 {
            hex.chars().take(3).flat_map(|c| [c, c]).collect()
        } else {
            hex[..6].to_owned()
        };
        return u32::from_str_radix(&full, 16).unwrap_or(0);
    }
    if let Some(rgb) = parse_rgb_fn(&s) {
        return rgb;
    }
    NAMED
        .iter()
        .find(|(name, _)| *name == s)
        .map_or(0, |(_, v)| *v)
}

fn parse_rgb_fn(s: &str) -> Option<u32> {
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    if !(3..=4).contains(&parts.len()) {
        return None;
    }
    let channel = |p: &str| -> Option<u32> {
        let (num, pct) = match p.strip_suffix('%') {
            Some(n) => (n, true),
            None => (p, false),
        };
        if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        let v: f64 = num.parse().ok()?;
        let v = if pct { v * 2.55 } else { v };
        Some(js_round(v).clamp(0, 255) as u32)
    };
    Some((channel(parts[0])? << 16) | (channel(parts[1])? << 8) | channel(parts[2])?)
}

/// `#rrggbb` for a `0xRRGGBB` number.
pub fn rgb_to_css(rgb: u32) -> String {
    format!("#{:06x}", rgb & 0xff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(mm_to_dots(10.0, 180), 71);
        assert_eq!(mm_to_dots(2.0, 180), 14);
        assert_eq!(mm_to_dots(1.0, 360), 14);
        assert!((dots_to_mm(128.0, 180) - 18.0622).abs() < 1e-3);
        assert_eq!(pt_to_px(72.0, 180, 3), 540.0);
        assert_eq!(js_round(-2.5), -2);
        assert_eq!(js_round(2.5), 3);
    }

    #[test]
    fn css_colours() {
        assert_eq!(css_to_rgb("#fff"), 0xffffff);
        assert_eq!(css_to_rgb("#11223344"), 0x112233);
        assert_eq!(css_to_rgb("rgb(255, 0, 10)"), 0xff000a);
        assert_eq!(css_to_rgb("rgba(100% 0% 0% / 0.5)"), 0xff0000);
        assert_eq!(css_to_rgb(" Navy "), 0x000080);
        assert_eq!(css_to_rgb("nonsense"), 0);
        assert_eq!(rgb_to_css(0xabc), "#000abc");
    }
}

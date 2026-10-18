//! Preferences (port of `web/src/doc/persist-prefs.ts`).
//!
//! Desktop differences: the browser-only fields (`lastPath`, `lastTransport`, `usbOnWindows`,
//! `designAnyway`) are replaced by `lastEndpoint`, a ptouch-transport endpoint string such as
//! `bt:00-11-22-33-44-55`. The default design model is the PT-E720BT, the printer this app was
//! built for (the studio defaults to the PT-P710BT, its only hardware-verified model).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Store, read_json, write_atomic};
use crate::doc::FONT_FAMILY_IDS;
use crate::doc::validate::{is_content_ref, is_model_name};

/// Model designed for when nothing is connected.
pub const DEFAULT_DESIGN_MODEL: &str = "PT-E720BT";
/// Most dismissed "untested model" notices remembered.
pub const MAX_DISMISSED_MODELS: usize = 40;
/// Most favourite fonts kept.
pub const MAX_FAVORITE_FONTS: usize = 50;
/// Most recent fonts kept.
pub const MAX_RECENT_FONTS: usize = 8;

/// A font in the picker's favourites/recents: `b:<family id>` (bundled), `u:<content ref>`
/// (uploaded) or `l:<PostScript name>` (installed).
pub type FontKey = String;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewMode {
    #[default]
    Design,
    Dots,
}

/// Size of a new text block (`DefaultTextSize`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum DefaultTextSize {
    /// Fit on narrow tape (≤ 9 mm), half the printable height on 12 mm and wider.
    #[default]
    Auto,
    Fit,
    Half,
    Third,
    Pt(f64),
}

impl Serialize for DefaultTextSize {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => s.serialize_str("auto"),
            Self::Fit => s.serialize_str("fit"),
            Self::Half => s.serialize_str("half"),
            Self::Third => s.serialize_str("third"),
            Self::Pt(pt) => serde_json::json!({ "pt": pt }).serialize(s),
        }
    }
}

/// The font of new text blocks (bundled fonts only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultFont {
    pub family: String,
    pub weight: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    /// Last successful connection, for "Reconnect" and auto-reconnect on start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_endpoint: Option<String>,
    /// Cleared by a deliberate "Disconnect".
    pub auto_reconnect: bool,
    pub theme: Theme,
    /// Id of the last open label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_label_id: Option<String>,
    /// Set when that label was made here as a new label and not edited since.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fresh_label_id: Option<String>,
    pub preview_zoom: f64,
    pub preview_mode: PreviewMode,
    /// Single-character keyboard shortcuts (1, 0, +, -, [, ], ?).
    pub single_key_shortcuts: bool,
    pub default_font: DefaultFont,
    pub default_text_size: DefaultTextSize,
    /// Starred fonts, in the order they were starred.
    pub favorite_fonts: Vec<FontKey>,
    /// Recently picked fonts, most recent first.
    pub recent_fonts: Vec<FontKey>,
    /// Model to design for while no printer is connected (and of the virtual printer).
    pub design_model: String,
    /// Models whose "untested model" notice was dismissed.
    pub dismissed_untested: Vec<String>,
    /// Look for a newer nightly build when the app starts.
    pub check_updates: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            last_endpoint: None,
            auto_reconnect: true,
            theme: Theme::System,
            last_label_id: None,
            fresh_label_id: None,
            preview_zoom: 1.0,
            preview_mode: PreviewMode::Design,
            single_key_shortcuts: true,
            default_font: DefaultFont {
                family: "fira-sans".into(),
                weight: 600,
            },
            default_text_size: DefaultTextSize::Auto,
            favorite_fonts: Vec::new(),
            recent_fonts: Vec::new(),
            design_model: DEFAULT_DESIGN_MODEL.into(),
            dismissed_untested: Vec::new(),
            check_updates: true,
        }
    }
}

/// `isFontKey`: a known bundled id, a content ref, or a safe local name.
pub fn is_font_key(v: &str) -> bool {
    let rest = v.get(2..).unwrap_or("");
    if v.starts_with("b:") {
        FONT_FAMILY_IDS.contains(&rest)
    } else if v.starts_with("u:") {
        is_content_ref(rest)
    } else if v.starts_with("l:") {
        (1..=100).contains(&rest.len())
            && rest.bytes().all(|b| (0x20..=0x7e).contains(&b))
            && !rest.contains(['"', '\'', '\\'])
    } else {
        false
    }
}

fn font_keys(v: Option<&Value>, max: usize) -> Vec<FontKey> {
    let mut out: Vec<FontKey> = Vec::new();
    for k in v.and_then(Value::as_array).into_iter().flatten() {
        if let Some(k) = k.as_str().filter(|k| is_font_key(k))
            && !out.iter().any(|o| o == k)
        {
            out.push(k.to_owned());
        }
    }
    out.truncate(max);
    out
}

fn non_empty_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `sanitizePrefs`: keeps only well-typed fields; anything else falls back to the default.
pub fn sanitize_prefs(raw: &Value) -> Prefs {
    let mut p = Prefs::default();
    let Some(r) = raw.as_object() else {
        return p;
    };
    let bool_of = |k: &str| r.get(k).and_then(Value::as_bool);
    p.last_endpoint = non_empty_str(r.get("lastEndpoint")).filter(|s| s.len() <= 256);
    if let Some(b) = bool_of("autoReconnect") {
        p.auto_reconnect = b;
    }
    if let Some(t) = r.get("theme").and_then(|t| Theme::deserialize(t).ok()) {
        p.theme = t;
    }
    p.last_label_id = non_empty_str(r.get("lastLabelId"));
    p.fresh_label_id = non_empty_str(r.get("freshLabelId"));
    if let Some(z) = r.get("previewZoom").and_then(Value::as_f64).filter(|z| z.is_finite()) {
        p.preview_zoom = z.clamp(0.1, 16.0);
    }
    if let Some(m) = r.get("previewMode").and_then(|m| PreviewMode::deserialize(m).ok()) {
        p.preview_mode = m;
    }
    if let Some(b) = bool_of("singleKeyShortcuts") {
        p.single_key_shortcuts = b;
    }
    if let Some(b) = bool_of("checkUpdates") {
        p.check_updates = b;
    }
    if let Some(f) = r.get("defaultFont").and_then(Value::as_object) {
        let family = f.get("family").and_then(Value::as_str);
        let weight = f.get("weight").and_then(Value::as_u64);
        if let (Some(family), Some(weight)) = (family, weight)
            && FONT_FAMILY_IDS.contains(&family)
            && [400, 500, 600, 700, 800].contains(&weight)
        {
            p.default_font = DefaultFont {
                family: family.into(),
                weight: weight as u16,
            };
        }
    }
    p.default_text_size = match r.get("defaultTextSize") {
        Some(Value::String(s)) => match s.as_str() {
            "fit" => DefaultTextSize::Fit,
            "half" => DefaultTextSize::Half,
            "third" => DefaultTextSize::Third,
            _ => DefaultTextSize::Auto,
        },
        Some(Value::Object(o)) => o
            .get("pt")
            .and_then(Value::as_f64)
            .filter(|pt| pt.is_finite())
            .map_or(DefaultTextSize::Auto, |pt| DefaultTextSize::Pt(pt.clamp(4.0, 144.0))),
        _ => DefaultTextSize::Auto,
    };
    p.favorite_fonts = font_keys(r.get("favoriteFonts"), MAX_FAVORITE_FONTS);
    p.recent_fonts = font_keys(r.get("recentFonts"), MAX_RECENT_FONTS);
    if let Some(m) = r.get("designModel").and_then(Value::as_str).filter(|m| is_model_name(m)) {
        p.design_model = m.into();
    }
    let mut dismissed: Vec<String> = Vec::new();
    for m in r
        .get("dismissedUntested")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(m) = m.as_str().filter(|m| is_model_name(m))
            && !dismissed.iter().any(|d| d == m)
        {
            dismissed.push(m.into());
        }
    }
    let skip = dismissed.len().saturating_sub(MAX_DISMISSED_MODELS);
    p.dismissed_untested = dismissed.split_off(skip);
    p
}

impl Store {
    /// `loadPrefs`: defaults when the file is missing or unreadable.
    pub fn load_prefs(&self) -> Prefs {
        read_json::<Value>(&self.path(&["prefs.json"]))
            .map(|v| sanitize_prefs(&v))
            .unwrap_or_default()
    }

    /// Saves the whole preferences object (the studio's `savePrefs(patch)` merges a patch; the
    /// app keeps the current `Prefs` in memory and saves it whole).
    pub fn save_prefs(&self, prefs: &Prefs) -> Result<()> {
        let mut json = serde_json::to_vec_pretty(prefs)?;
        json.push(b'\n');
        write_atomic(&self.path(&["prefs.json"]), &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitises_field_by_field() {
        let p = sanitize_prefs(&json!({
            "theme": "dark",
            "previewZoom": 99,
            "previewMode": "nope",
            "defaultFont": {"family": "oswald", "weight": 700},
            "defaultTextSize": {"pt": 1000},
            "favoriteFonts": ["b:oswald", "b:oswald", "b:nope", "u:sha256-0123456789abcdef0123456789abcdef", "l:Bad\"Name"],
            "designModel": "PT-P910BT",
            "dismissedUntested": ["PT-1", "PT-1", "!bad"],
        }));
        assert_eq!(p.theme, Theme::Dark);
        assert_eq!(p.preview_zoom, 16.0);
        assert_eq!(p.preview_mode, PreviewMode::Design);
        assert_eq!(p.default_font.family, "oswald");
        assert_eq!(p.default_text_size, DefaultTextSize::Pt(144.0));
        assert_eq!(
            p.favorite_fonts,
            ["b:oswald", "u:sha256-0123456789abcdef0123456789abcdef"]
        );
        assert_eq!(p.design_model, "PT-P910BT");
        assert_eq!(p.dismissed_untested, ["PT-1"]);
        assert_eq!(sanitize_prefs(&json!("junk")), Prefs::default());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.load_prefs(), Prefs::default());
        let p = Prefs {
            check_updates: false,
            last_endpoint: Some("bt:00-11-22-33-44-55".into()),
            recent_fonts: vec!["b:lexend".into()],
            default_text_size: DefaultTextSize::Third,
            ..Prefs::default()
        };
        store.save_prefs(&p).unwrap();
        assert_eq!(store.load_prefs(), p);
    }
}

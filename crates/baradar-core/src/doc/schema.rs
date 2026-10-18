//! Label document types (port of `web/src/doc/schema.ts`, schema version 4).
//!
//! Serialises to the same JSON the web studio reads and writes: camelCase keys, optional keys
//! omitted (never `null`), enums as the studio's strings. Lenient reading of untrusted JSON
//! (clamping, defaults, repairs) lives in [`super::validate`]; serde here is strict and used for
//! documents this app produced or already validated.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Current document schema.
pub const SCHEMA_VERSION: u32 = 4;

/// Tape widths the studio offers, in mm.
pub const TAPE_WIDTHS_MM: [f64; 7] = [3.5, 6.0, 9.0, 12.0, 18.0, 24.0, 36.0];

/// Built-in font family ids, in catalogue order.
pub const FONT_FAMILY_IDS: [&str; 25] = [
    "fira-sans",
    "archivo-narrow",
    "jetbrains-mono",
    "atkinson-hyperlegible",
    "oswald",
    "bebas-neue",
    "barlow-condensed",
    "anton",
    "barlow",
    "b612",
    "nunito",
    "fredoka",
    "quicksand",
    "caveat",
    "permanent-marker",
    "pacifico",
    "big-shoulders-stencil",
    "saira-stencil-one",
    "special-elite",
    "courier-prime",
    "roboto-slab",
    "silkscreen",
    "vt323",
    "pixelify-sans",
    "lexend",
];

/// Font weights with files in the catalogue.
pub const FONT_WEIGHTS: [u16; 5] = [400, 500, 600, 700, 800];

/// One label design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelDoc {
    /// Always [`SCHEMA_VERSION`] once validated.
    pub schema: u32,
    pub id: String,
    pub name: String,
    /// ISO 8601.
    pub created_at: String,
    /// ISO 8601.
    pub updated_at: String,
    pub tape: TapeSettings,
    pub length: LengthSettings,
    pub margins_mm: Margins,
    pub layout: LayoutSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<LabelFrame>,
    /// Flow order (left → right) or z-order (free layout).
    pub items: Vec<Item>,
    pub print: PrintSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TapeSettings {
    /// One of [`TAPE_WIDTHS_MM`].
    pub width_mm: f64,
    /// e.g. `"tze128-24"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    /// e.g. `"PT-P910BT"` (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Preview colours (CSS hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<TapeColors>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapeColors {
    pub tape: String,
    pub ink: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum LengthSettings {
    Auto,
    Fixed {
        mm: f64,
        /// Shrink text to fit the length. Written only when true.
        #[serde(default, skip_serializing_if = "is_false")]
        shrink: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum LayoutSettings {
    Flow {
        #[serde(rename = "gapMm")]
        gap_mm: f64,
        align: Align,
    },
    Free,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintSettings {
    pub copies: u32,
    pub auto_cut: bool,
    pub chain: bool,
    pub mirror: bool,
    /// Crisp-plane coverage threshold, 0–255 (the "Boldness" slider).
    pub threshold: u8,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            copies: 1,
            auto_cut: true,
            chain: false,
            mirror: false,
            threshold: 128,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelFrame {
    pub thickness_mm: f64,
    pub radius_mm: f64,
    pub inset_mm: f64,
}

impl LabelFrame {
    /// The UI's frame toggle default.
    pub const UI_DEFAULT: Self = Self {
        thickness_mm: 0.5,
        radius_mm: 1.0,
        inset_mm: 0.5,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchCounter {
    pub name: String,
    pub start: i64,
    pub step: i64,
    pub pad: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateFormat {
    #[default]
    Iso,
    Dmy,
    Mdy,
    Long,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchData {
    pub enabled: bool,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub count: u32,
    pub counters: Vec<BatchCounter>,
    pub date_format: DateFormat,
}

impl Default for BatchData {
    /// `createBatch()`.
    fn default() -> Self {
        Self {
            enabled: false,
            columns: Vec::new(),
            rows: Vec::new(),
            count: 10,
            counters: vec![BatchCounter {
                name: "n".into(),
                start: 1,
                step: 1,
                pad: 0,
            }],
            date_format: DateFormat::Iso,
        }
    }
}

/// Position of an item in free layout.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemFrame {
    pub x_mm: f64,
    pub y_mm: f64,
    pub w_mm: f64,
    pub h_mm: f64,
    /// 0, 90, 180 or 270.
    pub rotation: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ItemSize {
    Fit,
    Mm { mm: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum TextSize {
    Fit,
    /// Cap-to-descender height of the whole block.
    Mm { mm: f64 },
    /// Em size of every line (1 pt = 2.5 dots at 180 dpi).
    Pt { pt: f64 },
}

/// A block on the label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Item {
    Text(TextItem),
    Icon(IconItem),
    Code(CodeItem),
    Image(ImageItem),
    Shape(ShapeItem),
    Spacer(SpacerItem),
}

/// The discriminant of an [`Item`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Text,
    Icon,
    Code,
    Image,
    Shape,
    Spacer,
}

impl ItemKind {
    pub const ALL: [Self; 6] = [
        Self::Text,
        Self::Icon,
        Self::Code,
        Self::Image,
        Self::Shape,
        Self::Spacer,
    ];

    /// The JSON `kind` string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Icon => "icon",
            Self::Code => "code",
            Self::Image => "image",
            Self::Shape => "shape",
            Self::Spacer => "spacer",
        }
    }
}

macro_rules! item_common {
    ($($variant:ident),*) => {
        impl Item {
            pub fn id(&self) -> &str {
                match self { $(Self::$variant(i) => &i.id,)* }
            }
            pub fn set_id(&mut self, id: String) {
                match self { $(Self::$variant(i) => i.id = id,)* }
            }
            pub fn frame(&self) -> Option<&ItemFrame> {
                match self { $(Self::$variant(i) => i.frame.as_ref(),)* }
            }
            pub fn frame_mut(&mut self) -> &mut Option<ItemFrame> {
                match self { $(Self::$variant(i) => &mut i.frame,)* }
            }
            pub fn kind(&self) -> ItemKind {
                match self { $(Self::$variant(_) => ItemKind::$variant,)* }
            }
        }
    };
}
item_common!(Text, Icon, Code, Image, Shape, Spacer);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ItemFrame>,
    pub text: String,
    /// One of [`FONT_FAMILY_IDS`].
    pub font_family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_font: Option<FontSource>,
    /// One of [`FONT_WEIGHTS`].
    pub font_weight: u16,
    pub italic: bool,
    pub size: TextSize,
    pub align: Align,
    pub line_height: f64,
    pub invert: bool,
    /// Migrated v1 text: clip a too-tall size instead of shrinking it. Written only when true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clip_tall: bool,
}

/// A font outside the built-in catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FontSource {
    /// An uploaded font, stored by content hash (`sha256-<32 hex>`).
    User {
        #[serde(rename = "ref")]
        reference: String,
        family: String,
    },
    /// A font installed on the computer.
    Local {
        #[serde(rename = "postscriptName")]
        postscript_name: String,
        family: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ItemFrame>,
    pub icon_id: String,
    pub size: ItemSize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Symbology {
    #[default]
    Qr,
    Code128,
    Ean13,
    Datamatrix,
}

impl Symbology {
    /// Code 128 and EAN-13 (one-row bar codes).
    pub fn is_linear(self) -> bool {
        matches!(self, Self::Code128 | Self::Ean13)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QrEcc {
    L,
    #[default]
    M,
    Q,
    H,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeContent {
    #[default]
    Text,
    Wifi,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WifiSecurity {
    #[default]
    Wpa,
    Wep,
    Open,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuietZone {
    #[default]
    Standard,
    Compact,
    None,
}

/// Module size of a code: `"auto"` or a dot count (1–20).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ModuleSize {
    #[default]
    Auto,
    Dots(u8),
}

impl Serialize for ModuleSize {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => s.serialize_str("auto"),
            Self::Dots(n) => s.serialize_u8(*n),
        }
    }
}

impl<'de> Deserialize<'de> for ModuleSize {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) if s == "auto" => Ok(Self::Auto),
            serde_json::Value::Number(n) => n
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .map(Self::Dots)
                .ok_or_else(|| serde::de::Error::custom("moduleDots out of range")),
            other => Err(serde::de::Error::custom(format!(
                "moduleDots must be \"auto\" or a number, got {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiSettings {
    pub ssid: String,
    pub password: String,
    pub security: WifiSecurity,
    pub hidden: bool,
}

impl Default for WifiSettings {
    /// `createWifi()`.
    fn default() -> Self {
        Self {
            ssid: String::new(),
            password: String::new(),
            security: WifiSecurity::Wpa,
            hidden: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ItemFrame>,
    pub symbology: Symbology,
    pub content: CodeContent,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi: Option<WifiSettings>,
    pub module_dots: ModuleSize,
    pub quiet_zone: QuietZone,
    pub ecc: QrEcc,
    pub show_text: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DitherKind {
    Threshold,
    #[default]
    FloydSteinberg,
    Atkinson,
    Bayer4,
    Bayer8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAdjust {
    pub brightness: i32,
    pub contrast: i32,
    pub gamma_x100: u16,
    pub invert: bool,
    pub level: u8,
}

impl Default for ImageAdjust {
    fn default() -> Self {
        Self {
            brightness: 0,
            contrast: 0,
            gamma_x100: 100,
            invert: false,
            level: 128,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ItemFrame>,
    /// Content hash of the stored image (`sha256-…`), or empty.
    pub blob_ref: String,
    /// Inline image (`data:image/…`), used by share links and exported files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_url: Option<String>,
    pub size: ItemSize,
    pub dither: DitherKind,
    pub adjust: ImageAdjust,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeKind {
    #[default]
    Rect,
    Ellipse,
    Line,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ItemFrame>,
    pub shape: ShapeKind,
    pub width_mm: f64,
    pub size: ItemSize,
    pub stroke_mm: f64,
    pub fill: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacerItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ItemFrame>,
    pub width_mm: f64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A new random id (UUID v4, as `crypto.randomUUID()`).
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The current time as ISO 8601 with milliseconds and `Z` (as `Date.toISOString()`).
pub fn now_iso() -> String {
    let now = time::OffsetDateTime::now_utc();
    let fmt = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    );
    now.format(&fmt).unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".into())
}

impl Item {
    /// `createItem(kind)`: a new block with the studio's defaults.
    pub fn new(kind: ItemKind) -> Self {
        let id = new_id();
        match kind {
            ItemKind::Text => Self::Text(TextItem {
                id,
                frame: None,
                text: "Label".into(),
                font_family: "fira-sans".into(),
                custom_font: None,
                font_weight: 600,
                italic: false,
                size: TextSize::Fit,
                align: Align::Center,
                line_height: 1.1,
                invert: false,
                clip_tall: false,
            }),
            ItemKind::Icon => Self::Icon(IconItem {
                id,
                frame: None,
                icon_id: "bolt".into(),
                size: ItemSize::Fit,
            }),
            ItemKind::Code => Self::Code(CodeItem {
                id,
                frame: None,
                symbology: Symbology::Qr,
                content: CodeContent::Text,
                data: "https://example.com".into(),
                wifi: None,
                module_dots: ModuleSize::Auto,
                quiet_zone: QuietZone::Standard,
                ecc: QrEcc::M,
                show_text: false,
            }),
            ItemKind::Image => Self::Image(ImageItem {
                id,
                frame: None,
                blob_ref: String::new(),
                data_url: None,
                size: ItemSize::Fit,
                dither: DitherKind::FloydSteinberg,
                adjust: ImageAdjust::default(),
            }),
            ItemKind::Shape => Self::Shape(ShapeItem {
                id,
                frame: None,
                shape: ShapeKind::Rect,
                width_mm: 10.0,
                size: ItemSize::Fit,
                stroke_mm: 0.5,
                fill: false,
            }),
            ItemKind::Spacer => Self::Spacer(SpacerItem {
                id,
                frame: None,
                width_mm: 5.0,
            }),
        }
    }
}

impl LabelDoc {
    /// `createDoc()`: an "Untitled label" on 24 mm tape with one text block.
    pub fn new() -> Self {
        Self::with_items(vec![Item::new(ItemKind::Text)])
    }

    /// `createDoc({items})`.
    pub fn with_items(items: Vec<Item>) -> Self {
        let now = now_iso();
        Self {
            schema: SCHEMA_VERSION,
            id: new_id(),
            name: "Untitled label".into(),
            created_at: now.clone(),
            updated_at: now,
            tape: TapeSettings {
                width_mm: 24.0,
                media_id: None,
                model: None,
                colors: None,
            },
            length: LengthSettings::Auto,
            margins_mm: Margins {
                start: 2.0,
                end: 2.0,
            },
            layout: LayoutSettings::Flow {
                gap_mm: 3.0,
                align: Align::Center,
            },
            frame: None,
            items,
            print: PrintSettings::default(),
            batch: None,
        }
    }

    /// The item with `id`.
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id() == id)
    }

    /// The item with `id`, mutably.
    pub fn item_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id() == id)
    }
}

impl Default for LabelDoc {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_doc_serialises_like_the_studio() {
        let mut doc = LabelDoc::new();
        doc.id = "d".into();
        doc.items[0].set_id("t".into());
        let v = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["schema"], 4);
        assert_eq!(v["tape"], serde_json::json!({"widthMm": 24.0}));
        assert_eq!(v["length"], serde_json::json!({"mode": "auto"}));
        assert_eq!(
            v["layout"],
            serde_json::json!({"mode": "flow", "gapMm": 3.0, "align": "center"})
        );
        assert!(v.get("frame").is_none() && v.get("batch").is_none());
        let t = &v["items"][0];
        assert_eq!(t["kind"], "text");
        assert_eq!(t["fontFamily"], "fira-sans");
        assert_eq!(t["size"], serde_json::json!({"mode": "fit"}));
        assert!(t.get("clipTall").is_none() && t.get("customFont").is_none());
        let back: LabelDoc = serde_json::from_value(v).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn enums_use_the_studio_strings() {
        let Item::Code(mut c) = Item::new(ItemKind::Code) else {
            unreachable!()
        };
        c.module_dots = ModuleSize::Dots(3);
        let v = serde_json::to_value(Item::Code(c)).unwrap();
        assert_eq!(v["moduleDots"], 3);
        assert_eq!(v["quietZone"], "standard");
        assert_eq!(v["ecc"], "M");
        let v = serde_json::to_value(Item::new(ItemKind::Image)).unwrap();
        assert_eq!(v["dither"], "floyd-steinberg");
        assert_eq!(v["adjust"]["gammaX100"], 100);
        let f = FontSource::User {
            reference: "sha256-00".into(),
            family: "X".into(),
        };
        assert_eq!(
            serde_json::to_value(f).unwrap(),
            serde_json::json!({"kind": "user", "ref": "sha256-00", "family": "X"})
        );
    }
}

//! Fonts: the bundled library, custom fonts and the registry the renderer resolves text items
//! with (port of `web/src/render/font-catalog.ts`, `fonts.ts`, the name-table and validation
//! parts of `web/src/doc/persist-fonts.ts`, and system font discovery adapted from
//! `crates/ptouch-cli/src/render/fonts.rs`).
//!
//! The bundled faces are WOFF2 files embedded with `include_bytes!` (`assets/fonts/`, provenance
//! and licences in `assets/fonts/SOURCES.md`). Each file is decoded to TTF once, on first use, and
//! kept for the life of the process. The browser registers every family under a private name so
//! an installed copy never stands in; natively the embedded bytes are always used.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rustybuzz::ttf_parser::Tag;

use crate::doc::schema::{FontSource, Item, LabelDoc, Symbology, TextItem};
use crate::render::text::FaceMetrics;

// ------------------------------------------------------------------------------------------
// Catalogue (font-catalog.ts)
// ------------------------------------------------------------------------------------------

/// Picker sections (and filter chips), in display order (`FontCategory`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontCategory {
    Sans,
    Condensed,
    Industrial,
    Rounded,
    Handwritten,
    Stencil,
    Typewriter,
    Pixel,
    Accessible,
    Mono,
}

impl FontCategory {
    /// The id string of `FONT_CATEGORIES`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Sans => "sans",
            Self::Condensed => "condensed",
            Self::Industrial => "industrial",
            Self::Rounded => "rounded",
            Self::Handwritten => "handwritten",
            Self::Stencil => "stencil",
            Self::Typewriter => "typewriter",
            Self::Pixel => "pixel",
            Self::Accessible => "accessible",
            Self::Mono => "mono",
        }
    }
}

/// `FONT_CATEGORIES`: picker sections with their labels, in display order.
pub const FONT_CATEGORIES: [(FontCategory, &str); 10] = [
    (FontCategory::Sans, "Sans"),
    (FontCategory::Condensed, "Condensed & bold"),
    (FontCategory::Industrial, "Industrial"),
    (FontCategory::Rounded, "Rounded"),
    (FontCategory::Handwritten, "Handwritten"),
    (FontCategory::Stencil, "Stencil"),
    (FontCategory::Typewriter, "Typewriter & slab"),
    (FontCategory::Pixel, "Pixel"),
    (FontCategory::Accessible, "Easy to read"),
    (FontCategory::Mono, "Monospaced"),
];

/// SPDX id of a bundled font's licence (`FontLicense`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontLicense {
    Ofl11,
    Apache20,
}

impl FontLicense {
    pub fn spdx(self) -> &'static str {
        match self {
            Self::Ofl11 => "OFL-1.1",
            Self::Apache20 => "Apache-2.0",
        }
    }
}

/// CSS generic family the browser draws when a face failed to load (`FontDef.generic`). Natively
/// it only classifies the family (monospace, serif, cursive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GenericFamily {
    SansSerif,
    Serif,
    Monospace,
    Cursive,
}

impl GenericFamily {
    pub fn css(self) -> &'static str {
        match self {
            Self::SansSerif => "sans-serif",
            Self::Serif => "serif",
            Self::Monospace => "monospace",
            Self::Cursive => "cursive",
        }
    }
}

/// `FontDef.quality`: thin strokes or a script; warned about below [`MIN_QUALITY_CAP_MM`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontQuality {
    Thin,
    Script,
}

/// `FontDef.pixel`: `em_px` design pixels per em (one design pixel = unitsPerEm / emPx units).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelGrid {
    pub em_px: u32,
}

/// One bundled family (`FontDef`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontDef {
    pub id: &'static str,
    /// CSS family name the studio registers: always "ptouch <label>".
    pub family: &'static str,
    /// Display name.
    pub label: &'static str,
    pub category: FontCategory,
    /// Weights that have their own file; others use the nearest ([`resolve_weight`]).
    pub weights: &'static [u16],
    /// File per weight (in `assets/fonts/`).
    pub files: &'static [(u16, &'static str)],
    pub license: FontLicense,
    /// Licence text next to the files.
    pub license_file: &'static str,
    /// Short description for the font picker.
    pub hint: &'static str,
    /// Sample the picker shows when the text block is empty.
    pub preview: &'static str,
    pub generic: GenericFamily,
    /// Core family (precached by the studio's service worker).
    pub core: bool,
    pub pixel: Option<PixelGrid>,
    pub quality: Option<FontQuality>,
    /// Narrow letters: more text per label.
    pub condensed: bool,
}

impl FontDef {
    /// The file of a weight that has one.
    pub fn file(&self, weight: u16) -> Option<&'static str> {
        self.files
            .iter()
            .find(|(w, _)| *w == weight)
            .map(|(_, f)| *f)
    }
    pub fn monospace(&self) -> bool {
        self.generic == GenericFamily::Monospace
    }
    pub fn serif(&self) -> bool {
        self.generic == GenericFamily::Serif
    }
    pub fn cursive(&self) -> bool {
        self.generic == GenericFamily::Cursive
    }
    /// One variable file serves several weights (Quicksand): the `wght` axis is set per weight.
    pub fn variable(&self) -> bool {
        self.files.iter().any(|(_, f)| f.contains("-Variable."))
    }
    /// The licence text bundled with the family.
    pub fn license_text(&self) -> Option<&'static str> {
        bundled_license_text(self.license_file)
    }
}

/// Cap height (mm) below which a `quality` font prints poorly (`MIN_QUALITY_CAP_MM`).
pub const MIN_QUALITY_CAP_MM: f64 = 3.0;

/// The family and weight of the human-readable line under linear barcodes (`CODE_TEXT_FONT`).
pub const CODE_TEXT_FONT: (&str, u16) = ("jetbrains-mono", 400);

const BASE: FontDef = FontDef {
    id: "",
    family: "",
    label: "",
    category: FontCategory::Sans,
    weights: &[],
    files: &[],
    license: FontLicense::Ofl11,
    license_file: "",
    hint: "",
    preview: "",
    generic: GenericFamily::SansSerif,
    core: false,
    pixel: None,
    quality: None,
    condensed: false,
};

/// `FONTS`: the four core families first; `FONTS[0]` is the fallback for unknown ids.
pub static FONTS: [FontDef; 25] = [
    FontDef {
        id: "fira-sans",
        family: "ptouch Fira Sans",
        label: "Fira Sans",
        category: FontCategory::Sans,
        weights: &[400, 500, 600, 700, 800],
        files: &[
            (400, "FiraSans-Regular.woff2"),
            (500, "FiraSans-Medium.woff2"),
            (600, "FiraSans-SemiBold.woff2"),
            (700, "FiraSans-Bold.woff2"),
            (800, "FiraSans-ExtraBold.woff2"),
        ],
        license_file: "OFL-FiraSans.txt",
        hint: "Clear, sturdy sans-serif",
        preview: "Label 123",
        core: true,
        ..BASE
    },
    FontDef {
        id: "archivo-narrow",
        family: "ptouch Archivo Narrow",
        label: "Archivo Narrow",
        category: FontCategory::Condensed,
        weights: &[400, 500, 600, 700],
        files: &[
            (400, "ArchivoNarrow-Regular.woff2"),
            (500, "ArchivoNarrow-Medium.woff2"),
            (600, "ArchivoNarrow-SemiBold.woff2"),
            (700, "ArchivoNarrow-Bold.woff2"),
        ],
        license_file: "OFL-ArchivoNarrow.txt",
        hint: "Condensed: more text per label",
        preview: "Label 123",
        core: true,
        condensed: true,
        ..BASE
    },
    FontDef {
        id: "jetbrains-mono",
        family: "ptouch JetBrains Mono",
        label: "JetBrains Mono",
        category: FontCategory::Mono,
        weights: &[400, 700],
        files: &[
            (400, "JetBrainsMono-Regular.woff2"),
            (700, "JetBrainsMono-Bold.woff2"),
        ],
        license_file: "OFL-JetBrainsMono.txt",
        hint: "Monospaced: serial numbers, codes",
        preview: "SN 0O1lI",
        generic: GenericFamily::Monospace,
        core: true,
        ..BASE
    },
    FontDef {
        id: "atkinson-hyperlegible",
        family: "ptouch Atkinson Hyperlegible",
        label: "Atkinson Hyperlegible",
        category: FontCategory::Accessible,
        weights: &[400, 700],
        files: &[
            (400, "AtkinsonHyperlegible-Regular.woff2"),
            (700, "AtkinsonHyperlegible-Bold.woff2"),
        ],
        license_file: "OFL-AtkinsonHyperlegible.txt",
        hint: "Designed for low vision: distinct 0/O, 1/l/I",
        preview: "0O 1lI Label",
        core: true,
        ..BASE
    },
    // Library families, in schema order (by category).
    FontDef {
        id: "oswald",
        family: "ptouch Oswald",
        label: "Oswald",
        category: FontCategory::Condensed,
        weights: &[400, 700],
        files: &[(400, "Oswald-Regular.woff2"), (700, "Oswald-Bold.woff2")],
        license_file: "OFL-Oswald.txt",
        hint: "Tall condensed sans: long words, short labels",
        preview: "LABEL 123",
        condensed: true,
        ..BASE
    },
    FontDef {
        id: "bebas-neue",
        family: "ptouch Bebas Neue",
        label: "Bebas Neue",
        category: FontCategory::Condensed,
        weights: &[400],
        files: &[(400, "BebasNeue-Regular.woff2")],
        license_file: "OFL-BebasNeue.txt",
        hint: "Narrow capitals only: bold signage",
        preview: "TOOLS 24",
        condensed: true,
        ..BASE
    },
    FontDef {
        id: "barlow-condensed",
        family: "ptouch Barlow Condensed",
        label: "Barlow Condensed",
        category: FontCategory::Condensed,
        weights: &[500, 700],
        files: &[
            (500, "BarlowCondensed-Medium.woff2"),
            (700, "BarlowCondensed-Bold.woff2"),
        ],
        license_file: "OFL-BarlowCondensed.txt",
        hint: "Narrow road-sign grotesk",
        preview: "Cable 2.5 mm²",
        condensed: true,
        ..BASE
    },
    FontDef {
        id: "anton",
        family: "ptouch Anton",
        label: "Anton",
        category: FontCategory::Condensed,
        weights: &[400],
        files: &[(400, "Anton-Regular.woff2")],
        license_file: "OFL-Anton.txt",
        hint: "Heavy and narrow: big, loud words",
        preview: "BOLD 42",
        condensed: true,
        ..BASE
    },
    FontDef {
        id: "barlow",
        family: "ptouch Barlow",
        label: "Barlow",
        category: FontCategory::Industrial,
        weights: &[400, 600],
        files: &[
            (400, "Barlow-Regular.woff2"),
            (600, "Barlow-SemiBold.woff2"),
        ],
        license_file: "OFL-Barlow.txt",
        hint: "Plain technical grotesk with a DIN feel",
        preview: "Breaker 16 A",
        ..BASE
    },
    FontDef {
        id: "b612",
        family: "ptouch B612",
        label: "B612",
        category: FontCategory::Industrial,
        weights: &[400, 700],
        files: &[(400, "B612-Regular.woff2"), (700, "B612-Bold.woff2")],
        license_file: "OFL-B612.txt",
        hint: "Made for cockpit displays: open, unambiguous",
        preview: "ALT 3500 ft",
        ..BASE
    },
    FontDef {
        id: "nunito",
        family: "ptouch Nunito",
        label: "Nunito",
        category: FontCategory::Rounded,
        weights: &[400, 700],
        files: &[(400, "Nunito-Regular.woff2"), (700, "Nunito-Bold.woff2")],
        license_file: "OFL-Nunito.txt",
        hint: "Friendly rounded sans",
        preview: "Kitchen",
        ..BASE
    },
    FontDef {
        id: "fredoka",
        family: "ptouch Fredoka",
        label: "Fredoka",
        category: FontCategory::Rounded,
        weights: &[400, 600],
        files: &[
            (400, "Fredoka-Regular.woff2"),
            (600, "Fredoka-SemiBold.woff2"),
        ],
        license_file: "OFL-Fredoka.txt",
        hint: "Round and bubbly: toys, kids’ things",
        preview: "Toys & Games",
        ..BASE
    },
    // Reserved Font Name: shipped unmodified, one variable file for both weights.
    FontDef {
        id: "quicksand",
        family: "ptouch Quicksand",
        label: "Quicksand",
        category: FontCategory::Rounded,
        weights: &[500, 700],
        files: &[
            (500, "Quicksand-Variable.woff2"),
            (700, "Quicksand-Variable.woff2"),
        ],
        license_file: "OFL-Quicksand.txt",
        hint: "Light, round geometric sans",
        preview: "Spices",
        quality: Some(FontQuality::Thin),
        ..BASE
    },
    FontDef {
        id: "caveat",
        family: "ptouch Caveat",
        label: "Caveat",
        category: FontCategory::Handwritten,
        weights: &[400, 700],
        files: &[(400, "Caveat-Regular.woff2"), (700, "Caveat-Bold.woff2")],
        license_file: "OFL-Caveat.txt",
        hint: "Casual handwriting",
        preview: "Homemade jam",
        generic: GenericFamily::Cursive,
        quality: Some(FontQuality::Script),
        ..BASE
    },
    FontDef {
        id: "permanent-marker",
        family: "ptouch Permanent Marker",
        label: "Permanent Marker",
        category: FontCategory::Handwritten,
        weights: &[400],
        files: &[(400, "PermanentMarker-Regular.woff2")],
        license: FontLicense::Apache20,
        license_file: "Apache-PermanentMarker.txt",
        hint: "Felt-tip marker capitals",
        preview: "DO NOT TOUCH",
        ..BASE
    },
    FontDef {
        id: "pacifico",
        family: "ptouch Pacifico",
        label: "Pacifico",
        category: FontCategory::Handwritten,
        weights: &[400],
        files: &[(400, "Pacifico-Regular.woff2")],
        license_file: "OFL-Pacifico.txt",
        hint: "Retro surf-style brush script",
        preview: "Hello!",
        generic: GenericFamily::Cursive,
        quality: Some(FontQuality::Script),
        ..BASE
    },
    FontDef {
        id: "big-shoulders-stencil",
        family: "ptouch Big Shoulders Stencil",
        label: "Big Shoulders Stencil",
        category: FontCategory::Stencil,
        weights: &[400, 700],
        files: &[
            (400, "BigShouldersStencil-Regular.woff2"),
            (700, "BigShouldersStencil-Bold.woff2"),
        ],
        license_file: "OFL-BigShouldersStencil.txt",
        hint: "Narrow Chicago-style stencil",
        preview: "CRATE 07",
        condensed: true,
        ..BASE
    },
    FontDef {
        id: "saira-stencil-one",
        family: "ptouch Saira Stencil One",
        label: "Saira Stencil One",
        category: FontCategory::Stencil,
        weights: &[400],
        files: &[(400, "SairaStencilOne-Regular.woff2")],
        license_file: "OFL-SairaStencilOne.txt",
        hint: "Wide, heavy military stencil",
        preview: "CARGO 12",
        ..BASE
    },
    FontDef {
        id: "special-elite",
        family: "ptouch Special Elite",
        label: "Special Elite",
        category: FontCategory::Typewriter,
        weights: &[400],
        files: &[(400, "SpecialElite-Regular.woff2")],
        license: FontLicense::Apache20,
        license_file: "Apache-SpecialElite.txt",
        hint: "Worn, inky typewriter",
        preview: "Archive 1962",
        generic: GenericFamily::Serif,
        ..BASE
    },
    FontDef {
        id: "courier-prime",
        family: "ptouch Courier Prime",
        label: "Courier Prime",
        category: FontCategory::Typewriter,
        weights: &[400, 700],
        files: &[
            (400, "CourierPrime-Regular.woff2"),
            (700, "CourierPrime-Bold.woff2"),
        ],
        license_file: "OFL-CourierPrime.txt",
        hint: "Clean monospaced typewriter",
        preview: "Ref. 0042",
        generic: GenericFamily::Monospace,
        ..BASE
    },
    FontDef {
        id: "roboto-slab",
        family: "ptouch Roboto Slab",
        label: "Roboto Slab",
        category: FontCategory::Typewriter,
        weights: &[400, 700],
        files: &[
            (400, "RobotoSlab-Regular.woff2"),
            (700, "RobotoSlab-Bold.woff2"),
        ],
        license: FontLicense::Apache20,
        license_file: "Apache-RobotoSlab.txt",
        hint: "Sturdy slab serif",
        preview: "Pantry",
        generic: GenericFamily::Serif,
        ..BASE
    },
    // A 125-unit grid at 1000 units per em. VT323 and Pixelify Sans are not on a whole grid.
    FontDef {
        id: "silkscreen",
        family: "ptouch Silkscreen",
        label: "Silkscreen",
        category: FontCategory::Pixel,
        weights: &[400, 700],
        files: &[
            (400, "Silkscreen-Regular.woff2"),
            (700, "Silkscreen-Bold.woff2"),
        ],
        license_file: "OFL-Silkscreen.txt",
        hint: "Tiny 8-pixel capitals: crisp at whole-dot scales",
        preview: "PIXEL 8",
        generic: GenericFamily::Monospace,
        pixel: Some(PixelGrid { em_px: 8 }),
        ..BASE
    },
    FontDef {
        id: "vt323",
        family: "ptouch VT323",
        label: "VT323",
        category: FontCategory::Pixel,
        weights: &[400],
        files: &[(400, "VT323-Regular.woff2")],
        license_file: "OFL-VT323.txt",
        hint: "Retro DEC terminal lettering",
        preview: "> READY_",
        generic: GenericFamily::Monospace,
        ..BASE
    },
    FontDef {
        id: "pixelify-sans",
        family: "ptouch Pixelify Sans",
        label: "Pixelify Sans",
        category: FontCategory::Pixel,
        weights: &[400, 700],
        files: &[
            (400, "PixelifySans-Regular.woff2"),
            (700, "PixelifySans-Bold.woff2"),
        ],
        license_file: "OFL-PixelifySans.txt",
        hint: "Pixel-art look, smooth at any size",
        preview: "Level 1",
        ..BASE
    },
    FontDef {
        id: "lexend",
        family: "ptouch Lexend",
        label: "Lexend",
        category: FontCategory::Accessible,
        weights: &[400, 700],
        files: &[(400, "Lexend-Regular.woff2"), (700, "Lexend-Bold.woff2")],
        license_file: "OFL-Lexend.txt",
        hint: "Wide, evenly spaced letters for easier reading",
        preview: "Easy to read",
        ..BASE
    },
];

/// The registry entry of `id`, or `None` (`findFont`).
pub fn find_font(id: &str) -> Option<&'static FontDef> {
    FONTS.iter().find(|f| f.id == id)
}

/// The entry of `id`, falling back to `FONTS[0]` (Fira Sans) for an unknown id (`fontDef`).
pub fn font_def(id: &str) -> &'static FontDef {
    find_font(id).unwrap_or(&FONTS[0])
}

/// Nearest weight that has a file; ties go to the lighter weight (`resolveWeight`).
pub fn resolve_weight(def: &FontDef, weight: u16) -> u16 {
    let mut best = def.weights.first().copied().unwrap_or(400);
    for &w in def.weights {
        if (i32::from(w) - i32::from(weight)).abs() < (i32::from(best) - i32::from(weight)).abs() {
            best = w;
        }
    }
    best
}

/// Every file of the core families (`coreFontFiles`).
pub fn core_font_files() -> Vec<&'static str> {
    FONTS
        .iter()
        .filter(|f| f.core)
        .flat_map(|f| f.files.iter().map(|(_, file)| *file))
        .collect()
}

// ------------------------------------------------------------------------------------------
// Embedded files
// ------------------------------------------------------------------------------------------

macro_rules! embedded {
    ($fn_name:ident, [$($file:literal),* $(,)?]) => {
        fn $fn_name(name: &str) -> Option<&'static [u8]> {
            match name {
                $($file => Some(&include_bytes!(concat!("../../assets/fonts/", $file))[..]),)*
                _ => None,
            }
        }
    };
}

embedded!(
    embedded_font,
    [
        "Anton-Regular.woff2",
        "ArchivoNarrow-Bold.woff2",
        "ArchivoNarrow-Medium.woff2",
        "ArchivoNarrow-Regular.woff2",
        "ArchivoNarrow-SemiBold.woff2",
        "AtkinsonHyperlegible-Bold.woff2",
        "AtkinsonHyperlegible-Regular.woff2",
        "B612-Bold.woff2",
        "B612-Regular.woff2",
        "Barlow-Regular.woff2",
        "Barlow-SemiBold.woff2",
        "BarlowCondensed-Bold.woff2",
        "BarlowCondensed-Medium.woff2",
        "BebasNeue-Regular.woff2",
        "BigShouldersStencil-Bold.woff2",
        "BigShouldersStencil-Regular.woff2",
        "Caveat-Bold.woff2",
        "Caveat-Regular.woff2",
        "CourierPrime-Bold.woff2",
        "CourierPrime-Regular.woff2",
        "FiraSans-Bold.woff2",
        "FiraSans-ExtraBold.woff2",
        "FiraSans-Medium.woff2",
        "FiraSans-Regular.woff2",
        "FiraSans-SemiBold.woff2",
        "Fredoka-Regular.woff2",
        "Fredoka-SemiBold.woff2",
        "JetBrainsMono-Bold.woff2",
        "JetBrainsMono-Regular.woff2",
        "Lexend-Bold.woff2",
        "Lexend-Regular.woff2",
        "Nunito-Bold.woff2",
        "Nunito-Regular.woff2",
        "Oswald-Bold.woff2",
        "Oswald-Regular.woff2",
        "Pacifico-Regular.woff2",
        "PermanentMarker-Regular.woff2",
        "PixelifySans-Bold.woff2",
        "PixelifySans-Regular.woff2",
        "Quicksand-Variable.woff2",
        "RobotoSlab-Bold.woff2",
        "RobotoSlab-Regular.woff2",
        "SairaStencilOne-Regular.woff2",
        "Silkscreen-Bold.woff2",
        "Silkscreen-Regular.woff2",
        "SpecialElite-Regular.woff2",
        "VT323-Regular.woff2",
    ]
);

macro_rules! embedded_text {
    ($fn_name:ident, [$($file:literal),* $(,)?]) => {
        fn $fn_name(name: &str) -> Option<&'static str> {
            match name {
                $($file => Some(include_str!(concat!("../../assets/fonts/", $file))),)*
                _ => None,
            }
        }
    };
}

embedded_text!(
    bundled_license_text,
    [
        "Apache-PermanentMarker.txt",
        "Apache-RobotoSlab.txt",
        "Apache-SpecialElite.txt",
        "OFL-Anton.txt",
        "OFL-ArchivoNarrow.txt",
        "OFL-AtkinsonHyperlegible.txt",
        "OFL-B612.txt",
        "OFL-Barlow.txt",
        "OFL-BarlowCondensed.txt",
        "OFL-BebasNeue.txt",
        "OFL-BigShouldersStencil.txt",
        "OFL-Caveat.txt",
        "OFL-CourierPrime.txt",
        "OFL-FiraSans.txt",
        "OFL-Fredoka.txt",
        "OFL-JetBrainsMono.txt",
        "OFL-Lexend.txt",
        "OFL-Nunito.txt",
        "OFL-Oswald.txt",
        "OFL-Pacifico.txt",
        "OFL-PixelifySans.txt",
        "OFL-Quicksand.txt",
        "OFL-SairaStencilOne.txt",
        "OFL-Silkscreen.txt",
        "OFL-VT323.txt",
    ]
);

/// The embedded WOFF2 bytes of a bundled font file (as named in [`FontDef::files`]).
pub fn bundled_font_file(name: &str) -> Option<&'static [u8]> {
    embedded_font(name)
}

/// The decoded sfnt (TTF) of a bundled font file, decoded on first use and cached for the life of
/// the process.
pub fn bundled_sfnt(name: &str) -> Result<Arc<[u8]>, FontError> {
    let file = FONTS
        .iter()
        .flat_map(|d| d.files.iter().map(|(_, f)| *f))
        .find(|f| *f == name)
        .ok_or_else(|| FontError::NoBundledFile(name.into()))?;
    bundled_bytes(file)
}

/// Provenance of the bundled fonts (`SOURCES.md`).
pub const FONT_SOURCES_MD: &str = include_str!("../../assets/fonts/SOURCES.md");

// ------------------------------------------------------------------------------------------
// Errors
// ------------------------------------------------------------------------------------------

/// Why a font could not be read or loaded. `Display` gives the studio's user-facing message where
/// it has one.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FontError {
    #[error("This file is empty.")]
    Empty,
    #[error(
        "This font is too large ({} MB). Fonts up to {} MB can be added.",
        crate::render::text::to_fixed(*size as f64 / 1024.0 / 1024.0, 1),
        MAX_FONT_BYTES / 1024 / 1024
    )]
    TooLarge { size: usize },
    #[error("This is not a font file this app can use. Choose a TTF, OTF, WOFF or WOFF2 file.")]
    NotAFont,
    #[error("You already have {MAX_FONTS} fonts. Remove one before adding another.")]
    TooMany,
    /// The file has a font signature but cannot be decoded or parsed.
    #[error("The font could not be read. The file may be damaged or in an unsupported format.")]
    Unreadable(String),
    /// A bundled family is unknown or its file is missing.
    #[error("No bundled font file {0}.")]
    NoBundledFile(String),
    #[error("The font “{0}” is not installed on this computer.")]
    NotInstalled(String),
}

// ------------------------------------------------------------------------------------------
// Font file formats and decoding
// ------------------------------------------------------------------------------------------

/// Largest font file accepted (`MAX_FONT_BYTES`).
pub const MAX_FONT_BYTES: usize = 10 * 1024 * 1024;
/// Fonts kept per installation (`MAX_FONTS`).
pub const MAX_FONTS: usize = 50;
/// Longest display name, in characters (`LIMITS.fontNameChars`).
pub const FONT_NAME_CHARS: usize = 100;
/// Largest decoded (sfnt) font we accept from a WOFF/WOFF2 container.
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

/// A font file's container format (`FontFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFormat {
    Ttf,
    Otf,
    Woff,
    Woff2,
}

impl FontFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ttf => "ttf",
            Self::Otf => "otf",
            Self::Woff => "woff",
            Self::Woff2 => "woff2",
        }
    }
    /// `FONT_MIME`.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Ttf => "font/ttf",
            Self::Otf => "font/otf",
            Self::Woff => "font/woff",
            Self::Woff2 => "font/woff2",
        }
    }
}

/// File picker `accept` list (`FONT_ACCEPT`).
pub const FONT_ACCEPT: &str = ".ttf,.otf,.woff,.woff2,font/ttf,font/otf,font/woff,font/woff2";

/// Font format from the first bytes (`sniffFontFormat`): `00 01 00 00`/`true` = TrueType,
/// `OTTO` = CFF, `wOFF`, `wOF2`. Needs at least 12 bytes.
pub fn sniff_font_format(bytes: &[u8]) -> Option<FontFormat> {
    if bytes.len() < 12 {
        return None;
    }
    match &bytes[..4] {
        [0, 1, 0, 0] | b"true" => Some(FontFormat::Ttf),
        b"OTTO" => Some(FontFormat::Otf),
        b"wOFF" => Some(FontFormat::Woff),
        b"wOF2" => Some(FontFormat::Woff2),
        _ => None,
    }
}

/// Format of a font file, or the user-facing error (`checkFontFile`). `size` is the file size
/// when only a prefix was read.
pub fn check_font_file(bytes: &[u8], size: usize) -> Result<FontFormat, FontError> {
    if size == 0 {
        return Err(FontError::Empty);
    }
    if size > MAX_FONT_BYTES {
        return Err(FontError::TooLarge { size });
    }
    sniff_font_format(bytes).ok_or(FontError::NotAFont)
}

/// The store's count limit (`FontStore.add`): an error once `stored` fonts are kept.
pub fn check_font_count(stored: usize) -> Result<(), FontError> {
    if stored >= MAX_FONTS {
        Err(FontError::TooMany)
    } else {
        Ok(())
    }
}

/// Decodes a font file of any supported format to sfnt (TTF/OTF/TTC) bytes. TTF and OTF are
/// returned as they are; WOFF tables are inflated (zlib); WOFF2 is decoded with `woff2-patched`.
pub fn decode_font_bytes(bytes: &[u8]) -> Result<Vec<u8>, FontError> {
    if bytes.len() >= 4 && &bytes[..4] == b"ttcf" {
        return Ok(bytes.to_vec());
    }
    match sniff_font_format(bytes).ok_or(FontError::NotAFont)? {
        FontFormat::Ttf | FontFormat::Otf => Ok(bytes.to_vec()),
        FontFormat::Woff => woff_to_sfnt(bytes),
        FontFormat::Woff2 => woff2_to_sfnt(bytes),
    }
}

/// Decodes WOFF2 with `woff2-patched`. Files whose transformed `glyf` table carries an
/// `overlapSimpleBitmap` are retried with that bitmap padded (see [`pad_woff2_overlap_bitmap`]).
fn woff2_to_sfnt(bytes: &[u8]) -> Result<Vec<u8>, FontError> {
    let mut buf = bytes;
    match woff2_patched::convert_woff2_to_ttf(&mut buf) {
        Ok(ttf) => Ok(ttf),
        Err(e) => {
            let fixed = pad_woff2_overlap_bitmap(bytes)
                .ok_or_else(|| FontError::Unreadable(e.to_string()))?;
            let mut buf = fixed.as_slice();
            woff2_patched::convert_woff2_to_ttf(&mut buf)
                .map_err(|e| FontError::Unreadable(e.to_string()))
        }
    }
}

/// A WOFF2 table directory entry: flags, explicit tag, original length, transform length.
type Woff2Entry = (u8, Option<[u8; 4]>, u32, Option<u32>);

/// The 63 table tags a WOFF2 directory entry names by index (WOFF2 §5.1).
const WOFF2_KNOWN_TAGS: &[u8; 252] = b"cmapheadhheahmtxmaxpnameOS/2postcvt fpgmglyflocaprepCFF VORGEBDTEBLCgasphdmxkernLTSHPCLTVDMXvheavmtxBASEGDEFGPOSGSUBEBSCJSTFMATHCBDTCBLCCOLRCPALSVG sbixacntavarbdatblocbslncvarfdscfeatfmtxfvargvarhstyjustlcarmortmorxopbdproptrakZapfSilfGlatGlocFeatSill";

fn read_base128(b: &[u8], p: &mut usize) -> Option<u32> {
    let mut n = 0u32;
    for _ in 0..5 {
        let byte = *b.get(*p)?;
        *p += 1;
        n = n.checked_mul(128)? | u32::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some(n);
        }
    }
    None
}

fn write_base128(out: &mut Vec<u8>, mut v: u32) {
    let mut tmp = vec![(v & 0x7f) as u8];
    v >>= 7;
    while v > 0 {
        tmp.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.extend(tmp.iter().rev());
}

/// Works around a `woff2-patched` 0.4 bug: the transformed `glyf` table's optional
/// `overlapSimpleBitmap` is ⌈n/8⌉ bytes (WOFF2 spec, as fontTools writes it), but the crate
/// expects 4·⌈n/32⌉ and fails with "Stream truncated". Rebuilds the file with the bitmap padded
/// to that length (zero bits, so nothing changes) and the stream re-compressed. `None` when the
/// file has no such bitmap, is a collection, or cannot be parsed.
fn pad_woff2_overlap_bitmap(input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;
    if input.get(..4)? != b"wOF2" || be32(input, 4)? == u32::from_be_bytes(*b"ttcf") {
        return None;
    }
    let n = usize::from(be16(input, 12)?);
    let compressed_len = usize32(input, 20)?;
    let mut dir: Vec<Woff2Entry> = Vec::with_capacity(n);
    let mut p = 48;
    for _ in 0..n {
        let flags = *input.get(p)?;
        p += 1;
        let index = usize::from(flags & 0x3f);
        let (tag, explicit) = if index == 63 {
            let t: [u8; 4] = input.get(p..p + 4)?.try_into().ok()?;
            p += 4;
            (t, Some(t))
        } else {
            (
                WOFF2_KNOWN_TAGS
                    .get(index * 4..index * 4 + 4)?
                    .try_into()
                    .ok()?,
                None,
            )
        };
        let orig = read_base128(input, &mut p)?;
        let version = flags >> 6;
        let transformed = if &tag == b"glyf" || &tag == b"loca" {
            version != 3
        } else {
            version != 0
        };
        let tlen = if transformed {
            Some(read_base128(input, &mut p)?)
        } else {
            None
        };
        dir.push((flags, explicit, orig, tlen));
    }
    let compressed = input.get(p..p.checked_add(compressed_len)?)?;
    let mut stream = Vec::new();
    brotli::Decompressor::new(compressed, 4096)
        .take(MAX_DECODED_BYTES as u64 + 1)
        .read_to_end(&mut stream)
        .ok()?;
    if stream.len() > MAX_DECODED_BYTES {
        return None;
    }
    // Find the transformed glyf table in the stream.
    let mut off = 0usize;
    let mut glyf = None;
    for (i, &(flags, explicit, orig, tlen)) in dir.iter().enumerate() {
        let len = usize::try_from(tlen.unwrap_or(orig)).ok()?;
        let index = usize::from(flags & 0x3f);
        let is_glyf =
            explicit.is_none() && WOFF2_KNOWN_TAGS.get(index * 4..index * 4 + 4) == Some(b"glyf");
        if is_glyf && tlen.is_some() {
            glyf = Some((i, off, len));
        }
        off = off.checked_add(len)?;
    }
    let (gi, goff, glen) = glyf?;
    let table = stream.get(goff..goff.checked_add(glen)?)?;
    let option_flags = be16(table, 2)?;
    if option_flags & 1 == 0 {
        return None;
    }
    let glyphs = usize::from(be16(table, 4)?);
    let pad = glyphs.div_ceil(32) * 4 - glyphs.div_ceil(8);
    if pad == 0 {
        return None;
    }
    let mut fixed = Vec::with_capacity(stream.len() + pad);
    fixed.extend_from_slice(&stream[..goff + glen]);
    fixed.resize(fixed.len() + pad, 0);
    fixed.extend_from_slice(&stream[goff + glen..]);

    let mut out = input.get(..48)?.to_vec();
    // Metadata and private blocks are not needed to decode: drop them.
    out[28..48].fill(0);
    for (i, &(flags, explicit, orig, tlen)) in dir.iter().enumerate() {
        out.push(flags);
        if let Some(t) = explicit {
            out.extend_from_slice(&t);
        }
        write_base128(&mut out, orig);
        if let Some(t) = tlen {
            let t = if i == gi {
                t.checked_add(u32::try_from(pad).ok()?)?
            } else {
                t
            };
            write_base128(&mut out, t);
        }
    }
    let mut recompressed = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut recompressed, 4096, 1, 22);
        w.write_all(&fixed).ok()?;
    }
    out[20..24].copy_from_slice(&u32::try_from(recompressed.len()).ok()?.to_be_bytes());
    out.extend_from_slice(&recompressed);
    out.resize(out.len().div_ceil(4) * 4, 0);
    let total = u32::try_from(out.len()).ok()?;
    out[8..12].copy_from_slice(&total.to_be_bytes());
    Some(out)
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

fn usize32(b: &[u8], at: usize) -> Option<usize> {
    be32(b, at).and_then(|v| usize::try_from(v).ok())
}

/// One WOFF 1.0 table, inflated when compressed.
fn woff_table_data(b: &[u8], rec: usize, max_orig: usize) -> Result<Vec<u8>, FontError> {
    let bad = || FontError::Unreadable("broken WOFF table directory".into());
    let off = usize32(b, rec + 4).ok_or_else(bad)?;
    let comp = usize32(b, rec + 8).ok_or_else(bad)?;
    let orig = usize32(b, rec + 12).ok_or_else(bad)?;
    if orig > max_orig {
        return Err(bad());
    }
    let data = b
        .get(off..off.checked_add(comp).ok_or_else(bad)?)
        .ok_or_else(bad)?;
    if comp >= orig {
        return Ok(data.to_vec());
    }
    let mut out = Vec::with_capacity(orig);
    flate2::read::ZlibDecoder::new(data)
        .take(orig as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| FontError::Unreadable(e.to_string()))?;
    if out.len() != orig {
        return Err(FontError::Unreadable("WOFF table size mismatch".into()));
    }
    Ok(out)
}

/// Rebuilds the sfnt of a WOFF 1.0 file (tables inflated, 4-byte aligned).
fn woff_to_sfnt(b: &[u8]) -> Result<Vec<u8>, FontError> {
    let bad = || FontError::Unreadable("broken WOFF header".into());
    let flavor = be32(b, 4).ok_or_else(bad)?;
    let n = usize::from(be16(b, 12).ok_or_else(bad)?);
    if n == 0 {
        return Err(bad());
    }
    let mut tables = Vec::with_capacity(n);
    let mut total = 12 + 16 * n;
    for i in 0..n {
        let rec = 44 + i * 20;
        let tag = b.get(rec..rec + 4).ok_or_else(bad)?;
        let checksum = be32(b, rec + 16).ok_or_else(bad)?;
        let data = woff_table_data(b, rec, MAX_DECODED_BYTES)?;
        total += data.len().div_ceil(4) * 4;
        if total > MAX_DECODED_BYTES {
            return Err(FontError::Unreadable("WOFF font too large".into()));
        }
        tables.push(([tag[0], tag[1], tag[2], tag[3]], checksum, data));
    }
    tables.sort_by_key(|t| t.0);
    let n16 = u16::try_from(n).map_err(|_| bad())?;
    let entry_selector = 15 - n16.leading_zeros() as u16;
    let search_range = (1u16 << entry_selector).saturating_mul(16);
    let range_shift = n16.saturating_mul(16).saturating_sub(search_range);
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&flavor.to_be_bytes());
    for v in [n16, search_range, entry_selector, range_shift] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + 16 * n;
    for (tag, checksum, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        for v in [offset, data.len()] {
            let v = u32::try_from(v).map_err(|_| bad())?;
            out.extend_from_slice(&v.to_be_bytes());
        }
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, _, data) in &tables {
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------
// Names (persist-fonts.ts)
// ------------------------------------------------------------------------------------------

/// Unicode general category Cf (format characters) for the code points fonts realistically hold.
fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// Printable, single-line, trimmed, capped at [`FONT_NAME_CHARS`] (`cleanName`).
fn clean_name(s: &str) -> String {
    let no_ctl: String = s
        .chars()
        .filter(|&c| !c.is_control() && !is_format_char(c))
        .collect();
    let collapsed = no_ctl.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .take(FONT_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A display name from a file name: "Inter-Bold_v4.ttf" → "Inter Bold v4" (`nameFromFileName`).
pub fn name_from_file_name(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let lower = base.to_ascii_lowercase();
    let stem = [".ttf", ".otf", ".woff2", ".woff", ".ttc"]
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map_or(base, |ext| &base[..base.len() - ext.len()]);
    let spaced: String = stem
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // A leading or trailing separator becomes a space in the studio and is trimmed away.
    let name = clean_name(&spaced);
    if name.is_empty() {
        "Uploaded font".into()
    } else {
        name
    }
}

/// Family and subfamily of a `name` table (`parseNameTable`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameInfo {
    pub family: Option<String>,
    pub subfamily: Option<String>,
}

/// The best record per name ID in `ids`: Windows (platform 3) or Unicode (0) records, US English
/// preferred, before Mac Roman (platform 1). Cleaned with `cleanName`.
fn best_names(t: &[u8], ids: &[u16]) -> HashMap<u16, String> {
    let mut found: HashMap<u16, (u8, String)> = HashMap::new();
    let count = usize::from(be16(t, 2).unwrap_or(0));
    let storage = usize::from(be16(t, 4).unwrap_or(0));
    for i in 0..count {
        let rec = 6 + i * 12;
        if rec + 12 > t.len() {
            break;
        }
        let field = |k: usize| be16(t, rec + k).unwrap_or(0);
        let (platform, encoding, language, id) = (field(0), field(2), field(4), field(6));
        let len = usize::from(field(8));
        let off = storage + usize::from(field(10));
        if !ids.contains(&id) || off + len > t.len() {
            continue;
        }
        let raw = &t[off..off + len];
        let utf16 = || {
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        };
        let (text, score) = if platform == 3 && matches!(encoding, 0 | 1 | 10) {
            (utf16(), if language == 0x409 { 4 } else { 3 })
        } else if platform == 0 {
            (utf16(), 2)
        } else if platform == 1 && encoding == 0 {
            (
                raw.iter().map(|&b| char::from(b)).collect(),
                u8::from(language == 0),
            )
        } else {
            continue;
        };
        let text = clean_name(&text);
        if !text.is_empty() && found.get(&id).is_none_or(|(s, _)| score > *s) {
            found.insert(id, (score, text));
        }
    }
    found.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

/// Family and subfamily from a `name` table: typographic IDs 16/17 win over 1/2 (`parseNameTable`).
pub fn parse_name_table(t: &[u8]) -> NameInfo {
    let mut n = best_names(t, &[1, 2, 16, 17]);
    let typo = n.contains_key(&16);
    let family = n.remove(&16).or_else(|| n.remove(&1));
    let subfamily = if typo {
        n.remove(&17).or_else(|| n.remove(&2))
    } else {
        n.remove(&2)
    };
    NameInfo { family, subfamily }
}

/// Byte range of a table in an sfnt whose table directory starts at `dir` (`sfntTable`).
fn sfnt_table<'a>(b: &'a [u8], dir: usize, want: &[u8; 4]) -> Option<&'a [u8]> {
    let n = usize::from(be16(b, dir + 4)?);
    for i in 0..n {
        let rec = dir + 12 + i * 16;
        if b.get(rec..rec + 4)? != want {
            continue;
        }
        let off = usize32(b, rec + 8)?;
        let len = usize32(b, rec + 12)?;
        return b.get(off..off.checked_add(len)?);
    }
    None
}

/// Display name of a font file: "Family Subfamily", the subfamily omitted when Regular
/// (`fontDisplayName`). WOFF2 files are decoded first (the studio names those after the file).
pub fn font_display_name(bytes: &[u8], format: FontFormat) -> Option<String> {
    let table: Vec<u8> = match format {
        FontFormat::Ttf | FontFormat::Otf => sfnt_table(bytes, 0, b"name")?.to_vec(),
        FontFormat::Woff => {
            let n = usize::from(be16(bytes, 12)?);
            let rec = (0..n)
                .map(|i| 44 + i * 20)
                .find(|&rec| bytes.get(rec..rec + 4) == Some(b"name"))?;
            woff_table_data(bytes, rec, 1024 * 1024).ok()?
        }
        FontFormat::Woff2 => {
            let sfnt = decode_font_bytes(bytes).ok()?;
            sfnt_table(&sfnt, 0, b"name")?.to_vec()
        }
    };
    let NameInfo { family, subfamily } = parse_name_table(&table);
    let family = family?;
    let sub = match subfamily {
        Some(s)
            if !matches!(
                s.to_lowercase().as_str(),
                "regular" | "normal" | "book" | "roman"
            ) && !family.to_lowercase().ends_with(&s.to_lowercase()) =>
        {
            format!(" {s}")
        }
        _ => String::new(),
    };
    let name = clean_name(&format!("{family}{sub}"));
    (!name.is_empty()).then_some(name)
}

/// What an uploaded font file is (`UserFontInfo`, minus storage fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFileInfo {
    pub format: FontFormat,
    /// Display name: from the name table, else the file name.
    pub family: String,
}

/// Validates an uploaded font file and names it (`FontStore.add` minus storage): signature and
/// size, then the file must decode and parse, then the display name.
pub fn inspect_font_file(bytes: &[u8], file_name: &str) -> Result<FontFileInfo, FontError> {
    let format = check_font_file(bytes, bytes.len())?;
    let sfnt = decode_font_bytes(bytes)?;
    rustybuzz::ttf_parser::Face::parse(&sfnt, 0)
        .map_err(|e| FontError::Unreadable(e.to_string()))?;
    let family = font_display_name(bytes, format).unwrap_or_else(|| name_from_file_name(file_name));
    Ok(FontFileInfo { format, family })
}

// ------------------------------------------------------------------------------------------
// Faces
// ------------------------------------------------------------------------------------------

struct FaceData {
    /// Identifies the face for caches ("bundled:fira-sans:600", "user:<ref>", "local:<ps>").
    key: String,
    bytes: Arc<[u8]>,
    index: u32,
    /// `wght` axis value applied when shaping and outlining (variable fonts only).
    wght: Option<f32>,
    /// The weight drawn: the one asked for, else the font's own weight class.
    weight: u16,
    metrics: OnceLock<FaceMetrics>,
}

/// A loaded face ready for shaping and outlining: decoded sfnt bytes, the face index, the `wght`
/// variation and synthesised italic. Cheap to clone; `Send + Sync`.
#[derive(Clone)]
pub struct FontFace {
    data: Arc<FaceData>,
    italic: bool,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontFace")
            .field("key", &self.data.key)
            .field("wght", &self.data.wght)
            .field("italic", &self.italic)
            .finish()
    }
}

impl FontFace {
    /// A face from sfnt bytes (already decoded). `weight` sets the `wght` axis when the face has
    /// one (clamped to its range).
    pub fn from_sfnt(
        key: impl Into<String>,
        bytes: Arc<[u8]>,
        index: u32,
        weight: Option<u16>,
    ) -> Result<Self, FontError> {
        let face = rustybuzz::ttf_parser::Face::parse(&bytes, index)
            .map_err(|e| FontError::Unreadable(e.to_string()))?;
        let wght = weight.and_then(|w| {
            face.variation_axes()
                .into_iter()
                .find(|a| a.tag == Tag::from_bytes(b"wght"))
                .map(|a| f32::from(w).clamp(a.min_value, a.max_value))
        });
        let weight = weight.unwrap_or_else(|| face.weight().to_number());
        Ok(Self {
            data: Arc::new(FaceData {
                key: key.into(),
                bytes,
                index,
                wght,
                weight,
                metrics: OnceLock::new(),
            }),
            italic: false,
        })
    }

    /// The same face with synthesised italic on or off (a horizontal skew, as browsers do).
    pub fn with_italic(&self, italic: bool) -> Self {
        Self {
            data: Arc::clone(&self.data),
            italic,
        }
    }

    pub fn italic(&self) -> bool {
        self.italic
    }

    pub fn key(&self) -> &str {
        &self.data.key
    }

    /// The `wght` axis value applied, if the face is variable.
    pub fn wght(&self) -> Option<f32> {
        self.data.wght
    }

    /// Runs `f` with a shaping face (variations applied). `None` if the bytes no longer parse.
    pub fn with_shaper<R>(&self, f: impl FnOnce(&rustybuzz::Face<'_>) -> R) -> Option<R> {
        let mut face = rustybuzz::Face::from_slice(&self.data.bytes, self.data.index)?;
        if let Some(w) = self.data.wght {
            face.set_variations(&[rustybuzz::Variation {
                tag: Tag::from_bytes(b"wght"),
                value: w,
            }]);
        }
        Some(f(&face))
    }

    /// Per-face cache of [`crate::render::text::face_metrics`].
    pub(crate) fn metrics_cell(&self) -> &OnceLock<FaceMetrics> {
        &self.data.metrics
    }
}

/// Decoded bytes of every bundled file used so far.
fn bundled_bytes(file: &'static str) -> Result<Arc<[u8]>, FontError> {
    type Cache = Mutex<HashMap<&'static str, Arc<[u8]>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(b) = cache.lock().map_err(poisoned)?.get(file) {
        return Ok(Arc::clone(b));
    }
    // Decode outside the lock: another thread may decode the same file, which is harmless.
    let woff2 = embedded_font(file).ok_or_else(|| FontError::NoBundledFile(file.into()))?;
    let ttf: Arc<[u8]> = decode_font_bytes(woff2)?.into();
    let mut map = cache.lock().map_err(poisoned)?;
    Ok(Arc::clone(map.entry(file).or_insert(ttf)))
}

fn poisoned<T>(_: T) -> FontError {
    FontError::Unreadable("font cache lock poisoned".into())
}

/// The bundled face of `def` at `weight` resolved to a file (`loadFace`), cached per
/// (family, weight). Upright; use [`FontFace::with_italic`] for italic.
pub fn bundled_face(def: &'static FontDef, weight: u16) -> Result<FontFace, FontError> {
    static FACES: OnceLock<Mutex<HashMap<(&'static str, u16), FontFace>>> = OnceLock::new();
    let weight = resolve_weight(def, weight);
    let faces = FACES.get_or_init(Default::default);
    if let Some(f) = faces.lock().map_err(poisoned)?.get(&(def.id, weight)) {
        return Ok(f.clone());
    }
    let file = def
        .file(weight)
        .ok_or_else(|| FontError::NoBundledFile(format!("{} {weight}", def.label)))?;
    let bytes = bundled_bytes(file)?;
    let face = FontFace::from_sfnt(
        format!("bundled:{}:{weight}", def.id),
        bytes,
        0,
        Some(weight),
    )?;
    let mut map = faces.lock().map_err(poisoned)?;
    Ok(map.entry((def.id, weight)).or_insert(face).clone())
}

// ------------------------------------------------------------------------------------------
// System fonts ("local" custom fonts)
// ------------------------------------------------------------------------------------------

/// One face of an installed font file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemFont {
    pub path: PathBuf,
    /// Face index inside a collection (0 for single fonts).
    pub index: u32,
    /// Typographic family when present, e.g. `Helvetica Neue`.
    pub family: String,
    pub subfamily: String,
    /// Full name, e.g. `Helvetica Neue Bold`.
    pub full_name: String,
    /// PostScript name (`FontSource::Local.postscript_name`), may be empty.
    pub postscript_name: String,
    /// OS/2 weight class (400 when the table is missing).
    pub weight: u16,
    /// OS/2 italic bit.
    pub italic: bool,
    /// Has a `wght` variation axis (any weight can be drawn from this face).
    pub variable: bool,
}

/// Platform font directories (existing or not).
pub fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = dirs::home_dir();
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(h) = &home {
            dirs.push(h.join("Library/Fonts"));
        }
    } else if cfg!(windows) {
        let windir =
            std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        dirs.push(windir.join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(data) = dirs::data_dir() {
            dirs.push(data.join("fonts"));
        }
        if let Some(h) = &home {
            dirs.push(h.join(".fonts"));
        }
    }
    dirs
}

/// Every face found in [`system_font_dirs`], sorted by full name. Reads only the `name` tables.
pub fn scan_system_fonts() -> Vec<SystemFont> {
    let mut files = Vec::new();
    for dir in system_font_dirs() {
        collect_font_files(&dir, 0, &mut files);
    }
    files.sort();
    files.dedup();
    let mut faces: Vec<SystemFont> = files
        .iter()
        .flat_map(|p| read_system_faces(p).unwrap_or_default())
        .collect();
    faces.sort_by(|a, b| a.full_name.cmp(&b.full_name).then(a.path.cmp(&b.path)));
    faces
}

fn collect_font_files(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            collect_font_files(&path, depth + 1, out);
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        }) {
            out.push(path);
        }
    }
}

fn read_at(f: &mut File, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// The names of every face in a font file or collection (positioned reads only).
pub fn read_system_faces(path: &Path) -> std::io::Result<Vec<SystemFont>> {
    let mut f = File::open(path)?;
    let head = read_at(&mut f, 0, 12)?;
    let offsets: Vec<u32> = if head.get(..4) == Some(b"ttcf") {
        let n = be32(&head, 8).unwrap_or(0).min(256) as usize;
        let table = read_at(&mut f, 12, n * 4)?;
        (0..n).filter_map(|i| be32(&table, i * 4)).collect()
    } else {
        vec![0]
    };
    let mut faces = Vec::new();
    for (index, off) in (0u32..).zip(offsets) {
        let Ok(Some(FaceInfo { names, weight, italic, variable })) = read_face_info(&mut f, u64::from(off)) else {
            continue;
        };
        let typo = names.contains_key(&16);
        let get = |id: u16| names.get(&id).cloned().unwrap_or_default();
        let family = if typo { get(16) } else { get(1) };
        if family.is_empty() {
            continue;
        }
        let subfamily = match (typo, names.get(&17)) {
            (true, Some(s)) => s.clone(),
            _ => get(2),
        };
        let full = get(4);
        let full_name = if full.is_empty() {
            format!("{family} {subfamily}").trim().to_owned()
        } else {
            full
        };
        faces.push(SystemFont {
            path: path.to_path_buf(),
            index,
            family,
            subfamily,
            full_name,
            postscript_name: get(6),
            weight,
            italic,
            variable,
        });
    }
    Ok(faces)
}

/// What the scan reads from one face: names, weight class, italic bit, variable weight axis.
struct FaceInfo {
    names: HashMap<u16, String>,
    weight: u16,
    italic: bool,
    variable: bool,
}

fn read_face_info(f: &mut File, off: u64) -> std::io::Result<Option<FaceInfo>> {
    let dir = read_at(f, off, 12)?;
    let num_tables = usize::from(be16(&dir, 4).unwrap_or(0));
    let records = read_at(f, off + 12, num_tables * 16)?;
    let table = |tag: &[u8; 4]| {
        (0..num_tables).find_map(|i| {
            let r = records.get(i * 16..i * 16 + 16)?;
            (r.get(..4)? == tag).then(|| Some((be32(r, 8)?, be32(r, 12)?)))?
        })
    };
    let Some((name_off, name_len)) = table(b"name") else {
        return Ok(None);
    };
    let name = read_at(f, u64::from(name_off), name_len.min(1 << 20) as usize)?;
    let names = best_names(&name, &[1, 2, 4, 6, 16, 17]);
    let (mut weight, mut italic) = (400, false);
    if let Some((os2_off, os2_len)) = table(b"OS/2")
        && os2_len >= 64
    {
        let os2 = read_at(f, u64::from(os2_off), 64)?;
        weight = be16(&os2, 4).filter(|w| (1..=1000).contains(w)).unwrap_or(400);
        italic = be16(&os2, 62).is_some_and(|sel| sel & 1 != 0);
    }
    let mut variable = false;
    if let Some((fvar_off, fvar_len)) = table(b"fvar") {
        let fvar = read_at(f, u64::from(fvar_off), fvar_len.min(4096) as usize)?;
        let axes_off = usize::from(be16(&fvar, 4).unwrap_or(16));
        let count = usize::from(be16(&fvar, 8).unwrap_or(0));
        let size = usize::from(be16(&fvar, 10).unwrap_or(20));
        variable = (0..count).any(|i| fvar.get(axes_off + i * size..axes_off + i * size + 4) == Some(b"wght"));
    }
    Ok(Some(FaceInfo { names, weight, italic, variable }))
}

/// An installed font family for the picker: one entry per family, whose weights the Weight
/// control can choose between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFamily {
    /// Typographic family, e.g. "Vazirmatn".
    pub family: String,
    /// The face stored in documents (`FontSource::Local.postscript_name`): the upright face
    /// nearest regular, or the variable face.
    pub postscript_name: String,
    /// Upright weights the family has (every weight from 100 to 900 for a variable family).
    pub weights: Vec<u16>,
    pub variable: bool,
}

/// Groups installed faces into families (`to_local_fonts`, by family instead of by face).
/// Hidden system families (a leading dot) are left out.
pub fn to_local_families(list: &[SystemFont]) -> Vec<LocalFamily> {
    let mut by_family: HashMap<&str, Vec<&SystemFont>> = HashMap::new();
    for f in list {
        if f.family.starts_with('.') || !valid_postscript_name(&f.postscript_name) {
            continue;
        }
        by_family.entry(f.family.as_str()).or_default().push(f);
    }
    let mut out: Vec<LocalFamily> = by_family
        .into_iter()
        .filter_map(|(family, faces)| {
            let upright: Vec<&&SystemFont> = faces.iter().filter(|f| !f.italic).collect();
            let pool: Vec<&&SystemFont> = if upright.is_empty() { faces.iter().collect() } else { upright };
            let variable = pool.iter().find(|f| f.variable);
            let rep = variable.copied().or_else(|| pool.iter().copied().min_by_key(|f| (f.weight.abs_diff(400), f.weight)))?;
            let mut weights: Vec<u16> = if variable.is_some() {
                (1..=9).map(|w| w * 100).collect()
            } else {
                pool.iter().map(|f| f.weight).collect()
            };
            weights.sort_unstable();
            weights.dedup();
            Some(LocalFamily {
                family: family.chars().take(FONT_NAME_CHARS).collect(),
                postscript_name: rep.postscript_name.clone(),
                weights,
                variable: variable.is_some(),
            })
        })
        .collect();
    out.sort_by_key(|f| f.family.to_lowercase());
    out
}

/// An installed font the user can pick (`LocalFontInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFontInfo {
    /// `FontSource::Local.postscript_name`.
    pub postscript_name: String,
    /// Display name (the full name, e.g. "Helvetica Neue Bold").
    pub name: String,
}

/// `PS_RE` plus the quote check: printable ASCII, 1–100 characters, no quotes or backslashes.
pub fn valid_postscript_name(ps: &str) -> bool {
    (1..=100).contains(&ps.len())
        && ps.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && !ps.contains(['"', '\'', '\\'])
}

/// Usable entries, deduplicated by PostScript name, sorted by display name (`toLocalFonts`).
pub fn to_local_fonts(list: &[SystemFont]) -> Vec<LocalFontInfo> {
    let mut seen: HashMap<&str, LocalFontInfo> = HashMap::new();
    for f in list {
        let ps = f.postscript_name.as_str();
        if !valid_postscript_name(ps) || seen.contains_key(ps) {
            continue;
        }
        let full = if f.full_name.trim().is_empty() {
            ps
        } else {
            f.full_name.as_str()
        };
        let name: String = full
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(FONT_NAME_CHARS)
            .collect();
        seen.insert(
            ps,
            LocalFontInfo {
                postscript_name: ps.to_owned(),
                name,
            },
        );
    }
    let mut out: Vec<LocalFontInfo> = seen.into_values().collect();
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

// ------------------------------------------------------------------------------------------
// Registry (fonts.ts: ensureFonts, custom fonts, activeCustomFamily)
// ------------------------------------------------------------------------------------------

/// Loads an uploaded font's bytes by content ref (`loadFontBlob`, `FontStore.get`).
pub type FontBlobLoader = dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync;

/// A font problem the renderer reports as a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontIssue {
    /// A bundled face could not be loaded ("Family weight"); Fira Sans is drawn instead.
    Fallback(String),
    /// A custom font is not available (its display name); the bundled family is drawn instead.
    Missing(String),
}

impl FontIssue {
    /// The `RenderWarning.code`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Fallback(_) => "font-fallback",
            Self::Missing(_) => "font-missing",
        }
    }

    /// The user-facing message (renderer.ts).
    pub fn message(&self) -> String {
        match self {
            Self::Fallback(name) => format!(
                "The font “{name}” could not be loaded; a system font is used instead. Reload the page while online to fix this."
            ),
            Self::Missing(name) => format!(
                "The font “{name}” is not available on this device, so the label uses its built-in font instead. Add the font under Fonts to print it as designed."
            ),
        }
    }
}

/// Fonts a document could not get (`FontReport`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontReport {
    /// "Family weight" strings that fell back.
    pub fallbacks: Vec<String>,
    /// Custom fonts not available (display names, deduplicated).
    pub missing: Vec<String>,
}

impl FontReport {
    /// The warnings in the renderer's order: fallbacks, then missing custom fonts.
    pub fn issues(&self) -> Vec<FontIssue> {
        self.fallbacks
            .iter()
            .cloned()
            .map(FontIssue::Fallback)
            .chain(self.missing.iter().cloned().map(FontIssue::Missing))
            .collect()
    }
}

/// The face a text item draws with, and how it was chosen.
#[derive(Debug, Clone)]
pub struct ResolvedFont {
    /// The face to measure and draw with (italic set from the item).
    pub face: FontFace,
    /// The item's bundled family (also when a custom font is drawn).
    pub def: &'static FontDef,
    /// `resolveWeight(def, item.fontWeight)`.
    pub weight: u16,
    /// A custom font is drawn (`activeCustomFamily` is set): no pixel grid, no quality warning.
    pub custom: bool,
    /// What went wrong on the way, if anything.
    pub issue: Option<FontIssue>,
}

impl ResolvedFont {
    /// The design grid of a pixel font, when its bundled face is drawn (`pixelGrid`).
    pub fn pixel_grid(&self) -> Option<PixelGrid> {
        if self.custom { None } else { self.def.pixel }
    }
}

/// Every (family id, resolved weight) a document draws with, deduplicated (`fontsUsed`).
pub fn fonts_used(doc: &LabelDoc) -> Vec<(&'static str, u16)> {
    let mut out: Vec<(&'static str, u16)> = Vec::new();
    let mut add = |id: &str, weight: u16| {
        let def = font_def(id);
        let w = resolve_weight(def, weight);
        if !out.contains(&(def.id, w)) {
            out.push((def.id, w));
        }
    };
    for it in &doc.items {
        match it {
            Item::Text(t) if !t.text.trim().is_empty() => add(&t.font_family, t.font_weight),
            Item::Code(c) if c.show_text && c.symbology != Symbology::Qr => {
                add(CODE_TEXT_FONT.0, CODE_TEXT_FONT.1);
            }
            _ => {}
        }
    }
    out
}

/// Every distinct custom font a document's non-empty text items use (`customFontsUsed`).
pub fn custom_fonts_used(doc: &LabelDoc) -> Vec<FontSource> {
    let mut out: Vec<FontSource> = Vec::new();
    for it in &doc.items {
        if let Item::Text(t) = it
            && let Some(src) = &t.custom_font
            && !t.text.trim().is_empty()
            && !out.iter().any(|s| custom_key(s) == custom_key(src))
        {
            out.push(src.clone());
        }
    }
    out
}

fn custom_key(src: &FontSource) -> String {
    match src {
        FontSource::User { reference, .. } => format!("user:{reference}"),
        FontSource::Local {
            postscript_name, ..
        } => format!("local:{postscript_name}"),
    }
}

fn custom_display_name(src: &FontSource) -> &str {
    match src {
        FontSource::User { family, .. } | FontSource::Local { family, .. } => family,
    }
}

/// Resolves text items to faces: bundled families, uploaded fonts (registered from bytes or
/// fetched with a loader) and installed fonts. Share it across threads (`Arc<FontRegistry>`).
#[derive(Default)]
pub struct FontRegistry {
    /// Loaded custom faces by key (`customFaces`); `None` = tried and failed (`customFailed`).
    custom: RwLock<HashMap<String, Option<FontFace>>>,
    loader: RwLock<Option<Arc<FontBlobLoader>>>,
    system: OnceLock<Vec<SystemFont>>,
    families: OnceLock<Vec<LocalFamily>>,
}

impl std::fmt::Debug for FontRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontRegistry").finish_non_exhaustive()
    }
}

impl FontRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the loader for uploaded fonts (`RenderOptions.loadFontBlob`). Fonts that failed to
    /// load are retried with it.
    pub fn set_user_font_loader(&self, loader: Arc<FontBlobLoader>) {
        if let Ok(mut l) = self.loader.write() {
            *l = Some(loader);
        }
        if let Ok(mut c) = self.custom.write() {
            c.retain(|_, v| v.is_some());
        }
    }

    /// Registers an uploaded font from its file bytes under its content ref. Returns its display
    /// name (name table, else "Uploaded font").
    pub fn add_user_font(&self, reference: &str, bytes: &[u8]) -> Result<String, FontError> {
        let info = inspect_font_file(bytes, "")?;
        let face = load_user_face(reference, bytes)?;
        let mut c = self.custom.write().map_err(poisoned)?;
        c.insert(format!("user:{reference}"), Some(face));
        Ok(info.family)
    }

    /// Unregisters an uploaded font (`forgetCustomFont`): labels using it fall back from now on.
    pub fn forget_custom_font(&self, src: &FontSource) {
        let key = custom_key(src);
        let weighted = format!("{key}#");
        if let Ok(mut c) = self.custom.write() {
            c.retain(|k, _| *k != key && !k.starts_with(&weighted));
        }
    }

    /// `true` once `src` is loaded (`customFontReady`).
    pub fn custom_font_ready(&self, src: &FontSource) -> bool {
        self.custom
            .read()
            .is_ok_and(|c| matches!(c.get(&custom_key(src)), Some(Some(_))))
    }

    /// `true` once loading `src` was tried and failed (`customFontMissing`).
    pub fn custom_font_missing(&self, src: &FontSource) -> bool {
        self.custom
            .read()
            .is_ok_and(|c| matches!(c.get(&custom_key(src)), Some(None)))
    }

    /// Installed fonts (scanned once per registry).
    pub fn system_fonts(&self) -> &[SystemFont] {
        self.system.get_or_init(scan_system_fonts)
    }

    /// Installed fonts the user can pick, by PostScript name (`localFonts.query`).
    pub fn local_fonts(&self) -> Vec<LocalFontInfo> {
        to_local_fonts(self.system_fonts())
    }

    /// Installed fonts grouped by family, for the font picker (cached).
    pub fn local_families(&self) -> &[LocalFamily] {
        self.families.get_or_init(|| to_local_families(self.system_fonts()))
    }

    /// The installed family `postscript_name` belongs to.
    pub fn local_family_of(&self, postscript_name: &str) -> Option<&LocalFamily> {
        let family = &self
            .system_fonts()
            .iter()
            .find(|f| f.postscript_name == postscript_name)?
            .family;
        self.local_families().iter().find(|f| &f.family == family)
    }

    /// The PostScript name of the upright face of `postscript_name`'s family nearest `weight`
    /// (ties go lighter). A variable face covers every weight, so it is kept as is.
    fn local_face_for_weight(&self, postscript_name: &str, weight: u16) -> Option<String> {
        let named = || self.system_fonts().iter().filter(|f| f.postscript_name == postscript_name);
        if named().any(|f| f.variable) {
            return None;
        }
        let current = named().next()?;
        self.system_fonts()
            .iter()
            .filter(|f| f.family == current.family && !f.italic && valid_postscript_name(&f.postscript_name))
            .min_by_key(|f| (f.weight.abs_diff(weight), f.weight))
            .map(|f| f.postscript_name.clone())
    }

    /// Loads a custom font once (`loadCustomFont`); `None` when it is not available.
    pub fn load_custom_font(&self, src: &FontSource) -> Option<FontFace> {
        let key = custom_key(src);
        if let Some(hit) = self.custom.read().ok()?.get(&key) {
            return hit.clone();
        }
        let face = match src {
            FontSource::User { reference, .. } => {
                let loader = self.loader.read().ok()?.clone();
                loader
                    .and_then(|l| l(reference))
                    .and_then(|bytes| load_user_face(reference, &bytes).ok())
            }
            FontSource::Local {
                postscript_name, ..
            } => self.load_local_face(postscript_name).ok(),
        };
        let mut c = self.custom.write().ok()?;
        c.insert(key, face.clone());
        face
    }

    fn load_local_face(&self, ps: &str) -> Result<FontFace, FontError> {
        if !valid_postscript_name(ps) {
            return Err(FontError::NotInstalled(ps.into()));
        }
        // A variable file and a static one can share a PostScript name: prefer the variable
        // face, which draws every weight.
        let sys = self
            .system_fonts()
            .iter()
            .filter(|f| f.postscript_name == ps)
            .max_by_key(|f| f.variable)
            .ok_or_else(|| FontError::NotInstalled(ps.into()))?;
        let bytes = std::fs::read(&sys.path).map_err(|e| FontError::Unreadable(e.to_string()))?;
        if bytes.len() > MAX_DECODED_BYTES {
            return Err(FontError::Unreadable("font file too large".into()));
        }
        FontFace::from_sfnt(format!("local:{ps}"), bytes.into(), sys.index, None)
    }

    /// The face for a family/weight/italic and optional custom font, falling back as the studio
    /// does: a custom font that cannot load → the bundled family (`Missing`); a bundled face that
    /// cannot load → Fira Sans 400 (`Fallback`). Errors only if no bundled face loads at all.
    pub fn resolve(
        &self,
        family: &str,
        weight: u16,
        italic: bool,
        custom: Option<&FontSource>,
    ) -> Result<ResolvedFont, FontError> {
        let def = font_def(family);
        let mut issue = None;
        if let Some(src) = custom {
            // A custom font gets the weight as set: it has its own weights, not the bundled
            // family's.
            let custom_weight = weight.clamp(100, 900);
            if let Some(face) = self.load_custom_face_weighted(src, custom_weight) {
                return Ok(ResolvedFont {
                    face: face.with_italic(italic),
                    def,
                    weight: custom_weight,
                    custom: true,
                    issue: None,
                });
            }
            issue = Some(FontIssue::Missing(custom_display_name(src).to_owned()));
        }
        let weight = resolve_weight(def, weight);
        let face = match bundled_face(def, weight) {
            Ok(f) => f,
            Err(_) => {
                issue = Some(FontIssue::Fallback(format!("{} {weight}", def.label)));
                bundled_face(&FONTS[0], 400)?
            }
        };
        Ok(ResolvedFont {
            face: face.with_italic(italic),
            def,
            weight,
            custom: false,
            issue,
        })
    }

    /// [`Self::resolve`] for a text item (`itemFont`).
    pub fn resolve_item(&self, item: &TextItem) -> Result<ResolvedFont, FontError> {
        self.resolve(
            &item.font_family,
            item.font_weight,
            item.italic,
            item.custom_font.as_ref(),
        )
    }

    /// A custom face with its `wght` axis (if any) set like the browser's: the requested weight
    /// resolved against the bundled family (`cssFont` keeps the bundled weight). Cached per weight.
    fn load_custom_face_weighted(&self, src: &FontSource, weight: u16) -> Option<FontFace> {
        // An installed family with separate weight files: use the face nearest the weight.
        if let FontSource::Local { postscript_name, family } = src
            && let Some(ps) = self.local_face_for_weight(postscript_name, weight)
            && ps != *postscript_name
        {
            let nearest = FontSource::Local { postscript_name: ps, family: family.clone() };
            return self.load_custom_font(&nearest);
        }
        let face = self.load_custom_font(src)?;
        let variable = face
            .with_shaper(|hb| {
                hb.variation_axes()
                    .into_iter()
                    .any(|a| a.tag == Tag::from_bytes(b"wght"))
            })
            .unwrap_or(false);
        if !variable {
            return Some(face);
        }
        let key = format!("{}#{weight}", custom_key(src));
        if let Some(Some(hit)) = self.custom.read().ok()?.get(&key) {
            return Some(hit.clone());
        }
        let weighted = FontFace::from_sfnt(
            face.key().to_owned(),
            Arc::clone(&face.data.bytes),
            face.data.index,
            Some(weight),
        )
        .ok()?;
        self.custom
            .write()
            .ok()?
            .insert(key, Some(weighted.clone()));
        Some(weighted)
    }

    /// Loads every family/weight and custom font `doc` uses and reports what is not available
    /// (`ensureFonts`).
    pub fn ensure_fonts(&self, doc: &LabelDoc) -> FontReport {
        let mut report = FontReport::default();
        for (id, w) in fonts_used(doc) {
            let def = font_def(id);
            if bundled_face(def, w).is_err() {
                report.fallbacks.push(format!("{} {w}", def.label));
            }
        }
        for src in custom_fonts_used(doc) {
            if self.load_custom_font(&src).is_none() {
                let name = custom_display_name(&src).to_owned();
                if !report.missing.contains(&name) {
                    report.missing.push(name);
                }
            }
        }
        report
    }
}

fn load_user_face(reference: &str, bytes: &[u8]) -> Result<FontFace, FontError> {
    if bytes.len() > MAX_FONT_BYTES {
        return Err(FontError::TooLarge { size: bytes.len() });
    }
    let sfnt = decode_font_bytes(bytes)?;
    FontFace::from_sfnt(format!("user:{reference}"), sfnt.into(), 0, None)
}

// ------------------------------------------------------------------------------------------
// Per-character fallback (what Chrome does when a font lacks a character)
// ------------------------------------------------------------------------------------------

/// Installed families tried first when the chosen font lacks a character, in order. Others are
/// scanned only when none of these covers it.
const FALLBACK_FAMILIES: &[&str] = &[
    "Vazirmatn",
    "SF Arabic",
    "Geeza Pro",
    "Arial Unicode MS",
    "Arial Hebrew",
    "PingFang SC",
    "Hiragino Sans",
    "Apple SD Gothic Neo",
    "Kohinoor Devanagari",
    "Thonburi",
    "Apple Symbols",
    "Noto Sans",
    "DejaVu Sans",
];

/// Families never used as fallback (bitmap emoji, the "last resort" boxes).
const FALLBACK_SKIP: &[&str] = &["Apple Color Emoji", "LastResort", ".LastResort"];

struct Fallback {
    /// Candidate faces, preferred families first, regular before bold within a family.
    candidates: Vec<SystemFont>,
    files: Mutex<HashMap<PathBuf, Option<Arc<[u8]>>>>,
    /// Which candidate draws a character at a weight, once looked up.
    by_char: Mutex<HashMap<(char, u16), Option<usize>>>,
}

fn fallback() -> &'static Fallback {
    static FALLBACK: OnceLock<Fallback> = OnceLock::new();
    FALLBACK.get_or_init(|| {
        let mut fonts: Vec<SystemFont> = scan_system_fonts()
            .into_iter()
            .filter(|f| !FALLBACK_SKIP.iter().any(|s| f.family.eq_ignore_ascii_case(s)))
            .collect();
        fonts.sort_by_key(|f| (fallback_rank(f), f.family.clone(), is_bold_name(&f.subfamily)));
        Fallback {
            candidates: fonts,
            files: Mutex::new(HashMap::new()),
            by_char: Mutex::new(HashMap::new()),
        }
    })
}

/// Position in [`FALLBACK_FAMILIES`]. Matches the family name without Apple's leading dot for
/// hidden system faces (".SF Arabic"), or the file name, since some fonts report a localised
/// family name (Geeza Pro's is Persian).
fn fallback_rank(f: &SystemFont) -> usize {
    let squash = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let family = squash(f.family.trim_start_matches('.'));
    let stem = f
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(squash)
        .unwrap_or_default();
    FALLBACK_FAMILIES
        .iter()
        .position(|p| {
            let p = squash(p);
            family == p || stem == p
        })
        .unwrap_or(FALLBACK_FAMILIES.len())
}

fn is_bold_name(subfamily: &str) -> bool {
    let s = subfamily.to_ascii_lowercase();
    s.contains("bold") || s.contains("black") || s.contains("heavy")
}

impl Fallback {
    fn face(&self, ix: usize, weight: u16) -> Option<FontFace> {
        let font = self.candidates.get(ix)?;
        let bytes = {
            let mut files = self.files.lock().ok()?;
            files
                .entry(font.path.clone())
                .or_insert_with(|| std::fs::read(&font.path).ok().map(Arc::from))
                .clone()?
        };
        FontFace::from_sfnt(
            format!("fallback:{}:{}", font.path.display(), font.index),
            bytes,
            font.index,
            Some(weight),
        )
        .ok()
    }

    fn lookup(&self, c: char, weight: u16) -> Option<FontFace> {
        if let Some(hit) = self.by_char.lock().ok()?.get(&(c, weight)).copied() {
            return hit.and_then(|ix| self.face(ix, weight));
        }
        // Preferred families first; within a family, the upright face nearest the weight.
        // (Variable faces such as SF Arabic take the weight through their `wght` axis.)
        let mut order: Vec<usize> = (0..self.candidates.len()).collect();
        order.sort_by_key(|&ix| {
            let f = &self.candidates[ix];
            let distance = if f.variable { 0 } else { f.weight.abs_diff(weight) };
            (fallback_rank(f), f.family.clone(), f.italic, distance)
        });
        let found = order
            .into_iter()
            .find(|&ix| self.face(ix, weight).is_some_and(|f| f.has_char(c)));
        if let Ok(mut map) = self.by_char.lock() {
            map.insert((c, weight), found);
        }
        found.and_then(|ix| self.face(ix, weight))
    }
}

/// An installed face that draws `c`, for text in a font that lacks it (Chrome's per-character
/// fallback). `None` when no installed font has it.
pub fn fallback_face(c: char, weight: u16) -> Option<FontFace> {
    fallback().lookup(c, weight)
}

impl FontFace {
    /// Whether the face has a glyph for `c`.
    pub fn has_char(&self, c: char) -> bool {
        self.with_shaper(|hb| hb.glyph_index(c).is_some()).unwrap_or(false)
    }

    /// Whether two faces draw from the same font data and weight.
    pub fn same_source(&self, other: &FontFace) -> bool {
        Arc::ptr_eq(&self.data, &other.data) || (self.data.key == other.data.key && self.data.wght == other.data.wght)
    }

    /// The weight the face draws (asked-for weight, else its weight class).
    pub fn weight_or_regular(&self) -> u16 {
        self.data.weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_resolution() {
        let fira = font_def("fira-sans");
        assert_eq!(resolve_weight(fira, 650), 600);
        assert_eq!(resolve_weight(fira, 900), 800);
        let bc = font_def("barlow-condensed");
        assert_eq!(resolve_weight(bc, 400), 500);
        assert_eq!(resolve_weight(bc, 600), 500); // tie → lighter
        assert_eq!(font_def("nope").id, "fira-sans");
    }

    #[test]
    fn woff_header_maths() {
        assert_eq!(
            sniff_font_format(b"wOF2\0\0\0\0\0\0\0\0"),
            Some(FontFormat::Woff2)
        );
        assert_eq!(sniff_font_format(b"wOF2"), None);
    }
}

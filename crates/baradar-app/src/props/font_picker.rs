//! The font picker (port of `ui/fonts/FontPicker.svelte`, `FontPickerPanel.svelte`,
//! `font-list.ts` and `font-choice.ts`).
//!
//! A dialog "Choose a font" with a search field, style filter chips, grouped rows (Favourites,
//! Recent, each category, Your fonts, This computer), a live preview of the block's first line in
//! every font, quality tags, stars, and the footer (Favourite, Use for new text, Fonts on this
//! computer…, Manage fonts…).
//!
//! Previews draw with the real faces: the bundled families are decoded once in the background and
//! registered with GPUI's text system ([`PreviewFonts`]); uploaded fonts are registered when the
//! picker lists them, installed fonts are drawn by their family name.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use baradar_core::doc::{FontSource, Item, TextItem};
use baradar_core::render::fonts::{
    FONT_CATEGORIES, FONTS, FontCategory, FontDef, FontQuality, GenericFamily, LocalFamily, MIN_QUALITY_CAP_MM,
    bundled_sfnt, decode_font_bytes, font_def, parse_name_table, resolve_weight,
};
use baradar_core::store::{FontKey, Prefs};

/// Most favourite fonts kept (`MAX_FAVORITE_FONTS` in `persist-prefs.ts`).
pub const MAX_FAVORITE_FONTS: usize = 50;
/// Most recent fonts kept (`MAX_RECENT_FONTS`).
pub const MAX_RECENT_FONTS: usize = 8;
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::studio::Studio;

// ------------------------------------------------------------------------------------------
// List model (font-list.ts)
// ------------------------------------------------------------------------------------------

/// Where a picker entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontEntryKind {
    Bundled,
    User,
    Local,
}

/// One font the picker can list (`FontEntry`).
#[derive(Debug, Clone, PartialEq)]
pub struct FontEntry {
    pub key: FontKey,
    pub kind: FontEntryKind,
    /// Display name.
    pub label: String,
    pub def: Option<&'static FontDef>,
    /// What choosing it stores in `TextItem.customFont` (uploaded / local fonts).
    pub source: Option<FontSource>,
    /// A custom font the block uses that is not in the lists.
    pub missing: bool,
    /// Folded label.
    pub name: String,
    /// Folded search text (category, hint, flags, kind).
    pub words: String,
}

/// A filter chip: a category or "Your fonts".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFilter {
    Category(FontCategory),
    Yours,
}

/// A listbox group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupId {
    Favorites,
    Recent,
    Results,
    Category(FontCategory),
    User,
    Local,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FontGroup {
    pub id: GroupId,
    pub label: &'static str,
    pub entries: Vec<FontEntry>,
}

/// `GROUP_LABELS`.
pub const LABEL_FAVORITES: &str = "Favourites";
pub const LABEL_RECENT: &str = "Recent";
pub const LABEL_RESULTS: &str = "Search results";
pub const LABEL_USER: &str = "Your fonts";
pub const LABEL_LOCAL: &str = "This computer (varies by machine)";

/// Rows a Page Up / Page Down moves (`PAGE_ROWS`).
pub const PAGE_ROWS: usize = 8;

/// Lower case, common accents removed, runs of spaces and punctuation collapsed (`fold`).
/// (The studio uses Unicode NFD; this covers Latin-1 and Latin Extended-A, which is what font
/// names use.)
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = strip_accent(c);
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        } else {
            space = true;
        }
    }
    out
}

fn strip_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        c => c,
    }
}

fn category_label(c: FontCategory) -> &'static str {
    FONT_CATEGORIES.iter().find(|(x, _)| *x == c).map_or("", |(_, l)| *l)
}

fn bundled_words(def: &FontDef) -> String {
    let flags = [
        if def.condensed { "condensed narrow" } else { "" },
        if def.pixel.is_some() { "pixel retro bitmap" } else { "" },
        if def.quality == Some(FontQuality::Script) { "script" } else { "" },
        if def.generic == GenericFamily::Monospace { "monospaced" } else { "" },
    ];
    fold(&[category_label(def.category), def.hint, flags[0], flags[1], flags[2], flags[3]].join(" "))
}

/// An uploaded font the picker lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFontLike {
    pub reference: String,
    pub family: String,
}

/// Every font the picker can list, in canonical order: bundled fonts by category, then uploaded
/// fonts, then this computer's fonts (`fontEntries`).
pub fn font_entries(user: &[UserFontLike], local: &[LocalFamily]) -> Vec<FontEntry> {
    let mut out = Vec::new();
    for (c, _) in FONT_CATEGORIES {
        for def in FONTS.iter().filter(|d| d.category == c) {
            out.push(FontEntry {
                key: format!("b:{}", def.id),
                kind: FontEntryKind::Bundled,
                label: def.label.to_owned(),
                def: Some(def),
                source: None,
                missing: false,
                name: fold(def.label),
                words: bundled_words(def),
            });
        }
    }
    for f in user {
        out.push(FontEntry {
            key: format!("u:{}", f.reference),
            kind: FontEntryKind::User,
            label: f.family.clone(),
            def: None,
            source: Some(FontSource::User { reference: f.reference.clone(), family: f.family.clone() }),
            missing: false,
            name: fold(&f.family),
            words: fold("your fonts uploaded"),
        });
    }
    for f in local {
        out.push(FontEntry {
            key: format!("l:{}", f.postscript_name),
            kind: FontEntryKind::Local,
            label: f.family.clone(),
            def: None,
            source: Some(FontSource::Local { postscript_name: f.postscript_name.clone(), family: f.family.clone() }),
            missing: false,
            name: fold(&f.family),
            words: fold(&format!("this computer local installed {}", f.postscript_name)),
        });
    }
    out
}

/// The picker key of the font a block draws with (`fontKeyOf`).
pub fn font_key_of(item: &TextItem) -> FontKey {
    match &item.custom_font {
        None => format!("b:{}", item.font_family),
        Some(FontSource::User { reference, .. }) => format!("u:{reference}"),
        Some(FontSource::Local { postscript_name, .. }) => format!("l:{postscript_name}"),
    }
}

/// An entry for the block's own custom font when the lists do not have it (`missingEntry`).
pub fn missing_entry(item: &TextItem, entries: &[FontEntry]) -> Option<FontEntry> {
    let c = item.custom_font.as_ref()?;
    let key = font_key_of(item);
    if entries.iter().any(|e| e.key == key) {
        return None;
    }
    let (kind, family) = match c {
        FontSource::User { family, .. } => (FontEntryKind::User, family),
        FontSource::Local { family, .. } => (FontEntryKind::Local, family),
    };
    Some(FontEntry {
        key,
        kind,
        label: family.clone(),
        def: None,
        source: Some(c.clone()),
        missing: true,
        name: fold(family),
        words: String::new(),
    })
}

/// How well `entry` matches `query` (0 = no match): every query word must match the whole name
/// (100), the start of the name (80), the start of a name word (60), the name (40), the start of
/// a search word (20) or the search text (10) (`matchScore`).
pub fn match_score(entry: &FontEntry, query: &str) -> u32 {
    let q = fold(query);
    let tokens: Vec<&str> = q.split(' ').filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return 1;
    }
    let mut score = 0;
    for t in tokens {
        let s = if entry.name == t {
            100
        } else if entry.name.starts_with(t) {
            80
        } else if entry.name.split(' ').any(|w| w.starts_with(t)) {
            60
        } else if entry.name.contains(t) {
            40
        } else if entry.words.split(' ').any(|w| w.starts_with(t)) {
            20
        } else if entry.words.contains(t) {
            10
        } else {
            0
        };
        if s == 0 {
            return 0;
        }
        score += s;
    }
    score
}

fn passes(entry: &FontEntry, filters: &[FontFilter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    match entry.def {
        Some(def) if entry.kind == FontEntryKind::Bundled => filters.contains(&FontFilter::Category(def.category)),
        _ => filters.contains(&FontFilter::Yours),
    }
}

/// The chips worth showing: categories that have fonts, then "Your fonts" (`availableFilters`).
pub fn available_filters(entries: &[FontEntry]) -> Vec<(FontFilter, &'static str)> {
    let cats: HashSet<FontCategory> = entries.iter().filter_map(|e| e.def.map(|d| d.category)).collect();
    let mut out: Vec<(FontFilter, &'static str)> = FONT_CATEGORIES
        .iter()
        .filter(|(c, _)| cats.contains(c))
        .map(|(c, l)| (FontFilter::Category(*c), *l))
        .collect();
    if entries.iter().any(|e| e.kind != FontEntryKind::Bundled) {
        out.push((FontFilter::Yours, LABEL_USER));
    }
    out
}

/// The listbox groups in display order (`buildGroups`): browsing shows Favourites, Recent, each
/// category, Your fonts and This computer; chips keep only the chosen categories / yours; a
/// search shows one "Search results" group, best match first. Empty groups are left out.
pub fn build_groups(
    entries: &[FontEntry],
    favorites: &[FontKey],
    recent: &[FontKey],
    query: &str,
    filters: &[FontFilter],
) -> Vec<FontGroup> {
    let pool: Vec<&FontEntry> = entries.iter().filter(|e| passes(e, filters)).collect();
    if !fold(query).is_empty() {
        let mut scored: Vec<(usize, u32, &FontEntry)> = pool
            .iter()
            .enumerate()
            .map(|(i, e)| (i, match_score(e, query), *e))
            .filter(|(_, s, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        if scored.is_empty() {
            return Vec::new();
        }
        return vec![FontGroup {
            id: GroupId::Results,
            label: LABEL_RESULTS,
            entries: scored.into_iter().map(|(_, _, e)| e.clone()).collect(),
        }];
    }
    let by_key: HashMap<&str, &FontEntry> = entries.iter().map(|e| (e.key.as_str(), e)).collect();
    let pick = |keys: &[FontKey]| -> Vec<FontEntry> {
        let mut seen = HashSet::new();
        keys.iter()
            .filter(|k| seen.insert(k.as_str()))
            .filter_map(|k| by_key.get(k.as_str()).map(|e| (*e).clone()))
            .collect()
    };
    let mut groups = Vec::new();
    if filters.is_empty() {
        groups.push(FontGroup { id: GroupId::Favorites, label: LABEL_FAVORITES, entries: pick(favorites) });
        groups.push(FontGroup { id: GroupId::Recent, label: LABEL_RECENT, entries: pick(recent) });
    }
    for (c, l) in FONT_CATEGORIES {
        groups.push(FontGroup {
            id: GroupId::Category(c),
            label: l,
            entries: pool.iter().filter(|e| e.def.is_some_and(|d| d.category == c)).map(|e| (*e).clone()).collect(),
        });
    }
    groups.push(FontGroup {
        id: GroupId::User,
        label: LABEL_USER,
        entries: pool.iter().filter(|e| e.kind == FontEntryKind::User).map(|e| (*e).clone()).collect(),
    });
    groups.push(FontGroup {
        id: GroupId::Local,
        label: LABEL_LOCAL,
        entries: pool.iter().filter(|e| e.kind == FontEntryKind::Local).map(|e| (*e).clone()).collect(),
    });
    groups.retain(|g| !g.entries.is_empty());
    groups
}

/// The options in listbox order (`flatOptions`).
pub fn flat_options(groups: &[FontGroup]) -> Vec<&FontEntry> {
    groups.iter().flat_map(|g| g.entries.iter()).collect()
}

/// A key that moves the active row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveKey {
    Down,
    Up,
    Home,
    End,
    PageDown,
    PageUp,
}

/// The active option after a move, clamped to the list (`moveActive`); from none, ↓ goes to the
/// first option and ↑ to the last.
pub fn move_active(active: Option<usize>, key: MoveKey, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let last = count - 1;
    Some(match key {
        MoveKey::Down => active.map_or(0, |a| (a + 1).min(last)),
        MoveKey::Up => active.map_or(last, |a| a.saturating_sub(1)),
        MoveKey::Home => 0,
        MoveKey::End => last,
        MoveKey::PageDown => (active.unwrap_or(0) + PAGE_ROWS).min(last),
        MoveKey::PageUp => active.unwrap_or(0).saturating_sub(PAGE_ROWS),
    })
}

/// First non-empty line of the block's text, trimmed to about `max` characters (`previewText`).
pub fn preview_text(text: &str, max: usize) -> String {
    let line = text.split('\n').map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() > max {
        let head: String = chars[..max - 1].iter().collect();
        format!("{}…", head.trim_end())
    } else {
        line.to_owned()
    }
}

/// The sentence the picker shows when `def` prints poorly at the block's cap height
/// (`qualityNote`).
pub fn quality_note(def: &FontDef) -> Option<String> {
    let min = MIN_QUALITY_CAP_MM;
    match def.quality? {
        FontQuality::Thin => Some(format!("{} has thin strokes that print poorly below {min} mm; try M or larger.", def.label)),
        FontQuality::Script => Some(format!("{} is a script font that blurs below {min} mm; try M or larger.", def.label)),
    }
}

/// Short tag for a row whose font would print poorly at the current cap height (`qualityTag`).
pub fn quality_tag(def: Option<&FontDef>, cap_mm: Option<f64>) -> Option<&'static str> {
    let q = def?.quality?;
    let cap = cap_mm?;
    // `!(capMm < MIN)`: NaN counts as not small.
    if cap.partial_cmp(&MIN_QUALITY_CAP_MM) != Some(std::cmp::Ordering::Less) {
        return None;
    }
    Some(match q {
        FontQuality::Thin => "Thin at this size",
        FontQuality::Script => "Hard to read at this size",
    })
}

/// Recent fonts after picking `key`: moved to the front, deduplicated, capped (`pushRecent`).
pub fn push_recent(recent: &[FontKey], key: &str) -> Vec<FontKey> {
    std::iter::once(key.to_owned())
        .chain(recent.iter().filter(|k| *k != key).cloned())
        .take(MAX_RECENT_FONTS)
        .collect()
}

/// Favourites after starring / unstarring `key`: new stars go last, a full list stays as is
/// (`toggleFavorite`).
pub fn toggle_favorite(favorites: &[FontKey], key: &str) -> Vec<FontKey> {
    if favorites.iter().any(|k| k == key) {
        return favorites.iter().filter(|k| *k != key).cloned().collect();
    }
    let mut out = favorites.to_vec();
    if out.len() < MAX_FAVORITE_FONTS {
        out.push(key.to_owned());
    }
    out
}

// ------------------------------------------------------------------------------------------
// Preview faces registered with GPUI
// ------------------------------------------------------------------------------------------

/// GPUI family names of the faces registered for previews, by font key (`b:<id>`, `u:<ref>`,
/// `l:<postscript name>`).
#[derive(Default)]
pub struct PreviewFonts {
    families: HashMap<String, SharedString>,
    bundled_started: bool,
    tried: HashSet<String>,
}

impl Global for PreviewFonts {}

/// The `name` table of an sfnt (first face of a collection).
fn name_table(sfnt: &[u8]) -> Option<&[u8]> {
    let be16 = |o: usize| sfnt.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let be32 = |o: usize| sfnt.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let dir = if sfnt.get(..4) == Some(b"ttcf") { be32(12)? } else { 0 };
    let n = usize::from(be16(dir + 4)?);
    for i in 0..n {
        let rec = dir + 12 + i * 16;
        if sfnt.get(rec..rec + 4)? == b"name" {
            let (off, len) = (be32(rec + 8)?, be32(rec + 12)?);
            return sfnt.get(off..off.checked_add(len)?);
        }
    }
    None
}

/// The family name a face registers under (typographic family, else the family).
fn sfnt_family(sfnt: &[u8]) -> Option<String> {
    parse_name_table(name_table(sfnt)?).family
}

/// The shortest of the families (the shared family of a set of weight files).
fn shortest(names: Vec<String>) -> Option<String> {
    names.into_iter().min_by_key(String::len)
}

/// Picks the registered name: the candidate if GPUI lists it, else a listed name it starts with.
fn resolve_family(candidate: &str, listed: &HashSet<String>) -> Option<String> {
    if listed.contains(candidate) {
        return Some(candidate.to_owned());
    }
    listed
        .iter()
        .filter(|n| candidate.starts_with(n.as_str()) && !n.is_empty())
        .max_by_key(|n| n.len())
        .cloned()
}

/// Starts registering the bundled families for previews (once per process).
pub fn ensure_bundled(cx: &mut App) {
    let g = cx.default_global::<PreviewFonts>();
    if g.bundled_started {
        return;
    }
    g.bundled_started = true;
    let task = cx.background_executor().spawn(async move {
        let mut out: Vec<(String, Vec<Vec<u8>>, Option<String>)> = Vec::new();
        for def in FONTS.iter() {
            let mut bytes = Vec::new();
            let mut names = Vec::new();
            for (_, file) in def.files {
                if let Ok(b) = bundled_sfnt(file) {
                    if let Some(n) = sfnt_family(&b) {
                        names.push(n);
                    }
                    bytes.push(b.to_vec());
                }
            }
            out.push((format!("b:{}", def.id), bytes, shortest(names)));
        }
        out
    });
    cx.spawn(async move |cx| {
        let loaded = task.await;
        cx.update(|cx| register(loaded, cx));
    })
    .detach();
}

/// Registers decoded faces and records their family names; refreshes the windows.
fn register(loaded: Vec<(String, Vec<Vec<u8>>, Option<String>)>, cx: &mut App) {
    let mut names = Vec::with_capacity(loaded.len());
    let mut all: Vec<Cow<'static, [u8]>> = Vec::new();
    for (key, bytes, name) in loaded {
        all.extend(bytes.into_iter().map(Cow::Owned));
        names.push((key, name));
    }
    if all.is_empty() || cx.text_system().add_fonts(all).is_err() {
        return;
    }
    let listed: HashSet<String> = cx.text_system().all_font_names().into_iter().collect();
    let g = cx.default_global::<PreviewFonts>();
    for (key, name) in names {
        if let Some(fam) = name.and_then(|n| resolve_family(&n, &listed)) {
            g.families.insert(key, fam.into());
        }
    }
    cx.refresh_windows();
}

/// Registers uploaded fonts (by content ref) that are not registered yet.
pub fn ensure_user_fonts(refs: Vec<String>, studio: &Entity<Studio>, cx: &mut App) {
    let g = cx.default_global::<PreviewFonts>();
    let todo: Vec<String> = refs.into_iter().filter(|r| g.tried.insert(format!("u:{r}"))).collect();
    if todo.is_empty() {
        return;
    }
    let store = studio.read(cx).store.clone();
    let task = cx.background_executor().spawn(async move {
        todo.into_iter()
            .filter_map(|r| {
                let sfnt = decode_font_bytes(&store.font_bytes(&r)?).ok()?;
                let name = sfnt_family(&sfnt);
                Some((format!("u:{r}"), vec![sfnt], name))
            })
            .collect::<Vec<_>>()
    });
    cx.spawn(async move |cx| {
        let loaded = task.await;
        cx.update(|cx| register(loaded, cx));
    })
    .detach();
}

/// The GPUI family drawing font `key`, once registered.
pub fn preview_family(key: &str, cx: &App) -> Option<SharedString> {
    cx.try_global::<PreviewFonts>().and_then(|g| g.families.get(key).cloned())
}

/// Records installed fonts' families (drawn by name, nothing to register).
fn set_local_families(list: &[(String, String)], cx: &mut App) {
    let g = cx.default_global::<PreviewFonts>();
    for (ps, family) in list {
        g.families.insert(format!("l:{ps}"), family.clone().into());
    }
}

/// The font a text item draws with in the UI: GPUI family, weight and style.
pub fn item_font(item: &TextItem, cx: &App) -> (Option<SharedString>, FontWeight, bool) {
    let def = font_def(&item.font_family);
    let custom = item.custom_font.as_ref().and_then(|_| preview_family(&font_key_of(item), cx));
    match custom {
        Some(f) => (Some(f), FontWeight::NORMAL, item.italic),
        None => (
            preview_family(&format!("b:{}", def.id), cx),
            FontWeight(f32::from(resolve_weight(def, item.font_weight))),
            item.italic,
        ),
    }
}

// ------------------------------------------------------------------------------------------
// The picker dialog
// ------------------------------------------------------------------------------------------

/// Opens the picker for text block `item_id`.
pub fn open(studio: Entity<Studio>, item_id: String, window: &mut Window, cx: &mut App) {
    ensure_bundled(cx);
    let picker = cx.new(|cx| FontPicker::new(studio, item_id, window, cx));
    picker.update(cx, |p, cx| p.load_local(cx));
    let focus = picker.read(cx).search.clone();
    window.open_dialog(cx, move |d, _, _| d.title("Choose a font").w(px(420.)).child(picker.clone()));
    focus.update(cx, |s, cx| s.focus(window, cx));
}

/// State of the "Fonts on this computer…" list.
#[derive(Debug, Clone, PartialEq)]
enum LocalState {
    Idle,
    Loading,
    Ready(Vec<LocalFamily>),
}

pub struct FontPicker {
    studio: Entity<Studio>,
    item_id: String,
    pub search: Entity<InputState>,
    filters: Vec<FontFilter>,
    active: Option<usize>,
    local: LocalState,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// Child index in the list of each option (group headings are children too).
    row_children: Vec<usize>,
    _subs: Vec<Subscription>,
}

impl FontPicker {
    fn new(studio: Entity<Studio>, item_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search fonts"));
        let subs = vec![
            cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.active = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let (opts, _) = this.options(cx);
                    let pick = this.active.and_then(|a| opts.get(a).cloned()).or_else(|| opts.first().cloned());
                    this.pick(pick, window, cx);
                }
                _ => {}
            }),
            cx.observe(&studio, |_, _, cx| cx.notify()),
        ];
        let user: Vec<String> = studio.read(cx).store.list_fonts().into_iter().map(|f| f.reference).collect();
        ensure_user_fonts(user, &studio, cx);
        Self {
            studio,
            item_id,
            search,
            filters: Vec::new(),
            active: None,
            local: LocalState::Idle,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            row_children: Vec::new(),
            _subs: subs,
        }
    }

    fn item(&self, cx: &App) -> Option<TextItem> {
        match self.studio.read(cx).doc.item(&self.item_id) {
            Some(Item::Text(t)) => Some(t.clone()),
            _ => None,
        }
    }

    /// Every entry (plus the block's missing custom font) and the groups shown now.
    fn groups(&self, item: &TextItem, cx: &App) -> (Vec<FontEntry>, Vec<FontGroup>) {
        let s = self.studio.read(cx);
        let user: Vec<UserFontLike> = s
            .store
            .list_fonts()
            .into_iter()
            .map(|f| UserFontLike { reference: f.reference, family: f.family })
            .collect();
        let local: &[LocalFamily] = match &self.local {
            LocalState::Ready(l) => l,
            _ => &[],
        };
        let mut entries = font_entries(&user, local);
        if let Some(m) = missing_entry(item, &entries) {
            entries.push(m);
        }
        let q = self.search.read(cx).value();
        let groups = build_groups(&entries, &s.prefs.favorite_fonts, &s.prefs.recent_fonts, &q, &self.filters);
        (entries, groups)
    }

    fn options(&self, cx: &App) -> (Vec<FontEntry>, usize) {
        let Some(item) = self.item(cx) else { return (Vec::new(), 0) };
        let (_, groups) = self.groups(&item, cx);
        let opts: Vec<FontEntry> = flat_options(&groups).into_iter().cloned().collect();
        let n = opts.len();
        (opts, n)
    }

    /// `pick`: applies a bundled family (nearest weight) or a custom font, pushes it to the
    /// recent fonts and closes.
    fn pick(&mut self, entry: Option<FontEntry>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = entry
            && !e.missing
        {
            let id = self.item_id.clone();
            let key = e.key.clone();
            self.studio.update(cx, |s, cx| {
                let recent = push_recent(&s.prefs.recent_fonts, &key);
                s.props_update_prefs(|p| p.recent_fonts = recent, cx);
                s.update_item(
                    &id,
                    None,
                    |i| {
                        if let Item::Text(t) = i {
                            if let Some(def) = e.def {
                                t.font_family = def.id.into();
                                t.font_weight = resolve_weight(def, t.font_weight);
                                t.custom_font = None;
                            } else if let Some(src) = e.source.clone() {
                                t.custom_font = Some(src);
                            }
                        }
                    },
                    cx,
                );
            });
        }
        window.close_dialog(cx);
    }

    fn toggle_star(&mut self, entry: Option<&FontEntry>, cx: &mut Context<Self>) {
        let Some(e) = entry.filter(|e| !e.missing) else { return };
        let key = e.key.clone();
        self.studio.update(cx, |s, cx| {
            let favs = toggle_favorite(&s.prefs.favorite_fonts, &key);
            s.props_update_prefs(|p| p.favorite_fonts = favs, cx);
        });
    }

    fn move_by(&mut self, key: MoveKey, cx: &mut Context<Self>) {
        let (_, n) = self.options(cx);
        self.active = move_active(self.active, key, n);
        if let Some(ix) = self.active.and_then(|a| self.row_children.get(a)) {
            self.scroll.scroll_to_item(*ix);
        }
        cx.notify();
    }

    /// Lists the installed font families (scanned once in the background, then cached by the
    /// font registry). Starts as soon as the picker opens: a desktop app needs no permission.
    pub fn load_local(&mut self, cx: &mut Context<Self>) {
        if self.local != LocalState::Idle {
            return;
        }
        self.local = LocalState::Loading;
        let fonts = self.studio.read(cx).fonts.clone();
        let task = cx.background_executor().spawn(async move {
            let list = fonts.local_families().to_vec();
            let families: Vec<(String, String)> = fonts
                .system_fonts()
                .iter()
                .filter(|f| !f.postscript_name.is_empty())
                .map(|f| (f.postscript_name.clone(), f.family.clone()))
                .collect();
            (list, families)
        });
        cx.spawn(async move |this, cx| {
            let (list, families) = task.await;
            this.update(cx, |this, cx| {
                set_local_families(&families, cx);
                this.local = LocalState::Ready(list);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = ev.keystroke.key.as_str();
        let mv = match k {
            "down" => Some(MoveKey::Down),
            "up" => Some(MoveKey::Up),
            "pagedown" => Some(MoveKey::PageDown),
            "pageup" => Some(MoveKey::PageUp),
            "home" if self.active.is_some() => Some(MoveKey::Home),
            "end" if self.active.is_some() => Some(MoveKey::End),
            _ => None,
        };
        if let Some(m) = mv {
            self.move_by(m, cx);
            cx.stop_propagation();
            return;
        }
        let in_search = self.search.read(cx).focus_handle(cx).is_focused(window);
        if !in_search && (k == "f") && !ev.keystroke.modifiers.modified() {
            let (opts, _) = self.options(cx);
            let e = self.active.and_then(|a| opts.get(a).cloned());
            self.toggle_star(e.as_ref(), cx);
            cx.stop_propagation();
        } else if !in_search && (k == "enter" || k == "space") {
            let (opts, _) = self.options(cx);
            let e = self.active.and_then(|a| opts.get(a).cloned());
            self.pick(e, window, cx);
            cx.stop_propagation();
        }
    }
}

impl Render for FontPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(item) = self.item(cx) else {
            return div().child("The text block is gone.").into_any_element();
        };
        let (entries, groups) = self.groups(&item, cx);
        let chips = available_filters(&entries);
        let current_key = font_key_of(&item);
        let (favorites, cap_mm, quality_text, crisp, is_default, default_font) = {
            let s = self.studio.read(cx);
            let def = font_def(&item.font_family);
            let info = s.render.as_ref().and_then(|r| r.texts.iter().find(|t| t.item_id == item.id).cloned());
            let band = s.props_band(&Item::Text(item.clone()));
            let cap_mm = info.as_ref().map(|i| i.cap_dots * 25.4 / f64::from(band.dpi));
            let warning = s
                .render
                .as_ref()
                .and_then(|r| r.warnings.iter().find(|w| w.code == "font-quality" && w.item_id.as_deref() == Some(item.id.as_str())))
                .map(|w| w.message.clone());
            let quality_text = warning.map(|m| if item.custom_font.is_none() { quality_note(def).unwrap_or(m) } else { m });
            let crisp = if item.custom_font.is_some() { None } else { info.and_then(|i| i.pixel_scale) };
            let df = &s.prefs.default_font;
            let is_default = item.custom_font.is_none()
                && df.family == item.font_family
                && resolve_weight(def, df.weight) == resolve_weight(def, item.font_weight);
            (
                s.prefs.favorite_fonts.clone(),
                cap_mm,
                quality_text,
                crisp,
                is_default,
                {
                    let mut df = Prefs::default().default_font;
                    df.family = item.font_family.clone();
                    df.weight = resolve_weight(def, item.font_weight);
                    df
                },
            )
        };
        let opts: Vec<FontEntry> = flat_options(&groups).into_iter().cloned().collect();
        let active_entry = self.active.and_then(|a| opts.get(a).cloned());
        let star_target = active_entry.clone().or_else(|| entries.iter().find(|e| e.key == current_key).cloned());
        let query = self.search.read(cx).value().to_string();
        let sample_of = |e: &FontEntry| {
            let p = preview_text(&item.text, 24);
            if !p.is_empty() {
                p
            } else {
                e.def.map_or_else(|| e.label.clone(), |d| d.preview.to_owned())
            }
        };

        // Rows, with group headings, in a uniform scrolling column.
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut row_children = Vec::new();
        let mut index = 0usize;
        for g in &groups {
            rows.push(
                div()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(g.label)
                    .into_any_element(),
            );
            for e in &g.entries {
                let i = index;
                index += 1;
                row_children.push(rows.len());
                let fav = favorites.contains(&e.key);
                let weight = e.def.map_or(400, |d| resolve_weight(d, item.font_weight));
                let family = preview_family(&e.key, cx);
                let tag = quality_tag(e.def, cap_mm);
                let selected = e.key == current_key;
                let (e_pick, e_star) = (e.clone(), e.clone());
                rows.push(
                    v_flex()
                        .id(("font-row", i))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .when(Some(i) == self.active, |d| d.bg(theme.accent))
                        .when(selected, |d| d.border_1().border_color(theme.primary))
                        .hover(|d| d.bg(theme.muted))
                        .on_click(cx.listener(move |this, _, window, cx| this.pick(Some(e_pick.clone()), window, cx)))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .child(if e.missing { format!("{} (not on this device)", e.label) } else { e.label.clone() }),
                                )
                                .when_some(tag, |d, t| d.child(tag_el(t, theme.warning)))
                                .when(crisp.is_some() && selected, |d| d.child(tag_el("Crisp", theme.success)))
                                .when(!e.missing, |d| {
                                    d.child(
                                        div()
                                            .id(("font-star", i))
                                            .px_1()
                                            .text_color(if fav { theme.warning } else { theme.muted_foreground })
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(if fav {
                                                    "Remove from favourites"
                                                } else {
                                                    "Add to favourites"
                                                })
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.toggle_star(Some(&e_star), cx);
                                            }))
                                            .child(if fav { "★" } else { "☆" }),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_lg()
                                .font_weight(FontWeight(f32::from(weight)))
                                .when(item.italic, |d| d.italic())
                                .when_some(family, |d, f| d.font_family(f))
                                .child(sample_of(e)),
                        )
                        .test_support()
                        .into_any_element(),
                );
            }
        }

        self.row_children = row_children;
        let local_button = match &self.local {
            LocalState::Ready(_) => None,
            _ => Some(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Loading installed fonts…"),
            ),
        };
        let studio = self.studio.clone();
        let star_fav = star_target.as_ref().is_some_and(|e| favorites.contains(&e.key));
        let star_label = star_target.as_ref().map_or_else(|| "Favourite".to_owned(), |e| format!("Favourite: {}", e.label));
        let star_entry = star_target.clone();

        v_flex()
            .id("font-picker")
            .key_context("FontPicker")
            .track_focus(&self.focus)
            .gap_2()
            .on_key_down(cx.listener(Self::on_key))
            .child(Input::new(&self.search).small().cleanable(true))
            .when(chips.len() > 1, |d| {
                d.child(h_flex().flex_wrap().gap_1().children(chips.into_iter().map(|(f, label)| {
                    let on = self.filters.contains(&f);
                    Button::new(SharedString::from(format!("fp-chip-{label}")))
                        .label(label)
                        .xsmall()
                        .outline()
                        .selected(on)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = this.filters.iter().position(|x| *x == f) {
                                this.filters.remove(p);
                            } else {
                                this.filters.push(f);
                            }
                            this.active = None;
                            cx.notify();
                        }))
                })))
            })
            .when_some(quality_text, |d, q| d.child(div().text_xs().text_color(theme.warning).child(q)))
            .child(
                v_flex()
                    .id("font-list")
                    .h(px(360.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows)
                    .when(opts.is_empty(), |d| {
                        d.child(div().p_2().text_sm().text_color(theme.muted_foreground).child(format!("No fonts match “{}”.", query.trim())))
                    }),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        Button::new("fp-fav")
                            .icon(crate::icons::icon("star")).selected(star_fav).label(star_label.clone())
                            .small()
                            .outline()
                            .selected(star_fav)
                            .disabled(star_target.as_ref().is_none_or(|e| e.missing))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_star(star_entry.as_ref(), cx))),
                    )
                    .when(item.custom_font.is_none(), |d| {
                        let studio = studio.clone();
                        d.child(
                            Button::new("fp-default")
                                .icon(crate::icons::icon("check")).selected(is_default).label("Use for new text")
                                .small()
                                .outline()
                                .selected(is_default)
                                .on_click(move |_, _, cx| {
                                    let df = default_font.clone();
                                    studio.update(cx, |s, cx| {
                                        s.props_update_prefs(
                                            |p| p.default_font = if is_default { Prefs::default().default_font } else { df },
                                            cx,
                                        )
                                    })
                                }),
                        )
                    })
                    .children(local_button)
                    .child(Button::new("fp-manage").icon(crate::icons::icon("settings")).label("Manage fonts…").small().outline().on_click({
                        let studio = self.studio.clone();
                        move |_, window, cx| {
                            window.close_dialog(cx);
                            crate::fonts_ui::open_fonts(studio.clone(), window, cx);
                        }
                    })),
            )
            .into_any_element()
    }
}

use gpui_kit::component::Disableable as _;

fn tag_el(text: &'static str, color: Hsla) -> AnyElement {
    div()
        .px_1p5()
        .rounded_full()
        .bg(color.opacity(0.15))
        .text_color(color)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        FontFilter, GroupId, MoveKey, UserFontLike, available_filters, build_groups, fold, font_entries, match_score,
        move_active, preview_text, push_recent, quality_tag, toggle_favorite,
    };
    use baradar_core::render::fonts::{FONTS, FontCategory, find_font};

    #[test]
    fn folding_and_search() {
        assert_eq!(fold("  Fira–Sans  Café "), "fira sans cafe");
        let entries = font_entries(&[], &[]);
        assert_eq!(entries.len(), FONTS.len());
        let fira = entries.iter().find(|e| e.key == "b:fira-sans").cloned().unwrap_or_else(|| entries[0].clone());
        assert_eq!(match_score(&fira, "fira sans"), 140);
        assert_eq!(match_score(&fira, "zzz"), 0);
        let g = build_groups(&entries, &[], &[], "mono", &[]);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].id, GroupId::Results);
        assert!(g[0].entries.iter().any(|e| e.key == "b:jetbrains-mono"));
    }

    #[test]
    fn grouping() {
        let user = vec![UserFontLike { reference: "sha256-00".into(), family: "Mine".into() }];
        let entries = font_entries(&user, &[]);
        let favs = vec!["b:oswald".to_owned(), "b:gone".to_owned()];
        let g = build_groups(&entries, &favs, &[], "", &[]);
        assert_eq!(g[0].id, GroupId::Favorites);
        assert_eq!(g[0].entries.len(), 1);
        assert!(g.iter().any(|g| g.id == GroupId::User));
        let only = build_groups(&entries, &favs, &[], "", &[FontFilter::Category(FontCategory::Mono)]);
        assert!(only.iter().all(|g| g.id == GroupId::Category(FontCategory::Mono)));
        assert_eq!(available_filters(&entries).last().map(|f| f.0), Some(FontFilter::Yours));
    }

    #[test]
    fn keys_and_lists() {
        assert_eq!(move_active(None, MoveKey::Down, 5), Some(0));
        assert_eq!(move_active(None, MoveKey::Up, 5), Some(4));
        assert_eq!(move_active(Some(2), MoveKey::PageDown, 5), Some(4));
        assert_eq!(move_active(Some(1), MoveKey::Home, 0), None);
        assert_eq!(preview_text("\n  Hello world  \nx", 24), "Hello world");
        assert_eq!(preview_text("abcdefghij", 5), "abcd…");
        let r = push_recent(&["b:a".into(), "b:b".into()], "b:b");
        assert_eq!(r, vec!["b:b".to_owned(), "b:a".to_owned()]);
        let f = toggle_favorite(&["b:a".into()], "b:a");
        assert!(f.is_empty());
        let thin = FONTS.iter().find(|d| d.quality.is_some()).or_else(|| find_font("fira-sans"));
        if let Some(d) = thin.filter(|d| d.quality.is_some()) {
            assert!(quality_tag(Some(d), Some(2.0)).is_some());
            assert!(quality_tag(Some(d), Some(4.0)).is_none());
        }
    }
}

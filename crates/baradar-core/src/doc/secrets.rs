//! Secrets inside a label: Wi-Fi passwords (port of `web/src/doc/secrets.ts`, plus
//! `shareableDoc` and its notices from `persist-share.ts`).
//!
//! Passwords stay in local storage (label library, print history). Share links and exported
//! label files leave them out unless the user explicitly opts in. A password may be a
//! placeholder (`{{pw}}`): the real passwords then live in that batch column, so its cells are
//! blanked as well.

use super::schema::{Item, LabelDoc};
use crate::vars::variables::password_variable;

/// `NOTICE_WIFI_PASSWORD_OMITTED`.
pub const NOTICE_WIFI_PASSWORD_OMITTED: &str = "The Wi-Fi password was left out.";
/// `NOTICE_WIFI_PASSWORDS_OMITTED`.
pub const NOTICE_WIFI_PASSWORDS_OMITTED: &str = "The Wi-Fi passwords were left out.";
/// `NOTICE_CUSTOM_FONTS`.
pub const NOTICE_CUSTOM_FONTS: &str = "Custom fonts are not included. The recipient sees the built-in font unless they have the same font.";

/// The passwords came from batch columns (`{{pw}}`): their cells are blanked too.
/// `noticeWifiPasswordColumns`.
pub fn notice_wifi_password_columns(columns: &[String]) -> String {
    let names = columns
        .iter()
        .map(|c| format!("{{{{{c}}}}}"))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if columns.len() == 1 {
        "column"
    } else {
        "columns"
    };
    format!("The Wi-Fi passwords were left out, including the {names} {noun} of the data table.")
}

/// `true` when any code item (Wi-Fi, or one switched back to text) holds a Wi-Fi password.
/// `docHasSecrets`.
pub fn doc_has_secrets(doc: &LabelDoc) -> bool {
    doc.items.iter().any(
        |i| matches!(i, Item::Code(c) if c.wifi.as_ref().is_some_and(|w| !w.password.is_empty())),
    )
}

/// Result of [`strip_secrets`] (`StrippedSecrets`).
#[derive(Debug, Clone, PartialEq)]
pub struct StrippedSecrets {
    pub doc: LabelDoc,
    /// Passwords blanked: password fields plus non-empty cells of password columns.
    pub removed: usize,
    /// Batch columns whose cells were blanked (named by a `{{name}}` password).
    pub columns: Vec<String>,
}

/// A copy of `doc` with every Wi-Fi password blanked, including the batch columns a password
/// placeholder names. `stripSecrets`.
pub fn strip_secrets(doc: &LabelDoc) -> StrippedSecrets {
    let mut out = doc.clone();
    let mut removed = 0;
    let mut columns: Vec<String> = Vec::new();
    for item in &mut out.items {
        let Item::Code(c) = item else { continue };
        let Some(wifi) = c.wifi.as_mut().filter(|w| !w.password.is_empty()) else {
            continue;
        };
        removed += 1;
        if let Some(name) = password_variable(&wifi.password)
            && doc
                .batch
                .as_ref()
                .is_some_and(|b| b.columns.iter().any(|c| c == name))
            && !columns.iter().any(|c| c == name)
        {
            columns.push(name.to_string());
        }
        wifi.password.clear();
    }
    if removed == 0 {
        return StrippedSecrets {
            doc: out,
            removed,
            columns,
        };
    }
    if let Some(batch) = out.batch.as_mut()
        && !columns.is_empty()
    {
        let blank: Vec<usize> = columns
            .iter()
            .filter_map(|c| batch.columns.iter().position(|b| b == c))
            .collect();
        for row in &mut batch.rows {
            for (j, cell) in row.iter_mut().enumerate() {
                if blank.contains(&j) && !cell.is_empty() {
                    cell.clear();
                    removed += 1;
                }
            }
        }
    }
    StrippedSecrets {
        doc: out,
        removed,
        columns,
    }
}

/// The doc as it may leave this computer (share link, label file): Wi-Fi passwords blanked
/// unless `include_wifi_passwords`, plus the notices to show. Custom fonts stay referenced but
/// their files are never embedded, which gets a notice too. `shareableDoc`.
pub fn shareable_doc(doc: &LabelDoc, include_wifi_passwords: bool) -> (LabelDoc, Vec<String>) {
    let mut notices = Vec::new();
    let out = if include_wifi_passwords {
        doc.clone()
    } else {
        let s = strip_secrets(doc);
        if !s.columns.is_empty() {
            notices.push(notice_wifi_password_columns(&s.columns));
        } else if s.removed > 0 {
            notices.push(
                if s.removed == 1 {
                    NOTICE_WIFI_PASSWORD_OMITTED
                } else {
                    NOTICE_WIFI_PASSWORDS_OMITTED
                }
                .into(),
            );
        }
        s.doc
    };
    if out
        .items
        .iter()
        .any(|i| matches!(i, Item::Text(t) if t.custom_font.is_some()))
    {
        notices.push(NOTICE_CUSTOM_FONTS.into());
    }
    (out, notices)
}

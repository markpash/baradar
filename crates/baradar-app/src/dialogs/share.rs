//! Share link and label file export, with the Wi-Fi password question (port of
//! `requestShare` / `copyShareLink` / `exportFile` in ui/state/studio.svelte.ts and
//! ui/share/SecretsDialog.svelte, the studio's `share-secrets` dialog).
//!
//! Both actions leave Wi-Fi passwords out unless the user ticks "Include Wi-Fi password" in the
//! question, which is asked only when the label holds one and starts unticked every time.

use baradar_core::doc::secrets::doc_has_secrets;
use baradar_core::doc::share::{STUDIO_BASE_URL, create_share_link};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::common::{save_dir, toast};
use crate::studio::{Studio, Tone};

/// What the question is for. `ShareKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShareKind {
    Link,
    File,
}

impl ShareKind {
    fn noun(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::File => "file",
        }
    }
}

/// "Copy Share Link": copies a link that opens a copy of the label in the web studio.
pub fn copy_share_link(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    request_share(ShareKind::Link, studio, window, cx);
}

/// "Export File…" that asks about Wi-Fi passwords first, like the studio's Export file. (The
/// plain Export File action in app.rs always leaves them out.)
#[allow(dead_code, reason = "for the Export File action; app.rs is wired by its owner")]
pub fn request_export_file(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    request_share(ShareKind::File, studio, window, cx);
}

/// `requestShare(kind)`: asks first when the label holds a Wi-Fi password.
fn request_share(kind: ShareKind, studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    if doc_has_secrets(&studio.read(cx).doc) {
        open_secrets_dialog(kind, studio, window, cx);
    } else {
        run(kind, studio, false, window, cx);
    }
}

fn run(kind: ShareKind, studio: Entity<Studio>, include_wifi_passwords: bool, window: &mut Window, cx: &mut App) {
    match kind {
        ShareKind::Link => copy_link(&studio, include_wifi_passwords, window, cx),
        ShareKind::File => export_file(studio, include_wifi_passwords, cx),
    }
}

/// `copyShareLink(opts)`.
fn copy_link(studio: &Entity<Studio>, include_wifi_passwords: bool, window: &mut Window, cx: &mut App) {
    let s = studio.read(cx);
    let store = s.store.clone();
    match create_share_link(&s.doc, STUDIO_BASE_URL, |r| store.get_blob(r), include_wifi_passwords) {
        Ok(link) => {
            cx.write_to_clipboard(ClipboardItem::new_string(link.url));
            let detail = if link.notices.is_empty() {
                "Anyone with the link can open a copy of this label.".to_owned()
            } else {
                link.notices.join(" ")
            };
            toast(window, cx, Tone::Ok, "Share link copied", Some(detail));
        }
        Err(e) => toast(window, cx, Tone::Error, "Could not create a share link", Some(e.0)),
    }
}

/// `exportFile(opts)`: asks where, then writes `.ptlabel.json` (the studio toasts notices).
fn export_file(studio: Entity<Studio>, include_wifi_passwords: bool, cx: &mut App) {
    let name = studio.read(cx).export_file_name();
    let rx = cx.prompt_for_new_path(&save_dir(), Some(&name));
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(path))) = rx.await {
            studio.update(cx, |s, cx| s.export_file(path, include_wifi_passwords, cx));
        }
    })
    .detach();
}

fn open_secrets_dialog(kind: ShareKind, studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|_| SecretsDialog { studio, include: false, kind });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Share without the Wi-Fi password?").w(px(480.)).child(view.clone())
    });
}

/// The question's body: what happens, the checkbox, and the warning once it is ticked.
struct SecretsDialog {
    studio: Entity<Studio>,
    include: bool,
    kind: ShareKind,
}

impl Render for SecretsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(div().text_sm().child(
                "This label contains a Wi-Fi password. Links and files are easily forwarded, so the password is left out: the shared copy shows the network name, and the recipient types the password in.",
            ))
            .child(
                h_flex().child(
                    Checkbox::new("include-wifi-password")
                        .label("Include Wi-Fi password")
                        .checked(self.include)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.include = *checked;
                            cx.notify();
                        })),
                ),
            )
            .when(self.include, |d| {
                d.child(div().text_sm().text_color(cx.theme().danger).child(format!(
                    "Anyone with the {} can read the password and join your network.",
                    self.kind.noun()
                )))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("continue")
                            .primary()
                            .label(if self.include { "Continue with password" } else { "Continue without password" })
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                let (studio, include, kind) = (this.studio.clone(), this.include, this.kind);
                                window.close_dialog(cx);
                                run(kind, studio, include, window, cx);
                            })),
                    ),
            )
    }
}

//! The updater's state and its dialog ("Check for Updates…" and the top-bar button).

use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    notification::{Notification, NotificationType},
    progress::Progress,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::icon;
use crate::studio::Studio;
use crate::updater::{self, Release};

/// Where the updater is.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate(Release),
    Available(Release),
    Downloading { release: Release, done: u64, total: u64 },
    Installing,
    Failed(String),
}

pub struct Updater {
    pub state: UpdateState,
    task: Option<Task<()>>,
}

impl Updater {
    /// Creates the updater; checks in the background a few seconds after launch when the
    /// preference is on and this is an installed app (not a `cargo run` build).
    pub fn new(check_at_launch: bool, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            state: UpdateState::Idle,
            task: None,
        };
        if check_at_launch && updater::current_bundle().is_some() {
            this.task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                this.update(cx, |this, cx| this.check(None, cx)).ok();
            }));
        }
        this
    }

    pub fn update_available(&self) -> Option<&Release> {
        match &self.state {
            UpdateState::Available(r) => Some(r),
            _ => None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installing
        )
    }

    /// Looks up the nightly release. With a window, the result is also announced.
    pub fn check(&mut self, announce: Option<AnyWindowHandle>, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async { updater::fetch_release() }).await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(r) if updater::is_update(&r, updater::COMMIT) => UpdateState::Available(r),
                    Ok(r) => UpdateState::UpToDate(r),
                    Err(e) => UpdateState::Failed(e.to_string()),
                };
                if let Some(w) = announce {
                    let (kind, title) = match &this.state {
                        UpdateState::UpToDate(_) => (NotificationType::Success, "Baradar is up to date".to_owned()),
                        UpdateState::Available(r) => (NotificationType::Info, format!("{} is available", r.name)),
                        UpdateState::Failed(e) => (NotificationType::Error, format!("Could not check for updates: {e}")),
                        _ => return cx.notify(),
                    };
                    let _ = w.update(cx, |_, window, cx| {
                        window.push_notification(Notification::new().title(title).with_type(kind), cx)
                    });
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Downloads, verifies and installs the available release, then quits so the helper can
    /// relaunch the new version.
    pub fn install(&mut self, studio: Entity<Studio>, cx: &mut Context<Self>) {
        let Some(release) = self.update_available().cloned() else {
            return;
        };
        let Some(bundle) = updater::current_bundle() else {
            self.state = UpdateState::Failed(
                "Only the installed Baradar.app can update itself. Download the new build from the release page.".into(),
            );
            cx.notify();
            return;
        };
        let bundle_id = match updater::bundle_identifier(&bundle) {
            Ok(id) => id,
            Err(e) => {
                self.state = UpdateState::Failed(e.to_string());
                cx.notify();
                return;
            }
        };
        self.state = UpdateState::Downloading { release: release.clone(), done: 0, total: release.size };
        cx.notify();
        let (tx, rx) = async_channel::unbounded::<(u64, u64)>();
        self.task = Some(cx.spawn(async move |this, cx| {
            let work = cx.background_executor().spawn(async move {
                let dir = std::env::temp_dir().join(format!("baradar-update-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir)?;
                let zip = updater::download(&release, &dir, |done, total| {
                    let _ = tx.try_send((done, total));
                })?;
                let app = updater::unpack(&zip, &dir, &bundle_id)?;
                updater::install(&app, &bundle)?;
                anyhow::Ok(())
            });
            // Show progress while the download runs.
            let progress = async {
                while let Ok((done, total)) = rx.recv().await {
                    let more = this
                        .update(cx, |this, cx| {
                            if let UpdateState::Downloading { done: d, total: t, .. } = &mut this.state {
                                *d = done;
                                *t = total;
                                cx.notify();
                            }
                        })
                        .is_ok();
                    if !more {
                        break;
                    }
                }
            };
            let (result, ()) = futures_join(work, progress).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.state = UpdateState::Installing;
                    cx.notify();
                    // Save the open label before the app quits for the relaunch.
                    studio.update(cx, |s, _| s.flush_autosave());
                    cx.quit();
                }
                Err(e) => {
                    this.state = UpdateState::Failed(e.to_string());
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

/// Runs two futures to completion (the progress loop ends when the sender is dropped).
async fn futures_join<A, B>(a: impl std::future::Future<Output = A>, b: impl std::future::Future<Output = B>) -> (A, B) {
    let a = std::pin::pin!(a);
    let b = std::pin::pin!(b);
    let mut a = Some(a);
    let mut b = Some(b);
    let mut ra = None;
    let mut rb = None;
    std::future::poll_fn(move |cx| {
        if let Some(f) = a.as_mut()
            && let std::task::Poll::Ready(v) = f.as_mut().poll(cx)
        {
            ra = Some(v);
            a = None;
        }
        if let Some(f) = b.as_mut()
            && let std::task::Poll::Ready(v) = f.as_mut().poll(cx)
        {
            rb = Some(v);
            b = None;
        }
        if a.is_none() && b.is_none() {
            std::task::Poll::Ready((ra.take().expect("first result"), rb.take().expect("second result")))
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}

/// What this build is, for the dialog.
fn this_build() -> String {
    let kind = if updater::CI_BUILD { "nightly" } else { "local build" };
    if updater::COMMIT.is_empty() {
        format!("unknown commit ({kind})")
    } else {
        let dirty = if updater::COMMIT.ends_with("-dirty") { ", with local changes" } else { "" };
        format!("{} ({kind}{dirty})", updater::short(updater::COMMIT))
    }
}

/// The update dialog's content.
pub struct UpdateView {
    updater: Entity<Updater>,
    studio: Entity<Studio>,
    _subs: Vec<Subscription>,
}

impl UpdateView {
    fn new(updater: Entity<Updater>, studio: Entity<Studio>, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&updater, |_, _, cx| cx.notify()),
            cx.observe(&studio, |_, _, cx| cx.notify()),
        ];
        Self { updater, studio, _subs: subs }
    }
}

impl Render for UpdateView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let state = self.updater.read(cx).state.clone();
        let auto = self.studio.read(cx).prefs.check_updates;
        let installed = updater::current_bundle().is_some();
        let window_handle = window.window_handle();
        let status: AnyElement = match &state {
            UpdateState::Idle => div().child("Not checked yet.").into_any_element(),
            UpdateState::Checking => div().child("Checking for a newer nightly…").into_any_element(),
            UpdateState::UpToDate(r) => div()
                .child(format!("You have the latest nightly ({}).", updater::short(&r.commit)))
                .into_any_element(),
            UpdateState::Available(r) => v_flex()
                .gap_1()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} is available.", r.name)))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!("Published {} · {:.1} MB", r.published_at.replace('T', " ").trim_end_matches('Z'), r.size as f64 / 1e6)),
                )
                .when(!installed, |d| {
                    d.child(div().text_sm().text_color(theme.warning).child(
                        "This copy isn't an installed Baradar.app (it was started with cargo), so it can't replace itself. Download the build from the release page.",
                    ))
                })
                .into_any_element(),
            UpdateState::Downloading { done, total, .. } => {
                let pct = if *total > 0 { *done as f32 / *total as f32 * 100.0 } else { 0.0 };
                v_flex()
                    .gap_1()
                    .child(format!("Downloading… {:.1} of {:.1} MB", *done as f64 / 1e6, *total as f64 / 1e6))
                    .child(Progress::new("update-progress").value(pct))
                    .into_any_element()
            }
            UpdateState::Installing => div().child("Installing; Baradar will restart…").into_any_element(),
            UpdateState::Failed(e) => div().text_color(theme.danger).child(e.clone()).into_any_element(),
        };
        let release = match &state {
            UpdateState::Available(r) | UpdateState::UpToDate(r) | UpdateState::Downloading { release: r, .. } => Some(r.clone()),
            _ => None,
        };
        let busy = self.updater.read(cx).busy();
        let can_install = matches!(state, UpdateState::Available(_)) && installed;
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(div().text_color(theme.muted_foreground).child("This build:"))
                    .child(this_build()),
            )
            .child(status)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("update-check")
                            .icon(icon("refresh"))
                            .label("Check now")
                            .small()
                            .outline()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.updater.update(cx, |u, cx| u.check(Some(window_handle), cx))
                            })),
                    )
                    .when(can_install, |d| {
                        d.child(
                            Button::new("update-install")
                                .icon(icon("download"))
                                .label("Update and restart")
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let studio = this.studio.clone();
                                    this.updater.update(cx, |u, cx| u.install(studio, cx))
                                })),
                        )
                    })
                    .when_some(release.filter(|r| !r.page.is_empty()), |d, r| {
                        d.child(
                            Button::new("update-page")
                                .icon(icon("link"))
                                .label("View release")
                                .small()
                                .ghost()
                                .on_click(move |_, _, cx| cx.open_url(&r.page)),
                        )
                    }),
            )
            .child(
                Checkbox::new("update-auto")
                    .label("Check for updates when Baradar starts")
                    .checked(auto)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        this.studio.update(cx, |s, cx| {
                            s.prefs.check_updates = v;
                            let _ = s.store.save_prefs(&s.prefs);
                            cx.notify();
                        })
                    })),
            )
    }
}

/// Opens the update dialog and starts a check unless one already ran.
pub fn open_update_dialog(updater: Entity<Updater>, studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    if matches!(updater.read(cx).state, UpdateState::Idle | UpdateState::Failed(_)) {
        updater.update(cx, |u, cx| u.check(None, cx));
    }
    let view = cx.new(|cx| UpdateView::new(updater, studio, cx));
    window.open_dialog(cx, move |d, _, _| d.title("Software update").w(px(460.)).child(view.clone()));
}

#[cfg(test)]
mod ui_tests {
    use super::{UpdateState, UpdateView, Updater};
    use crate::batch::ui_tests::{new_studio, open_test_window};
    use crate::updater::Release;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext};

    fn release() -> Release {
        Release {
            commit: "437ce59f911c43760e5e88a11b76bdecdcb99e4f".into(),
            name: "Nightly (437ce59)".into(),
            published_at: "2026-10-18T04:48:20Z".into(),
            url: "https://example.invalid/Baradar-macos-arm64.zip".into(),
            size: 13_969_186,
            digest: None,
            page: "https://example.invalid/releases/tag/nightly".into(),
        }
    }

    /// The dialog renders in every state (no network: the states are set directly).
    #[gpui_kit::test]
    fn update_dialog_renders_every_state(cx: &mut TestAppContext) {
        let studio = new_studio(cx, "updates");
        let updater = cx.update(|cx| cx.new(|cx| Updater::new(false, cx)));
        let states = [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate(release()),
            UpdateState::Available(release()),
            UpdateState::Downloading { release: release(), done: 5_000_000, total: 13_969_186 },
            UpdateState::Installing,
            UpdateState::Failed("could not reach GitHub".into()),
        ];
        let (window, _view) = {
            let (u, s) = (updater.clone(), studio.clone());
            open_test_window(cx, move |_, cx| UpdateView::new(u, s, cx))
        };
        for state in states {
            cx.update(|cx| updater.update(cx, |u, cx| {
                u.state = state;
                cx.notify();
            }));
            cx.update_window(window, |_, window, cx| window.render_frame(cx)).expect("render");
        }
    }
}

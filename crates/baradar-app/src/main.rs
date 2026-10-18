//! baradar label studio: a GPUI desktop port of the ptouch web studio.

mod actions;
mod app;
mod batch;
mod blocks;
mod connect;
mod diagnostics;
mod dialogs;
mod fonts_ui;
mod icons;
mod media;
mod preview;
mod print_bar;
mod props;
mod studio;
mod update_ui;
mod updater;
mod view_model;

use anyhow::Context as _;
use baradar_core::store::Store;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::*;

use crate::actions::*;
use crate::app::StudioView;

fn main() -> anyhow::Result<()> {
    // IOBluetooth runs on GPUI's main run loop instead of ptouch-transport's private one.
    #[cfg(target_os = "macos")]
    ptouch_transport::macos::use_host_run_loop();

    let store = Store::open(Store::default_root()?).context("opening the label library")?;
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            actions::bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_action(|_: &ToggleTheme, cx| {
                let mode = if cx.theme().is_dark() { ThemeMode::Light } else { ThemeMode::Dark };
                Theme::change(mode, None, cx);
            });
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.set_menus(actions::menus());

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1360.), px(860.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Baradar".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let store = store.clone();
            let (handle, view) = gpui_kit::open_window(options, cx, move |window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
                cx.new(|cx| StudioView::new(store, window, cx))
            })
            .expect("open window");
            handle
                .update(cx, |_, window, cx| {
                    window.focus(&view.read(cx).focus_handle(cx), cx);
                    window.activate_window();
                })
                .ok();
            cx.activate(true);
        });
    Ok(())
}

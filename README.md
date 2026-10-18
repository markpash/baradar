# baradar

A native macOS label studio for Brother P-touch printers, written in Rust with
[GPUI](https://gpui.rs) via [gpui-kit](https://github.com/longbridge/gpui-kit). It is a port of
the [fjanicki/ptouch](https://github.com/fjanicki/ptouch) web studio. It reuses that project's
Rust protocol core and transports (pinned git dependencies) and ports the studio's TypeScript:
document model, renderer, templates, batches and UI.

Built for the PT-E720BT, but every model in ptouch's table works the same way.

## Workspace

| Crate | What it is |
|---|---|
| `crates/baradar-core` | Label documents (schema v4, `.ptlabel.json`, share links), variables and batches, templates, the renderer (text, codes, icons, shapes, images, layout), PNG/PDF export, and local storage. No UI. |
| `crates/baradar-printer` | The printer client: a thread that drives `ptouch::Session` over a transport, with connection states, the waking retry, keepalive, print progress, cancel and the studio's problem messages. Also Bluetooth scan and pairing on macOS. |
| `crates/baradar-app` | The GPUI desktop app. |
| `crates/baradar-cli` | A small CLI: `baradar print`, `baradar scan`, `baradar pair`. |
| `vendor/ptouch-transport` | ptouch-transport at the pinned rev, plus a patch that lets its macOS Bluetooth backend run on GPUI's main run loop (see `PATCHES.md`). |
| `xtask` | `cargo xtask bundle` builds `target/Baradar.app`. |

Renders were compared with the live web studio: layout, codes and icons match dot for dot, and
anti-aliased text is within a few percent. `crates/baradar-core/examples/golden_docs.rs`
regenerates the comparison cases.

## Running

```sh
cargo run -p baradar-app                 # the studio
cargo xtask bundle && open target/Baradar.app
cargo run -p baradar-cli -- print "Hello" --printer virtual:12
```

**Bluetooth on macOS.** macOS lets an app use Bluetooth only if the app that launched it declares
`NSBluetoothAlwaysUsageDescription`. `Baradar.app` declares it, and so do Terminal and iTerm. When
it's started from somewhere that doesn't, such as an editor or agent tool, macOS kills the
process the moment it touches Bluetooth. Pair the printer in System Settings, or use
`baradar scan` / `baradar pair` from Terminal.

Labels, images, fonts, print history and preferences are stored in
`~/Library/Application Support/baradar`.

## Tests

```sh
cargo test --workspace
```

None of the tests talks to real hardware. Printing is exercised against ptouch's virtual printer.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT licence](LICENSE-MIT),
at your option, like the [fjanicki/ptouch](https://github.com/fjanicki/ptouch) project it ports.
The bundled fonts keep their own licences (OFL-1.1 or Apache-2.0, next to each font in
`crates/baradar-core/assets/fonts/`), and the icon catalogue comes from Lucide (ISC) and Feather
(MIT).

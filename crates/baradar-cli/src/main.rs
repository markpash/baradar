use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use baradar_core::doc::{Item, ItemKind, LabelDoc};
use baradar_core::render::fonts::{FontRegistry, find_font};
use baradar_core::render::renderer::{RenderOptions, RenderTarget, Renderer};
use baradar_printer::Printer;
use clap::{Parser, Subcommand};

/// Print text labels on a Brother PT-E720BT.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a text label.
    Print {
        /// Text to print; `\n` starts a new line.
        text: String,
        /// Printer endpoint (`bt:XX-XX-XX-XX-XX-XX`, `serial:/dev/cu.…`, `usb:`, `virtual:12`).
        /// Default: the paired PT-E720BT.
        #[arg(long, env = "BARADAR_PRINTER")]
        printer: Option<String>,
        /// Built-in font family id (e.g. `fira-sans`, `oswald`, `jetbrains-mono`).
        #[arg(long, default_value = "fira-sans")]
        font: String,
    },
    /// List nearby Bluetooth devices (printers must be switched on).
    #[cfg(target_os = "macos")]
    Scan {
        /// Inquiry length in seconds (1–48).
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// Also list devices that don't look like P-touch printers.
        #[arg(long)]
        all: bool,
    },
    /// Pair a printer: the given address, or the first unpaired PT-E720BT found by a scan.
    #[cfg(target_os = "macos")]
    Pair {
        /// Bluetooth address (XX:XX:XX:XX:XX:XX or XX-XX-XX-XX-XX-XX).
        address: Option<String>,
        /// Scan length in seconds when no address is given.
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // macOS delivers IOBluetooth callbacks on the main thread, which must pump its run loop;
    // the printer work runs on a worker thread meanwhile.
    ptouch_transport::run_with_event_loop(move || run(cli.command))
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Print {
            text,
            printer,
            font,
        } => print(&text, printer.as_deref(), font),
        #[cfg(target_os = "macos")]
        Command::Scan { seconds, all } => scan(seconds, all),
        #[cfg(target_os = "macos")]
        Command::Pair { address, seconds } => pair(address.as_deref(), seconds),
    }
}

fn print(text: &str, endpoint: Option<&str>, font: String) -> Result<()> {
    if find_font(&font).is_none() {
        bail!("unknown font {font:?}");
    }
    let mut printer = match endpoint {
        Some(endpoint) => Printer::open(endpoint)?,
        None => Printer::find()?,
    };
    let (profile, tape) = (printer.profile()?, printer.tape()?);
    eprintln!(
        "Printing on {} mm {:?} tape ({})…",
        tape.width_mm_x10 as f32 / 10.0,
        tape.kind,
        profile.name
    );
    // One text block at Fit, rendered exactly as the studio would.
    let mut item = Item::new(ItemKind::Text);
    if let Item::Text(t) = &mut item {
        t.text = text.replace("\\n", "\n");
        t.font_family = font;
    }
    let mut doc = LabelDoc::with_items(vec![item]);
    doc.tape.width_mm = f64::from(tape.width_mm_x10) / 10.0;
    let renderer = Renderer::new(Arc::new(FontRegistry::new()));
    let render = renderer.render(&doc, &RenderTarget { profile, tape }, &RenderOptions::default())?;
    if let Some(w) = render.warnings.iter().find(|w| w.blocking) {
        bail!("{}", w.message);
    }
    printer.print_bitmap(&render.bitmap)?;
    eprintln!("Done.");
    Ok(())
}

#[cfg(target_os = "macos")]
fn scan(seconds: u64, all: bool) -> Result<()> {
    eprintln!("Scanning for {seconds} s…");
    let devices = baradar_printer::bluetooth::scan(Duration::from_secs(seconds))?;
    let shown: Vec<_> = devices.iter().filter(|d| all || d.is_printer).collect();
    for d in &shown {
        let paired = if d.paired { "paired" } else { "not paired" };
        println!("{}  {:<24} {paired}", d.addr, d.name);
    }
    if shown.is_empty() {
        eprintln!(
            "No {} found. Is the printer on? If nothing at all shows up with --all, allow this \
             terminal Bluetooth access (System Settings → Privacy & Security → Bluetooth).",
            if all { "devices" } else { "printers" }
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn pair(address: Option<&str>, seconds: u64) -> Result<()> {
    use baradar_printer::bluetooth::PairEvent;

    let (addr, name) = match address {
        Some(a) => (a.parse()?, a.to_owned()),
        None => {
            eprintln!("Scanning for {seconds} s…");
            let found = baradar_printer::bluetooth::scan(Duration::from_secs(seconds))?
                .into_iter()
                .find(|d| d.is_printer && !d.paired && d.name.contains("E720"))
                .ok_or_else(|| {
                    anyhow::anyhow!("no unpaired PT-E720BT found; is it on and not paired already?")
                })?;
            (found.addr, found.name)
        }
    };
    eprintln!("Pairing with {name}…");
    baradar_printer::bluetooth::pair(addr, Duration::from_secs(30), |ev| match ev {
        PairEvent::PinRequested => eprintln!("Printer asked for a PIN; sent 0000."),
        PairEvent::ConfirmNumber(n) => eprintln!("Confirming pairing code {n:06}."),
        PairEvent::Passkey(n) => eprintln!("Enter {n:06} on the printer."),
        PairEvent::SimplePairingComplete(_) | PairEvent::Finished(_) => {}
    })?;
    eprintln!("Paired. Print with: baradar print \"Hello\"");
    Ok(())
}

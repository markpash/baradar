//! Repository tasks.
//!
//! `cargo xtask bundle [--debug]` builds `baradar-app` and wraps it in `target/Baradar.app` with
//! the Info.plist macOS requires for Bluetooth (`NSBluetoothAlwaysUsageDescription`): without
//! it, macOS kills the app the first time it touches Bluetooth.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

const BUNDLE_ID: &str = "me.markpash.baradar";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("bundle") => bundle(!args.iter().any(|a| a == "--debug")),
        _ => {
            eprintln!("usage: cargo xtask bundle [--debug]");
            Ok(())
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn bundle(release: bool) -> Result<()> {
    let root = workspace_root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(cargo);
    build.current_dir(&root).args(["build", "-p", "baradar-app"]);
    if release {
        build.arg("--release");
    }
    if !build.status().context("running cargo build")?.success() {
        bail!("cargo build failed");
    }
    let profile = if release { "release" } else { "debug" };
    let binary = root.join("target").join(profile).join("baradar-app");
    let app = root.join("target").join("Baradar.app");
    let contents = app.join("Contents");
    if app.exists() {
        std::fs::remove_dir_all(&app).with_context(|| format!("removing {}", app.display()))?;
    }
    std::fs::create_dir_all(contents.join("MacOS"))?;
    std::fs::create_dir_all(contents.join("Resources"))?;
    std::fs::copy(&binary, contents.join("MacOS").join("baradar-app"))
        .with_context(|| format!("copying {}", binary.display()))?;
    std::fs::write(contents.join("Info.plist"), info_plist())?;
    std::fs::write(contents.join("PkgInfo"), "APPL????")?;
    // Ad-hoc signature, so the Info.plist is bound to the executable for macOS privacy checks.
    let signed = Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(&app)
        .status()
        .context("running codesign")?;
    if !signed.success() {
        bail!("codesign failed");
    }
    println!("{}", app.display());
    Ok(())
}

fn info_plist() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>baradar-app</string>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>Baradar</string>
	<key>CFBundleDisplayName</key>
	<string>Baradar</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSBluetoothAlwaysUsageDescription</key>
	<string>Baradar connects to your Brother P-touch label printer over Bluetooth.</string>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>
			<string>P-touch label</string>
			<key>CFBundleTypeRole</key>
			<string>Editor</string>
			<key>CFBundleTypeExtensions</key>
			<array>
				<string>json</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}

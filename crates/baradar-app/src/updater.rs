//! Nightly updates: compares this build's commit with the `nightly` release on GitHub, then
//! downloads, verifies and installs the new `Baradar.app` in place and relaunches.
//!
//! Every step here is blocking; the UI runs them on the background executor.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

/// The commit this build was made from (`build.rs`), possibly with `-dirty`; empty when unknown.
pub const COMMIT: &str = env!("BARADAR_COMMIT");

/// Built by the nightly workflow (as opposed to a local build).
pub const CI_BUILD: bool = matches!(env!("BARADAR_CI_BUILD").as_bytes(), b"1");

/// The release asset the workflow publishes.
pub const ASSET_NAME: &str = "Baradar-macos-arm64.zip";

/// `owner/repo` from the workspace's `repository` URL.
pub fn repo() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
        .trim_end_matches('/')
        .trim_start_matches("https://github.com/")
}

/// The first seven characters of a commit, for display.
pub fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// The published nightly build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Full commit hash the release was built from.
    pub commit: String,
    pub name: String,
    /// ISO 8601.
    pub published_at: String,
    pub url: String,
    pub size: u64,
    /// `sha256:<hex>` when GitHub publishes one.
    pub digest: Option<String>,
    /// The release page, for "View release".
    pub page: String,
}

/// Parses `GET /repos/{repo}/releases/tags/nightly`.
pub fn parse_release(json: &str) -> Result<Release> {
    let v: serde_json::Value = serde_json::from_str(json).context("reading the release information")?;
    let commit = v["target_commitish"].as_str().unwrap_or_default().to_owned();
    if commit.len() < 7 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("the nightly release does not name the commit it was built from");
    }
    let asset = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(ASSET_NAME))
        .context("the nightly release has no macOS build")?;
    Ok(Release {
        commit,
        name: v["name"].as_str().unwrap_or("Nightly").to_owned(),
        published_at: v["published_at"].as_str().unwrap_or_default().to_owned(),
        url: asset["browser_download_url"].as_str().context("the macOS build has no download link")?.to_owned(),
        size: asset["size"].as_u64().unwrap_or(0),
        digest: asset["digest"].as_str().map(str::to_owned),
        page: v["html_url"].as_str().unwrap_or_default().to_owned(),
    })
}

/// Whether `release` is a different build from `current` (a nightly always moves forward, so
/// "different" means "newer"). An unknown or locally modified build counts as different.
pub fn is_update(release: &Release, current: &str) -> bool {
    current.is_empty() || current.ends_with("-dirty") || !release.commit.eq_ignore_ascii_case(current)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .user_agent(concat!("baradar/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Fetches the nightly release.
pub fn fetch_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{}/releases/tags/nightly", repo());
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => anyhow::anyhow!("there is no nightly release yet"),
            other => anyhow::anyhow!("could not reach GitHub: {other}"),
        })?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .context("reading the release information")?;
    parse_release(&text)
}

/// Downloads the release's zip into `dir`, checking its SHA-256 against GitHub's digest.
/// `progress(done, total)` is called as bytes arrive.
pub fn download(release: &Release, dir: &Path, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let mut resp = agent()
        .get(&release.url)
        .call()
        .map_err(|e| anyhow::anyhow!("could not download the update: {e}"))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(release.size);
    let path = dir.join(ASSET_NAME);
    let mut file = std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    let mut reader = resp.body_mut().with_config().limit(512 * 1024 * 1024).reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).context("downloading the update")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    file.sync_all()?;
    let got = hex(&hasher.finalize());
    if let Some(want) = release.digest.as_deref().and_then(|d| d.strip_prefix("sha256:"))
        && !want.eq_ignore_ascii_case(&got)
    {
        bail!("the download is damaged (its checksum does not match the release)");
    }
    Ok(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks the zip into `dir` with macOS's `ditto` (keeps the bundle and its signature intact)
/// and returns the `Baradar.app` inside, after checking it is a Baradar bundle with an intact
/// signature.
pub fn unpack(zip: &Path, dir: &Path, bundle_id: &str) -> Result<PathBuf> {
    let out = dir.join("unpacked");
    let _ = std::fs::remove_dir_all(&out);
    run("ditto", &[OsArg::S("-x"), OsArg::S("-k"), OsArg::P(zip), OsArg::P(&out)]).context("unpacking the update")?;
    let app = out.join("Baradar.app");
    if !app.join("Contents/MacOS/baradar-app").is_file() {
        bail!("the update does not contain Baradar.app");
    }
    let id = bundle_identifier(&app)?;
    if id != bundle_id {
        bail!("the update is a different app ({id})");
    }
    run("codesign", &[OsArg::S("--verify"), OsArg::S("--deep"), OsArg::P(&app)])
        .context("the update's signature is not intact")?;
    Ok(app)
}

enum OsArg<'a> {
    S(&'a str),
    P(&'a Path),
}

fn run(program: &str, args: &[OsArg<'_>]) -> Result<String> {
    let mut cmd = Command::new(program);
    for a in args {
        match a {
            OsArg::S(s) => cmd.arg(s),
            OsArg::P(p) => cmd.arg(p),
        };
    }
    let out = cmd.output().with_context(|| format!("running {program}"))?;
    if !out.status.success() {
        bail!("{program} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// `CFBundleIdentifier` of a bundle.
pub fn bundle_identifier(app: &Path) -> Result<String> {
    let plist = app.join("Contents/Info.plist");
    run("plutil", &[OsArg::S("-extract"), OsArg::S("CFBundleIdentifier"), OsArg::S("raw"), OsArg::P(&plist)])
}

/// The `.app` bundle this process runs from, if any (`cargo run` builds are not bundles).
pub fn current_bundle() -> Option<PathBuf> {
    bundle_of(&std::env::current_exe().ok()?.canonicalize().ok()?)
}

/// `…/Name.app/Contents/MacOS/exe` → `…/Name.app`.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension().is_some_and(|e| e == "app"))
    .then(|| app.to_path_buf())
}

/// Replaces `bundle` with `new_app` and starts a helper that reopens it once this process has
/// exited (the caller quits right after).
pub fn install(new_app: &Path, bundle: &Path) -> Result<()> {
    let previous = swap_bundles(new_app, bundle)?;
    relaunch_after_exit(bundle, &previous)
}

/// Moves `bundle` aside and `new_app` into its place; returns where the previous version went
/// (put back if the swap fails).
fn swap_bundles(new_app: &Path, bundle: &Path) -> Result<PathBuf> {
    let parent = bundle.parent().context("the app has no folder")?;
    let old = parent.join(".Baradar-previous.app");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(bundle, &old).with_context(|| {
        format!("could not replace {} (is its folder writable?)", bundle.display())
    })?;
    let placed = std::fs::rename(new_app, bundle)
        .or_else(|_| run("ditto", &[OsArg::P(new_app), OsArg::P(bundle)]).map(|_| ()).map_err(std::io::Error::other));
    if let Err(e) = placed {
        let _ = std::fs::remove_dir_all(bundle);
        let _ = std::fs::rename(&old, bundle);
        bail!("could not install the update: {e}");
    }
    Ok(old)
}

/// Starts a helper that waits for this process to exit, reopens `bundle`, then deletes the
/// previous version.
fn relaunch_after_exit(bundle: &Path, old: &Path) -> Result<()> {
    let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; /usr/bin/open "$2"; rm -rf "$3""#;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .arg(std::process::id().to_string())
            .arg(bundle)
            .arg(old)
            .process_group(0)
            .spawn()
            .context("starting the relaunch helper")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{
        "tag_name": "nightly", "name": "Nightly (437ce59)",
        "target_commitish": "437ce59f911c43760e5e88a11b76bdecdcb99e4f",
        "published_at": "2026-10-18T04:48:20Z",
        "html_url": "https://github.com/markpash/baradar/releases/tag/nightly",
        "assets": [
            {"name": "other.txt", "size": 1, "browser_download_url": "https://x/other.txt"},
            {"name": "Baradar-macos-arm64.zip", "size": 13969186,
             "digest": "sha256:8bcb1be6067cc1efea4ca634002c41d0060006c57725785264a6dab2736b0e0a",
             "browser_download_url": "https://github.com/markpash/baradar/releases/download/nightly/Baradar-macos-arm64.zip"}
        ]}"#;

    #[test]
    fn parses_the_release() {
        let r = parse_release(JSON).unwrap();
        assert_eq!(r.commit, "437ce59f911c43760e5e88a11b76bdecdcb99e4f");
        assert_eq!(r.size, 13_969_186);
        assert!(r.url.ends_with("/Baradar-macos-arm64.zip"));
        assert!(r.digest.as_deref().is_some_and(|d| d.starts_with("sha256:")));
        assert!(parse_release(r#"{"target_commitish": "main", "assets": []}"#).is_err());
        assert!(parse_release(r#"{"target_commitish": "437ce59f9", "assets": []}"#).is_err());
    }

    #[test]
    fn decides_when_to_update() {
        let r = parse_release(JSON).unwrap();
        assert!(!is_update(&r, "437ce59f911c43760e5e88a11b76bdecdcb99e4f"));
        assert!(is_update(&r, "0000000000000000000000000000000000000000"));
        assert!(is_update(&r, "437ce59f911c43760e5e88a11b76bdecdcb99e4f-dirty"));
        assert!(is_update(&r, ""));
        assert_eq!(short(&r.commit), "437ce59");
    }

    #[test]
    fn finds_the_bundle_of_an_executable() {
        let exe = Path::new("/Applications/Baradar.app/Contents/MacOS/baradar-app");
        assert_eq!(bundle_of(exe).as_deref(), Some(Path::new("/Applications/Baradar.app")));
        assert_eq!(bundle_of(Path::new("/x/target/debug/baradar-app")), None);
        assert_eq!(repo(), "markpash/baradar");
    }

    #[test]
    fn installs_by_swapping_bundles() {
        let dir = std::env::temp_dir().join(format!("baradar-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mk = |root: &Path, marker: &str| {
            std::fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
            std::fs::write(root.join("Contents/MacOS/baradar-app"), marker).unwrap();
        };
        let bundle = dir.join("Apps/Baradar.app");
        let new_app = dir.join("new/Baradar.app");
        mk(&bundle, "old");
        mk(&new_app, "new");
        let previous = swap_bundles(&new_app, &bundle).unwrap();
        let exe = std::fs::read_to_string(bundle.join("Contents/MacOS/baradar-app")).unwrap();
        assert_eq!(exe, "new");
        assert_eq!(std::fs::read_to_string(previous.join("Contents/MacOS/baradar-app")).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Downloads, verifies and unpacks the live nightly (network; run with `--ignored`).
    #[test]
    #[ignore]
    fn live_nightly_downloads_and_verifies() {
        let release = fetch_release().expect("fetch");
        let dir = std::env::temp_dir().join(format!("baradar-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut last = 0;
        let zip = download(&release, &dir, |done, _| last = done).expect("download");
        assert_eq!(last, release.size);
        let app = unpack(&zip, &dir, "me.markpash.baradar").expect("unpack");
        assert!(app.join("Contents/MacOS/baradar-app").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

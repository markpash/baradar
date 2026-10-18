//! Records which commit this build is (`BARADAR_COMMIT`), so the updater can tell whether the
//! published nightly is newer. CI builds use `GITHUB_SHA`; local builds ask git and add
//! `-dirty` when the working tree has uncommitted changes.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        // HEAD's reflog changes on every commit and checkout.
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    let commit = match std::env::var("GITHUB_SHA").ok().filter(|s| !s.is_empty()) {
        Some(sha) => sha,
        None => match git(&["rev-parse", "HEAD"]) {
            Some(sha) => {
                let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
                if dirty { format!("{sha}-dirty") } else { sha }
            }
            None => String::new(),
        },
    };
    println!("cargo:rustc-env=BARADAR_COMMIT={commit}");
    let ci = std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true");
    println!("cargo:rustc-env=BARADAR_CI_BUILD={}", if ci { "1" } else { "0" });
}

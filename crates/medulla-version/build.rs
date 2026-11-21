//! Embeds the build identity (MEDULLA-OBSERVE-R009.1): the git SHA, the enabled feature
//! list and the profile name. Nothing here reads the clock, so the build stays
//! byte-reproducible for one commit and one feature set (MEDULLA-CONTRACT-R053.14).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MEDULLA_GIT_SHA");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let sha = std::env::var("MEDULLA_GIT_SHA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_head)
        .unwrap_or_else(|| "unknown".to_string());
    let mut features: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| key.strip_prefix("CARGO_FEATURE_").map(str::to_ascii_lowercase))
        .collect();
    features.sort();
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=MEDULLA_BUILD_GIT_SHA={}", sha.trim());
    println!("cargo:rustc-env=MEDULLA_BUILD_FEATURES={}", features.join(","));
    println!("cargo:rustc-env=MEDULLA_BUILD_PROFILE={profile}");
}

/// The commit the source tree is at, from `git rev-parse HEAD`; `None` outside a checkout.
fn git_head() -> Option<String> {
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (sha.len() >= 7 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(sha)
}

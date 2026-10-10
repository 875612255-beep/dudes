//! Enforce the supported macOS deployment floor before compiling the host.

use std::env;

const MINIMUM_MACOS: (u64, u64, u64) = (12, 0, 0);

fn main() {
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let value = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| {
        panic!(
            "DUDES_MACOS_DEPLOYMENT_TARGET_REFUSED: MACOSX_DEPLOYMENT_TARGET is required and must be 12.0 or later"
        )
    });
    let parsed = parse_version(&value).unwrap_or_else(|| {
        panic!(
            "DUDES_MACOS_DEPLOYMENT_TARGET_REFUSED: malformed MACOSX_DEPLOYMENT_TARGET `{value}`; expected one to three numeric components"
        )
    });
    assert!(
        parsed >= MINIMUM_MACOS,
        "DUDES_MACOS_DEPLOYMENT_TARGET_REFUSED: MACOSX_DEPLOYMENT_TARGET `{value}` is below supported minimum 12.0"
    );
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let major = parse_component(parts.next()?)?;
    let minor = parts.next().map_or(Some(0), parse_component)?;
    let patch = parts.next().map_or(Some(0), parse_component)?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn parse_component(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

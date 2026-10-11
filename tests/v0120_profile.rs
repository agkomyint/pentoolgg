//! Public profile discovery, capability reporting, limits and CLI failures.
use pentool::color::{
    icc::{Direction, Intent},
    profile,
};
use serde_json::Value;
use std::process::Command;

struct Workspace(std::path::PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-print-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const RGB: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
const CMYK: &[u8] = include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc");

#[test]
fn discovery_pagination_and_capability_reports_describe_exact_output() {
    let all = profile::inspect(RGB, 0, 256).unwrap();
    assert_eq!(all.matches, all.returned);
    assert!(!all.has_more);
    assert!(all.capabilities.iter().all(|c| c.supported));
    assert!(all.metadata.description.unwrap().text.contains("Pentool"));
    for offset in [0, 1, all.matches, usize::MAX] {
        let result = profile::inspect(RGB, offset, 1).unwrap();
        assert_eq!(result.returned, usize::from(offset < all.matches));
        assert_eq!(result.profile.tags.len(), result.returned);
        assert_eq!(
            result.has_more,
            offset.saturating_add(result.returned) < result.matches
        );
    }
    for limit in [0, 257, usize::MAX] {
        assert!(profile::inspect(RGB, 0, limit).is_err());
    }
    let cmyk = profile::inspect(CMYK, 0, 256).unwrap();
    assert_eq!(cmyk.capabilities.iter().filter(|c| c.supported).count(), 1);
    assert!(profile::validate(CMYK, Some(Direction::DeviceToPcs), Intent::Perceptual).is_ok());
    assert!(profile::validate(
        CMYK,
        Some(Direction::DeviceToPcs),
        Intent::RelativeColorimetric
    )
    .is_err());
    assert!(profile::validate(RGB, None, Intent::Perceptual).is_err());
}

#[test]
fn real_press_inspection_reports_original_license_and_all_transform_paths() {
    let bytes = profile::read_file(std::path::Path::new(
        "docs/fixtures/print-CGATS21_CRPC7.icc",
    ))
    .unwrap();
    let result = profile::inspect(&bytes, 0, 1).unwrap();
    assert_eq!(result.profile.byte_length, 3462496);
    assert!(result
        .metadata
        .description
        .unwrap()
        .text
        .contains("CGATS21_CRPC7"));
    assert!(result.metadata.copyright.unwrap().text.contains("X-Rite"));
    assert!(result.capabilities.iter().all(|c| c.supported));
    assert!(result.has_more);
}

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn cli_inspection_and_validation_are_read_only_and_fail_requested_missing_paths() {
    let temp = Workspace::new("cli");
    let path = temp.path().join("probe.icc");
    std::fs::write(&path, CMYK).unwrap();
    let file = path.to_str().unwrap();
    let output = cli(&[
        "color", "profile", "inspect", file, "--limit", "1", "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["version"], 1);
    assert_eq!(result["icc_version"], "4.4.0");
    assert_eq!(result["returned"], 1);
    assert_eq!(result["tags"].as_array().unwrap().len(), 1);
    let output = cli(&[
        "color",
        "profile",
        "validate",
        file,
        "--intent",
        "perceptual",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["scope"],
        "requested-profile-path"
    );
    let output = cli(&[
        "color",
        "profile",
        "validate",
        file,
        "--intent",
        "relative-colorimetric",
        "--json",
    ]);
    assert!(!output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["ok"],
        false
    );
    assert_eq!(std::fs::read(&path).unwrap(), CMYK);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    assert!(
        !cli(&["color", "profile", "inspect", file, "--page", "page-1"])
            .status
            .success()
    );
}

#[test]
fn invalid_limits_fail_before_missing_file_io_and_loading_is_bounded() {
    let output = cli(&[
        "color",
        "profile",
        "inspect",
        "absent.icc",
        "--limit",
        "0",
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("limit must"));
    let temp = Workspace::new("limits");
    let path = temp.path().join("large.icc");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len((pentool::color::icc::MAX_PROFILE_BYTES + 1) as u64)
        .unwrap();
    assert!(profile::read_file(&path)
        .unwrap_err()
        .to_string()
        .contains("limit-exceeded"));
    assert!(profile::read_file(temp.path()).is_err());
}

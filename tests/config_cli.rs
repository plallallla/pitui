use std::{fs, process::Command};

#[test]
fn check_config_validates_without_entering_the_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 1\n[diff]\ndefault_mode = \"side-by-side\"\n",
    )
    .unwrap();
    let lower_priority = directory.path().join("from-env.toml");
    fs::write(&lower_priority, "schema_version = 99\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pitui"))
        .env("PITUI_CONFIG", lower_priority)
        .arg("--check-config")
        .arg("--config")
        .arg(&path)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("configuration valid:"));
    assert!(!stdout.contains("\u{1b}[?1049"));
}

#[test]
fn invalid_config_uses_the_documented_diagnostic_exit_code() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(&path, "schema_version = 99\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pitui"))
        .arg("--check-config")
        .arg("--config")
        .arg(&path)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unsupported schema_version 99"));
}

#[test]
fn print_effective_config_includes_the_resolved_diff_default() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 1\n[diff]\ndefault_mode = \"side-by-side\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pitui"))
        .arg("--print-effective-config")
        .arg("--config")
        .arg(&path)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[diff]\ndefault_mode = \"side-by-side\""));
    assert!(toml::from_str::<toml::Value>(&stdout).is_ok());
}

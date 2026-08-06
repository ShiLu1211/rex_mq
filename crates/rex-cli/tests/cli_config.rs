//! End-to-end CLI tests for the rex.toml surface.

use assert_cmd::Command;
use predicates::prelude::*;

fn rex_cli() -> Command {
    #[allow(clippy::unwrap_used)]
    // The crate's `[[bin]] name = "rex"` declares the binary file as
    // `rex`, not `rex-cli`. `assert_cmd` looks up the env var
    // `CARGO_BIN_EXE_<bin-name>` at test time, so the lookup string
    // must match the Cargo.toml `[[bin]] name` value.
    Command::cargo_bin("rex").unwrap()
}

#[test]
fn print_default_config_runs_and_is_parseable() {
    rex_cli()
        .arg("--print-default-config")
        .assert()
        .success()
        .stdout(predicate::str::contains("server_id = \"rex-server\""));
}

#[test]
fn print_default_config_round_trips() {
    let out = rex_cli()
        .arg("--print-default-config")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    let parsed: toml::Value = toml::from_str(&s).unwrap();
    assert_eq!(
        parsed["server"]["server_id"].as_str().unwrap(),
        "rex-server"
    );
}

#[test]
fn unknown_toml_key_fails_validation() {
    let toml_src = r#"
        [server]
        server_id = "x"
        bogus = 1
    "#;
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("r.toml");
    std::fs::write(&p, toml_src).unwrap();
    rex_cli()
        .arg("--config")
        .arg(p)
        .arg("--print-effective-config")
        .assert()
        .failure()
        .code(78);
}

#[test]
fn print_effective_config_without_file_uses_defaults() {
    rex_cli()
        .arg("--print-effective-config")
        .assert()
        .success()
        .stdout(predicate::str::contains("[server]"))
        .stdout(predicate::str::contains("[cluster]"));
}

//! End-to-end tests that run the `ig-mgr` binary against the filesystem.
//!
//! These are smoke tests over the CLI surface. Once the pipeline lands, add
//! fixture-driven cases that run against `tests/fixtures/sample_export/` and
//! snapshot the emitted CSV/Markdown with `insta`.

use assert_cmd::Command;
use predicates::str::contains;

fn ig_mgr() -> Command {
    Command::cargo_bin("ig-mgr").expect("binary `ig-mgr` should build")
}

#[test]
fn help_mentions_instagram() {
    ig_mgr()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Instagram"));
}

#[test]
fn version_is_printed() {
    ig_mgr()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn missing_export_dir_is_an_error() {
    // No positional argument: clap should reject the invocation.
    ig_mgr().assert().failure();
}

#[test]
fn nonexistent_export_dir_fails_gracefully() {
    ig_mgr()
        .arg("/no/such/export/dir")
        .assert()
        .failure()
        .stderr(contains("export directory"));
}

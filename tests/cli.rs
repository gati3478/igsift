//! End-to-end tests that run the `ig-mgr` binary against the filesystem.
//!
//! Smoke tests cover the CLI surface; the fixture-driven case asserts the
//! parser-pass acceptance criteria — exact follower/following/thread/message
//! counts against a hand-built sanitized export at
//! `tests/fixtures/sample_export/`. Snapshot tests on emitted CSV/Markdown
//! will land alongside the output writers (ROADMAP).

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::str::contains;

fn ig_mgr() -> Command {
    Command::cargo_bin("ig-mgr").expect("binary `ig-mgr` should build")
}

fn sample_export() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_export")
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

#[test]
fn fixture_counts_match_expected() {
    // Sanitized fixture: 3 followings, 2 followers, 2 threads, 7 total
    // messages (one thread is multi-part: 3 in message_1 + 2 in message_2).
    // Drifting any of these numbers means the parser silently dropped data.
    ig_mgr()
        .arg(sample_export())
        .assert()
        .success()
        .stdout(contains("following count: 3"))
        .stdout(contains("followers count: 2"))
        .stdout(contains("DM thread count: 2"))
        .stdout(contains("total DM messages: 7"));
}

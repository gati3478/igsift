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
    // Sanitized fixture: 3 followings, 2 followers, 2 inbox threads, 7 total
    // inbox messages (one thread is multi-part: 3 in message_1 + 2 in
    // message_2), the seven relationship-flag files and one message request
    // thread from the second slice, the four nested-`Owner` activity files
    // from the third slice — 2 liked posts (distinct owners), 1 story like,
    // 1 stories viewed, 1 saved post — the eight shape-A activity files
    // from the fourth slice — 2 liked comments, 1 of each of the seven
    // story_interactions files — and the three shape-D comment files from
    // the fifth slice — 2 post comments (distinct targets), 1 reel
    // comment, 1 hype (story comment). The activity counts come from
    // honest extraction (`owner_username` for nested-Owner, empty-title
    // filter for shape A, empty `Media Owner` filter for shape D), not
    // raw entry count, so they double as a structural assertion that the
    // deserializer walks the wrapper key and entry interior correctly.
    //
    // Drifting any of these numbers means the parser silently dropped data
    // — diagnose, don't relax the assertion.
    ig_mgr()
        .arg(sample_export())
        .assert()
        .success()
        .stdout(contains("following count: 3"))
        .stdout(contains("followers count: 2"))
        .stdout(contains("DM thread count: 2"))
        .stdout(contains("total DM messages: 7"))
        .stdout(contains("close friends count: 1"))
        .stdout(contains("favorited count: 2"))
        .stdout(contains("blocked count: 1"))
        .stdout(contains("restricted count: 1"))
        .stdout(contains("hide_story_from count: 1"))
        .stdout(contains("recently unfollowed count: 1"))
        .stdout(contains("removed suggestions count: 1"))
        .stdout(contains("message request thread count: 1"))
        .stdout(contains("liked posts count: 2"))
        .stdout(contains("story likes count: 1"))
        .stdout(contains("stories viewed count: 1"))
        .stdout(contains("saved posts count: 1"))
        .stdout(contains("liked comments count: 2"))
        .stdout(contains("story polls count: 1"))
        .stdout(contains("story quizzes count: 1"))
        .stdout(contains("story questions count: 1"))
        .stdout(contains("story emoji sliders count: 1"))
        .stdout(contains("story emoji reactions count: 1"))
        .stdout(contains("story reaction stickers count: 1"))
        .stdout(contains("story countdowns count: 1"))
        .stdout(contains("post comments count: 2"))
        .stdout(contains("reels comments count: 1"))
        .stdout(contains("hype count: 1"));
}

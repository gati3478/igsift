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
fn trace_unknown_handle_fails_loudly() {
    // A `--trace <handle>` that doesn't match any aggregated account is
    // almost certainly a typo at the command line. The run errors with
    // the offending handle named in the message so the user can fix it
    // immediately rather than seeing an empty trace and wondering why.
    ig_mgr()
        .arg(sample_export())
        .arg("--trace")
        .arg("no_such_handle_xyz")
        .assert()
        .failure()
        .stderr(contains("no_such_handle_xyz"));
}

#[test]
fn trace_known_handle_prints_contributions() {
    // `alice_synth` is in the fixture's following.json AND has the
    // close_friend flag, so the trace must surface a non-zero
    // `close_friend_boost` contribution. Pinning the label keeps the
    // term-contributions array order independent of how rustc lays out
    // the literal — a future term reordering shouldn't silently drop
    // the boost from the trace.
    ig_mgr()
        .arg(sample_export())
        .arg("--trace")
        .arg("alice_synth")
        .assert()
        .success()
        .stdout(contains("trace for \"alice_synth\""))
        .stdout(contains("close_friend_boost"));
}

#[test]
fn fixture_counts_match_expected() {
    // Sanitized fixture: 3 followings, 2 followers, 3 inbox threads, 9 total
    // inbox messages (alice_thread = 2 msgs, bob_thread = 5 across two parts,
    // carol_thread = 2 msgs), the seven relationship-flag files and one
    // message request thread from the second slice, the four nested-`Owner`
    // activity files from the third slice — 2 liked posts (distinct owners),
    // 1 story like, 1 stories viewed, 1 saved post — the eight shape-A
    // activity files from the fourth slice — 2 liked comments, 1 of each of
    // the seven story_interactions files — the three shape-D comment files
    // from the fifth slice — 2 post comments (distinct targets), 1 reel
    // comment, 1 hype (story comment) — and the slice-6 resolver
    // infrastructure: `me` identity from `personal_information.json`
    // (`me_synth` / `Test User`) plus a NameResolver over the eight
    // (Name, Username) pairs in the seven `label_values` fixtures (no
    // collisions on the synthetic data).
    //
    // `resolvable DM threads: 1` covers `carol_thread`, whose other
    // participant `Carol Synth` (display name) appears in `favorited` →
    // `carol_synth`. `alice_thread` and `bob_thread` spell their other
    // participants with the handle (`alice_synth` / `bob_synth`), which
    // the resolver correctly refuses to match — exercising both the
    // positive resolution path (catches a regression where the resolver
    // stops matching display-name spellings) and the negative path
    // (catches a regression where it suddenly starts matching the
    // handle spelling). The lib::run wiring — `participants` filter,
    // `others.len() == 1` predicate, `me.name` exclusion — is exactly
    // what this `1` count pins.
    //
    // The activity counts come from honest extraction (`owner_username`
    // for nested-Owner, empty-title filter for shape A, empty
    // `Media Owner` filter for shape D), not raw entry count, so they
    // double as a structural assertion that the deserializer walks the
    // wrapper key and entry interior correctly.
    //
    // Drifting any of these numbers means the parser silently dropped data
    // — diagnose, don't relax the assertion.
    ig_mgr()
        .arg(sample_export())
        .assert()
        .success()
        .stdout(contains("following count: 3"))
        .stdout(contains("followers count: 2"))
        .stdout(contains("DM thread count: 3"))
        .stdout(contains("total DM messages: 9"))
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
        .stdout(contains("hype count: 1"))
        .stdout(contains("me handle: me_synth"))
        .stdout(contains("me name: Test User"))
        .stdout(contains("name resolver entries: 8"))
        .stdout(contains("name resolver collisions: 0"))
        .stdout(contains("resolvable DM threads: 1"))
        // Slice-7A handle-keyed aggregator: 3 followings → 3 aggregated
        // accounts (no blocked/recently_unfollowed handle appears in
        // following.json, so both filters are no-ops on this fixture).
        // close_friends ∩ followings = {alice_synth} → 1. favorited ∩
        // followings = {bob_synth, carol_synth} → 2. None of the activity
        // targets in the fixture are in followings, so the with-likes /
        // with-comments counts are 0 — the activity-summation path is
        // pinned independently by the structural unit tests in
        // `src/features/aggregate.rs::tests`.
        .stdout(contains("aggregated accounts: 3"))
        .stdout(contains("aggregated close friends: 1"))
        .stdout(contains("aggregated favorited: 2"))
        .stdout(contains("aggregated with likes_given > 0: 0"))
        .stdout(contains("aggregated with comments_given > 0: 0"))
        // Slice-7B-1 DM aggregator: only `carol_thread` resolves (display
        // name "Carol Synth" → handle `carol_synth` via the favorited
        // bridge, and `carol_synth` is a followee). `alice_thread` and
        // `bob_thread` spell their other participants with the handle
        // (`alice_synth` / `bob_synth`) — the resolver refuses to match.
        // So only carol picks up DM features. `carol_thread` carries one
        // reaction in each direction (me reacting to her message →
        // given; her reacting to my message → received). `stranger_synth`
        // is not in any `label_values` file → message_requests doesn't
        // resolve → no followee gets `inbound_dm_request = true`.
        .stdout(contains("DM-attributed accounts: 1"))
        .stdout(contains("DM reactions given total: 1"))
        .stdout(contains("DM reactions received total: 1"))
        .stdout(contains("inbound DM requests: 0"))
        // First-pass scoring: alice (close_friend, boost 5.0), bob and
        // carol (favorited, boost 3.0) all land in Keep. carol picks up
        // additional DM signal (1 message in each direction, 1 reaction
        // each way) but the boost dominates regardless. No fixture
        // account is restricted, hide_story'd, or removed_suggestion'd,
        // so the review band is empty and unfollow is empty.
        .stdout(contains("bucket keep: 3"))
        .stdout(contains("bucket review: 0"))
        .stdout(contains("bucket unfollow: 0"));
}

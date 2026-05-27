//! End-to-end tests that run the `ig-mgr` binary against the filesystem.
//!
//! Smoke tests cover the CLI surface; the fixture-driven case asserts the
//! parser-pass acceptance criteria — exact follower/following/thread/message
//! counts against a hand-built sanitized export at
//! `tests/fixtures/sample_export/`. The CSV / Markdown writer slice adds
//! one further end-to-end test that asserts the two artifacts exist at
//! the `--out` path and that the CSV header matches DESIGN.md verbatim.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::str::contains;

fn ig_mgr() -> Command {
    // Spawn with cwd = OS temp dir so the binary's cwd-relative config
    // lookups (`config/scoring.toml`, `config/labels.txt`,
    // `config/keep_allowlist.txt`) miss the per-user files at the repo
    // root. Without this, a developer running `cargo test` after laying
    // down their own `config/labels.txt` or `config/keep_allowlist.txt`
    // sees the fixture-count test contaminate itself with real labels
    // and a non-zero allowlist size. Each spawned command gets its own
    // cwd, so parallel test execution is safe.
    let mut cmd = Command::cargo_bin("ig-mgr").expect("binary `ig-mgr` should build");
    cmd.current_dir(std::env::temp_dir());
    cmd
}

fn sample_export() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_export")
}

/// Per-test stem under `env::temp_dir()`. Tests pass this as `--out` so
/// the writer's default-path logic doesn't drop artifacts next to the
/// fixture directory (gitignored but still noise). Each test name maps
/// to a unique path so tests can run in parallel without colliding.
fn out_stem(test_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ig-mgr-test-{test_name}"))
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
fn check_subcommand_validates_fixture_and_exits_zero() {
    // `ig-mgr check <export>` exits zero against a well-shaped fixture
    // and prints a per-source success line for each parser. Acts as the
    // dispatch test (subcommand reaches the `check` handler) AND the
    // per-source-status format test in one go.
    ig_mgr()
        .arg("check")
        .arg(sample_export())
        .assert()
        .success()
        .stdout(contains("Validating export:"))
        .stdout(contains("following.json"))
        .stdout(contains("All sources parsed cleanly"));
}

#[test]
fn check_subcommand_fails_on_non_export_dir() {
    // Point check at a directory that isn't an IG export — the
    // pre-flight validator should surface every missing top-level
    // marker in one error.
    let empty = std::env::temp_dir().join(format!("ig-mgr-check-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).expect("mktemp");
    let result = ig_mgr().arg("check").arg(&empty).assert().failure();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr).into_owned();
    std::fs::remove_dir(&empty).ok();
    assert!(
        stderr.contains("does not look like"),
        "expected pre-flight diagnosis in stderr, got:\n{stderr}",
    );
}

#[test]
fn check_subcommand_accepts_a_zip_archive() {
    // Zip the sanitized fixture into a tempfile, then run
    // `ig-mgr check <tempfile.zip>`. End-to-end coverage of the
    // archive resolver -> validate_shape -> per-parser path.
    use std::io::Write as _;
    let zip_path = std::env::temp_dir().join(format!("ig-mgr-cli-zip-{}.zip", std::process::id()));
    let _ = std::fs::remove_file(&zip_path);
    {
        let file = std::fs::File::create(&zip_path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let root = sample_export();
        fn walk(
            writer: &mut zip::ZipWriter<std::fs::File>,
            base: &std::path::Path,
            dir: &std::path::Path,
            opts: zip::write::SimpleFileOptions,
        ) {
            for entry in std::fs::read_dir(dir).expect("read dir") {
                let entry = entry.expect("entry");
                let path = entry.path();
                let rel = path
                    .strip_prefix(base)
                    .expect("strip")
                    .to_string_lossy()
                    .into_owned();
                if path.is_dir() {
                    writer.add_directory(&rel, opts).expect("add_dir");
                    walk(writer, base, &path, opts);
                } else {
                    writer.start_file(&rel, opts).expect("start_file");
                    let bytes = std::fs::read(&path).expect("read");
                    writer.write_all(&bytes).expect("write");
                }
            }
        }
        walk(&mut writer, &root, &root, opts);
        writer.finish().expect("finish zip");
    }

    ig_mgr()
        .arg("check")
        .arg(&zip_path)
        .assert()
        .success()
        .stdout(contains("All sources parsed cleanly"));

    // Cleanup: cache dir and zip file.
    let parent = zip_path.parent().expect("parent");
    let stem = zip_path.file_stem().and_then(|s| s.to_str()).expect("stem");
    let _ = std::fs::remove_dir_all(parent.join(format!(".ig-mgr-extracted-{stem}")));
    let _ = std::fs::remove_file(&zip_path);
}

#[test]
fn init_subcommand_writes_template_files() {
    // Spawn from an empty temp dir so the cwd-relative `config/`
    // doesn't exist; init must create it and lay down the two
    // template files.
    let cwd = std::env::temp_dir().join(format!("ig-mgr-init-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).expect("mktemp");

    let mut cmd = Command::cargo_bin("ig-mgr").expect("binary");
    cmd.current_dir(&cwd)
        .arg("init")
        .assert()
        .success()
        .stdout(contains("wrote: config/keep_allowlist.txt"))
        .stdout(contains("wrote: config/labels.txt"));

    assert!(cwd.join("config/keep_allowlist.txt").is_file());
    assert!(cwd.join("config/labels.txt").is_file());

    // Re-running without --force skips both files.
    let mut cmd2 = Command::cargo_bin("ig-mgr").expect("binary");
    cmd2.current_dir(&cwd)
        .arg("init")
        .assert()
        .success()
        .stdout(contains("skipped: config/keep_allowlist.txt"))
        .stdout(contains("skipped: config/labels.txt"));

    std::fs::remove_dir_all(&cwd).ok();
}

#[test]
fn trace_unknown_handle_fails_loudly() {
    // A `--trace <handle>` that doesn't match any aggregated account is
    // almost certainly a typo at the command line. The run errors with
    // the offending handle named in the message so the user can fix it
    // immediately rather than seeing an empty trace and wondering why.
    ig_mgr()
        .arg(sample_export())
        .arg("--out")
        .arg(out_stem("trace_unknown"))
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
        .arg("--out")
        .arg(out_stem("trace_known"))
        .arg("--trace")
        .arg("alice_synth")
        .assert()
        .success()
        .stdout(contains("trace for \"alice_synth\""))
        .stdout(contains("close_friend_boost"));
}

#[test]
fn fixture_counts_match_expected() {
    // Sanitized fixture: 4 followings (alice/bob/carol_synth + the
    // brand-suffixed nytimes_official added in the account-class slice,
    // which has an empty `string_list_data` so its `followed_at` is None
    // and the brand-gate test isn't contaminated by tenure), 2 followers,
    // 3 inbox threads, 9 total inbox messages (alice_thread = 2 msgs,
    // bob_thread = 5 across two parts, carol_thread = 2 msgs), the seven
    // relationship-flag files and one message request thread from the
    // second slice, the four nested-`Owner` activity files from the
    // third slice — 2 liked posts (distinct owners), 1 story like, 1
    // stories viewed, 1 saved post — the eight shape-A activity files
    // from the fourth slice — 2 liked comments, 1 of each of the seven
    // story_interactions files — the three shape-D comment files from
    // the fifth slice — 2 post comments (distinct targets), 1 reel
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
    // -v is required: per-source smoke counts move behind verbose in
    // the default stdout. The counts themselves are still load-bearing
    // (catch silent parser drift); the test just opts back in.
    ig_mgr()
        .arg(sample_export())
        .arg("--out")
        .arg(out_stem("fixture_counts"))
        .arg("-v")
        .assert()
        .success()
        .stdout(contains("following count: 4"))
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
        // Slice-7A handle-keyed aggregator: 4 followings → 4 aggregated
        // accounts (no blocked/recently_unfollowed handle appears in
        // following.json, so both filters are no-ops on this fixture).
        // close_friends ∩ followings = {alice_synth} → 1. favorited ∩
        // followings = {bob_synth, carol_synth} → 2. nytimes_official is
        // Brand by lexicon hit on "official" — exercises the
        // account-class slice's classifier wire-through; no allowlist
        // overrides on this fixture so `keep-allowlisted: 0`. None of
        // the activity targets in the fixture are in followings, so the
        // with-likes / with-comments counts are 0 — the activity-summation
        // path is pinned independently by the structural unit tests in
        // `src/features/aggregate.rs::tests`.
        .stdout(contains("aggregated accounts: 4"))
        .stdout(contains("aggregated close friends: 1"))
        .stdout(contains("aggregated favorited: 2"))
        .stdout(contains("aggregated brands: 1"))
        .stdout(contains("aggregated keep-allowlisted: 0"))
        .stdout(contains("keep-allowlist size on disk: 0"))
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
        // First-pass scoring + brand gate:
        //   - alice (close_friend, boost 5.0) → Keep
        //   - bob (favorited, boost 3.0) → Keep
        //   - carol (favorited, boost 3.0; faint DM signal) → Keep
        //   - nytimes_official (no tenure, no engagement) scores below
        //     unfollow_max; without the brand-class gate it would be
        //     Unfollow, but `account_class == Brand` floors it at
        //     Review per DESIGN.md "Public figures / brands with low
        //     keep_prob get review, never unfollow."
        // No fixture account is restricted or hide_story'd, so all
        // three remaining buckets are accounted for by the above.
        .stdout(contains("bucket keep: 3"))
        .stdout(contains("bucket review: 1"))
        .stdout(contains("bucket unfollow: 0"));
}

#[test]
fn writes_csv_and_markdown_at_out_path() {
    // End-to-end pin for the output writer slice (extended for the
    // account-class slice). Runs the binary against the sample fixture
    // (4 followings: 3 in Keep + nytimes_official in Review via the
    // brand-class gate), asserts both files land at the `--out` stem
    // with the right extensions, and checks the load-bearing surface
    // in each: the CSV header (the inter-run diff contract per
    // DESIGN.md "Output"), the row count, and the Markdown summary
    // including the brand-gated Review entry.
    let stem = out_stem("writes_csv_and_md");
    let csv_path = stem.with_extension("csv");
    let md_path = stem.with_extension("md");
    // Pre-clean so a prior test run doesn't mask a failure to write.
    let _ = std::fs::remove_file(&csv_path);
    let _ = std::fs::remove_file(&md_path);

    ig_mgr()
        .arg(sample_export())
        .arg("--out")
        .arg(&stem)
        .assert()
        .success()
        .stdout(contains("wrote:"))
        .stdout(contains(csv_path.to_string_lossy().as_ref()))
        .stdout(contains(md_path.to_string_lossy().as_ref()));

    let csv = std::fs::read_to_string(&csv_path).expect("CSV should exist");
    let md = std::fs::read_to_string(&md_path).expect("Markdown should exist");

    // CSV header must match DESIGN.md verbatim — this is the diff contract.
    let header = csv.lines().next().expect("CSV has at least one line");
    assert_eq!(
        header,
        "username,display_name,profile_url,bucket,keep_prob,dm_msgs,last_dm_days,\
         reactions_given_180d,reactions_received_180d,\
         likes_given_90d,comments_given_90d,follow_tenure_days,\
         account_class,mutual,notes",
    );
    // 1 header + 4 rows (fixture has 4 followings: alice/bob/carol_synth
    // + nytimes_official as the brand-gate test case).
    assert_eq!(csv.lines().count(), 5, "expected 5 lines, got:\n{csv}");

    // nytimes_official rendered with account_class=brand — pins the
    // CSV column's connection to the classifier wire-through, not just
    // the unit tests in account_class.rs.
    assert!(
        csv.contains("nytimes_official"),
        "brand fixture row must appear in CSV:\n{csv}",
    );
    assert!(
        csv.lines()
            .any(|line| line.starts_with("nytimes_official,") && line.contains(",brand,")),
        "nytimes_official row must carry account_class=brand:\n{csv}",
    );

    // Mutual flag: fixture followers = [alice_synth, dave_synth]; only
    // alice_synth is also in following. Pin both directions (alice=true,
    // bob=false) so a regression in the followers-input wire-through or
    // the HashSet intersection fails here, not silently.
    let alice_row = csv
        .lines()
        .find(|l| l.starts_with("alice_synth,"))
        .expect("alice_synth row");
    assert!(
        alice_row.contains(",true,"),
        "alice_synth must be mutual=true: {alice_row}",
    );
    let bob_row = csv
        .lines()
        .find(|l| l.starts_with("bob_synth,"))
        .expect("bob_synth row");
    assert!(
        bob_row.contains(",false,"),
        "bob_synth must be mutual=false: {bob_row}",
    );

    // Markdown self-documents the run.
    assert!(md.contains("# ig-mgr following audit"));
    assert!(md.contains("Accounts scored: **4**"));
    assert!(md.contains("Keep: **3**"));
    assert!(md.contains("Review: **1**"));
    // `carol_synth` has a `display_name` populated via the NameResolver
    // reverse map (she appears in `favorited` with Name "Carol Synth").
    // The Markdown writer surfaces that — pinning it here means a
    // regression in the reverse-map join fails this test, not just the
    // unit test in `name_resolution.rs`.
    assert!(
        md.contains("Carol Synth"),
        "expected resolved display_name in Markdown body, got:\n{md}",
    );
}

//! `igsift` — local-first analysis of an Instagram personal data export.
//!
//! The library crate holds the pipeline; the `igsift` binary ([`main`]) is a
//! thin shell that parses arguments and calls [`run`]. Integration tests in
//! `tests/` drive the same code paths.
//!
//! Pipeline shape (see `docs/DESIGN.md` for the full design):
//!
//! ```text
//! input ──▶ archive::resolve  (dir / .zip / multipart .zip → extracted dir)
//!       ──▶ export::*          (parse JSON, schema-drift survivable)
//!       ──▶ features           (per-account feature aggregation)
//!       ──▶ scoring            (keep-probability + bucketing)
//!       ──▶ output::*          (CSV + Markdown + HTML writers)
//! ```
//!
//! The pipeline composes a `keep_prob` per account, assigns a bucket
//! (`keep` / `review` / `unfollow`) via the DESIGN.md formula plus the
//! bucket gates (restricted floor, droplist, deep-mutual keep-floor,
//! reciprocity keep-ceiling, and the boost / brand / keeplist
//! Unfollow→Review gates), and writes the CSV + Markdown + HTML audit next
//! to the export directory.

pub mod archive;
pub mod cli;
pub mod config;
pub mod export;
pub mod features;
pub mod labels;
pub mod lists;
pub mod output;
pub mod progress;
pub mod scoring;
pub mod summary;
pub mod term_style;
pub mod text;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cli::RunArgs;

/// Initialize structured logging. Verbosity comes from `-v`/`-vv` flags, with
/// `RUST_LOG` (if set) taking precedence.
pub fn init_tracing(verbose: u8) {
    use tracing_subscriber::EnvFilter;

    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("igsift={default_level}")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// `igsift init` — scaffold per-user `config/` files from their
/// checked-in templates. The .example files are embedded in the
/// binary at compile time so a fresh user who has only the binary
/// (no repo checkout) can still bootstrap.
///
/// Writes to `./config/<file>`. Skips if the target exists unless
/// `force` is true. Creates `./config/` if missing.
pub fn init(force: bool) -> Result<()> {
    use std::fs;

    const KEEP_ALLOWLIST_TEMPLATE: &str = include_str!("../config/keeplist.txt.example");
    const DROP_LIST_TEMPLATE: &str = include_str!("../config/droplist.txt.example");
    const LABELS_TEMPLATE: &str = include_str!("../config/labels.txt.example");

    let config_dir = PathBuf::from("config");
    fs::create_dir_all(&config_dir)
        .map_err(|e| anyhow::anyhow!("create config/ directory: {e}"))?;

    let targets: &[(&str, &str)] = &[
        ("config/keeplist.txt", KEEP_ALLOWLIST_TEMPLATE),
        ("config/droplist.txt", DROP_LIST_TEMPLATE),
        ("config/labels.txt", LABELS_TEMPLATE),
    ];

    let mut wrote = 0u32;
    let mut skipped = 0u32;
    for (rel_path, body) in targets {
        let path = PathBuf::from(rel_path);
        if path.exists() && !force {
            println!("skipped: {rel_path} (exists; pass --force to overwrite)");
            skipped += 1;
            continue;
        }
        fs::write(&path, body).map_err(|e| anyhow::anyhow!("write {rel_path}: {e}"))?;
        println!("wrote: {rel_path}");
        wrote += 1;
    }
    println!("\n{wrote} written, {skipped} skipped.");
    if wrote > 0 {
        println!("Edit config/keeplist.txt to mark accounts that must stay in Keep.");
        println!("Edit config/droplist.txt to force accounts into Unfollow.");
        println!("Edit config/labels.txt with your hand-labels to enable the accuracy report.");
    }
    Ok(())
}

/// `igsift check <export>` — validate that an export folder is
/// parseable without scoring it. Runs the shape pre-flight, then
/// attempts every parser, surfacing per-source status, and finishes
/// with a config sanity check (the keeplist and droplist must
/// parse and be disjoint). Exits non-zero (Err) on any failure —
/// a parse failure, or a both-listed handle that `run` would reject.
///
/// Useful for verifying a freshly-extracted multipart archive
/// before paying the cost of a full run, or as a CI dry-run
/// against a sanitized fixture.
pub fn check(input: &Path, rebuild_cache: bool) -> Result<()> {
    use anyhow::ensure;

    // Accept dirs and archives transparently; resolve returns the
    // extracted root or the input as-is if it's already extracted.
    // Progress is gated on stderr being a TTY (indicatif's stderr
    // draw target self-hides off-TTY, so `2>log` stays clean of
    // spinner escape codes for both directory and archive inputs).
    let progress_enabled = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let export_dir = &archive::resolve(input, rebuild_cache, progress_enabled)?;

    ensure!(
        export_dir.is_dir(),
        "export directory does not exist or is not a directory: {}",
        export_dir.display()
    );
    export::validate_shape(export_dir)?;

    println!("Validating export: {}", export_dir.display());
    println!("✓ shape: top-level markers present\n");

    // Each reader is called in isolation so one failure doesn't mask
    // others. The closure pattern keeps the listing dense — adding a
    // new parser is one new line.
    let mut failures: u32 = 0;
    let mut report = |label: &str, result: Result<usize>| match result {
        Ok(n) => println!("✓ {label:30} {n} entries"),
        Err(e) => {
            println!("✗ {label:30} {e}");
            failures += 1;
        }
    };

    report(
        "following.json",
        export::read_following(export_dir).map(|v| v.len()),
    );
    report(
        "followers.json",
        export::read_followers(export_dir).map(|v| v.len()),
    );
    report(
        "DM inbox threads",
        export::read_inbox(export_dir).map(|v| v.len()),
    );
    report(
        "message_requests threads",
        export::read_message_requests(export_dir).map(|v| v.len()),
    );

    report(
        "close_friends.json",
        export::read_close_friends(export_dir).map(|v| v.len()),
    );
    report(
        "favorited.json",
        export::read_favorited(export_dir).map(|v| v.len()),
    );
    report(
        "blocked.json",
        export::read_blocked(export_dir).map(|v| v.len()),
    );
    report(
        "restricted.json",
        export::read_restricted(export_dir).map(|v| v.len()),
    );
    report(
        "hide_story_from.json",
        export::read_hide_story_from(export_dir).map(|_| 1),
    );
    report(
        "recently_unfollowed.json",
        export::read_recently_unfollowed(export_dir).map(|v| v.len()),
    );
    report(
        "removed_suggestions.json",
        export::read_removed_suggestions(export_dir).map(|v| v.len()),
    );

    report(
        "liked_posts.json",
        export::read_liked_posts(export_dir).map(|v| v.len()),
    );
    report(
        "story_likes.json",
        export::read_story_likes(export_dir).map(|v| v.len()),
    );
    report(
        "stories_viewed.json",
        export::read_stories_viewed(export_dir).map(|v| v.len()),
    );
    report(
        "saved_posts.json",
        export::read_saved_posts(export_dir).map(|v| v.len()),
    );

    report(
        "liked_comments.json",
        export::read_liked_comments(export_dir).map(|v| v.len()),
    );
    report(
        "story polls/quizzes/etc",
        export::read_story_polls(export_dir).and_then(|polls| {
            let quizzes = export::read_story_quizzes(export_dir)?;
            let questions = export::read_story_questions(export_dir)?;
            let sliders = export::read_story_emoji_sliders(export_dir)?;
            let reactions = export::read_story_emoji_reactions(export_dir)?;
            let stickers = export::read_story_reaction_stickers(export_dir)?;
            let countdowns = export::read_story_countdowns(export_dir)?;
            Ok(polls.len()
                + quizzes.len()
                + questions.len()
                + sliders.len()
                + reactions.len()
                + stickers.len()
                + countdowns.len())
        }),
    );

    report(
        "post_comments.json",
        export::read_post_comments(export_dir).map(|v| v.len()),
    );
    report(
        "reels_comments.json",
        export::read_reels_comments(export_dir).map(|v| v.len()),
    );
    report("hype.json", export::read_hype(export_dir).map(|v| v.len()));

    report(
        "personal_information.json",
        export::read_me_identity(export_dir).map(|_| 1),
    );

    // Config sanity (independent of the export): the two per-user handle
    // lists must parse and be disjoint. A handle on both is a contradiction
    // `run` rejects at load — surfacing it here lets the dry-run catch it
    // before a full scoring pass, mirroring run's load-time gate. Tracked
    // separately from `failures` so the summary wording stays accurate: a
    // parse/conflict here is not a "source failed to parse".
    let config_result: Result<(usize, usize)> = (|| {
        let keep = lists::load_default()?;
        let drop = lists::load_droplist()?;
        lists::ensure_disjoint(&keep, &drop)?;
        Ok((keep.len(), drop.len()))
    })();
    let config_ok = config_result.is_ok();
    match config_result {
        Ok((keep, drop)) => {
            println!(
                "✓ {:30} {keep} keep + {drop} drop, disjoint",
                "config: handle lists"
            )
        }
        Err(e) => println!("✗ {:30} {e}", "config: handle lists"),
    }

    if failures == 0 && config_ok {
        println!("\nAll sources parsed cleanly.");
        return Ok(());
    }
    let mut problems = Vec::new();
    if failures > 0 {
        problems.push(format!("{failures} source(s) failed to parse"));
    }
    if !config_ok {
        // Terse + self-contained, mirroring the parser summary above: the
        // offending handle/file is on the stdout `✗` line, so this verdict
        // must not reference "above" (the streams split under redirection).
        problems.push("keeplist / droplist invalid (not disjoint or unparseable)".to_owned());
    }
    anyhow::bail!("{}", problems.join("; "))
}

/// Entry point for the analysis run.
///
/// The pipeline parses every export source, loads the scoring config, builds
/// the `me` identity and the `display_name → handle` resolver, runs the
/// feature aggregator, scores every account, and writes the CSV + Markdown +
/// HTML audit — plus a smoke-count summary and, when `config/labels.txt`
/// exists, the confusion-matrix report.
pub fn run(args: RunArgs) -> Result<()> {
    use anyhow::{anyhow, ensure};

    let input = args
        .export_dir
        .as_deref()
        .ok_or_else(|| anyhow!("export directory required — pass a path or use `igsift --help`"))?;

    let progress_enabled = args.verbose == 0;

    // Archive resolution runs BEFORE the phase spinner so the
    // extraction bar (a real bytes bar, not a spinner) is the active
    // UI while it's working. On an already-extracted directory this
    // is a near-zero-cost passthrough.
    let export_dir = archive::resolve(input, args.rebuild_cache, progress_enabled)?;
    let export_dir = export_dir.as_path();

    // Progress spinner: visible at default verbosity on a TTY, hidden
    // when -v / -vv (logs would interleave) or when stderr is piped.
    let progress = progress::Reporter::new(progress_enabled);

    progress.phase("Validating export shape");
    ensure!(
        export_dir.is_dir(),
        "export directory does not exist or is not a directory: {}",
        export_dir.display()
    );
    export::validate_shape(export_dir)?;

    progress.phase("Parsing connections (followings, followers, flag files)");
    let following = export::read_following(export_dir)?;
    let followers = export::read_followers(export_dir)?;
    let close_friends = export::read_close_friends(export_dir)?;
    let favorited = export::read_favorited(export_dir)?;
    let blocked = export::read_blocked(export_dir)?;
    let restricted = export::read_restricted(export_dir)?;
    let hide_story_from = export::read_hide_story_from(export_dir)?;
    let recently_unfollowed = export::read_recently_unfollowed(export_dir)?;
    let removed_suggestions = export::read_removed_suggestions(export_dir)?;

    progress.phase("Parsing DM threads");
    let threads = export::read_inbox(export_dir)?;
    let total_messages: usize = threads.iter().map(|t| t.messages.len()).sum();
    let message_request_threads = export::read_message_requests(export_dir)?;

    progress.phase("Parsing activity (likes, stories, saves)");
    let liked_posts = export::read_liked_posts(export_dir)?;
    let story_likes = export::read_story_likes(export_dir)?;
    let stories_viewed = export::read_stories_viewed(export_dir)?;
    let saved_posts = export::read_saved_posts(export_dir)?;

    progress.phase("Parsing activity (story interactions)");
    let liked_comments = export::read_liked_comments(export_dir)?;
    let story_polls = export::read_story_polls(export_dir)?;
    let story_quizzes = export::read_story_quizzes(export_dir)?;
    let story_questions = export::read_story_questions(export_dir)?;
    let story_emoji_sliders = export::read_story_emoji_sliders(export_dir)?;
    let story_emoji_reactions = export::read_story_emoji_reactions(export_dir)?;
    let story_reaction_stickers = export::read_story_reaction_stickers(export_dir)?;
    let story_countdowns = export::read_story_countdowns(export_dir)?;

    progress.phase("Parsing comments");
    let post_comments = export::read_post_comments(export_dir)?;
    let reels_comments = export::read_reels_comments(export_dir)?;
    let hype = export::read_hype(export_dir)?;

    progress.phase("Building name resolver");
    let me = export::read_me_identity(export_dir)?;
    let resolver = features::name_resolution::NameResolver::build(&[
        close_friends.as_slice(),
        favorited.as_slice(),
        blocked.as_slice(),
        restricted.as_slice(),
        recently_unfollowed.as_slice(),
        removed_suggestions.as_slice(),
        std::slice::from_ref(&hide_story_from),
    ]);

    // Sanity count: how many 1:1 DM threads have a resolvable other
    // participant under the current resolver? Cross-references the
    // 240/581 figure from the recon (real export) and gates regressions
    // when the seven label_values parsers, the DM thread parser, or the
    // resolver change shape. Uses the same `attributable_handle`
    // predicate as the aggregator — single source of truth for the
    // 1:1 / resolved / non-collision filter so this count and the
    // aggregator's `DM-attributed accounts` can't drift on a future
    // refactor.
    let resolvable_dm_threads = threads
        .iter()
        .filter(|thread| {
            features::aggregate::attributable_handle(thread, &me.name, &resolver).is_some()
        })
        .count();

    // `hide_story_from.json` is a single shape-C entry, not an array. With
    // every field carrying `#[serde(default)]`, an empty object `{}` parses
    // successfully — so "the file is shaped right" isn't enough to count
    // someone as a real hide. Treat the entry as real iff it carries at
    // least one label value (the username sits inside that list).
    let hide_story_from_count = usize::from(!hide_story_from.label_values.is_empty());

    // Same shape-hardening concern for the nested-Owner activity files: an
    // entry whose `label_values` parsed but contains no extractable Owner is
    // schema drift, not a real interaction. Count only entries that yield a
    // username so the count line answers "how many like signals fed the
    // scoring", not "how many objects deserialized".
    let liked_posts_count = liked_posts
        .iter()
        .filter_map(export::owner_username)
        .count();
    let story_likes_count = story_likes
        .iter()
        .filter_map(export::owner_username)
        .count();
    let stories_viewed_count = stories_viewed
        .iter()
        .filter_map(export::owner_username)
        .count();
    let saved_posts_count = saved_posts
        .iter()
        .filter_map(export::owner_username)
        .count();

    // Per-source smoke counts: load-bearing parser-correctness signal,
    // but noisy in the common run. Gate behind `-v` so the default
    // stdout stays focused on the scoring + audit summary. The
    // fixture-counts integration test passes `-v` so these assertions
    // still see the lines.
    if args.verbose > 0 {
        println!("following count: {}", following.len());
        println!("followers count: {}", followers.len());
        println!("DM thread count: {}", threads.len());
        println!("total DM messages: {total_messages}");
        println!("close friends count: {}", close_friends.len());
        println!("favorited count: {}", favorited.len());
        println!("blocked count: {}", blocked.len());
        println!("restricted count: {}", restricted.len());
        println!("hide_story_from count: {hide_story_from_count}");
        println!("recently unfollowed count: {}", recently_unfollowed.len());
        println!("removed suggestions count: {}", removed_suggestions.len());
        println!(
            "message request thread count: {}",
            message_request_threads.len()
        );
        println!("liked posts count: {liked_posts_count}");
        println!("story likes count: {story_likes_count}");
        println!("stories viewed count: {stories_viewed_count}");
        println!("saved posts count: {saved_posts_count}");
        println!("liked comments count: {}", liked_comments.len());
        println!("story polls count: {}", story_polls.len());
        println!("story quizzes count: {}", story_quizzes.len());
        println!("story questions count: {}", story_questions.len());
        println!("story emoji sliders count: {}", story_emoji_sliders.len());
        println!(
            "story emoji reactions count: {}",
            story_emoji_reactions.len()
        );
        println!(
            "story reaction stickers count: {}",
            story_reaction_stickers.len()
        );
        println!("story countdowns count: {}", story_countdowns.len());
        println!("post comments count: {}", post_comments.len());
        println!("reels comments count: {}", reels_comments.len());
        println!("hype count: {}", hype.len());
        println!("me handle: {}", me.handle);
        println!("me name: {}", me.name);
        println!("name resolver entries: {}", resolver.unique_name_count());
        println!("name resolver collisions: {}", resolver.collision_count());
        println!("resolvable DM threads: {resolvable_dm_threads}");
    }

    progress.phase("Loading scoring config + handle lists");
    let scoring_config = config::read_scoring_config(args.config.as_deref(), args.preset)?;
    let keeplist = lists::load_default()?;
    let droplist = lists::load_droplist()?;
    // A handle on both lists is a keep/drop contradiction — fail before
    // scoring so `assign_bucket` never sees a both-listed handle.
    lists::ensure_disjoint(&keeplist, &droplist)?;
    let keeplist_size = keeplist.len();
    let droplist_size = droplist.len();
    let classifier = features::Classifier::new(keeplist, droplist);

    progress.phase("Aggregating per-account features");
    let inputs = features::aggregate::AggregateInputs {
        followings: &following,
        followers: &followers,
        close_friends: &close_friends,
        favorited: &favorited,
        blocked: &blocked,
        restricted: &restricted,
        hide_story_from: &hide_story_from,
        recently_unfollowed: &recently_unfollowed,
        removed_suggestions: &removed_suggestions,
        liked_posts: &liked_posts,
        liked_comments: &liked_comments,
        story_likes: &story_likes,
        stories_viewed: &stories_viewed,
        saved_posts: &saved_posts,
        story_polls: &story_polls,
        story_quizzes: &story_quizzes,
        story_questions: &story_questions,
        story_emoji_sliders: &story_emoji_sliders,
        story_emoji_reactions: &story_emoji_reactions,
        story_reaction_stickers: &story_reaction_stickers,
        story_countdowns: &story_countdowns,
        post_comments: &post_comments,
        reels_comments: &reels_comments,
        hype: &hype,
        inbox_threads: &threads,
        message_request_threads: &message_request_threads,
        me: &me,
        resolver: &resolver,
        classifier: &classifier,
        decay: &scoring_config.decay,
    };
    let aggregated = features::aggregate(&inputs, jiff::Timestamp::now());
    let agg_close_friends = aggregated.iter().filter(|f| f.is_close_friend).count();
    let agg_favorited = aggregated.iter().filter(|f| f.is_favorited).count();
    let agg_with_likes = aggregated.iter().filter(|f| f.likes_given > 0).count();
    let agg_with_comments = aggregated.iter().filter(|f| f.comments_given > 0).count();
    let agg_dm_attributed = aggregated
        .iter()
        .filter(|f| f.dm_messages_total > 0)
        .count();
    let agg_dm_reactions_given: u64 = aggregated.iter().map(|f| f.dm_reactions_given as u64).sum();
    let agg_dm_reactions_received: u64 = aggregated
        .iter()
        .map(|f| f.dm_reactions_received as u64)
        .sum();
    let agg_inbound_dm_requests = aggregated.iter().filter(|f| f.inbound_dm_request).count();
    let agg_brands = aggregated
        .iter()
        .filter(|f| f.account_class == features::AccountClass::Brand)
        .count();
    let agg_keeplisted = aggregated.iter().filter(|f| f.is_keeplisted).count();
    let agg_droplisted = aggregated.iter().filter(|f| f.is_droplisted).count();

    if args.verbose > 0 {
        println!("aggregated accounts: {}", aggregated.len());
        println!("aggregated close friends: {agg_close_friends}");
        println!("aggregated favorited: {agg_favorited}");
        println!("aggregated brands: {agg_brands}");
        // The keeplist file may carry handles that aren't followees (a
        // stale entry, or an aspirational keep). The aggregated count
        // reflects only those that intersect the followings set; the
        // file-size line below is the loaded-from-disk count for sanity.
        println!("aggregated keeplisted: {agg_keeplisted}");
        println!("keeplist size on disk: {keeplist_size}");
        // Same file-vs-followee distinction as the keeplist: the on-disk
        // count includes droplist handles that aren't (or are no longer)
        // followees.
        println!("aggregated droplisted: {agg_droplisted}");
        println!("droplist size on disk: {droplist_size}");
        println!("aggregated with likes_given > 0: {agg_with_likes}");
        println!("aggregated with comments_given > 0: {agg_with_comments}");
        println!("DM-attributed accounts: {agg_dm_attributed}");
        println!("DM reactions given total: {agg_dm_reactions_given}");
        println!("DM reactions received total: {agg_dm_reactions_received}");
        println!("inbound DM requests: {agg_inbound_dm_requests}");
    }

    let decayed_dm_messages: f64 = aggregated.iter().map(|f| f.dm_messages_total_decayed).sum();
    let decayed_reactions_received: f64 = aggregated
        .iter()
        .map(|f| f.dm_reactions_received_decayed)
        .sum();
    let likes_90d_total: u64 = aggregated
        .iter()
        .map(|f| u64::from(f.likes_given_90d))
        .sum();
    let comments_90d_total: u64 = aggregated
        .iter()
        .map(|f| u64::from(f.comments_given_90d))
        .sum();
    let reactions_given_180d_total: u64 = aggregated
        .iter()
        .map(|f| u64::from(f.dm_reactions_given_180d))
        .sum();
    let reactions_received_180d_total: u64 = aggregated
        .iter()
        .map(|f| u64::from(f.dm_reactions_received_180d))
        .sum();

    if args.verbose > 0 {
        println!("decayed DM messages sum: {decayed_dm_messages:.2}");
        println!("decayed reactions received sum: {decayed_reactions_received:.2}");
        println!("90d likes total: {likes_90d_total}");
        println!("90d comments total: {comments_90d_total}");
        println!("180d reactions given total: {reactions_given_180d_total}");
        println!("180d reactions received total: {reactions_received_180d_total}");
    }

    progress.phase("Scoring");
    let scored = scoring::score(&aggregated, &scoring_config);
    // Finish before any stdout println — indicatif draws on stderr but
    // a still-ticking spinner can race with the run summary on fast
    // terminals. The write phase is fast (<100ms) and doesn't need its
    // own progress frame.
    progress.finish();

    let keep_count = scored
        .iter()
        .filter(|s| s.bucket == scoring::Bucket::Keep)
        .count();
    let review_count = scored
        .iter()
        .filter(|s| s.bucket == scoring::Bucket::Review)
        .count();
    let unfollow_count = scored
        .iter()
        .filter(|s| s.bucket == scoring::Bucket::Unfollow)
        .count();
    println!("bucket keep: {keep_count}");
    println!("bucket review: {review_count}");
    println!("bucket unfollow: {unfollow_count}");

    print_keep_prob_histogram(&scored);

    // Top/bottom 10 as the human-readable sanity surface. Borrows into
    // `scored` rather than cloning — the full ranking stays a Vec<ScoredAccount>
    // that the output writers (CSV/Markdown/HTML) consume.
    let mut by_prob: Vec<&scoring::ScoredAccount> = scored.iter().collect();
    by_prob.sort_by(|a, b| {
        b.keep_prob
            .partial_cmp(&a.keep_prob)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    println!("top 10 keep candidates:");
    for s in by_prob.iter().take(10) {
        println!(
            "  {}  keep_prob={:.3}  bucket={}  dominant={}",
            s.features.username,
            s.keep_prob,
            s.bucket.as_str(),
            s.dominant_feature,
        );
    }
    println!("bottom 10 unfollow candidates:");
    for s in by_prob.iter().rev().take(10) {
        println!(
            "  {}  keep_prob={:.3}  bucket={}  dominant={}",
            s.features.username,
            s.keep_prob,
            s.bucket.as_str(),
            s.dominant_feature,
        );
    }

    // Labels report (optional). `config/labels.txt` is opt-in — a tuning
    // session that hasn't committed labels yet sees no report and no error.
    match labels::load_default()? {
        Some(label_set) => labels::report(&label_set, &scored),
        None => println!("labels: config/labels.txt not found (accuracy report skipped)"),
    }

    if let Some(handle) = args.trace.as_deref() {
        print_trace(handle, &scored, &scoring_config.weights)?;
    }

    // Output stem is derived from the user-provided INPUT path (not
    // the extracted dir): when input was a .zip, the audit should
    // land next to that zip, not buried inside the cache.
    let stem = resolve_output_stem(args.out.as_deref(), input);
    let paths = output::write(&scored, &stem)?;
    println!("wrote: {}", paths.csv.display());
    println!("wrote: {}", paths.md.display());
    println!("wrote: {}", paths.html.display());

    Ok(())
}

/// Resolve the filename stem the output writer should use. `--out` wins
/// if given (with `with_extension` handling either bare-stem or trailing
/// `.csv` / `.md` symmetrically). Otherwise the default is
/// `following-audit_<YYYY-MM-DD>` placed next to the export directory —
/// DESIGN.md's "Output" contract.
fn resolve_output_stem(cli_out: Option<&std::path::Path>, export_dir: &std::path::Path) -> PathBuf {
    if let Some(p) = cli_out {
        return p.to_path_buf();
    }
    let date = jiff::Zoned::now().date();
    let name = format!("following-audit_{date}");
    // `parent()` on an absolute path goes one directory up; on a relative
    // path it can be `Some("")` (which `File::create` rejects). Fall back
    // to `.` so the default behaviour places the artifact alongside the
    // export folder when run from elsewhere, OR in the cwd when no
    // sibling directory is meaningful.
    let parent = export_dir.parent().filter(|p| !p.as_os_str().is_empty());
    match parent {
        Some(p) => p.join(name),
        None => PathBuf::from(name),
    }
}

/// 10-bucket histogram over `keep_prob`. Buckets are half-open
/// `[i*0.1, (i+1)*0.1)` except the last is `[0.9, 1.0]` so a
/// `keep_prob == 1.0` lands in the rightmost bucket rather than
/// falling off the end (no extra branching at the use sites).
fn print_keep_prob_histogram(scored: &[scoring::ScoredAccount]) {
    let mut counts = [0u32; 10];
    for s in scored {
        let idx = ((s.keep_prob * 10.0).floor() as usize).min(9);
        counts[idx] += 1;
    }
    let max_count = counts.iter().copied().max().unwrap_or(0);
    // Each '█' represents ~max/40 accounts; scale=0 sentinel means "no
    // data" and suppresses the bar entirely. `checked_div` cleanly
    // collapses both the empty-data case and the per-bucket divide.
    let bar_scale = max_count.div_ceil(40);
    println!("keep_prob histogram:");
    for (i, c) in counts.iter().enumerate() {
        let lo = i as f64 / 10.0;
        let hi = (i + 1) as f64 / 10.0;
        let right = if i == 9 { ']' } else { ')' };
        let bar = c
            .checked_div(bar_scale)
            .map(|n| "█".repeat(n as usize))
            .unwrap_or_default();
        println!("  [{lo:.1}, {hi:.1}{right}: {c:>4} {bar}");
    }
}

/// Print every term's signed contribution for one handle, sorted by
/// `|contribution|` so the dominant terms surface first. The handle must
/// be in the scored set; otherwise an error names it so a typo at the
/// command line fails loudly rather than producing an empty trace.
fn print_trace(
    handle: &str,
    scored: &[scoring::ScoredAccount],
    weights: &config::WeightsConfig,
) -> anyhow::Result<()> {
    let acct = scored
        .iter()
        .find(|s| s.features.username == handle)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--trace handle {handle:?} not found in scored followings \
                 (blocked / recently_unfollowed handles are excluded)"
            )
        })?;
    let mut contribs = scoring::term_contributions(&acct.features, weights);
    contribs.sort_by(|a, b| {
        b.1.abs()
            .partial_cmp(&a.1.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    println!(
        "trace for {handle:?}: score_raw={:.3}  keep_prob={:.3}  bucket={}",
        acct.score_raw,
        acct.keep_prob,
        acct.bucket.as_str(),
    );
    for (label, value) in &contribs {
        if *value == 0.0 {
            continue;
        }
        println!("  {label:<28} {value:+.4}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_output_stem_honors_explicit_out() {
        let stem = resolve_output_stem(Some(Path::new("/tmp/custom")), Path::new("/data/export"));
        assert_eq!(stem, PathBuf::from("/tmp/custom"));
    }

    #[test]
    fn resolve_output_stem_default_lands_beside_export_dir() {
        // No --out: the default stem goes in the export's PARENT directory,
        // dated. Pins the `!is_empty` parent filter — a deleted `!` would
        // drop the parent and write into the cwd instead of beside the
        // export.
        let stem = resolve_output_stem(None, Path::new("/data/export"));
        assert_eq!(stem.parent(), Some(Path::new("/data")));
        assert!(
            stem.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("following-audit_"),
            "default stem must be the dated audit name: {}",
            stem.display(),
        );
    }
}

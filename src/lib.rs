//! `ig-mgr` — local-first analysis of an Instagram personal data export.
//!
//! The library crate holds the pipeline; the `ig-mgr` binary ([`main`]) is a
//! thin shell that parses arguments and calls [`run`]. Integration tests in
//! `tests/` drive the same code paths.
//!
//! Pipeline shape (see `docs/DESIGN.md` for the full design):
//!
//! ```text
//! export dir ──▶ export::*  (parse JSON)
//!            ──▶ features    (per-account feature aggregation)
//!            ──▶ scoring     (keep-probability + bucketing)
//!            ──▶ output::*   (CSV + Markdown writers)
//! ```
//!
//! Status: parser layer, feature aggregation, first-pass scoring, CSV +
//! Markdown writers, and the brand / public-figure account-class
//! heuristic (with the user-maintained keep-allowlist override) have all
//! landed. The pipeline composes a `keep_prob` per account, assigns a
//! bucket (`keep` / `review` / `unfollow`) via the DESIGN.md formula plus
//! the restricted / boost / brand / allowlist gates, and writes the
//! CSV + Markdown artifacts next to the export directory.

pub mod allowlist;
pub mod cli;
pub mod config;
pub mod export;
pub mod features;
pub mod labels;
pub mod output;
pub mod scoring;
pub mod text;

use std::path::PathBuf;

use anyhow::Result;

use crate::cli::Cli;

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
        .unwrap_or_else(|_| EnvFilter::new(format!("ig_mgr={default_level}")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// Entry point for the analysis run.
///
/// At this stage the pipeline parses every export source, loads the
/// scoring config, builds the `me` identity and the `display_name → handle`
/// resolver, then runs the feature aggregator and emits a row of smoke
/// counts (per-source totals plus aggregator-level totals for handle-keyed
/// flags, DM signals, decay-weighted sums, and 90d/180d windowed counts).
/// Scoring composition and the CSV / Markdown output writers land in
/// later ROADMAP steps.
pub fn run(cli: Cli) -> Result<()> {
    use anyhow::ensure;

    ensure!(
        cli.export_dir.is_dir(),
        "export directory does not exist or is not a directory: {}",
        cli.export_dir.display()
    );
    export::validate_shape(&cli.export_dir)?;

    let following = export::read_following(&cli.export_dir)?;
    let followers = export::read_followers(&cli.export_dir)?;
    let threads = export::read_inbox(&cli.export_dir)?;
    let total_messages: usize = threads.iter().map(|t| t.messages.len()).sum();

    let close_friends = export::read_close_friends(&cli.export_dir)?;
    let favorited = export::read_favorited(&cli.export_dir)?;
    let blocked = export::read_blocked(&cli.export_dir)?;
    let restricted = export::read_restricted(&cli.export_dir)?;
    let hide_story_from = export::read_hide_story_from(&cli.export_dir)?;
    let recently_unfollowed = export::read_recently_unfollowed(&cli.export_dir)?;
    let removed_suggestions = export::read_removed_suggestions(&cli.export_dir)?;
    let message_request_threads = export::read_message_requests(&cli.export_dir)?;

    let liked_posts = export::read_liked_posts(&cli.export_dir)?;
    let story_likes = export::read_story_likes(&cli.export_dir)?;
    let stories_viewed = export::read_stories_viewed(&cli.export_dir)?;
    let saved_posts = export::read_saved_posts(&cli.export_dir)?;

    let liked_comments = export::read_liked_comments(&cli.export_dir)?;
    let story_polls = export::read_story_polls(&cli.export_dir)?;
    let story_quizzes = export::read_story_quizzes(&cli.export_dir)?;
    let story_questions = export::read_story_questions(&cli.export_dir)?;
    let story_emoji_sliders = export::read_story_emoji_sliders(&cli.export_dir)?;
    let story_emoji_reactions = export::read_story_emoji_reactions(&cli.export_dir)?;
    let story_reaction_stickers = export::read_story_reaction_stickers(&cli.export_dir)?;
    let story_countdowns = export::read_story_countdowns(&cli.export_dir)?;

    let post_comments = export::read_post_comments(&cli.export_dir)?;
    let reels_comments = export::read_reels_comments(&cli.export_dir)?;
    let hype = export::read_hype(&cli.export_dir)?;

    let me = export::read_me_identity(&cli.export_dir)?;
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
    if cli.verbose > 0 {
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

    let scoring_config = config::read_scoring_config(cli.config.as_deref())?;
    let keep_allowlist = allowlist::load_default()?;
    let keep_allowlist_size = keep_allowlist.len();
    let classifier = features::Classifier::new(keep_allowlist);

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
    let agg_keep_allowlisted = aggregated.iter().filter(|f| f.is_keep_allowlisted).count();

    if cli.verbose > 0 {
        println!("aggregated accounts: {}", aggregated.len());
        println!("aggregated close friends: {agg_close_friends}");
        println!("aggregated favorited: {agg_favorited}");
        println!("aggregated brands: {agg_brands}");
        // The allowlist file may carry handles that aren't followees (a
        // stale entry, or an aspirational keep). The aggregated count
        // reflects only those that intersect the followings set; the
        // file-size line below is the loaded-from-disk count for sanity.
        println!("aggregated keep-allowlisted: {agg_keep_allowlisted}");
        println!("keep-allowlist size on disk: {keep_allowlist_size}");
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

    if cli.verbose > 0 {
        println!("decayed DM messages sum: {decayed_dm_messages:.2}");
        println!("decayed reactions received sum: {decayed_reactions_received:.2}");
        println!("90d likes total: {likes_90d_total}");
        println!("90d comments total: {comments_90d_total}");
        println!("180d reactions given total: {reactions_given_180d_total}");
        println!("180d reactions received total: {reactions_received_180d_total}");
    }

    let scored = scoring::score(&aggregated, &scoring_config);
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
    // for the CSV writer to consume in a later slice.
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

    if let Some(handle) = cli.trace.as_deref() {
        print_trace(handle, &scored, &scoring_config.weights)?;
    }

    let stem = resolve_output_stem(cli.out.as_deref(), &cli.export_dir);
    let (csv_path, md_path) = output::write(&scored, &stem)?;
    println!("wrote: {}", csv_path.display());
    println!("wrote: {}", md_path.display());

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

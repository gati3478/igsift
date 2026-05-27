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
//! Status: parser layer complete (relationship-flag, DM, nested-`Owner`
//! activity, shape-A activity, shape-D comment files), plus the slice-6
//! resolver infrastructure ([`features::name_resolution`] + the `me`
//! identity from `personal_information.json`). Feature aggregation,
//! scoring, and output writers remain stubs — see `ROADMAP.md`.

pub mod cli;
pub mod config;
pub mod export;
pub mod features;
pub mod output;
pub mod scoring;

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
/// At this stage the pipeline parses every export source and prints
/// per-source count lines that gate the parser-pass acceptance criteria,
/// plus the `me` identity from `personal_information.json` and a name →
/// handle resolver built from the seven `label_values` files (the bridge
/// that lets DM display names attribute to followed handles in the
/// feature aggregator). Aggregation, scoring, and output writers land in
/// later ROADMAP steps.
pub fn run(cli: Cli) -> Result<()> {
    use anyhow::ensure;

    ensure!(
        cli.export_dir.is_dir(),
        "export directory does not exist or is not a directory: {}",
        cli.export_dir.display()
    );

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
    // resolver change shape.
    let resolvable_dm_threads = threads
        .iter()
        .filter(|thread| {
            let others: Vec<&str> = thread
                .participants
                .iter()
                .filter(|name| name.as_str() != me.name.as_str())
                .map(String::as_str)
                .collect();
            others.len() == 1 && resolver.resolve(others[0]).is_some()
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

    Ok(())
}

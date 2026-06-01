//! Showcase data generator for the README screenshots.
//!
//! Builds a curated, *obviously synthetic* set of accounts, runs them
//! through the real `scoring::score` + `output::write` + `summary::render`
//! path, and emits both the CLI dashboard (ANSI on stdout) and the HTML
//! report (`<stem>.html`). No real handles, no export parsing — only the
//! input `AccountFeatures` are fabricated; everything downstream is the
//! genuine pipeline a user sees.
//!
//! ```bash
//! cargo run --example showcase -- /tmp/igsift-showcase/audit > /tmp/igsift-showcase/dash.ansi
//! ```

use std::path::Path;

use igsift::cli::Preset;
use igsift::config::read_scoring_config;
use igsift::features::{AccountClass, AccountFeatures};
use igsift::output;
use igsift::scoring::score;
use igsift::summary::{RunMeta, render};
use igsift::term_style::Caps;

/// All-zero baseline; archetype builders override the few fields they care
/// about. Mirrors the field set of `AccountFeatures` exactly.
fn base(username: &str, display: &str) -> AccountFeatures {
    AccountFeatures {
        username: username.to_owned(),
        display_name: Some(display.to_owned()),
        account_class: AccountClass::Personal,
        follow_tenure_days: Some(365),
        mutual_age_days: None,
        is_close_friend: false,
        is_favorited: false,
        is_blocked: false,
        is_restricted: false,
        is_hide_story_from: false,
        is_removed_suggestion: false,
        recently_unfollowed: false,
        is_mutual: false,
        is_keeplisted: false,
        is_droplisted: false,
        likes_given: 0,
        comments_given: 0,
        story_interactions_out: 0,
        stories_viewed: 0,
        saved_their_content: 0,
        dm_messages_total: 0,
        dm_recency_days: None,
        dm_balance: None,
        dm_inbound_replies: 0,
        dm_reactions_given: 0,
        dm_reactions_received: 0,
        inbound_dm_request: false,
        likes_given_decayed: 0.0,
        comments_given_decayed: 0.0,
        story_interactions_out_decayed: 0.0,
        stories_viewed_decayed: 0.0,
        saved_their_content_decayed: 0.0,
        dm_messages_total_decayed: 0.0,
        dm_reactions_given_decayed: 0.0,
        dm_reactions_received_decayed: 0.0,
        likes_given_90d: 0,
        comments_given_90d: 0,
        dm_reactions_given_180d: 0,
        dm_reactions_received_180d: 0,
    }
}

/// A close DM relationship: high two-way message volume, recent, mutual.
fn dm_friend(u: &str, d: &str, msgs: f64, tenure: u32, reactions_in: f64) -> AccountFeatures {
    let mut a = base(u, d);
    a.is_mutual = true;
    a.mutual_age_days = Some(tenure.saturating_sub(40));
    a.follow_tenure_days = Some(tenure);
    a.dm_messages_total = (msgs * 60.0) as u32;
    a.dm_messages_total_decayed = msgs;
    a.dm_inbound_replies = (msgs * 26.0) as u32;
    a.dm_balance = Some(0.52);
    a.dm_recency_days = Some(4);
    a.dm_reactions_received = (reactions_in * 10.0) as u32;
    a.dm_reactions_received_decayed = reactions_in;
    a.likes_given = 40;
    a.likes_given_decayed = 0.6;
    a.comments_given_decayed = 0.2;
    a
}

/// A flagged close friend — the close-friends list boost dominates.
fn close_friend(u: &str, d: &str, tenure: u32) -> AccountFeatures {
    let mut a = base(u, d);
    a.is_close_friend = true;
    a.is_mutual = true;
    a.mutual_age_days = Some(tenure.saturating_sub(60));
    a.follow_tenure_days = Some(tenure);
    a.likes_given_decayed = 0.8;
    a.comments_given_decayed = 0.3;
    a.dm_messages_total = 90;
    a.dm_messages_total_decayed = 0.4;
    a.dm_inbound_replies = 40;
    a.dm_balance = Some(0.5);
    a.dm_recency_days = Some(18);
    a
}

/// A favorited account — strong but second-tier boost.
fn favorite(u: &str, d: &str, tenure: u32, likes: f64) -> AccountFeatures {
    let mut a = base(u, d);
    a.is_favorited = true;
    a.is_mutual = true;
    a.mutual_age_days = Some(tenure.saturating_sub(30));
    a.follow_tenure_days = Some(tenure);
    a.likes_given = (likes * 50.0) as u32;
    a.likes_given_decayed = likes;
    a.comments_given_decayed = likes * 0.3;
    a
}

/// A long mutual with little recent activity — kept by the deep-mutual floor.
fn deep_mutual(u: &str, d: &str, tenure: u32) -> AccountFeatures {
    let mut a = base(u, d);
    a.is_mutual = true;
    a.mutual_age_days = Some(tenure.saturating_sub(20));
    a.follow_tenure_days = Some(tenure);
    a.likes_given_decayed = 0.15;
    a.stories_viewed_decayed = 0.4;
    a
}

/// An account you actively engage (likes/comments) but isn't mutual.
fn engaged(u: &str, d: &str, tenure: u32, likes: f64, comments: f64) -> AccountFeatures {
    let mut a = base(u, d);
    a.follow_tenure_days = Some(tenure);
    a.likes_given = (likes * 55.0) as u32;
    a.likes_given_decayed = likes;
    a.comments_given = (comments * 30.0) as u32;
    a.comments_given_decayed = comments;
    a.story_interactions_out_decayed = 0.3;
    a.stories_viewed_decayed = 0.6;
    a
}

/// A brand / business page — content you consume, never personal. Capped at
/// Review by the unfollow gate no matter how low it scores.
fn brand(u: &str, d: &str, tenure: u32, likes: f64, saved: f64) -> AccountFeatures {
    let mut a = base(u, d);
    a.account_class = AccountClass::Brand;
    a.follow_tenure_days = Some(tenure);
    a.likes_given = (likes * 50.0) as u32;
    a.likes_given_decayed = likes;
    a.saved_their_content = (saved * 10.0) as u32;
    a.saved_their_content_decayed = saved;
    a.stories_viewed_decayed = 0.3;
    a
}

/// A dormant one-way follow — followed once, no recent signal. The bulk of
/// the Unfollow bucket.
fn dormant(u: &str, d: &str, tenure: u32, residual: f64) -> AccountFeatures {
    let mut a = base(u, d);
    a.follow_tenure_days = Some(tenure);
    a.likes_given = (residual * 30.0) as u32;
    a.likes_given_decayed = residual;
    a.stories_viewed_decayed = residual * 0.5;
    a
}

/// A muted-story personal account — the hide-story penalty drags it down.
fn muted(u: &str, d: &str, tenure: u32) -> AccountFeatures {
    let mut a = base(u, d);
    a.is_hide_story_from = true;
    a.follow_tenure_days = Some(tenure);
    a.likes_given_decayed = 0.3;
    a
}

// Built incrementally with archetype helpers and loops interleaved, so
// `vec![]` can't express it.
#[allow(clippy::vec_init_then_push)]
fn main() {
    let mut a: Vec<AccountFeatures> = Vec::new();

    // ── Hero keeps: real relationships, named, top of the dashboard ──────
    a.push(dm_friend("maya.renteria", "Maya Rentería", 2.4, 2600, 0.9));
    a.push(dm_friend("deniz.aydn", "Deniz Aydın", 2.1, 1900, 0.7));
    a.push(dm_friend(
        "theo.lindqvist",
        "Theo Lindqvist",
        1.8,
        1450,
        0.6,
    ));
    a.push(dm_friend("priya.n", "Priya Nair", 1.6, 980, 0.8));
    a.push(close_friend("luca.bianchi", "Luca Bianchi", 3100));
    a.push(close_friend("sara.okonkwo", "Sara Okonkwo", 2200));
    a.push(close_friend("noah.feldman", "Noah Feldman", 1700));
    a.push(favorite("amelie.dubois", "Amélie Dubois", 2400, 1.3));
    a.push(favorite("kenji.watanabe", "Kenji Watanabe", 1300, 1.0));
    a.push(dm_friend("rosa.mendez", "Rosa Méndez", 1.2, 760, 0.5));

    // ── Solid keeps: engagement + mutual mix ────────────────────────────
    let mids = [
        ("ines.carvalho", "Inês Carvalho", 1500u32, 1.4f64, 0.5f64),
        ("omar.haddad", "Omar Haddad", 900, 1.6, 0.6),
        ("greta.svenson", "Greta Svenson", 2000, 1.1, 0.3),
        ("tomas.varga", "Tomáš Varga", 1100, 1.3, 0.4),
        ("yuki.tanaka", "Yuki Tanaka", 800, 1.5, 0.7),
        ("hana.kovac", "Hana Kovač", 1600, 1.0, 0.2),
        ("milo.fischer", "Milo Fischer", 700, 1.7, 0.5),
        ("aria.kapoor", "Aria Kapoor", 1300, 1.2, 0.4),
    ];
    for (u, d, t, l, c) in mids {
        a.push(engaged(u, d, t, l, c));
    }

    // Deep mutuals kept on history alone.
    a.push(deep_mutual("dad.travels", "Eduardo R.", 3600));
    a.push(deep_mutual("the.wewers", "Hannah Weber", 2900));
    a.push(deep_mutual("old.flatmate.j", "Jonas K.", 2400));

    // ── Review: brands + judgment calls near the 50% line ───────────────
    a.push(brand(
        "tbilisicamerashop",
        "Tbilisi Camera Shop",
        1400,
        0.9,
        0.7,
    ));
    a.push(brand("nomad.coffee.co", "Nomad Coffee Co.", 1100, 0.7, 0.5));
    a.push(brand("kinfolk", "Kinfolk", 1800, 0.5, 0.4));
    a.push(brand("a24", "A24", 1500, 0.6, 0.3));
    a.push(brand("studio.kestrel", "Studio Kestrel", 900, 0.4, 0.6));
    a.push(brand("left.bank.books", "Left Bank Books", 1200, 0.5, 0.5));
    let reviews = [
        (
            "camille.fontaine",
            "Camille Fontaine",
            600u32,
            0.7f64,
            0.3f64,
        ),
        ("dario.ricci", "Dario Ricci", 1000, 0.6, 0.4),
        ("lena.brandt", "Lena Brandt", 850, 0.8, 0.2),
        ("samir.qadir", "Samir Qadir", 500, 0.5, 0.5),
        ("nina.holm", "Nina Holm", 1300, 0.6, 0.3),
        ("paulo.ferreira", "Paulo Ferreira", 700, 0.7, 0.2),
    ];
    for (u, d, t, l, c) in reviews {
        a.push(engaged(u, d, t, l, c));
    }
    // A restricted account — floored to Review, hint calls it out.
    let mut r = engaged("blocked.ex.acct", "—", 1100, 0.2, 0.0);
    r.is_restricted = true;
    r.display_name = None;
    a.push(r);

    // ── Unfollow: dormant one-way follows + muted stories ───────────────
    // Tenure is kept short here on purpose: a long-tenure zero-engagement
    // follow floats up to ~0.35 keep_prob on the tenure term alone and lands
    // in Review (faithful to the algorithm). The genuine drops are recent
    // follows you already regret, plus penalty-driven cases below.
    let dormants = [
        ("throwback.2014", "", 150u32, 0.0f64),
        ("random.meme.page", "daily lols", 130, 0.04),
        ("that.one.conference", "DevConf EU", 220, 0.0),
        ("ex.coworker.bob", "Bob T.", 180, 0.03),
        ("hostel.barcelona", "Sant Jordi Hostels", 240, 0.0),
        ("forgot.who.this.is", "", 110, 0.0),
        ("startup.that.died", "Layra (closed)", 260, 0.0),
        ("gym.i.quit", "PowerHouse Fitness", 170, 0.05),
        ("travel.inspo.99", "Wanderlust Daily", 200, 0.06),
        ("old.uni.acquaintance", "Marek W.", 95, 0.02),
        ("dropshipping.store", "LuxeFinds", 80, 0.0),
        ("influencer.i.dont.recall", "Bella M.", 140, 0.04),
        ("crypto.bro.2021", "", 230, 0.0),
        ("food.truck.gone", "El Camión", 190, 0.0),
        ("photographer.met.once", "Andrei P.", 160, 0.03),
    ];
    for (u, d, t, res) in dormants {
        a.push(dormant(u, d, t, res));
    }
    a.push(muted("loud.opinions.daily", "—", 500));
    a.push(muted("constant.story.poster", "Vlog Life", 420));
    // Removed-from-suggestions, recent, nothing back — clean drops.
    for (u, d, t) in [
        ("spam.followback.4u", "FOLLOW BACK ⚡", 90u32),
        ("giveaway.bot.x", "WIN AN IPHONE", 70),
    ] {
        let mut m = dormant(u, d, t, 0.0);
        m.is_removed_suggestion = true;
        a.push(m);
    }

    // ── Fill: a believable long tail across all buckets ─────────────────
    let tail_first = [
        "ana", "ben", "cleo", "dan", "eva", "finn", "gaia", "hugo", "iris", "joel", "kira", "leo",
        "mara", "nico", "ola", "pia", "rafa", "sven", "tara", "uma", "vera", "wes", "xan", "yara",
        "zoe", "remy", "esme", "kai", "lior", "noor",
    ];
    let tail_last = [
        "park", "nguyen", "silva", "ahmed", "rossi", "kim", "patel", "lopez", "chen", "muller",
        "novak", "costa", "haas", "ivanov", "reyes",
    ];
    let mut i = 0usize;
    for (fi, fl) in tail_first.iter().enumerate() {
        // Rotate the surname window per first name so no last name clusters
        // in the top/bottom cards.
        for k in 0..3 {
            let ll = tail_last[(fi * 3 + k) % tail_last.len()];
            let u = format!("{fl}.{ll}");
            let d = format!(
                "{}{} {}{}",
                fl[..1].to_uppercase(),
                &fl[1..],
                ll[..1].to_uppercase(),
                &ll[1..]
            );
            // Spread across archetypes deterministically (no RNG). Tail
            // accounts stay weaker than the ten named heroes so those own the
            // Top-keeps card; tenure varies by index to break score ties.
            let t = 700 + (i as u32 % 9) * 130;
            let acct = match i % 6 {
                0 => engaged(&u, &d, t, 1.1, 0.3),
                1 => dm_friend(&u, &d, 0.6 + (i as f64 % 3.0) * 0.05, t, 0.25),
                2 => dormant(&u, &d, 100 + (i as u32 % 19) * 8, 0.02 * (i as f64 % 4.0)),
                3 => brand(&u, &d, t, 0.5, 0.4),
                4 => favorite(&u, &d, t, 0.7),
                _ => engaged(&u, &d, t - 200, 0.6, 0.2),
            };
            a.push(acct);
            i += 1;
        }
    }

    // ── Real pipeline: balanced preset → score → render + write ─────────
    let cfg = read_scoring_config(None, Some(Preset::Balanced)).expect("balanced preset");
    let scored = score(&a, &cfg);

    let caps = Caps {
        color: true,
        unicode: true,
        width: 92,
    };
    let meta = RunMeta {
        total: scored.len(),
        config_label: "balanced preset",
        date: jiff::civil::date(2026, 6, 1),
    };
    render(&scored, &meta, &caps);

    let stem = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "showcase".to_owned());
    output::write(&scored, Path::new(&stem)).expect("write audit");
    eprintln!("\nwrote {stem}.{{csv,md,html}} — {} accounts", scored.len());
}

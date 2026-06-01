# Design — `igsift`

The full design for the Instagram following-cleanup CLI. Status, build, and the
short pitch live in the [README](../README.md); the task list in
[ROADMAP.md](../ROADMAP.md). The full pipeline is implemented and tuned:
parsers, feature aggregation, decay-weighted scoring with the relationship
gates (deep-mutual keep-floor, reciprocity keep-ceiling) and the effort-skew
gate, the brand/public-figure account-class heuristic, the keeplist/droplist
overrides, and the CSV/Markdown/HTML writers. The weight/decay calibration
journal is [`docs/TUNING.md`](TUNING.md) (9 rounds through 2026-06-01).

## Inputs

> **Schema validated 2026-05-26** against the 2026-05-11 personal export by
> walking every JSON file with [`scripts/walk_export_schema.sh`](../scripts/walk_export_schema.sh).
> The paths, shapes, and field names below match what Instagram actually
> ships today. Re-run the walker after every new export to detect drift.

The full Instagram "Download Your Information" export in **JSON** format,
unzipped and merged into one root. Instagram chunks large exports by ~2 GB
file-size budget across multiple zips; the same DM thread folder can appear
in multiple chunks with disjoint files inside (JSON metadata in one chunk,
media in others). Merge with `rsync -a chunk/ merged/` for each chunk —
files are unioned without conflict.

### Files we consume

| Path                                                                                                                                                        | Shape group                            | Used for                                                        |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | --------------------------------------------------------------- |
| `connections/followers_and_following/following.json`                                                                                                        | **A** (wrapped + flat)                 | the set we score, **plus** per-account follow-tenure timestamp  |
| `connections/followers_and_following/followers_*.json` †                                                                                                    | **B** (bare array + flat)              | follower set (mutual-follow detection)                          |
| `connections/followers_and_following/close_friends.json`                                                                                                    | **C** (`label_values`)                 | hard `keep` boost                                               |
| `connections/followers_and_following/profiles_you've_favorited.json`                                                                                        | **C**                                  | hard `keep` boost (distinct tier from close_friends)            |
| `connections/followers_and_following/blocked_profiles.json`                                                                                                 | **C**                                  | hard exclude from set                                           |
| `connections/followers_and_following/restricted_profiles.json`                                                                                              | **C**                                  | floor bucket to `review` minimum                                |
| `connections/followers_and_following/hide_story_from.json`                                                                                                  | object, single-entry                   | weak negative                                                   |
| `connections/followers_and_following/recently_unfollowed_profiles.json`                                                                                     | **C**                                  | exclude from set (already dropped)                              |
| `connections/followers_and_following/removed_suggestions.json`                                                                                              | **C**                                  | very weak negative (PYMK dismissals)                            |
| `your_instagram_activity/messages/inbox/<thread>/message_*.json` ‡                                                                                          | thread-specific (see below)            | DM volume, recency, direction, **reactions in both directions** |
| `your_instagram_activity/messages/message_requests/<thread>/...`                                                                                            | same as inbox                          | weak inbound interest (DM attempts from non-followings)         |
| `your_instagram_activity/likes/liked_posts.json`                                                                                                            | **C** with nested `Owner.dict[0].dict` | likes I gave (target = Owner.Username)                          |
| `your_instagram_activity/likes/liked_comments.json`                                                                                                         | **A**                                  | comment-likes I gave (target = `title`)                         |
| `your_instagram_activity/comments/post_comments_1.json` §                                                                                                   | **D** (`string_map_data`)              | comments I left on posts                                        |
| `your_instagram_activity/comments/reels_comments.json`                                                                                                      | **A** wrapping **D**-shaped entries    | comments I left on reels                                        |
| `your_instagram_activity/comments/hype.json`                                                                                                                | **A** wrapping **D**-shaped entries    | comments I left on stories                                      |
| `your_instagram_activity/story_interactions/{polls,quizzes,questions,emoji_sliders,emoji_story_reactions,story_reaction_sticker_reactions,countdowns}.json` | **A**                                  | outbound story engagement (target = `title`)                    |
| `your_instagram_activity/story_interactions/{story_likes,stories_viewed}.json`                                                                              | **C** with nested `Owner`              | outbound story likes / passive view tracking                    |
| `your_instagram_activity/saved/saved_posts.json`                                                                                                            | **C** with nested `Owner`              | saves of someone's content                                      |

† Followers are numbered (`followers_1.json`, `_2`, `_3` for larger accounts) — glob.
‡ DM thread schema: `{messages:[{sender_name, timestamp_ms, content?, reactions:[{reaction, actor}], photos?, videos?, share?, ...}], participants:[{name}], thread_path, title, ...}`.
§ Post comments are numbered (`post_comments_1.json`, `_2`, …) for high-volume accounts — glob.

### Files we deliberately skip

- `messages/broadcast/<channel>/*` — pub/sub channels (other user publishes, I subscribe); not a 1:1 relationship.
- `messages/ai_conversations.json` — chats with the AI assistant.
- `messages/secret_conversations.json` — E2EE chats; only `armadillo_devices` / `calls` metadata, no content.
- `your_instagram_activity/threads/*` — the _Threads_ social network app, not IG.
- `your_instagram_activity/{events,monetization,other_activity,shopping,subscriptions}/` — no per-account signal.
- `personal_information/personal_information/note_and_repost_interactions.json` — ships exactly one entry with no content, no timestamp, ambiguous direction. Too sparse to be useful.
- `connections/contacts/synced_contacts.json` — phone contacts (244 entries). Possible v2 "IRL overlap" join; not in v1.
- `media/posts/<YYYYMM>/*.jpg` — my own uploaded media. No metadata JSON ships alongside — so no caption, no tagged users.

### Signals the export does NOT ship (dropped from spec)

Schema validation killed four features that earlier drafts of this doc assumed
existed. Their source files simply aren't in the personal export:

- **`searched_for_them`** — no `profile_searches.json` exists anywhere.
- **`tagged_them`** — no post/reel/story metadata; `media/posts/` has only raw JPGs.
- **`they_tagged_me`** — no `archived_posts/` either.
- **`story_interactions_in`** — every `story_interactions/*.json` file has me as
  the actor; `title` and nested `Owner` are always the story owner (them).

Inbound follow requests are also absent: there is **no** `follow_requests_you've_received.json`. Instagram exports `pending_follow_requests.json` and `recent_follow_requests.json` but both are **outbound** (mine, awaiting target approval).

### Partial observability — narrower than it looks

Instagram doesn't ship who liked _my_ posts, who commented on _my_ posts, or
who reacted to _my_ stories. Reciprocity has to be inferred from indirect
evidence — but it's not zero. The export **does** ship one true inbound
channel:

**DM `reactions[].actor`**. Each message in `messages/inbox/<thread>/message_*.json`
carries a `reactions` array; when `actor != me`, that record is _them reacting
to one of my messages_. Both directions are visible. This is the single most
valuable bidirectional signal in the export and underpins `dm_reactions_received`.

`messages/message_requests/` (10 threads in this export) is also inbound — DM
attempts from accounts I never accepted. Weak signal but real.

### DM `display_name ↔ handle` bridge — 37% coverage by design

DM threads ship `participants[].name` and `messages[].sender_name` as **display
names**, never handles, while every other source keys by handle. The
aggregator joins them via the seven `label_values` files (`close_friends`,
`profiles_you've_favorited`, `blocked_profiles`, `restricted_profiles`,
`recently_unfollowed_profiles`, `removed_suggestions`, `hide_story_from`) —
each entry carries both a `Name` and a `Username` label at the outer
`label_values` level. Recon on the 2026-05-11 export:

- **281 unique `(Name, Username)` pairs** across the seven files.
- **12 name collisions** — `"Mike"` → {`bermudalckt`, `hairycub81`,
  `leahcim333`} and similar. The resolver returns `None` on collision
  rather than guessing; misattribution is worse than missing attribution.
- **217/581 (37%) of 1:1 DM threads** resolve to a followee handle under
  this strict policy. The remaining 63% of followings score from
  activity-side features (handle-keyed at source) only — `dm_*` features
  are sparse by design, not by bug.

The DM thread folder name `<inbox>/<thread>/` looks like a bridge but
isn't: validation showed the prefix is the participant's display name
sanitized (lowercased, spaces stripped), not the handle. Only 3 of 30
sampled followings appeared as folder prefixes — those were coincidences
where display name happens to equal handle. Don't read the folder.

Group chats (≥ 3 participants — 9 threads in this export) and abandoned
threads (only `me` as participant — 3 threads) are dropped from `dm_*`
aggregation: per-participant attribution in groups doesn't fit the 1:1
feature model and group counts are weak signal in v1.

`me.name` (`"Gati Petriashvili"`) comes from
`personal_information/personal_information/personal_information.json`
→ `profile_user[0].string_map_data["Name"].value`. Missing or empty here
is a HARD ERROR — every DM-direction classification depends on it.

### Parsing notes

These bit the validation pass and will bite the parser too — call them out
explicitly so `src/export.rs` is designed for them:

- **Four distinct JSON entry shapes** across activity files. Per-file
  deserializer, not a single generic struct:
    - **A** — wrapped + flat: `{wrapper_key: [{title, string_list_data:[{href, timestamp}]}]}`
    - **B** — bare array + flat: `[{title, media_list_data, string_list_data:[{href, value, timestamp}]}]`
    - **C** — bare array + `label_values` with optional nested `dict.dict` (used for `Owner`): `[{fbid, timestamp, label_values:[{label, value, …}], media:[]}]`
    - **D** — `string_map_data` dict: `{string_map_data: {<FieldName>: {value, timestamp, href}}}`
- **Wrapper keys are inconsistent** — codified file-by-file as a private
  struct in `src/export.rs`. The full known set (validated 2026-05-27):

    | File                                                       | Wrapper key                                   |
    | ---------------------------------------------------------- | --------------------------------------------- |
    | `following.json`                                           | `relationships_following`                     |
    | `likes/liked_comments.json`                                | `likes_comment_likes`                         |
    | `comments/reels_comments.json`                             | `comments_reels_comments` †                   |
    | `comments/hype.json`                                       | `comments_story_comments` †                   |
    | `story_interactions/polls.json`                            | `story_activities_polls`                      |
    | `story_interactions/quizzes.json`                          | `story_activities_quizzes`                    |
    | `story_interactions/questions.json`                        | `story_activities_questions`                  |
    | `story_interactions/emoji_sliders.json`                    | `story_activities_emoji_sliders`              |
    | `story_interactions/emoji_story_reactions.json`            | `story_activities_emoji_quick_reactions` ‡    |
    | `story_interactions/story_reaction_sticker_reactions.json` | `story_activities_reaction_sticker_reactions` |
    | `story_interactions/countdowns.json`                       | `story_activities_countdowns`                 |

    † Shape D entries wrapped in shape A — parsed in a separate slice.
    ‡ File name and wrapper key are NOT symmetric. The file says
    `emoji_story_reactions` but the wrapper says `emoji_quick_reactions`
    — keep the constant explicit so this asymmetry does not surprise
    future maintenance.

- **`Owner` extraction** is three levels deep on shape **C**:
  `label_values[?.title == "Owner"].dict[0].dict[?.label == "Username"].value`.
- **Timestamps are inconsistent**: most files use Unix **seconds** in
  `timestamp`; DM messages use **milliseconds** in `timestamp_ms`; a few use
  `timestamp_value`. Normalize at the parse boundary into `jiff::Timestamp`.
- **Multi-part DM threads**: 14 of 593 threads span 2–10 `message_*.json` files
  (e.g., `validoli` has 10 parts). Concatenate all parts per thread before
  computing features, or the largest conversations silently truncate.
- **Message-like shadows**: a double-tap message like is serialized **twice** —
  in the target message's `reactions[]` AND as a standalone message with
  `content == "Liked a message"` from the reactor. The aggregator excludes that
  shadow from `dm_messages_total` / `dm_balance` / `dm_inbound_replies` (the
  reaction is the canonical record, already counted) — see
  `aggregate::LIKE_SHADOW_CONTENT`. 31,155 such shadows across 394 of 594
  threads in the validated export; counting them as messages was masking
  one-sidedness in `dm_balance`.

## Algorithm sketch

Score each followed account on `keep_probability ∈ [0, 1]`. **Scope:** the
ranking covers accounts **you follow** (643 in the 2026-05-11 export).
"Remove from followers" is the manual companion action — when you unfollow a
mutual you also drop them as a follower — not a separate ranked set. One list,
one decision per account.

### Per-account features

| Feature                  | Source                                              | Direction / handling                                           | Weight (initial)                          |
| ------------------------ | --------------------------------------------------- | -------------------------------------------------------------- | ----------------------------------------- |
| `dm_messages_total`      | inbox `<thread>` messages                           | outbound + inbound, log-scaled                                 | high                                      |
| `dm_recency_days`        | last `timestamp_ms` in thread                       | recency enters via decayed counts; field surfaces in CSV only  | (no separate weight)                      |
| `dm_balance`             | outbound / (outbound + inbound) message count       | penalize one-sided threads (post-shadow-dedup)                 | medium                                    |
| `dm_inbound_replies`     | inbox `<thread>` real messages (shadows excluded)   | the other party's real replies, not taps; effort-skew evidence | (gate evidence, not scored)               |
| `dm_reactions_given`     | `reactions[?actor == me]`                           | log-scaled, recency-weighted                                   | medium                                    |
| `dm_reactions_received`  | `reactions[?actor != me]` — **inbound** reciprocity | log-scaled, recency-weighted                                   | medium-high                               |
| `dm_reaction_balance`    | given / (given + received)                          | penalize one-sided reactions                                   | low-medium                                |
| `inbound_dm_request`     | thread present in `message_requests/`               | boolean                                                        | low keep-bias                             |
| `likes_given`            | `liked_posts` + `liked_comments`                    | log-scaled, recency-weighted                                   | medium                                    |
| `comments_given`         | `post_comments_*` + `reels_comments` + `hype`       | log-scaled, recency-weighted                                   | medium                                    |
| `story_interactions_out` | all `story_interactions/*` aggregated               | log-scaled, recency-weighted                                   | medium                                    |
| `stories_viewed`         | `stories_viewed.json`                               | log-scaled, recency-weighted                                   | low                                       |
| `saved_their_content`    | `saved_posts.json`                                  | log-scaled                                                     | low                                       |
| `follow_tenure_days`     | `following.json` per-account `timestamp`            | `log(days_since_follow + 1)`                                   | low                                       |
| `is_close_friend`        | `close_friends.json`                                | boolean                                                        | hard boost                                |
| `is_favorited`           | `profiles_you've_favorited.json`                    | boolean                                                        | hard boost (separate from close_friend)   |
| `blocked`                | `blocked_profiles.json`                             | boolean                                                        | **excludes from input set**               |
| `is_restricted`          | `restricted_profiles.json`                          | boolean                                                        | floor bucket to `review`                  |
| `is_hide_story_from`     | `hide_story_from.json`                              | boolean                                                        | weak negative                             |
| `is_removed_suggestion`  | `removed_suggestions.json`                          | boolean                                                        | very weak negative                        |
| `recently_unfollowed`    | `recently_unfollowed_profiles.json`                 | boolean                                                        | **excludes from input set**               |
| `account_class`          | username/name heuristic (below)                     | personal / brand (PublicFigure deferred — see heuristic below) | gates the `unfollow` recommendation       |
| `is_keeplisted`          | `config/keeplist.txt`                               | boolean                                                        | parallel Unfollow→Review override         |
| `is_droplisted`          | `config/droplist.txt`                               | boolean                                                        | forces → Unfollow (below `is_restricted`) |

**Decay.** Every interaction count is recency-weighted with exponential decay
so a 2019 like is worth far less than a 2026 like. τ is configurable; start
≈ 180 days for DMs, 365 for content interactions.

### Account-class heuristic (gates "suggest unfollow")

Lives in [`src/features/account_class.rs`](../src/features/account_class.rs);
`Classifier` is built once per run from the user-maintained keeplist and
threaded into the aggregator via `AggregateInputs`.

- **Lexicon match.** Username AND display name are case-insensitively
  substring-matched against `BRAND_LEXICON` — a curated 16-token list
  (`official`, `studio`, `magazine`, `records`, `gallery`, `news`, `media`,
  `agency`, `books`, `press`, `games`, `store`, `comics`, `zine`, `shop`,
  `cafe`) using **`aho-corasick`** (single automaton, one pass over each
  input). Floor is 4 chars; the rule is **empirical 0-false-positives
  against the real export's followee list**, not a length cutoff per se.
  **Adding a token under 5 chars requires both** (a) the 0-FP grep
  against the export AND (b) a plausibility check that the substring
  isn't a common personal-handle root in English, Georgian, or Russian
  (the project's user language mix). When (b) is in doubt, defer the
  token until word-boundary matcher semantics is on the table — the
  `bar` / `art` deferrals model the right discipline: each was rejected
  on plausibility grounds (`bardic.cub`, `barbara`, `martin`, `bart`)
  before the FP grep was even run. 3-char tokens are uniformly deferred
  pending word-boundary semantics regardless of how clean the FP grep
  looks on the current export. False positives are
  costlier here than false negatives — a missed brand stays Personal and
  remains eligible for the close_friend / favorited / keeplist gates,
  whereas a falsely-flagged brand silently suppresses a real Unfollow
  recommendation. Per-token audit lives in
  [`docs/TUNING.md`](TUNING.md) round 4.
- **`AccountClass` variants.** `Personal` (default) and `Brand` (lexicon hit
  on either surface). `PublicFigure` is **deliberately omitted** from the
  variant set: the text-only heuristic can't reliably distinguish brand from
  public_figure, and the downstream gating is identical (block Unfollow,
  surface as Review). Adding a variant the aggregator can't populate would
  be a lie about what it knows; if a future labeled set proves the distinction
  matters, add the variant then.
- **Keeplist override.** [`config/keeplist.txt`](../config/keeplist.txt)
  is a separate user-maintained list of handles that must never bucket as
  Unfollow — brands the lexicon misses, public figures, and personal accounts
  the export under-represents (sparse signal, out-of-band relationship). The
  keeplist is **NOT** classification: a personal close-friend the user
  keeplists stays `Personal` at the `AccountClass` level so the CSV column
  doesn't misrepresent the profile. The override surfaces as a separate
  `is_keeplisted: bool` on `AccountFeatures`, parallel to
  `is_close_friend` / `is_favorited`. Scoring's `assign_bucket` folds both
  signals into the Unfollow gate: `account_class != Personal ||
is_keeplisted` downgrades to Review.
- **Droplist override.** [`config/droplist.txt`](../config/droplist.txt)
  is the exact inverse of the keeplist: a user-maintained list of
  handles forced to `unfollow` regardless of score or keep-signals. It
  exists because keep/drop intent is not separable on the current features
  (see [`TUNING.md`](TUNING.md) round 5) — the keeplist handles the
  low-engagement-keep failure mode, the droplist handles the story-heavy
  drop that scores into `keep`. Like the keeplist it is **NOT**
  classification (a droplisted handle keeps its real `account_class`). It
  surfaces as `is_droplisted: bool` on `AccountFeatures`; `assign_bucket`
  applies it as the rung directly below the `is_restricted` floor (the one
  signal it yields to). A handle on both lists is rejected at load
  (`lists::ensure_disjoint`).
- **Honest uncertainty.** Follower count cannot be inferred from the export,
  so the heuristic is limited to name patterns + keeplist. The text-only
  surface is inherently lossy. **If uncertain, never auto-suggest unfollow —
  the gate fires conservatively (Brand OR keeplisted → Review).**

### Scoring composition (initial form, iterate empirically)

```
engagement     = w_dm·dm_messages + w_likes·likes_given + w_comments·comments_given
               + w_story_out·story_interactions_out + w_stories_viewed·stories_viewed
               + w_saved·saves_of_their_content + w_reactions_given·dm_reactions_given
reciprocity    = w_reactions_received·dm_reactions_received
tenure         = w_tenure·log(follow_tenure_days + 1)
boosts         = (is_close_friend ? w_close_friend_boost : 0)
               + (is_favorited   ? w_favorite_boost     : 0)
               + (inbound_dm_request ? w_inbound_request : 0)
penalties      = w_dm_balance_penalty·dm_balance_penalty
               + w_reaction_balance_penalty·reaction_balance_penalty
               + (is_hide_story_from   ? w_hide_story_penalty   : 0)
               + (is_removed_suggestion ? w_removed_suggestion_penalty : 0)
               + (nonmutual_close_tie ? w_nonmutual_close_tie_penalty : 0)
score_raw      = engagement + reciprocity + tenure + boosts - penalties
keep_prob      = sigmoid((score_raw - threshold) / scale)
```

Every key in [`config/scoring.toml`](../config/scoring.toml) `[weights]` appears
exactly once above. `nonmutual_close_tie` is the predicate `account_class ==
Personal && !mutual && (is_close_friend || is_favorited) && !is_keeplisted` —
an explicit relationship marker the followee never reciprocated with a
follow-back (the mirror-inverse of the reciprocity ceiling below). Recency
enters through the exponential decay applied to
each count (`[decay]` τ constants), not as an additive term — `dm_recency_days`
is materialized on `AccountFeatures` for the CSV (`last_dm_days`) but never
fed into `score_raw`. Weights and constants live in TOML so they can be
tuned without a rebuild.

#### Balance-penalty form (volume-gated, asymmetric)

`dm_balance_penalty` and `reaction_balance_penalty` are not stored on
`AccountFeatures` — they're derived in `src/scoring.rs` from the raw
`dm_balance` ratio (and from the raw `dm_reactions_given` /
`dm_reactions_received` counts for the reaction variant) so the volume-
gating policy lives next to the weight rather than baked into the
aggregator:

```
volume_gate(count, k)  = (count >= k)         # k = 5 today, tunable later
balance_penalty(ratio) = max(0, ratio - 0.5) * 2   ∈ [0, 1]
```

Asymmetric on purpose: `balance = 1.0` (fully one-sided me) returns `1.0`,
the full penalty; `balance = 0.0` (fully one-sided them) returns `0.0`.
One-sided-them is reciprocity in the inbound direction, not over-extension
— `reactions_received` scores it through its own weight. The volume gate
prevents 1-message threads with `balance = 1.0` from dominating: a fresh
thread isn't a relationship signal yet.

#### Top signal label (dominant feature)

The "top signal" — the `dominant_feature` on `ScoredAccount` — is the term
with the largest **signed** contribution to `score_raw`: positive engagement
terms compete in their natural sign, while penalty terms enter the comparison
as **negative** of their `weight * value` product. This surfaces
"hide_story_penalty" or "dm_balance_penalty" by name when a penalty
dominates, rather than burying the negative driver under a smaller
positive term. Labels match the corresponding `WeightsConfig` field
(and `[weights]` TOML key) verbatim so a call-out traces back to one
line of code and one line of config. It is materialized into the CSV
`top_signal` column (renamed from `notes` in the v2 header) and the
Markdown "top signal" table column / card "Why" line.

### Buckets

- `keep` (`keep_prob ≥ 0.7`) — solid two-way relationship or boosted account.
- `review` (`0.3 ≤ keep_prob < 0.7`) — ambiguous; needs my eyes.
- `unfollow` (`keep_prob < 0.3` **and** `account_class == personal` **and** not
  `is_close_friend`/`is_favorited`/`is_restricted`) — confident recommendation.

`blocked` and `recently_unfollowed` filter the input set **entirely** — they
never appear in output. Public figures / brands with low `keep_prob` get
`review`, never `unfollow` — that decision criterion ("do I still care about
their content?") is different and out of scope for v1.

#### Bucket precedence (top wins)

`assign_bucket` resolves overrides in a fixed order. Two user-maintained
lists bracket the inferred score:

```
1. is_restricted        → Review     hard floor; the manual "look first" signal
2. is_droplisted       → Unfollow   config/droplist.txt — forces drop
3. effort-skew HARD     → Review     !keeplisted & DM evidence & reply_skew ≥ effort_skew_hard
                                      — overrides close-friend / favorite / mutual + the deep-mutual floor
4. deep-mutual floor    → Keep        mutual & mutual_age_days ≥ deep_mutual_keep_days
5. keep_prob ≥ keep_min → Keep        …unless effort-skew SOFT (unmarked & DM evidence &
                                      reply_skew ≥ effort_skew_soft), the non-reciprocal close-tie
                                      gate (personal & !mutual & marked & !keeplisted), the
                                      dead-mutual gate (personal & mutual & no DM either way &
                                      no inbound & ≤1 like/comment_90d & tenure <
                                      dead_mutual_review_max_tenure_days), or the
                                      reciprocity gate fires → Review
6. keep_prob < unfollow_max:
     is_close_friend | is_favorited | is_keeplisted | non-Personal → Review
     else → Unfollow
7. otherwise            → Review
```

`is_restricted` floors at `review` even when `keep_prob` is below
`unfollow_max` — and it is the **one** floor the droplist yields to
("restricted" means human attention is required before any drop call, which
outranks a standing drop intent). The **droplist**
([`config/droplist.txt`](../config/droplist.txt)) is the exact inverse of
the keeplist: a listed handle is forced to `unfollow` regardless of
score, close-friend/favorited boosts, or brand class. A handle present in
**both** lists is a contradiction and is rejected at load
(`lists::ensure_disjoint`) before scoring, so rung 2 never competes with
rung 6's keeplist gate by construction.

The **effort-skew gate** (rungs 3 + 5) is evidence-guarded: both tiers only act
inside a 1:1 DM thread the owner invested in (`my_dm_out ≥ effort_skew_min_dm_out`,
where `my_dm_out` = post-dedup outbound messages). `effort_skew_min_dm_out = 0`
disables it. It is monotonic (Keep → Review only, never Unfollow) and is the
evidence-based successor to the `require_reciprocity_for_keep` ceiling — acting
only where Instagram actually exports both sides of the conversation, so it never
touches relationships that live off-platform. Full design:
[`docs/specs/2026-05-31-effort-skew-gate-design.md`](specs/2026-05-31-effort-skew-gate-design.md).

**Inert-account floor.** A personal account reaching the Unfollow band purely
for lack of positive signal — zero engagement in any direction, no DM, no
reactions, no inbound, and no negative owner action (`hide_story` /
`removed_suggestion`) — is floored to **Review**, not Unfollow. Tenure is not
a drop signal: an account you have never interacted with is an absence of
evidence, not evidence to drop. `__deleted__` accounts are exempt (gone =
safe, certain drop). Config: `floor_inert_to_review` (default on); monotonic,
Review-only. See `docs/specs/2026-06-01-inert-account-floor-design.md`.

**Three relationship gates bracket the score (rungs 4–5), each monotonic — each
can only move an account one direction, so none can manufacture a wrongful
`unfollow`.** They encode the core principle that _keep = relationship, not
consumption_:

- **Deep-mutual keep-floor** (rung 4, `deep_mutual_keep_days`, default 730):
  a mutual account whose **reciprocal age** (`mutual_age_days` — days since the
  later of {you followed them, they followed you back}) is ≥ the threshold
  floors to `keep`. A long two-way history is a real relationship worth keeping
  even with no recent engagement. Only moves up to `keep`; `0` disables it.
- **Non-reciprocal close-tie ceiling** (rung 5, `demote_nonmutual_close_ties`,
  default **on**): the mirror-inverse of the reciprocity ceiling — an account
  that scored into `keep` is demoted to `review` when the owner applied an
  explicit relationship marker the followee never reciprocated:
  `account_class == Personal`, **not** mutual, `is_close_friend || is_favorited`,
  and not keeplisted. A paired **penalty** (`nonmutual_close_tie_penalty`) also
  erodes `score_raw` so the report reflects the red flag, but the gate is what
  guarantees the floor; only moves down to `review`, never `unfollow` (a
  heavily-penalized marked account is caught by rung 6's marker guard). Ships
  **on across all presets** — unlike the other two gates — because it is
  high-precision (the explicit marker _and_ non-mutuality _and_ personal-class)
  and Review-only. Full design:
  [`docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md`](specs/2026-06-01-nonmutual-close-tie-gate-design.md).
- **Reciprocity keep-ceiling** (rung 5, `require_reciprocity_for_keep`, default
  **off** — opt-in): when enabled, an account that scored into `keep` is
  demoted to `review` when its _entire_ relationship is one-directional
  consumption — `account_class == Personal`, not mutual, no inbound signal
  (`inbound_dm_request`, `dm_reactions_received`, or a two-way DM thread), and
  no explicit keep override. The exact mirror of rung 6's brand/favorite
  `unfollow` gate; only moves down to `review`. **Off by default**: the only
  labeled data to date (docs/TUNING.md round 7) showed it demotes
  deliberately-curated one-way creator/brand follows, halving agreement. It
  stays a toggle for mutual-heavy users who want non-mutual strangers
  surfaced.

Why gates and not weights: a weight tuned to the labeled set inherits its
noise and can swing decisions both ways; a monotonic gate encodes a
one-sentence principle whose correctness doesn't depend on the calibration
labels being clean. See [`docs/specs/2026-05-30-reciprocity-aware-scoring.md`](specs/2026-05-30-reciprocity-aware-scoring.md).

## Output

**Primary: CSV** — sortable, filterable, easy to diff between runs.

```
username,display_name,profile_url,bucket,keep_score,dm_msgs,last_dm_days,reactions_given_180d,reactions_received_180d,likes_given_90d,comments_given_90d,follow_tenure_days,account_class,mutual,top_signal,reply_skew,dm_inbound_replies
```

`keep_score` is the `keep_prob` sigmoid output (§ Scoring), kept as a raw
`0.0–1.0` three-decimal float — **not** a percentage string — so spreadsheet
math (`AVERAGE`, conditional formatting, sorting) works without stripping a
`%`. The human-friendly percentage treatment is a Markdown / HTML concern.
`top_signal` is the single dominant scoring term (`tenure`, `likes`, `dm`, …),
the largest signed contribution to the raw score. (Both columns were renamed
from `keep_prob` / `notes` in the v2 header; the row order and column
positions are unchanged.)

`profile_url` is `https://www.instagram.com/<username>/` — handles are
ASCII-restricted by Instagram, so no URL encoding is needed.

`mutual` is `true`/`false` indicating reciprocity (handle appears in
`followers_*.json`). Decision support only — scoring intentionally
does not penalize one-sided follows (see `one_sided_them_is_not_a_penalty`
in scoring tests).

`reply_skew` is the post-dedup `dm_balance` (owner messages ÷ total real
messages in resolved 1:1 DM threads); `1.0` means the owner does all the
talking. `dm_inbound_replies` is the other party's real message count, taps
("Liked a message") excluded. Both are decision support for the effort-skew
gate and appear on every row with a resolvable thread.

> **Two aggregations — don't conflate them.** `keep_prob` is computed from
> exponential-decay-weighted signals (continuous τ). The `*_90d` / `*_180d`
> columns are **raw counts in a fixed window**, emitted only as human-readable
> sanity context for skim-review — they are _not_ the values that feed scoring.
> `features.rs` computes both: the decay-weighted score inputs and these
> plain windowed counts.

> **`display_name` is the inverse of the `NameResolver` join.** The seven
> `label_values` files ship `(Name, Username)` pairs — `NameResolver`
> already uses them to map display name → handle for DM attribution; the
> CSV writer needs the reverse direction (handle → display name), with
> the same collision policy: collisions emit empty string, never a
> guess. The CSV slice should either materialize `display_name:
Option<String>` onto `AccountFeatures` from those same pairs (single
> source of truth, single pass) or extend `NameResolver` with a
> `display_name_for(handle)` accessor.

**Secondary: Markdown summary** alongside the CSV — grouped per-bucket
cards (Unfollow + Review) with the top signal and a decision hint, plus
top/bottom Keep tables. Built for "decide whether to open the CSV at all".
Human-facing details that differ from the CSV's machine layer:

- `keep_prob` renders as an integer percentage (`keep 87%`), not the raw
  float — the CSV keeps the float for spreadsheet math.
- The **Summary** block is a proportion bar (`Keep ███░░ 572 88%`); the
  three percentages use largest-remainder rounding so they sum to exactly 100.
- A decision hint is **suppressed** when it would only restate an attribute
  badge already on the card — specifically the `one-sided` hint, which the
  attribute line already shows (`HINT_ONE_SIDED` in `output/mod.rs` is the
  shared string both the hint and the suppression check compare against).
- Droplist-forced Unfollow rows are quarantined under a **"Forced by
  droplist"** subhead, split from the score-sorted "Scored low" list, so a
  hand-flagged account at `keep 100%` doesn't read as a score anomaly.
- The **Review** section splits into a **Faded — once engaged, now cold**
  subsection (full cards, hardest-call-first) and an **Inert — never engaged**
  subsection (compact table, skim in bulk), gated on `is_review_inert`
  (`output/mod.rs`, reusing `scoring::is_inert`). The split fires only when at
  least one inert account exists; an inert-free Review stays flat. The HTML
  report carries the same split as a per-row `data-inert` flag plus a
  "Hide never-engaged" filter toggle. The CSV is unchanged — an inert account
  is already `bucket=review, top_signal=tenure` with a low `keep_score`.

**Tertiary: HTML report** alongside the CSV+MD — single self-contained
file (inline CSS+JS, no deps, no server). Sortable + filterable per-bucket
tables for browser-based triage; "Keep likelihood" shows the percentage
plus a bucket-keyed bar (raw float in the cell `title`, exact float in
`data-p` so rounding never reorders a sort). Built for the "open in a
browser, type to filter, click to sort" workflow — plus **in-report
triage**: each row has Keep / Drop toggles (mutually exclusive, mirroring
`lists::ensure_disjoint`) that persist in `localStorage`; a floating bar
then **Copies** or **Downloads** the appendable handle lists to paste into
`config/keeplist.txt` / `config/droplist.txt`. A `file://` page can't write
to disk, so collect-and-paste is the model — nothing leaves the browser.
Rows are server-rendered with HTML-escaping as the security boundary; the
JS reads handles back from `data-` attributes only to build the export
text, never as `innerHTML`. The report tracks the OS `prefers-color-scheme`
by default and adds a header **Auto / Light / Dark** switcher (ARIA
radiogroup, choice persisted in `localStorage`) to override it; theming is
driven by a `data-theme` attribute on `<html>`, with the dark token set
emitted from one source under both `:root[data-theme="dark"]` and a
media query scoped to `:root[data-theme="auto"]` so manual and system dark
can't drift. An anti-FOUC boot script applies the saved theme before first
paint; JS-disabled falls back to `auto` + the media query.

Filenames: `following-audit_YYYY-MM-DD.{csv,md,html}`, written next to
the input by default, overridable via `--out`.

## Implementation notes (tech choices)

**Format: CLI, not TUI.** One-shot batch job; the CSV + Markdown output is the
interface. A `ratatui` review subcommand (`igsift review`) is an optional v2
direction — out of scope for v1.

**Language: Rust** (edition 2024, stable). The workload is its sweet spot: file
I/O + JSON deserialization + numeric scoring, no async, no networking. Same
shape as `ripgrep` / `fd`; ships a single static binary.

### Core crates

| Concern      | Pick                                           | Why over the obvious alternative                                                                              |
| ------------ | ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| CLI parsing  | `clap` v4 (derive)                             | Universal standard; `bpaf` more elegant but smaller ecosystem.                                                |
| JSON parsing | `serde` + `serde_json` + `serde_path_to_error` | Schema-drift survival: `#[serde(default)]` + `Option<T>` + named path on parse failure.                       |
| Config       | `toml` (`config/scoring.toml`)                 | Weights/τ tunable without rebuilds — neutralizes Rust's data-tuning ergonomic gap.                            |
| Date/time    | `jiff`                                         | Correct-by-default vs. `chrono`'s timezone footguns.                                                          |
| CSV output   | `csv`                                          | Serializes directly from `#[derive(Serialize)]` structs.                                                      |
| Errors       | `anyhow`                                       | `anyhow::Result` throughout; `.context()` / `serde_path_to_error` carry the offending path on parse failures. |
| Logging      | `tracing` + `tracing-subscriber`               | Structured logs; `--verbose` wiring trivial.                                                                  |
| Progress UX  | `indicatif`                                    | Parse-phase progress bar + bytes bar.                                                                         |
| E2E tests    | `assert_cmd` + `predicates`                    | Run the binary against fixtures, assert on stdout + emitted CSV.                                              |
| Test runner  | `cargo-nextest`                                | Faster + better output than `cargo test`.                                                                     |

### Schema-drift survival

`serde_path_to_error` wraps every `from_reader` so a missing/renamed key fails
with the exact JSON path. The four shape groups (A/B/C/D in "Parsing notes")
get distinct deserializer types; `#[serde(default)]` + `Option<T>` cover
optional fields. Every release of Instagram's export schema is re-validated by
running [`scripts/walk_export_schema.sh`](../scripts/walk_export_schema.sh)
against the fresh dump — diff its output against the last-known-good run. The
test-side drift guard is `tests/cli.rs::fixture_counts_match_expected` (~40
exact-count assertions) paired with the structural field-pinning unit tests in
`src/export.rs`, so a parser that silently drops or defaults data fails loudly.

### Cutting-edge calls (flagged deliberately)

- **`jiff` over `chrono`** — newer, smaller community, better foundation. Worth
  the bet for a personal project.
- **`tracing` over `log` + `env_logger`** — slight ceremony tax, modern direction.
- **`miette`** considered for fancy parse-error diagnostics with source spans;
  defer unless parse errors get noisy.
- **Skipped for now:** `simd-json` / `sonic-rs` — 2–5× faster JSON, premature
  until a real bottleneck is measured. Drop-in upgrade path stays open.

### Deliberately not using

- **No `tokio` / async** — no concurrent I/O; async-coloring would be dead weight.
- **No DB layer** (`sqlx` / `diesel`) — the export is the source of truth.
- **No `reqwest`** — no network surface.
- **No workspace split** — single package (lib + bin) until the parser earns
  its own crate.
- **No `pyo3` / Python FFI** — pure Rust; Python notebooks for _post-hoc_ CSV
  analysis if needed, no language coupling in the codebase.

### Release profile

```toml
[profile.release]
lto = "thin"        # 2026 sweet spot — `fat` compiles much slower for marginal gain
codegen-units = 1
strip = true
```

### Fallback (not chosen)

**Python** with `pydantic` + `pandas` is the escape hatch _only_ if
scoring-weight iteration becomes the bottleneck and the TOML-config approach
above doesn't relieve it. Faster to ship, slower to align with the Rust goal.

## Open questions

- **`stories_viewed` weight** — 2,247 entries in the validated export is high,
  but viewing a story is a low-intent action (you scroll through everyone's).
  Pure noise, or a real "I'm interested in their content" signal? Worth A/B
  with `w_stories_viewed = 0` vs. `> 0`.
- **DM reactions weighting** — same scale as message volume, or distinct? A
  thread with 100 messages and 0 reactions vs. 10 messages and 10 reactions
  represents different relationship shapes; the model should reflect that.
- **`recently_unfollowed` as input exclusion vs. negative feature** — currently
  set to exclude entirely. Useful (never re-suggest someone I already dropped)
  or too churny? Empirically only 5 entries in this export — likely a non-issue.
- **`synced_contacts.json` join (v2)** — match 244 phone contacts against the
  follower set to identify IRL connections (strong implicit `keep`). Defer
  unless v1 misclassifies real friends.
- **Threshold tuning** — calibrate against personal judgment on a small labeled
  sample (~30 accounts I already know I want to keep/drop), or just iterate on
  the top/bottom of the ranking?

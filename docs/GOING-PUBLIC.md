# Going public — manual steps

Most of this list happens **outside the repo** (GitHub UI / one-off git
commands) — the kind of setup that can't be committed as code. The one
exception is the release (step 6): it's now automated by
`.github/workflows/release.yml`, so "cutting a release" is just promoting the
changelog and pushing a tag. Work top to bottom. Items marked **done** are
recorded for auditability.

## 0. Purge the leaked blob from the remote (GATING — ✅ DONE 2026-05-30)

> **Resolved 2026-05-30.** The repo was deleted and recreated from the clean
> local clone (see "reliable fix" below), then clean `main` was pushed.
> Verified: `gh api .../git/blobs/08d58c4c…` returns **404**, and
> `git ls-remote origin` shows only `refs/heads/main` — no `dependabot/*`, no
> `refs/pull/*`. The blob is gone from the remote.

A real audit report (`following-audit_2026-05-27.html`, ~643 real handles + the
maintainer's keep/drop intent, blob `08d58c4`) was accidentally committed in
`fc1ea49` and untracked in `5b1186d`, but remained reachable in history.

The **local** history was rewritten and `main` force-pushed:

```bash
git bundle create ~/ig-pre-public-backup.bundle --all   # real pre-rewrite backup (the .bundle, not a branch)
git filter-repo --invert-paths --path following-audit_2026-05-27.html --force
git push --force-with-lease origin main
```

> **This was NOT enough.** `filter-repo` + `push --force origin main` rewrite
> **only `main`**. The blob is still live on `origin` because:
>
> - Dependabot branches (e.g. `dependabot/.../taiki-e/install-action-2.79.12`)
>   were cut from pre-rewrite `main` and still carry `fc1ea49` in their history.
> - **`refs/pull/*` refs are immutable** — GitHub maintains them; you cannot
>   `git push --delete` them. PR #17's `refs/pull/17/head` reaches the blob, and
>   closing the PR / deleting the branch does **not** remove that ref.
> - The blob is directly downloadable today:
>   `gh api repos/gati3478/igsift/git/blobs/08d58c4c4b989b1747f8f8d2b66ececcc18f7857 --jq .size`
>   returns `232745`.

### The reliable fix: delete and recreate the repo

Because `refs/pull/*` cannot be purged with `filter-repo`, the only dependable
way to remove the blob from `origin` (short of GitHub Support) is to delete the
GitHub repo and re-create it from the clean local clone. The repo is private and
unshared, so this is low-cost (Dependabot PRs regenerate against clean `main`):

```bash
# Local main is already clean (no blob). Confirm:
git rev-list --all --objects | grep following-audit   # must print nothing (local)

gh repo delete gati3478/igsift --yes               # destructive; requires delete_repo scope
gh repo create gati3478/igsift --private --source=. --remote=origin --push
git push origin --tags                                 # if any tags exist

# Definitive remote verification — must 404, not return a size:
gh api repos/gati3478/igsift/git/blobs/08d58c4c4b989b1747f8f8d2b66ececcc18f7857 --jq .size
git ls-remote origin | grep -i following-audit || echo "clean"
```

Do **not** flip the repo public (step 1) until that blobs-API call returns
`404`. If you maintain any clone from before the rewrite, **re-clone** — do not
`git pull`.

## 1. Flip the repo to public

Settings → General → Danger Zone → **Change visibility → Public**. Do this only
after step 0 verifies clean.

## 2. Description and topics

Settings → top of the repo page (the gear next to "About"):

- **Description:** `Local-first Rust CLI that audits your Instagram following from a data export — who to unfollow vs. keep, fully offline.`
- **Topics:** `rust`, `cli`, `instagram`, `data-export`, `local-first`, `privacy`
  (mirrors `Cargo.toml` `keywords`/`categories`).

## 3. Private vulnerability reporting

Settings → Security → **Enable private vulnerability reporting** (free for public
repos). This is the channel `SECURITY.md` points contributors to.

## 4. Branch protection on `main`

Free for public repos. Settings → Branches → add a rule for `main`:

- Require a pull request before merging.
- Require status checks to pass: select **`check`** and **`cargo-deny`** (the two
  CI jobs from `ci.yml`). Don't add the Release workflow here — it's
  tag-triggered, not a PR check, so it never reports a status on a PR.
- Require branches to be up to date before merging.
- Require linear history (matches the existing clean linear log).

## 5. Confirm Dependabot is active

`.github/dependabot.yml` is already committed (weekly `cargo` + `github-actions`
updates). After going public, confirm Settings → Code security → Dependabot is
enabled.

## 6. Cutting a release (automated)

Releases are built by `.github/workflows/release.yml`. Pushing a `v*` tag
triggers it; there is **no** manual "draft a release from the tag" step
anymore — the workflow creates the Release itself.

What it does, on a single `v*` tag push:

1. A `create-release` job runs first and creates the GitHub Release once, up
   front (separate job so the parallel build matrix can't race to create it).
   It auto-marks `v0.x` tags as `--prerelease` and `v1.0.0`+ as full releases,
   and generates notes categorized by `.github/release.yml`
   (New Features / Bug Fixes / Dependencies / Other Changes).
2. A build matrix cross-compiles `igsift` for five targets and uploads each
   archive (with a `.sha256` checksum, plus README + LICENSE) to that Release:
    - macOS arm64 — `aarch64-apple-darwin`
    - Windows x64 / arm64 — `x86_64` / `aarch64-pc-windows-msvc`
    - Linux x64 / arm64 — `x86_64` / `aarch64-unknown-linux-musl`
      (statically linked against musl, so they run on Fedora and any other
      distro regardless of the builder's libc)

**Status today:** `v0.1.0` is already published as a **pre-release** with all
five binaries attached.

### Cutting the 1.0 release

Once `v0.1.0` is validated and the repo is public:

```bash
# 1. Promote CHANGELOG.md: rename the [Unreleased] heading to [1.0.0] (dated).
# 2. Commit that change, then tag and push:
git tag v1.0.0 && git push origin v1.0.0
```

The `v1.0.0` tag (not `v0.*`) makes `create-release` publish a **full**
release, not a pre-release. Users install by downloading a prebuilt archive
from the Release, or with `cargo install --path .` / `cargo build --release`
from a clone.

## Not enabled (deliberate)

- **Publishing to crates.io** — not done; the tool is consumed by cloning, not as
  a dependency. The `repository` URL in `Cargo.toml` is already correct
  (`github.com/gati3478/igsift`).
- **GitHub Advanced Security extras** beyond the free public-repo defaults — not
  needed; the supply chain is covered by `cargo-deny` + Dependabot.

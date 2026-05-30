# Going public — manual steps

Everything in this list happens **outside the repo** (GitHub UI / one-off git
commands) and cannot be committed as code. Work top to bottom. Items marked
**done** are recorded for auditability.

## 0. Purge the leaked blob from the remote (GATING — not done yet)

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
>   `gh api repos/gati3478/ig-manager/git/blobs/08d58c4c4b989b1747f8f8d2b66ececcc18f7857 --jq .size`
>   returns `232745`.

### The reliable fix: delete and recreate the repo

Because `refs/pull/*` cannot be purged with `filter-repo`, the only dependable
way to remove the blob from `origin` (short of GitHub Support) is to delete the
GitHub repo and re-create it from the clean local clone. The repo is private and
unshared, so this is low-cost (Dependabot PRs regenerate against clean `main`):

```bash
# Local main is already clean (no blob). Confirm:
git rev-list --all --objects | grep following-audit   # must print nothing (local)

gh repo delete gati3478/ig-manager --yes               # destructive; requires delete_repo scope
gh repo create gati3478/ig-manager --private --source=. --remote=origin --push
git push origin --tags                                 # if any tags exist

# Definitive remote verification — must 404, not return a size:
gh api repos/gati3478/ig-manager/git/blobs/08d58c4c4b989b1747f8f8d2b66ececcc18f7857 --jq .size
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
  CI jobs).
- Require branches to be up to date before merging.
- Require linear history (matches the existing clean linear log).

## 5. Confirm Dependabot is active

`.github/dependabot.yml` is already committed (weekly `cargo` + `github-actions`
updates). After going public, confirm Settings → Code security → Dependabot is
enabled.

## 6. (Optional) First tagged release

The crate is `v0.1.0` and unreleased. If/when you want a GitHub Release:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

Then draft a release from the tag — GitHub's auto-generated notes are
categorized by `.github/release.yml` (New Features / Bug Fixes / Dependencies /
Other), and `CHANGELOG.md`'s `[Unreleased]` section should be promoted to
`[0.1.0]` at the same time.

**Not yet automated (deliberate):** there is no binary-build release workflow.
For an unreleased solo CLI a cross-compile matrix (macOS/Linux/Windows targets
uploaded to the Release) is premature — add `.github/workflows/release.yml`
when the first real release demands prebuilt binaries, rather than carrying
speculative CI now. Until then, users install with `cargo install --path .` or
`cargo build --release`.

## Not enabled (deliberate)

- **Publishing to crates.io** — not done; the tool is consumed by cloning, not as
  a dependency. The `repository` URL in `Cargo.toml` is already correct
  (`github.com/gati3478/ig-manager`).
- **GitHub Advanced Security extras** beyond the free public-repo defaults — not
  needed; the supply chain is covered by `cargo-deny` + Dependabot.

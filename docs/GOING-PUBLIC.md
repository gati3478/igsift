# Going public — manual steps

Everything in this list happens **outside the repo** (GitHub UI / one-off git
commands) and cannot be committed as code. Work top to bottom. Items marked
**done** are recorded for auditability.

## 0. History rewrite (gating — do before the first public push)

A real audit report (`following-audit_2026-05-27.html`, ~640 real handles + the
maintainer's keep/drop intent) was accidentally committed in `fc1ea49` and
untracked in `5b1186d`, but remained reachable in history. It was purged from
**all** history before the repo went public:

```bash
git branch pre-public-backup            # safety: pre-rewrite SHA kept locally
git filter-repo --invert-paths --path following-audit_2026-05-27.html --force
# verify nothing remains:
git rev-list --all --objects | grep following-audit   # must print nothing
git push --force origin main
```

If you maintain a fork or local clone from before the rewrite, **re-clone** —
do not `git pull`, or you will resurrect the purged blob.

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

Then draft a release from the tag. A `CHANGELOG.md` is intentionally deferred
until the first tagged release.

## Not enabled (deliberate)

- **Publishing to crates.io** — not done; the tool is consumed by cloning, not as
  a dependency. The `repository` URL in `Cargo.toml` is already correct
  (`github.com/gati3478/ig-manager`).
- **GitHub Advanced Security extras** beyond the free public-repo defaults — not
  needed; the supply chain is covered by `cargo-deny` + Dependabot.

# Test fixtures

## `sample_export/`

A small, **sanitized** Instagram export used by parser and end-to-end tests.

Conventions when editing it:

- Mirror the real export layout (`connections/…`, `your_instagram_activity/…`)
  but with a handful of synthetic accounts — no real usernames or message
  content.
- Keep it small enough to read in a diff; it is the ground truth that the
  exact-count assertions in `tests/cli.rs::fixture_counts_match_expected`
  (paired with the structural unit tests in `src/export.rs`) check against, so
  a parser regression surfaces as a failing count rather than silent data loss.
- Commit it. Real personal exports must **never** be committed — see the root
  `.gitignore`.

# Test fixtures

## `sample_export/`

A small, **sanitized** Instagram export used by parser and end-to-end tests.
Not present yet — to be created during implementation.

When you add it:

- Mirror the real export layout (`connections/…`, `your_instagram_activity/…`)
  but with a handful of synthetic accounts — no real usernames or message
  content.
- Keep it small enough to read in a diff; it is the ground truth that `insta`
  snapshots assert against, so schema drift surfaces as a reviewable change.
- Commit it. Real personal exports must **never** be committed — see the root
  `.gitignore`.

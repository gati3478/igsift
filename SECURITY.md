# Security Policy

## Reporting a vulnerability

Please report security or privacy issues **privately**, not as a public issue:

- Preferred: open a private advisory via the repository's
  **Security → Report a vulnerability** tab (GitHub private vulnerability
  reporting).
- Alternative: email <gatipetriashvili@gmail.com>.

I aim to acknowledge reports within a few days. As a single-maintainer
personal project there is no formal SLA, but privacy-impacting issues are
treated as the top priority.

## Threat model / what to look for

`ig-mgr` is a **local-first, offline** tool. It reads an Instagram data export
from disk and writes report files to disk. It performs no network I/O, has no
server, no database, no telemetry, and never transmits export data anywhere.
The most relevant classes of issue are therefore:

- **Local data exposure** — a code path that writes export-derived personal
  data somewhere unexpected, or a report that leaks more than intended.
- **Untrusted-input handling** — the export (zip + JSON) is third-party data.
  Path traversal during archive extraction, panics, or resource exhaustion on
  a malformed/hostile export are in scope. (Zip-slip is guarded and tested; see
  `src/archive.rs`.)
- **Output injection** — CSV formula injection and HTML/JS injection in the
  generated report are guarded and tested; bypasses are in scope.

Supply-chain advisories are tracked automatically by `cargo-deny` in CI and
Dependabot; you do not need to report a known RUSTSEC advisory — it will
already be flagged.

## Supported versions

Pre-1.0: only the latest `main` is supported. There are no backported fixes.

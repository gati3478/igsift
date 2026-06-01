#!/usr/bin/env bash
# Regenerate the README showcase images in docs/assets/ from synthetic data.
#
# Pipeline (all local, no network beyond the one-time browser install):
#   examples/showcase.rs  ──▶  dashboard ANSI + HTML report (real scorer)
#         ANSI  ──▶  a terminal-styled HTML page  ──▶  Chromium screenshot
#         HTML report (triage + theme pre-seeded)  ──▶  Chromium screenshot ×2 (light/dark)
#         all three  ──▶  pngquant
#
# Why Chromium and not `freeze` for the terminal shot: freeze/resvg renders the
# `review` bucket glyph U+25D0 (◐) as a notdef box regardless of font (verified
# across JetBrains Mono / Monaco / Menlo). A real browser renders it correctly.
#
# Requirements:
#   - Rust toolchain (cargo)
#   - python3 (stdlib only)
#   - Playwright Chromium:  npx playwright install chromium
#   - pngquant (optional; skipped with a warning if absent)
#
# Usage:  scripts/showcase-shots.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ASSETS="$ROOT/docs/assets"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$ASSETS"

# Triage handles pre-selected so the floating export bar is visible in the
# report screenshot. Must exist in the synthetic dataset (examples/showcase.rs).
SEED_JSON='{"giveaway.bot.x":"drop","spam.followback.4u":"drop","dropshipping.store":"drop","maya.renteria":"keep","luca.bianchi":"keep","sara.okonkwo":"keep","priya.n":"keep"}'

echo "▶ 1/5  Running the example through the real scorer…"
( cd "$ROOT" && cargo run --quiet --example showcase -- "$WORK/audit" ) > "$WORK/dash.ansi"
#                                                       └ writes audit.{csv,md,html}; dashboard ANSI on stdout

echo "▶ 2/5  ANSI → terminal-styled HTML…"
SEED_JSON="$SEED_JSON" python3 - "$WORK/dash.ansi" "$WORK/terminal.html" "$WORK/audit.html" "$WORK/audit-light.html" "$WORK/audit-dark.html" <<'PY'
import html, os, re, sys
ansi_path, term_out, report_in, report_light, report_dark = sys.argv[1:6]

# --- ANSI (SGR 0/2/31/32/33 only) → colored <span>s ---
CLS = {'2': 'dim', '31': 'r', '32': 'g', '33': 'y'}
def to_spans(line):
    fg = None; dim = False; out = ''; open_ = False
    for tok in re.split(r'(\x1b\[[0-9;]*m)', line):
        m = re.match(r'\x1b\[([0-9;]*)m', tok)
        if m:
            if open_: out += '</span>'; open_ = False
            for code in (m.group(1) or '0').split(';'):
                if code in ('', '0'): fg = None; dim = False
                elif code == '2': dim = True
                elif code in CLS: fg = CLS[code]
            continue
        if not tok: continue
        cls = ' '.join(c for c in (fg, 'dim' if dim else None) if c)
        out += (f'<span class="{cls}">' if cls else '<span>') + html.escape(tok) + '</span>'
        open_ = True
    return out

ansi = open(ansi_path, encoding='utf-8').read().rstrip('\n')
body = '\n'.join(to_spans(l) for l in ansi.split('\n'))
page = f'''<!doctype html><meta charset=utf-8><style>
*{{margin:0;box-sizing:border-box}}
body{{background:#0c0c0e;padding:52px;font:0;display:inline-block}}
.win{{background:#161618;border-radius:11px;overflow:hidden;
 box-shadow:0 24px 60px -12px rgba(0,0,0,.7),0 8px 20px -8px rgba(0,0,0,.5);
 border:1px solid #2a2a2e}}
.bar{{height:38px;display:flex;align-items:center;gap:8px;padding:0 16px;
 background:#1d1d20;border-bottom:1px solid #2a2a2e}}
.dot{{width:12px;height:12px;border-radius:50%}}
.c1{{background:#ff5f57}}.c2{{background:#febc2e}}.c3{{background:#28c840}}
pre{{margin:0;padding:22px 26px 26px;color:#dcdcdc;
 font-family:'SF Mono',Menlo,Monaco,monospace;font-size:14px;line-height:1.42;
 -webkit-font-smoothing:antialiased;font-variant-ligatures:none}}
.dim{{color:#8b8b93}} .g{{color:#3fb950}} .y{{color:#e3b341}} .r{{color:#f0716f}}
</style><div class="win"><div class="bar">
<span class="dot c1"></span><span class="dot c2"></span><span class="dot c3"></span>
</div><pre>{body}</pre></div>'''
open(term_out, 'w', encoding='utf-8').write(page)

# --- Pre-seed triage (export bar visible) + theme, one file per shot. The
#     theme boot script runs in <head>, so seeding localStorage at <body> is
#     too late for this load — stamp data-theme on <html> directly so the
#     render and the selected radio are deterministic; seed localStorage too
#     so the persisted state is realistic. ---
report = open(report_in, encoding='utf-8').read()
def seed(theme):
    inject = ('<script>try{'
              'localStorage.setItem("igsift.triage.v1",JSON.stringify(%s));'
              'localStorage.setItem("igsift.theme.v1","%s");'
              '}catch(e){}</script>\n') % (os.environ['SEED_JSON'], theme)
    out = report.replace('<body>\n', '<body>\n' + inject, 1)
    return out.replace('data-theme="auto"', 'data-theme="%s"' % theme, 1)
open(report_light, 'w', encoding='utf-8').write(seed('light'))
open(report_dark, 'w', encoding='utf-8').write(seed('dark'))
print('   terminal.html + seeded reports (light/dark) ready')
PY

shoot() { # url  out  [extra flags…]
  npx --yes playwright screenshot -b chromium "$@" >/dev/null 2>&1
}

echo "▶ 3/5  Screenshotting the terminal dashboard…"
shoot --full-page --wait-for-timeout 400 "$WORK/terminal.html" "$ASSETS/cli-dashboard.png"

echo "▶ 4/5  Screenshotting the HTML report (light + dark)…"
shoot --viewport-size 1300,1040 --color-scheme light --wait-for-timeout 700 "$WORK/audit-light.html" "$ASSETS/html-report-light.png"
shoot --viewport-size 1300,1040 --color-scheme dark  --wait-for-timeout 700 "$WORK/audit-dark.html" "$ASSETS/html-report-dark.png"

echo "▶ 5/5  Optimizing…"
if command -v pngquant >/dev/null 2>&1; then
  for f in cli-dashboard html-report-light html-report-dark; do
    pngquant --force --skip-if-larger --quality 80-96 --strip --output "$ASSETS/$f.png" "$ASSETS/$f.png" || true
  done
else
  echo "   pngquant not found — skipping optimization (brew install pngquant)"
fi

echo "✓ Wrote:"
for f in cli-dashboard html-report-light html-report-dark; do
  printf '   %-26s %s\n' "docs/assets/$f.png" "$(du -h "$ASSETS/$f.png" | cut -f1)"
done

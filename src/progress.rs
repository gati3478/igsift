//! Spinner-based progress reporter for the run pipeline.
//!
//! One [`Reporter`] per run. Drives a single re-rendering line on
//! stderr that shows the current phase ("Parsing connections",
//! "Aggregating features", …) while the work happens. The spinner
//! ticks on its own timer so the call site only needs to update the
//! message at phase boundaries — no inner instrumentation.
//!
//! ## When the spinner is hidden
//!
//! - `args.verbose > 0` (the caller passes `enabled = false`) — log
//!   output is going to the same stderr stream the spinner would
//!   draw on, and interleaving them is unreadable.
//! - stderr is not a TTY — handled automatically by indicatif's
//!   default stderr draw target, which collapses to a no-op when
//!   the stream is redirected.
//!
//! In either case the bar is constructed against a hidden draw
//! target, so every call to [`Reporter::phase`] / [`Reporter::finish`]
//! is a no-op and the caller does not need a branch.

use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Single spinner that updates its message at each pipeline phase.
pub struct Reporter {
    bar: ProgressBar,
}

impl Reporter {
    /// Build the reporter. When `enabled` is false, every method is a
    /// no-op (the underlying bar draws to a hidden target). Passing
    /// `enabled = true` does not force the bar visible on a redirected
    /// stderr — indicatif's stderr target self-hides off-TTY.
    pub fn new(enabled: bool) -> Self {
        let target = if enabled {
            ProgressDrawTarget::stderr()
        } else {
            ProgressDrawTarget::hidden()
        };
        let bar = ProgressBar::with_draw_target(None, target);
        bar.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .expect("static template is valid")
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        Self { bar }
    }

    /// Update the phase label shown next to the spinner. Cheap — the
    /// next steady-tick render picks up the new message.
    pub fn phase(&self, label: &str) {
        self.bar.set_message(label.to_owned());
    }

    /// Stop ticking and clear the line so the run's stdout summary
    /// starts at column 0 rather than after a half-rendered spinner
    /// frame.
    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }
}

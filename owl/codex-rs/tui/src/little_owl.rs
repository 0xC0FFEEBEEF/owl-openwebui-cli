//! The Little Owl: this fork's mascot.
//!
//! A small four-row ASCII owl whose expression follows what Owl is doing: it blinks
//! while it waits for input, ponders with a question mark while a turn is running, and
//! looks happy for a moment after the turn finishes.
//!
//! The frames, the captions, and every timing value below come from the user's
//! "Little Owl" package (`frames.json`): an 8-column x 4-row owl, a 8300 ms blink cycle
//! (half closed at 4900-5020, closed at 5020-5270, half closed through 5390), a question
//! mark during the third 1400 ms segment of each 5600 ms thinking cycle, and a static
//! happy frame when the task is done. Like the package's frame function, `owl_frame()`
//! is pure: it returns a string, writes nothing, and pads every row to 8 columns so old
//! characters get cleared.

use std::time::Duration;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::render::renderable::Renderable;
use crate::tui::FrameRequester;

/// Every frame of the owl is exactly this many columns wide and this many rows tall.
pub(crate) const OWL_WIDTH: usize = 8;
pub(crate) const OWL_HEIGHT: usize = 4;
/// The four art rows plus the caption row.
pub(crate) const OWL_BLOCK_HEIGHT: u16 = OWL_HEIGHT as u16 + 1;

/// How long the happy "all done!" owl stays on screen after a turn finishes.
pub(crate) const DONE_WINDOW: Duration = Duration::from_secs(4);

// Frames, kept verbatim from the Little Owl package (trailing spaces included: they are
// what clears the previous frame's characters).
const FRAME_OPEN: &str = concat!("  ,_,   \n", " (o,o)  \n", " (   )  \n", "  \" \"   ",);
const FRAME_HALF_CLOSED: &str = concat!("  ,_,   \n", " (-,-)  \n", " (   )  \n", "  \" \"   ",);
const FRAME_CLOSED: &str = concat!("  ,_,   \n", " (_,_)  \n", " (   )  \n", "  \" \"   ",);
const FRAME_HAPPY: &str = concat!("  ,_,   \n", " (^,^)  \n", " (   )  \n", "  \" \"   ",);
const FRAME_PONDERING: &str = concat!("  ,_,   \n", " (o,o) ?\n", " (   )  \n", "  \" \"   ",);

// Timing values from the package's frames.json, in milliseconds.
const BLINK_CYCLE_MS: u128 = 8300;
const HALF_CLOSED_START_MS: u128 = 4900;
const CLOSED_START_MS: u128 = 5020;
const CLOSED_END_MS: u128 = 5270;
const BLINK_END_MS: u128 = 5390;
const THINKING_SEGMENT_MS: u128 = 1400;
const THINKING_CYCLE_SEGMENTS: u128 = 4;
const QUESTION_SEGMENT_INDEX: u128 = 2;

/// What Owl is doing, which decides which expression the owl shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwlState {
    /// Waiting for the user to ask something.
    Idle,
    /// A turn is running.
    Thinking,
    /// A turn just finished.
    Done,
}

/// The frame to show for `state` at `elapsed` since the state began.
///
/// `elapsed` of zero is the static frame, which is what to render when animations are
/// off (the same thing the package's `{ animate: false }` mode gives you).
pub(crate) fn owl_frame(state: OwlState, elapsed: Duration) -> &'static str {
    match state {
        OwlState::Idle => {
            let t = elapsed.as_millis() % BLINK_CYCLE_MS;
            if (HALF_CLOSED_START_MS..CLOSED_START_MS).contains(&t)
                || (CLOSED_END_MS..BLINK_END_MS).contains(&t)
            {
                FRAME_HALF_CLOSED
            } else if (CLOSED_START_MS..CLOSED_END_MS).contains(&t) {
                FRAME_CLOSED
            } else {
                FRAME_OPEN
            }
        }
        OwlState::Thinking => {
            let cycle_ms = THINKING_SEGMENT_MS * THINKING_CYCLE_SEGMENTS;
            let t = elapsed.as_millis() % cycle_ms;
            let segment = t / THINKING_SEGMENT_MS;
            if segment == QUESTION_SEGMENT_INDEX {
                FRAME_PONDERING
            } else {
                FRAME_OPEN
            }
        }
        OwlState::Done => FRAME_HAPPY,
    }
}

/// The one-line note shown under the owl.
pub(crate) fn owl_caption(state: OwlState) -> &'static str {
    match state {
        OwlState::Idle => "ready when you are.",
        OwlState::Thinking => "hmm, one moment\u{2026}",
        OwlState::Done => "all done!",
    }
}

/// Milliseconds until the animated frame would change, or `None` when it never will (the
/// happy owl is static). Used to ask for a redraw exactly when something changes instead of
/// redrawing on a fixed tick.
fn ms_until_next_frame_change(state: OwlState, elapsed: Duration) -> Option<u64> {
    let elapsed_ms = elapsed.as_millis();
    let delta = match state {
        OwlState::Done => return None,
        OwlState::Idle => {
            let t = elapsed_ms % BLINK_CYCLE_MS;
            match [
                HALF_CLOSED_START_MS,
                CLOSED_START_MS,
                CLOSED_END_MS,
                BLINK_END_MS,
            ]
            .into_iter()
            .find(|&boundary| boundary > t)
            {
                Some(boundary) => boundary - t,
                // Past the blink: the next change is the blink of the next cycle.
                None => (BLINK_CYCLE_MS - t) + HALF_CLOSED_START_MS,
            }
        }
        OwlState::Thinking => {
            let cycle_ms = THINKING_SEGMENT_MS * THINKING_CYCLE_SEGMENTS;
            let question_start = QUESTION_SEGMENT_INDEX * THINKING_SEGMENT_MS;
            let t = elapsed_ms % cycle_ms;
            let segment = t / THINKING_SEGMENT_MS;
            if segment == QUESTION_SEGMENT_INDEX {
                question_start + THINKING_SEGMENT_MS - t
            } else if segment < QUESTION_SEGMENT_INDEX {
                question_start - t
            } else {
                cycle_ms + question_start - t
            }
        }
    };
    Some(u64::try_from(delta.max(1)).unwrap_or(u64::MAX))
}

/// Milliseconds until the happy owl's window ends and it should step aside. The owl asks
/// for that draw itself, because a request made from outside a draw can be coalesced away
/// by `FrameRequester` (it keeps the earliest deadline) and then the owl would hang around
/// until the user happened to touch something.
fn ms_until_done_window_ends(elapsed: Duration) -> Option<u64> {
    let remaining_ms = DONE_WINDOW.saturating_sub(elapsed).as_millis();
    Some(u64::try_from(remaining_ms.max(1)).unwrap_or(u64::MAX))
}

/// The owl plus the timer that decides which frame to draw right now.
pub(crate) struct LittleOwl {
    state: OwlState,
    state_started: Instant,
    frame_requester: FrameRequester,
    animations_enabled: bool,
}

impl LittleOwl {
    pub(crate) fn new(
        state: OwlState,
        frame_requester: FrameRequester,
        animations_enabled: bool,
    ) -> Self {
        Self {
            state,
            state_started: Instant::now(),
            frame_requester,
            animations_enabled,
        }
    }

    /// Move to a new state, restarting the elapsed timer (as the package requires).
    pub(crate) fn set_state(&mut self, state: OwlState) {
        if self.state != state {
            self.state = state;
            self.state_started = Instant::now();
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> OwlState {
        self.state
    }

    pub(crate) fn elapsed_at(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.state_started)
    }

    fn elapsed_for_frame(&self, now: Instant) -> Duration {
        if self.animations_enabled {
            self.elapsed_at(now)
        } else {
            Duration::ZERO
        }
    }

    pub(crate) fn frame_at(&self, now: Instant) -> &'static str {
        owl_frame(self.state, self.elapsed_for_frame(now))
    }

    pub(crate) fn caption(&self) -> &'static str {
        owl_caption(self.state)
    }

    /// True once the happy owl has had its moment and should stop taking up rows.
    pub(crate) fn expired_at(&self, now: Instant) -> bool {
        self.state == OwlState::Done && self.elapsed_at(now) >= DONE_WINDOW
    }

    /// Ask for the next draw at the moment the owl would look different: the next animation
    /// frame, or the end of the happy owl's window. Idle and thinking frames are only chased
    /// when animations are on; the happy owl's removal is always scheduled.
    pub(crate) fn schedule_next_frame_at(&self, now: Instant) {
        let elapsed = self.elapsed_at(now);
        let delay_ms = if self.state == OwlState::Done {
            ms_until_done_window_ends(elapsed)
        } else if self.animations_enabled {
            ms_until_next_frame_change(self.state, elapsed)
        } else {
            None
        };
        if let Some(delay_ms) = delay_ms {
            self.frame_requester
                .schedule_frame_in(Duration::from_millis(delay_ms));
        }
    }

    /// The owl art rows followed by the caption row, ready for a `Paragraph`.
    pub(crate) fn lines_at(&self, now: Instant) -> Vec<Line<'static>> {
        let frame = self.frame_at(now);
        let mut lines: Vec<Line<'static>> = frame
            .lines()
            .map(|row| {
                debug_assert_eq!(row.width(), OWL_WIDTH, "owl frames are 8 columns wide");
                Line::from(vec![row.to_string().magenta()])
            })
            .collect();
        debug_assert_eq!(lines.len(), OWL_HEIGHT);
        lines.push(Line::from(vec![self.caption().to_string().dim()]));
        lines
    }
}

impl Renderable for LittleOwl {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let now = Instant::now();
        if self.expired_at(now) {
            return;
        }
        self.schedule_next_frame_at(now);
        Paragraph::new(self.lines_at(now)).render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        if self.expired_at(Instant::now()) {
            0
        } else {
            OWL_BLOCK_HEIGHT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn frames() -> Vec<(&'static str, &'static str)> {
        vec![
            ("open", FRAME_OPEN),
            ("halfClosed", FRAME_HALF_CLOSED),
            ("closed", FRAME_CLOSED),
            ("happy", FRAME_HAPPY),
            ("pondering", FRAME_PONDERING),
        ]
    }

    fn owl_at(state: OwlState, started: Instant) -> LittleOwl {
        LittleOwl::new(state, FrameRequester::test_dummy(), true).with_start(started)
    }

    impl LittleOwl {
        /// Move the state clock back so timing tests can inspect a given elapsed time.
        fn with_start(mut self, started: Instant) -> Self {
            self.state_started = started;
            self
        }
    }

    #[test]
    fn every_frame_is_eight_columns_by_four_rows() {
        for (name, frame) in frames() {
            let rows: Vec<&str> = frame.lines().collect();
            assert_eq!(rows.len(), OWL_HEIGHT, "{name} should have 4 rows");
            for row in rows {
                assert_eq!(row.width(), OWL_WIDTH, "{name} row {row:?} width");
            }
        }
    }

    #[test]
    fn idle_blinks_on_the_packages_windows() {
        let checks: &[(u64, &str)] = &[
            (0, FRAME_OPEN),
            (HALF_CLOSED_START_MS as u64 - 1, FRAME_OPEN),
            (HALF_CLOSED_START_MS as u64, FRAME_HALF_CLOSED),
            (CLOSED_START_MS as u64 - 1, FRAME_HALF_CLOSED),
            (CLOSED_START_MS as u64, FRAME_CLOSED),
            (CLOSED_END_MS as u64 - 1, FRAME_CLOSED),
            (CLOSED_END_MS as u64, FRAME_HALF_CLOSED),
            (BLINK_END_MS as u64 - 1, FRAME_HALF_CLOSED),
            (BLINK_END_MS as u64, FRAME_OPEN),
            (BLINK_CYCLE_MS as u64 - 1, FRAME_OPEN),
            // The cycle repeats: 4900 ms into the second cycle is half closed again.
            (
                (BLINK_CYCLE_MS * 2 + HALF_CLOSED_START_MS) as u64,
                FRAME_HALF_CLOSED,
            ),
        ];
        for &(elapsed_ms, expected) in checks {
            assert_eq!(
                owl_frame(OwlState::Idle, Duration::from_millis(elapsed_ms)),
                expected,
                "idle at {elapsed_ms} ms"
            );
        }
    }

    #[test]
    fn thinking_shows_the_question_mark_only_in_the_third_segment() {
        let checks: &[(u64, &str)] = &[
            (0, FRAME_OPEN),
            (THINKING_SEGMENT_MS as u64 - 1, FRAME_OPEN),
            (THINKING_SEGMENT_MS as u64, FRAME_OPEN),
            (2 * THINKING_SEGMENT_MS as u64, FRAME_PONDERING),
            (3 * THINKING_SEGMENT_MS as u64 - 1, FRAME_PONDERING),
            (3 * THINKING_SEGMENT_MS as u64, FRAME_OPEN),
            (5 * THINKING_SEGMENT_MS as u64, FRAME_OPEN),
            // Second cycle, third segment.
            (
                (THINKING_SEGMENT_MS * THINKING_CYCLE_SEGMENTS + 2 * THINKING_SEGMENT_MS) as u64,
                FRAME_PONDERING,
            ),
        ];
        for &(elapsed_ms, expected) in checks {
            assert_eq!(
                owl_frame(OwlState::Thinking, Duration::from_millis(elapsed_ms)),
                expected,
                "thinking at {elapsed_ms} ms"
            );
        }
    }

    #[test]
    fn done_is_the_happy_frame_whenever_you_look() {
        for elapsed_ms in [0, 1_000, 60_000] {
            assert_eq!(
                owl_frame(OwlState::Done, Duration::from_millis(elapsed_ms)),
                FRAME_HAPPY
            );
        }
    }

    #[test]
    fn captions_match_the_package() {
        assert_eq!(owl_caption(OwlState::Idle), "ready when you are.");
        assert_eq!(owl_caption(OwlState::Thinking), "hmm, one moment\u{2026}");
        assert_eq!(owl_caption(OwlState::Done), "all done!");
    }

    #[test]
    fn animations_off_uses_the_static_frame() {
        // 3000 ms into a thinking turn is the third segment, so animations on shows the
        // question mark.
        let started = Instant::now() - Duration::from_millis(3_000);
        let owl = owl_at(OwlState::Thinking, started);
        assert_eq!(owl.frame_at(Instant::now()), FRAME_PONDERING);

        // With animations off the same owl draws its static frame instead.
        let owl = LittleOwl::new(OwlState::Thinking, FrameRequester::test_dummy(), false)
            .with_start(started);
        assert_eq!(owl.frame_at(Instant::now()), FRAME_OPEN);
    }

    #[test]
    fn happy_owl_stops_taking_rows_after_its_window() {
        let owl = owl_at(OwlState::Done, Instant::now());
        assert_eq!(owl.desired_height(60), OWL_BLOCK_HEIGHT);
        assert!(!owl.expired_at(Instant::now()));

        let owl = owl_at(OwlState::Done, Instant::now() - DONE_WINDOW);
        assert!(owl.expired_at(Instant::now()));
        assert_eq!(owl.desired_height(60), 0);

        // Thinking and idle keep their rows.
        assert!(
            !owl_at(OwlState::Thinking, Instant::now() - DONE_WINDOW).expired_at(Instant::now())
        );
    }

    #[test]
    fn set_state_restarts_the_elapsed_clock() {
        let mut owl = owl_at(OwlState::Done, Instant::now() - DONE_WINDOW);
        assert!(owl.expired_at(Instant::now()));
        owl.set_state(OwlState::Thinking);
        assert_eq!(owl.state(), OwlState::Thinking);
        assert!(!owl.expired_at(Instant::now()));
        assert!(owl.elapsed_at(Instant::now()) < Duration::from_millis(100));
    }

    #[test]
    fn redraw_is_scheduled_at_the_next_frame_change() {
        let idle = ms_until_next_frame_change(OwlState::Idle, Duration::from_millis(1_000));
        assert_eq!(idle, Some(HALF_CLOSED_START_MS as u64 - 1_000));

        // Past the blink, the next change is the blink of the next cycle.
        let later = ms_until_next_frame_change(OwlState::Idle, Duration::from_millis(6_000));
        assert_eq!(
            later,
            Some((BLINK_CYCLE_MS - 6_000 + HALF_CLOSED_START_MS) as u64)
        );

        let pondering =
            ms_until_next_frame_change(OwlState::Thinking, Duration::from_millis(3_000));
        assert_eq!(pondering, Some(4_200 - 3_000));
        let waiting = ms_until_next_frame_change(OwlState::Thinking, Duration::from_millis(1_000));
        assert_eq!(waiting, Some(2_800 - 1_000));

        assert_eq!(
            ms_until_next_frame_change(OwlState::Done, Duration::ZERO),
            None
        );
    }

    #[test]
    fn the_happy_owl_asks_for_the_draw_that_removes_it() {
        assert_eq!(
            ms_until_done_window_ends(Duration::ZERO),
            Some(DONE_WINDOW.as_millis() as u64)
        );
        assert_eq!(
            ms_until_done_window_ends(Duration::from_millis(2_500)),
            Some(1_500)
        );
        // A window that is already over still asks for a draw rather than dropping the owl
        // with no redraw scheduled.
        assert_eq!(ms_until_done_window_ends(Duration::from_secs(9)), Some(1));
    }

    #[test]
    fn rendered_block_is_the_art_plus_the_caption() {
        // 3000 ms in: the pondering frame.
        let owl = owl_at(
            OwlState::Thinking,
            Instant::now() - Duration::from_millis(3_000),
        );
        let area = Rect::new(0, 0, 20, OWL_BLOCK_HEIGHT);
        let mut buf = Buffer::empty(area);
        owl.render(area, &mut buf);

        let rows: Vec<String> = (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let expected: Vec<String> = FRAME_PONDERING
            .lines()
            .map(|row| format!("{row}{}", " ".repeat(12)))
            .chain(std::iter::once(format!(
                "{}{}",
                owl_caption(OwlState::Thinking),
                " ".repeat(20 - owl_caption(OwlState::Thinking).width())
            )))
            .collect();
        assert_eq!(rows, expected);
    }
}

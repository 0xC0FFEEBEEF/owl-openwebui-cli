use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use std::time::Instant;

use crate::little_owl::LittleOwl;
use crate::little_owl::OwlState;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::StepStateProvider;
use crate::tui::FrameRequester;

use super::onboarding_screen::StepState;

pub(crate) struct WelcomeWidget {
    owl: LittleOwl,
    show_owl: bool,
}

impl KeyboardHandler for WelcomeWidget {
    fn handle_key_event(&mut self, _key_event: KeyEvent) {
        // The Little Owl has no variants to cycle through, so the welcome step ignores
        // keys and leaves them to the steps that take input.
    }
}

impl WelcomeWidget {
    pub(crate) fn new(
        request_frame: FrameRequester,
        animations_enabled: bool,
        show_owl: bool,
    ) -> Self {
        Self {
            owl: LittleOwl::new(OwlState::Idle, request_frame, animations_enabled),
            show_owl,
        }
    }
}

impl WidgetRef for &WelcomeWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let brand_line = Line::from(vec![
            "  ".into(),
            "Welcome to ".into(),
            "Owl".bold(),
            ", a coding agent for Open WebUI".into(),
        ]);
        // The owl is 8 columns by 4 rows plus a caption, so it fits any normal viewport; on a
        // very short one the brand line is kept and the owl steps aside.
        let mut header: Vec<Line> = Vec::new();
        if self.show_owl {
            header.extend(self.owl.lines_at(Instant::now()));
            header.push("".into());
        }
        let fits = (header.len() + 1) as u16 <= area.height;
        if fits && self.show_owl {
            self.owl.schedule_next_frame_at(Instant::now());
        }
        let lines: Vec<Line> = if fits {
            header
                .into_iter()
                .chain(std::iter::once(brand_line))
                .collect()
        } else {
            vec![brand_line]
        };

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

impl StepStateProvider for WelcomeWidget {
    fn get_step_state(&self) -> StepState {
        StepState::Complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn row_containing(buf: &Buffer, needle: &str) -> Option<u16> {
        (0..buf.area.height).find(|&y| {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            row.contains(needle)
        })
    }

    fn owl_row_count() -> u16 {
        crate::little_owl::OWL_BLOCK_HEIGHT
    }

    #[test]
    fn welcome_renders_the_owl_above_the_brand_line() {
        let widget = WelcomeWidget::new(FrameRequester::test_dummy(), true, true);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        (&widget).render(area, &mut buf);

        let welcome_row = row_containing(&buf, "Welcome");
        assert_eq!(welcome_row, Some(owl_row_count() + 1));
    }

    #[test]
    fn welcome_shows_the_owl_on_a_small_viewport() {
        let widget = WelcomeWidget::new(FrameRequester::test_dummy(), true, true);
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        (&widget).render(area, &mut buf);

        assert_eq!(row_containing(&buf, ",_,"), Some(0), "owl art drawn");
        assert_eq!(row_containing(&buf, "ready when you are"), Some(4));
        assert_eq!(row_containing(&buf, "Welcome"), Some(6));
    }

    #[test]
    fn welcome_skips_owl_animation_frames_when_animations_are_off() {
        let widget = WelcomeWidget::new(FrameRequester::test_dummy(), false, true);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        (&widget).render(area, &mut buf);

        assert_eq!(row_containing(&buf, ",_,"), Some(0));
        assert_eq!(row_containing(&buf, "Welcome"), Some(6));
    }

    #[test]
    fn welcome_steps_aside_on_a_very_short_viewport() {
        let widget = WelcomeWidget::new(FrameRequester::test_dummy(), true, true);
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        (&widget).render(area, &mut buf);

        assert_eq!(
            row_containing(&buf, ",_,"),
            None,
            "no owl art when it does not fit"
        );
        assert_eq!(row_containing(&buf, "Welcome"), Some(0), "brand line kept");
    }

    #[test]
    fn welcome_gives_way_when_the_connect_step_brings_its_own_owl() {
        let widget = WelcomeWidget::new(FrameRequester::test_dummy(), true, false);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        (&widget).render(area, &mut buf);

        assert_eq!(row_containing(&buf, ",_,"), None, "one owl per screen");
        assert_eq!(row_containing(&buf, "ready when you are"), None);
        assert_eq!(row_containing(&buf, "Welcome"), Some(0));
    }
}

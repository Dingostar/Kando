pub mod board_view;
pub mod card_detail;
pub mod card_image;
pub mod help;
pub mod input_modal;
pub mod metrics;
pub mod status_bar;
pub mod theme;

use chrono::{DateTime, Utc};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::AppState;
use kando_core::board::Board;

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 10;

/// Standard overlay rectangle used by Help, CardDetail, and Metrics.
pub fn overlay_rect(area: Rect) -> Rect {
    centered_rect(area, 70, 85, 60, 24)
}

/// Create a centered rect within `area` using percentage-based sizing with minimums.
pub fn centered_rect(area: Rect, w_pct: u16, h_pct: u16, min_w: u16, min_h: u16) -> Rect {
    let width = (area.width * w_pct / 100).max(min_w).min(area.width);
    let height = (area.height * h_pct / 100).max(min_h).min(area.height);
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    Rect::new(x, y, width, height)
}

/// Clear an overlay's area, draw its frame, and hand back the inner rect to
/// write into. The border style, corner type and padding live here so the
/// three full-panel overlays — help, card detail and metrics — cannot drift
/// apart from each other. The `input_modal` popups are a different shape
/// (small, transient, positioned by content) and deliberately do not use this.
pub fn render_overlay_frame(
    f: &mut Frame,
    area: Rect,
    bounds: Rect,
    title: ratatui::text::Line<'_>,
    border_style: ratatui::style::Style,
) -> Rect {
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding};
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title)
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(area);
    // Clear a cell beyond the frame as well, so board cards never sit flush
    // against the panel border and read as part of it.
    // `bounds` is the region the overlay owns, not the whole screen: the ring
    // must not reach past it and wipe the status bar underneath.
    f.render_widget(Clear, with_margin(area, bounds));
    f.render_widget(block, area);
    inner
}

/// `area` grown by one cell on every side, clipped to `bounds`.
fn with_margin(area: Rect, bounds: Rect) -> Rect {
    let left = area.x.saturating_sub(1).max(bounds.x);
    let top = area.y.saturating_sub(1).max(bounds.y);
    let right = area.right().saturating_add(1).min(bounds.right());
    let bottom = area.bottom().saturating_add(1).min(bounds.bottom());
    Rect::new(
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

pub fn render(f: &mut Frame, board: &Board, state: &mut AppState, now: DateTime<Utc>, kando_dir: &std::path::Path) {
    let area = f.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = format!("Terminal too small ({}x{}). Need at least {MIN_WIDTH}x{MIN_HEIGHT}.", area.width, area.height);
        let line = Line::from(Span::styled(msg, Style::default().fg(Color::Yellow)));
        let y = area.height / 2;
        let paragraph = Paragraph::new(line).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(paragraph, Rect::new(area.x, y, area.width, 1));
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(f.area());

    // Main board area
    board_view::render_board(f, chunks[0], board, state, now);

    // Status bar
    status_bar::render_status_bar(f, chunks[1], state, board);

    // Overlays
    match &mut state.mode {
        crate::app::Mode::Goto | crate::app::Mode::GotoColumn | crate::app::Mode::Space | crate::app::Mode::Column | crate::app::Mode::ColMove | crate::app::Mode::FilterMenu | crate::app::Mode::Template => {
            input_modal::render_hint_popup(f, chunks[0], &state.mode, board, &state.keys);
        }
        crate::app::Mode::Picker { title, items, selected, .. } => {
            input_modal::render_picker(f, chunks[0], title, items, *selected);
        }
        crate::app::Mode::CardDetail { scroll } => {
            let col_idx = state.focused_column;
            let card_idx = state.selected_card;
            let nerd_font = state.nerd_font;
            if let Some(column) = board.columns.get(col_idx) {
                if let Some(card) = column.cards.get(card_idx) {
                    let mut view = card_detail::DetailView {
                        card,
                        board,
                        now,
                        icons: theme::icons(nerd_font),
                        image: card_image::ImagePane {
                            images: state.images.as_mut(),
                            kando_dir,
                            column_slug: &column.slug,
                            index: state.detail_image,
                            collapsed: state.detail_image_collapsed,
                        },
                    };
                    card_detail::render_card_detail(f, chunks[0], scroll, &mut view);
                }
            }
        }
        crate::app::Mode::Help { scroll, page } => {
            help::render_help(f, chunks[0], scroll, page, &state.keys);
        }
        crate::app::Mode::Metrics { scroll } => {
            metrics::render_metrics(f, chunks[0], board, scroll, now, kando_dir);
        }
        _ => {}
    }

    // Completion popup (rendered on top of Mode::Input)
    if let Some(hint) = &state.completion_hint {
        input_modal::render_completion_popup(f, chunks[0], hint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kando_core::board::{Column, Policies};

    fn help_board() -> Board {
        Board {
            name: "Kando Test Board".into(),
            next_card_id: 100,
            policies: Policies::default(),
            sync_branch: None,
            created_at: None,
            nerd_font: false,
            columns: vec![Column {
                slug: "backlog".into(),
                name: "Backlog".into(),
                order: 0,
                wip_limit: None,
                hidden: false,
                cards: vec![kando_core::Card::new("0100".into(), "Card".into())],
            }],
        }
    }

    /// The overlay's clear ring extends one cell past the panel. It used to be
    /// clipped to the whole screen, so a full-height panel wiped the status bar
    /// underneath it. Render the real frame and read the bottom row back.
    #[test]
    fn an_overlay_never_wipes_the_status_bar() {
        let board = help_board();
        for height in [20u16, 24, 25, 26, 40] {
            let mut state = AppState::new();
            state.mode = crate::app::Mode::Help {
                scroll: 0,
                page: crate::app::HelpPage::Keybindings,
            };
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, height)).unwrap();
            let now: DateTime<Utc> = "2026-01-15T12:00:00Z".parse().unwrap();
            terminal
                .draw(|f| {
                    render(f, &board, &mut state, now, std::path::Path::new("/tmp/fake"))
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            let bottom: String = (0..80).map(|x| buf[(x, height - 1)].symbol()).collect();
            assert!(
                bottom.contains("Kando Test Board"),
                "h={height}: the help overlay's ring wiped the status bar: {bottom:?}"
            );
        }
    }

    #[test]
    fn centered_rect_basic() {
        let area = Rect::new(0, 0, 100, 100);
        let r = centered_rect(area, 80, 60, 0, 0);
        assert_eq!(r.width, 80);
        assert_eq!(r.height, 60);
        assert_eq!(r.x, 10);
        assert_eq!(r.y, 20);
    }

    #[test]
    fn centered_rect_min_width_enforced() {
        let area = Rect::new(0, 0, 100, 100);
        // 10% of 100 = 10, but min_w = 50
        let r = centered_rect(area, 10, 50, 50, 0);
        assert_eq!(r.width, 50);
    }

    #[test]
    fn centered_rect_min_height_enforced() {
        let area = Rect::new(0, 0, 100, 100);
        let r = centered_rect(area, 50, 10, 0, 40);
        assert_eq!(r.height, 40);
    }

    #[test]
    fn centered_rect_capped_at_area() {
        let area = Rect::new(0, 0, 50, 50);
        // min_w=200 exceeds area width=50, should be clamped
        let r = centered_rect(area, 100, 100, 200, 200);
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 50);
    }

    #[test]
    fn centered_rect_zero_area() {
        let area = Rect::new(0, 0, 0, 0);
        let r = centered_rect(area, 80, 60, 0, 0);
        assert_eq!(r.width, 0);
        assert_eq!(r.height, 0);
    }

    #[test]
    fn centered_rect_with_offset() {
        let area = Rect::new(10, 20, 100, 100);
        let r = centered_rect(area, 50, 50, 0, 0);
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 50);
        assert_eq!(r.x, 35); // 10 + (100-50)/2
        assert_eq!(r.y, 45); // 20 + (100-50)/2
    }

    #[test]
    fn overlay_rect_returns_expected_size() {
        let area = Rect::new(0, 0, 100, 100);
        let r = overlay_rect(area);
        assert_eq!(r.width, 70); // 70% of 100
        assert_eq!(r.height, 85); // 85% of 100
        assert_eq!(r.x, 15);     // centered
        assert_eq!(r.y, 7);      // centered (rounding down)
    }

    #[test]
    fn overlay_rect_enforces_minimums() {
        let area = Rect::new(0, 0, 50, 20);
        let r = overlay_rect(area);
        assert_eq!(r.width, 50); // min 60 clamped to area width 50
        assert_eq!(r.height, 20); // min 24 clamped to area height 20
    }

    #[test]
    fn overlay_margin_grows_by_one_and_clips_to_the_screen() {
        let screen = Rect::new(0, 0, 100, 40);
        let panel = Rect::new(10, 5, 30, 10);
        let ringed = with_margin(panel, screen);
        assert_eq!((ringed.x, ringed.y), (9, 4));
        assert_eq!((ringed.right(), ringed.bottom()), (41, 16));

        // A panel flush against the edges must not grow outside the screen.
        let flush = Rect::new(0, 0, 100, 40);
        let ringed = with_margin(flush, screen);
        assert_eq!(ringed, screen);
    }
}

use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Wrap,
};
use ratatui::Frame;
use ratatui_image::StatefulImage;

use super::board_view::card_border_style;
use super::card_image::{self, ImagePane};
use super::theme::{Icons, Theme};
use kando_core::board::age::{format_age, staleness};
use kando_core::board::{Board, Card};
use chrono::{DateTime, Utc};

/// Everything the detail pane needs, gathered into one value so the render
/// signature stays readable.
pub struct DetailView<'a> {
    pub card: &'a Card,
    /// The whole board, to resolve blockers and derive what this card blocks.
    pub board: &'a Board,
    pub now: DateTime<Utc>,
    pub icons: &'a Icons,
    pub image: ImagePane<'a>,
}

/// "Blocked by" and the derived "Blocking" lines, one card per line.
/// Resolved blockers stay listed but dimmed and struck through, so it is
/// visible why a card stopped being blocked.
fn blocker_lines<'a>(board: &'a Board, card: &'a Card, icons: &Icons) -> Vec<Line<'a>> {
    const INDENT: &str = "            ";
    let card_ref = |id: &str| -> String {
        match board.find_card(id) {
            Some((c, k)) => format!("#{id} {}", board.columns[c].cards[k].title),
            None => format!("#{id} (not on board)"),
        }
    };
    let mut lines = Vec::new();

    for (i, id) in card.blocked_by.iter().enumerate() {
        let label = if i == 0 { "Blocked by: " } else { INDENT };
        let entry = if board.blocker_is_resolved(id) {
            Span::styled(
                card_ref(id),
                Theme::dim_style().add_modifier(Modifier::CROSSED_OUT),
            )
        } else {
            Span::styled(
                format!("{} {}", icons.blocker, card_ref(id)),
                Style::default().fg(Theme::BLOCKER),
            )
        };
        lines.push(Line::from(vec![Span::styled(label, Theme::dim_style()), entry]));
    }

    for (i, id) in board.is_blocking(&card.id).iter().enumerate() {
        let label = if i == 0 { "Blocking:   " } else { INDENT };
        lines.push(Line::from(vec![
            Span::styled(label, Theme::dim_style()),
            Span::raw(card_ref(id)),
        ]));
    }

    if let Some(note) = &card.legacy_blocked {
        let note = if note.is_empty() { "(no reason)".to_string() } else { note.clone() };
        lines.push(Line::from(vec![
            Span::styled("Old block:  ", Theme::dim_style()),
            Span::styled(
                format!("{note}: re-block with card IDs"),
                Theme::dim_style(),
            ),
        ]));
    }

    lines
}

pub fn render_card_detail(f: &mut Frame, area: Rect, scroll: &mut u16, view: &mut DetailView<'_>) {
    let card = view.card;
    let board = view.board;
    let policies = &board.policies;
    let now = view.now;
    let icons = view.icons;

    let panel_area = super::overlay_rect(area);

    let stale = staleness(card, policies, now);
    let is_overdue = card.is_overdue(now.date_naive());
    let border_style = card_border_style(board.is_blocked(card), is_overdue, true, stale, false);

    let title = Line::from(Span::styled(
        format!(" {} ", card.id),
        Style::default().fg(Theme::FG).add_modifier(Modifier::BOLD),
    ));
    let inner = super::render_overlay_frame(f, panel_area, area, title, border_style);

    if inner.height == 0 {
        return;
    }

    let mut lines = Vec::new();

    // Title
    lines.push(Line::from(Span::styled(
        &card.title,
        Style::default()
            .fg(Theme::FG)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Metadata
    let age = format_age(card.created, now);
    lines.push(Line::from(vec![
        Span::styled("Age:      ", Theme::dim_style()),
        Span::raw(age),
    ]));

    // Priority with glyph
    let priority_spans = {
        let mut spans = vec![Span::styled("Priority: ", Theme::dim_style())];
        if let Some(sym) = icons.priority(card.priority) {
            let color = Theme::priority_color(card.priority);
            spans.push(Span::styled(format!("{sym} "), Style::default().fg(color)));
        }
        spans.push(Span::raw(card.priority.as_str()));
        spans
    };
    lines.push(Line::from(priority_spans));

    lines.extend(blocker_lines(board, card, icons));

    // Due date
    if let Some(due) = card.due {
        let due_str = due.format("%Y-%m-%d").to_string();
        let (label, style) = if is_overdue {
            (format!("{due_str} (overdue)"), Style::default().fg(Theme::OVERDUE))
        } else {
            (due_str, Style::default())
        };
        lines.push(Line::from(vec![
            Span::styled("Due:      ", Theme::dim_style()),
            Span::styled(label, style),
        ]));
    }

    // Assignees
    if !card.assignees.is_empty() {
        let mut spans = vec![Span::styled("Assigned: ", Theme::dim_style())];
        for (i, assignee) in card.assignees.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(", "));
            }
            spans.push(Span::styled(
                assignee.as_str(),
                Style::default().fg(Theme::ASSIGNEE),
            ));
        }
        lines.push(Line::from(spans));
    }

    // Tags
    if !card.tags.is_empty() {
        let mut tag_spans = vec![Span::styled("Tags:     ", Theme::dim_style())];
        for (i, tag) in card.tags.iter().enumerate() {
            if i > 0 {
                tag_spans.push(Span::raw(" · "));
            }
            tag_spans.push(Span::styled(
                tag.as_str(),
                Style::default().fg(Theme::tag_color(tag)),
            ));
        }
        lines.push(Line::from(tag_spans));
    }

    // Timestamps
    lines.push(Line::from(vec![
        Span::styled("Created:  ", Theme::dim_style()),
        Span::styled(
            card.created.format("%Y-%m-%d %H:%M").to_string(),
            Theme::dim_style(),
        ),
    ]));

    if let Some(started) = card.started {
        lines.push(Line::from(vec![
            Span::styled("Started:  ", Theme::dim_style()),
            Span::styled(
                started.format("%Y-%m-%d %H:%M").to_string(),
                Theme::dim_style(),
            ),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("Updated:  ", Theme::dim_style()),
        Span::styled(
            card.updated.format("%Y-%m-%d %H:%M").to_string(),
            Theme::dim_style(),
        ),
    ]));

    if let Some(completed) = card.completed {
        lines.push(Line::from(vec![
            Span::styled("Done:     ", Theme::dim_style()),
            Span::styled(
                completed.format("%Y-%m-%d %H:%M").to_string(),
                Theme::dim_style(),
            ),
        ]));
    }

    // Body
    if !card.body.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Theme::dim_style(),
        )));
        lines.push(Line::from(""));
        for body_line in card.body.lines() {
            lines.push(Line::from(body_line.to_string()));
        }
    }

    let refs = card_image::image_refs(&card.body);
    let (text_area, image_area) = split_for_image(inner, &refs, &view.image);

    let line_count = lines.len();
    let max_scroll = (line_count as u16).saturating_sub(text_area.height);
    *scroll = (*scroll).min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((*scroll, 0));
    f.render_widget(paragraph, text_area);

    if let Some(image_area) = image_area {
        render_image(f, image_area, &refs, &mut view.image);
    }

    // Scrollbar
    if max_scroll > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some("│"))
            .track_style(border_style)
            .thumb_symbol("▐")
            .thumb_style(border_style)
            .begin_symbol(None)
            .end_symbol(None);
        let mut scrollbar_state =
            ScrollbarState::new(max_scroll as usize + 1).position(*scroll as usize);
        let scrollbar_area = panel_area.inner(Margin {
            vertical: 2,
            horizontal: 0,
        });
        f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

/// Bottom slice of the detail pane reserved for the image.
///
/// Returns the whole pane for text when there is nothing to show, when the user
/// collapsed the image with `i`, or when the pane is too short to split without
/// squeezing the text into uselessness.
fn split_for_image(inner: Rect, refs: &[String], pane: &ImagePane<'_>) -> (Rect, Option<Rect>) {
    const MIN_IMAGE_HEIGHT: u16 = 7;
    const MIN_TEXT_HEIGHT: u16 = 7;

    if refs.is_empty() || pane.collapsed || inner.height < MIN_IMAGE_HEIGHT + MIN_TEXT_HEIGHT {
        return (inner, None);
    }

    let image_height = inner.height / 2;
    let text_height = inner.height - image_height;
    (
        Rect::new(inner.x, inner.y, inner.width, text_height),
        Some(Rect::new(
            inner.x,
            inner.y + text_height,
            inner.width,
            image_height,
        )),
    )
}

/// Draw the selected image with a caption line under it. Anything that goes
/// wrong — unresolvable path, undecodable file, a terminal without graphics
/// support — is reported in the caption rather than hidden.
fn render_image(f: &mut Frame, area: Rect, refs: &[String], pane: &mut ImagePane<'_>) {
    if area.height < 2 || refs.is_empty() {
        return;
    }

    let index = pane.index % refs.len();
    let reference = refs[index].as_str();
    let resolved = card_image::resolve_image(pane.kando_dir, pane.column_slug, reference);
    let name = resolved
        .as_deref()
        .map_or_else(|| reference.to_string(), card_image::short_name);

    let picture_area = Rect::new(area.x, area.y, area.width, area.height - 1);
    let caption_area = Rect::new(area.x, area.bottom() - 1, area.width, 1);

    let problem = match (resolved.as_deref(), pane.images.as_mut()) {
        (None, _) => Some(format!("not found: {reference}")),
        (Some(_), None) => Some("this terminal cannot display images".to_string()),
        (Some(path), Some(images)) => match images.protocol_for(path) {
            Ok(protocol) => {
                f.render_stateful_widget(StatefulImage::default(), picture_area, protocol);
                None
            }
            Err(e) => Some(e),
        },
    };

    let mut spans = Vec::new();
    if refs.len() > 1 {
        spans.push(Span::styled(
            format!("{}/{}  ", index + 1, refs.len()),
            Theme::dim_style(),
        ));
    }
    match &problem {
        Some(msg) => spans.push(Span::styled(
            msg.clone(),
            Style::default().fg(Theme::OVERDUE),
        )),
        None => spans.push(Span::styled(name, Theme::dim_style())),
    }
    spans.push(Span::styled(
        if refs.len() > 1 {
            "   i hide · n/p next"
        } else {
            "   i hide"
        },
        Theme::dim_style(),
    ));

    f.render_widget(Paragraph::new(Line::from(spans)), caption_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::card_image::ImagePane;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::Path;

    fn pane<'a>(kando_dir: &'a Path, collapsed: bool) -> ImagePane<'a> {
        ImagePane {
            images: None,
            kando_dir,
            column_slug: "in-progress",
            index: 0,
            collapsed,
        }
    }

    #[test]
    fn a_card_without_images_keeps_the_whole_pane_for_text() {
        let dir = Path::new("/tmp/fake/.kando");
        let inner = Rect::new(0, 0, 60, 30);
        let (text, image) = split_for_image(inner, &[], &pane(dir, false));
        assert_eq!(text, inner);
        assert!(image.is_none());
    }

    #[test]
    fn collapsing_gives_the_pane_back_to_the_text() {
        let dir = Path::new("/tmp/fake/.kando");
        let inner = Rect::new(0, 0, 60, 30);
        let refs = vec!["a.png".to_string()];
        let (text, image) = split_for_image(inner, &refs, &pane(dir, true));
        assert_eq!(text, inner);
        assert!(image.is_none());
    }

    #[test]
    fn a_short_pane_is_not_split() {
        let dir = Path::new("/tmp/fake/.kando");
        let refs = vec!["a.png".to_string()];
        for height in [1u16, 7, 13] {
            let inner = Rect::new(0, 0, 60, height);
            let (text, image) = split_for_image(inner, &refs, &pane(dir, false));
            assert_eq!(text, inner, "h={height}");
            assert!(image.is_none(), "h={height}");
        }
    }

    #[test]
    fn splitting_covers_the_pane_exactly_and_leaves_room_to_read() {
        let dir = Path::new("/tmp/fake/.kando");
        let refs = vec!["a.png".to_string()];
        for height in [14u16, 15, 30, 61] {
            let inner = Rect::new(3, 5, 60, height);
            let (text, image) = split_for_image(inner, &refs, &pane(dir, false));
            let image = image.expect("tall enough to split");

            assert_eq!(text.y, inner.y, "h={height}");
            assert_eq!(text.bottom(), image.y, "h={height}: no gap or overlap");
            assert_eq!(image.bottom(), inner.bottom(), "h={height}");
            assert!(text.height >= 7, "h={height}: text squeezed to {}", text.height);
            assert!(image.height >= 7, "h={height}: image squeezed to {}", image.height);
        }
    }

    fn render_body(body: &str, kando_dir: &Path) -> String {
        let mut card = kando_core::Card::new("1".into(), "Learn kanban".into());
        card.body = body.to_string();
        render_card_on(&card, &blocker_board(), kando_dir)
    }

    /// backlog: 2 "Write docs" · done: 3 "Set up CI"
    fn blocker_board() -> Board {
        let col = |slug: &str, order: u32, cards: Vec<Card>| kando_core::Column {
            slug: slug.into(),
            name: slug.into(),
            order,
            wip_limit: None,
            hidden: false,
            cards,
        };
        Board {
            name: "Test".into(),
            next_card_id: 10,
            policies: kando_core::Policies::default(),
            sync_branch: None,
            nerd_font: false,
            created_at: None,
            columns: vec![
                col("backlog", 0, vec![Card::new("2".into(), "Write docs".into())]),
                col("done", 1, vec![Card::new("3".into(), "Set up CI".into())]),
            ],
        }
    }

    fn render_card_on(card: &Card, board: &Board, kando_dir: &Path) -> String {
        let icons = crate::ui::theme::icons(false);
        let now: DateTime<Utc> = "2026-01-15T12:00:00Z".parse().unwrap();

        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        let mut scroll = 0;
        terminal
            .draw(|f| {
                let mut view = DetailView {
                    card,
                    board,
                    now,
                    icons,
                    image: pane(kando_dir, false),
                };
                render_card_detail(f, f.area(), &mut scroll, &mut view);
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        (0..40)
            .map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Without terminal graphics support the pane must say so rather than
    /// silently leaving a hole where the picture should be.
    #[test]
    fn a_terminal_without_graphics_support_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let kando_dir = tmp.path().join(".kando");
        std::fs::create_dir_all(kando_dir.join("assets")).unwrap();
        std::fs::write(kando_dir.join("assets/shot.png"), b"not really a png").unwrap();

        let screen = render_body("![shot](shot.png)", &kando_dir);
        assert!(
            screen.contains("cannot display images"),
            "expected a caption explaining the missing image:\n{screen}"
        );
    }

    #[test]
    fn an_unresolvable_reference_is_reported_in_the_caption() {
        let tmp = tempfile::tempdir().unwrap();
        let kando_dir = tmp.path().join(".kando");
        std::fs::create_dir_all(&kando_dir).unwrap();

        let screen = render_body("![gone](nope.png)", &kando_dir);
        assert!(
            screen.contains("not found: nope.png"),
            "expected the caption to name the missing file:\n{screen}"
        );
    }

    #[test]
    fn a_card_without_images_renders_no_caption() {
        let tmp = tempfile::tempdir().unwrap();
        let kando_dir = tmp.path().join(".kando");
        std::fs::create_dir_all(&kando_dir).unwrap();

        let screen = render_body("Just some notes.", &kando_dir);
        assert!(screen.contains("Just some notes."));
        assert!(!screen.contains("i hide"), "no image, so no image caption:\n{screen}");
    }

    #[test]
    fn detail_lists_blockers_and_what_the_card_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut board = blocker_board();
        board.columns[0].cards[0].blocked_by = vec!["1".into()]; // 2 waits on 1
        let mut card = Card::new("1".into(), "Learn kanban".into());
        card.blocked_by = vec!["2".into(), "3".into()];
        board.columns[0].cards.push(card.clone());

        let screen = render_card_on(&card, &board, tmp.path());
        assert!(screen.contains("Blocked by: X #2 Write docs"), "{screen}");
        assert!(screen.contains("#3 Set up CI"), "resolved blocker still listed:\n{screen}");
        assert!(screen.contains("Blocking:   #2 Write docs"), "{screen}");
    }

    #[test]
    fn detail_shows_legacy_block_note() {
        let tmp = tempfile::tempdir().unwrap();
        let mut card = Card::new("1".into(), "Learn kanban".into());
        card.legacy_blocked = Some("waiting for 1".into());
        let screen = render_card_on(&card, &blocker_board(), tmp.path());
        assert!(screen.contains("waiting for 1: re-block with card IDs"), "{screen}");
    }
}

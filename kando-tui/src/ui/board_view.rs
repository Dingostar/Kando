use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use ratatui::Frame;

use super::theme::{self, Icons, Theme};
use crate::app::AppState;
use kando_core::board::age::{format_age, staleness, Staleness};
use kando_core::board::{Board, Card};
use chrono::{DateTime, Utc};

/// Total display width of an icon list, including one-space separators between items.
pub(crate) fn total_icon_width(icons: &[(&str, Style)]) -> usize {
    icons.iter().map(|(t, _)| t.width()).sum::<usize>()
        + icons.len().saturating_sub(1)
}

/// Return the subset of `candidates` that fits within `avail_width`.
///
/// Icons are dropped from the left (least important first) until the remaining
/// set fits. If even the single last icon exceeds `avail_width`, returns empty.
/// Input order is preserved in the output.
pub(crate) fn fit_icons<'a>(candidates: &[(&'a str, Style)], avail_width: usize) -> Vec<(&'a str, Style)> {
    // Advance a start index from the left (dropping least-important icons first)
    // until the remaining slice fits. This avoids Vec::remove(0) element-shifting.
    let mut start = 0;
    while start + 1 < candidates.len() && total_icon_width(&candidates[start..]) > avail_width {
        start += 1;
    }
    let remaining = &candidates[start..];
    if total_icon_width(remaining) > avail_width {
        return Vec::new();
    }
    remaining.to_vec()
}

/// Border color for a card based on blocked state, column focus, and staleness.
///
/// Semantic border colors (blocker, stale, very-stale) are always visible
/// regardless of column focus, since these are information-carrying signals.
/// Only fresh cards in unfocused columns are dimmed.
///
/// Selection is expressed via `BorderType::Thick + Modifier::BOLD`, not via
/// color, so `is_selected` is intentionally absent from this function.
pub(crate) fn card_border_style(
    blocked: bool,
    is_overdue: bool,
    is_col_focused: bool,
    stale: Staleness,
    is_new: bool,
) -> Style {
    if blocked {
        return Style::default().fg(Theme::BLOCKER);
    }
    if is_overdue {
        return Style::default().fg(Theme::OVERDUE);
    }
    match stale {
        Staleness::VeryStale => Style::default().fg(Theme::VERY_STALE),
        Staleness::Stale => Style::default().fg(Theme::STALE),
        Staleness::Fresh if is_new => Style::default().fg(Theme::NEW_CARD_TITLE),
        Staleness::Fresh if is_col_focused => Style::default().fg(Theme::CARD_BORDER),
        // Receding is a modifier, not a colour. Returning a Color here meant
        // this case became indistinguishable from the focused one the moment
        // both constants resolved to Reset, and nothing could catch it.
        Staleness::Fresh => Style::default().fg(Theme::CARD_BORDER).add_modifier(Modifier::DIM),
    }
}

/// Whether a card was created on the same UTC day as `now`.
pub(crate) fn is_new_card(created: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    created.date_naive() == now.date_naive()
}

/// Blank columns between adjacent board columns.
const COLUMN_GAP: u16 = 2;

/// Columns kept clear at a column's right edge when it scrolls: one for the
/// bar, one so it does not touch the card.
const SCROLLBAR_RESERVE: u16 = 2;

pub fn render_board(f: &mut Frame, area: Rect, board: &Board, state: &AppState, now: DateTime<Utc>) {

    // Filter visible columns
    let visible_columns: Vec<(usize, &kando_core::board::Column)> = board
        .columns
        .iter()
        .enumerate()
        .filter(|(_, col)| !col.hidden || state.show_hidden_columns)
        .collect();

    if visible_columns.is_empty() {
        let msg = Paragraph::new("No visible columns. Press c then s to show hidden columns.");
        f.render_widget(msg, area);
        return;
    }

    // Split area evenly among visible columns
    let constraints: Vec<Constraint> = visible_columns
        .iter()
        .map(|_| Constraint::Ratio(1, visible_columns.len() as u32))
        .collect();
    // Columns no longer carry side borders, so the gap between them is what
    // separates one column's cards from the next.
    let col_areas = Layout::default()
        .direction(Direction::Horizontal)
        .spacing(COLUMN_GAP)
        .constraints(constraints)
        .split(area);

    let matcher = SkimMatcherV2::default();
    let icons = theme::icons(state.nerd_font);
    for (vis_idx, &(col_idx, col)) in visible_columns.iter().enumerate() {
        let is_focused = state.focused_column == col_idx;
        render_column(
            f,
            col_areas[vis_idx],
            col,
            is_focused,
            state,
            board,
            now,
            &matcher,
            icons,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_column(
    f: &mut Frame,
    area: Rect,
    col: &kando_core::board::Column,
    is_focused: bool,
    state: &AppState,
    board: &Board,
    now: chrono::DateTime<Utc>,
    matcher: &SkimMatcherV2,
    icons: &Icons,
) {
    // Text search (/) takes precedence; tag/assignee/staleness picker filters apply otherwise
    let cards: Vec<(usize, &Card)> = col
        .cards
        .iter()
        .enumerate()
        .filter(|(_, card)| {
            kando_core::board::card_is_visible(
                card,
                state.active_filter.as_deref(),
                &state.active_tag_filters,
                &state.active_assignee_filters,
                &state.active_staleness_filters,
                state.active_overdue_filter,
                &board.policies,
                now,
                matcher,
            )
        })
        .collect();

    // Build header
    let card_count = if state.has_active_filter() {
        format!("{}/{}", cards.len(), col.cards.len())
    } else {
        format!("{}", col.cards.len())
    };

    let wip_text = if let Some(limit) = col.wip_limit {
        let wip_color = if col.cards.len() as u32 > limit {
            Theme::WIP_OVER
        } else if col.cards.len() as u32 == limit {
            Theme::WIP_NEAR
        } else {
            Theme::WIP_OK
        };
        Span::styled(
            format!(" [{}/{}]", col.cards.len(), limit),
            Style::default().fg(wip_color),
        )
    } else {
        Span::raw("")
    };

    let focused_mod = if is_focused { Modifier::BOLD } else { Modifier::empty() };

    let header_line = Line::from(vec![
        Span::styled(
            format!(" {} ", col.name),
            Style::default()
                .fg(Theme::COLUMN_HEADER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({card_count})"), Theme::dim_style()),
        wip_text,
    ]);

    // Both borders keep the terminal foreground; the focused one gains weight
    // and the unfocused one recedes, rather than either claiming a palette slot.
    let border_style = if is_focused {
        Style::default()
            .fg(Theme::COLUMN_FOCUSED_BORDER)
            .add_modifier(focused_mod)
    } else {
        Style::default()
            .fg(Theme::COLUMN_BORDER)
            .add_modifier(Modifier::DIM)
    };

    // A heading and a rule, not a box. Cards already carry borders, so wrapping
    // them in another one put four vertical rules between adjacent cards and
    // spent two columns per column on chrome.
    if area.height == 0 || area.width == 0 {
        return;
    }
    f.render_widget(header_line, Rect::new(area.x, area.y, area.width, 1));
    if area.height >= 2 {
        // A hidden column keeps its dashed treatment, now on the rule.
        let rule = if col.hidden { "╌" } else { "─" };
        f.render_widget(
            Line::styled(rule.repeat(area.width as usize), border_style),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
    if area.height < 3 {
        return;
    }
    // Cards start immediately under the rule. A blank row here looked calmer but
    // cost a row in every column, and the box this replaced spent only one row
    // on chrome by using its top border as the heading.
    let inner = Rect::new(area.x, area.y + 2, area.width, area.height - 2);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Render cards
    let heights: Vec<u16> = if state.compact {
        vec![1; cards.len()]
    } else {
        cards.iter().map(|&(_, card)| card_height(card)).collect()
    };

    // Calculate scroll offset (position within the filtered list, not unfiltered index)
    let selected_in_col = if is_focused {
        cards
            .iter()
            .position(|&(real_idx, _)| real_idx == state.selected_card)
            .unwrap_or(0)
    } else {
        0
    };
    let scroll_offset = scroll_offset_for(&heights, selected_in_col, inner.height);
    let max_visible = visible_count(&heights, scroll_offset, inner.height);

    // A scrollbar needs its own column and a gap, or it draws straight onto the
    // card border beside it.
    let scrolling = cards.len() > max_visible;
    let card_width = if scrolling && inner.width > SCROLLBAR_RESERVE {
        inner.width - SCROLLBAR_RESERVE
    } else {
        inner.width
    };

    let mut y = inner.y;
    for &(real_idx, card) in cards.iter().skip(scroll_offset) {
        let height = if state.compact { 1 } else { card_height(card) };
        if y + height > inner.bottom() {
            break;
        }
        let card_area = Rect::new(inner.x, y, card_width, height);

        let is_selected = is_focused && state.selected_card == real_idx;
        if state.compact {
            render_compact_card(
                f, card_area, card, board.is_blocked(card), is_selected, is_focused, &board.policies, now, icons,
            );
        } else {
            render_card(f, card_area, card, board.is_blocked(card), is_selected, is_focused, &board.policies, now, icons);
        }
        y += height;
    }

    // Scroll indicator
    if scrolling {
        let track = if col.hidden { "╎" } else { " " };
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_symbol(Some(track))
            .track_style(border_style)
            .thumb_symbol("▐")
            .thumb_style(border_style)
            .begin_symbol(None)
            .end_symbol(None);
        let mut scrollbar_state = ScrollbarState::new(cards.len())
            .position(selected_in_col);
        let scrollbar_area = inner;
        f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

/// Truncate to `width` display columns on grapheme boundaries, marking the cut
/// with `…`. Slicing by bytes or chars would split a combining sequence or
/// mis-measure a wide glyph.
fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        // An ellipsis is still one cell wide, so returning one here overflows
        // the caller's rect by a column.
        return String::new();
    }
    let avail = width - 1; // room for the ellipsis
    let truncated: String = text
        .graphemes(true)
        .scan(0, |used, g| {
            let gw = g.width();
            (*used + gw <= avail).then(|| {
                *used += gw;
                g
            })
        })
        .collect();
    format!("{truncated}…")
}

/// One row per card: the selection marker, the title, and the same glyph
/// cluster a card carries, flushed right. No border, no id, no age — a compact
/// board is for seeing how much is where, and the detail is one Enter away.
#[allow(clippy::too_many_arguments)]
fn render_compact_card(
    f: &mut Frame,
    area: Rect,
    card: &Card,
    is_blocked: bool,
    is_selected: bool,
    is_col_focused: bool,
    policies: &kando_core::board::Policies,
    now: chrono::DateTime<Utc>,
    icons: &Icons,
) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let stale = staleness(card, policies, now);
    let is_overdue = card.is_overdue(now.date_naive());
    let is_new = is_new_card(card.created, now);

    let mut glyphs: Vec<(&str, Style)> = Vec::new();
    match stale {
        Staleness::Stale => glyphs.push((icons.stale, Style::default().fg(Theme::STALE))),
        Staleness::VeryStale => {
            glyphs.push((icons.very_stale, Style::default().fg(Theme::VERY_STALE)))
        }
        Staleness::Fresh => {}
    }
    if let Some(sym) = icons.priority(card.priority) {
        glyphs.push((sym, Style::default().fg(Theme::priority_color(card.priority))));
    }
    if is_overdue {
        glyphs.push((icons.overdue, Style::default().fg(Theme::OVERDUE)));
    }
    if is_blocked {
        glyphs.push((icons.blocker, Style::default().fg(Theme::BLOCKER)));
    }

    let marker = if is_selected {
        format!("{} ", icons.chevron)
    } else {
        "  ".to_string()
    };
    let glyph_width: usize = glyphs
        .iter()
        .map(|(t, _)| t.width())
        .sum::<usize>()
        + glyphs.len().saturating_sub(1);
    let title_width = (area.width as usize)
        .saturating_sub(marker.width() + glyph_width + 1)
        .max(1);

    // Compact rows have no border to carry the focus signal, so the title does
    // it: a row in an unfocused column recedes by modifier, exactly as a card's
    // border does. New cards are exempt here for the same reason they are in
    // `card_border_style` — being new outranks being out of focus.
    let mut title_modifier = if is_selected {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    if !is_col_focused && !is_new {
        title_modifier |= Modifier::DIM;
    }
    let title_style = Style::default()
        .fg(if is_new {
            Theme::NEW_CARD_TITLE
        } else {
            Theme::CARD_TITLE
        })
        .add_modifier(title_modifier);

    let title = truncate_to_width(&card.title, title_width);
    let mut spans = vec![
        Span::styled(marker, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(title.clone(), title_style),
    ];
    let used = spans.iter().map(|s| s.width()).sum::<usize>();
    let pad = (area.width as usize).saturating_sub(used + glyph_width);
    spans.push(Span::raw(" ".repeat(pad)));
    for (i, (text, style)) in glyphs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(text, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A card is two border rows, an id line and a title line, plus a metadata row
/// only when there are assignees or tags to put in it.
///
/// Height follows the card's contents, which means it converges to a uniform
/// grid whenever a board is used consistently: a team that tags and assigns
/// everything gets 5 rows throughout, and someone who uses neither gets 4 rows
/// throughout and a denser board for it. Only a board that mixes the two is
/// ragged, and there the difference in height is carrying the same information
/// the metadata row is.
///
/// A fixed height would match exactly one of those boards and put a blank row
/// inside every bare card on the others, which reads as metadata that failed to
/// load rather than as breathing room. Alignment across columns is not the
/// tradeoff it first appears to be either: a row is not a record here, so the
/// third card in one column has nothing to do with the third in the next.
/// Compact view is where scanning many titles wants a regular grid.
pub(crate) fn card_height(card: &Card) -> u16 {
    let has_metadata = !card.assignees.is_empty() || !card.tags.is_empty();
    if has_metadata { 5 } else { 4 }
}

/// Smallest offset that still fits the selected card in `viewport`.
pub(crate) fn scroll_offset_for(heights: &[u16], selected: usize, viewport: u16) -> usize {
    if heights.is_empty() {
        return 0;
    }
    let mut start = selected.min(heights.len() - 1);
    let mut used = u32::from(heights[start]);
    while start > 0 {
        let next = used + u32::from(heights[start - 1]);
        if next > u32::from(viewport) {
            break;
        }
        used = next;
        start -= 1;
    }
    start
}

/// How many cards fit from `offset` onward, for the scroll indicator.
pub(crate) fn visible_count(heights: &[u16], offset: usize, viewport: u16) -> usize {
    let mut used = 0u32;
    let mut count = 0;
    for height in heights.iter().skip(offset) {
        used += u32::from(*height);
        if used > u32::from(viewport) {
            break;
        }
        count += 1;
    }
    count.max(1)
}

#[allow(clippy::too_many_arguments)]
fn render_card(
    f: &mut Frame,
    area: Rect,
    card: &Card,
    is_blocked: bool,
    is_selected: bool,
    is_col_focused: bool,
    policies: &kando_core::board::Policies,
    now: chrono::DateTime<Utc>,
    icons: &Icons,
) {
    debug_assert!(!is_selected || is_col_focused, "a card cannot be selected in an unfocused column");
    if area.width < 4 || area.height < 3 {
        return;
    }

    let stale = staleness(card, policies, now);
    let age_str = format_age(card.created, now);
    let is_new = is_new_card(card.created, now);
    let is_overdue = card.is_overdue(now.date_naive());

    let selected_mod = if is_selected { Modifier::BOLD } else { Modifier::empty() };

    let border_style =
        card_border_style(is_blocked, is_overdue, is_col_focused, stale, is_new)
            .add_modifier(selected_mod);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .border_type(if is_selected { BorderType::Thick } else { BorderType::Rounded });

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 || inner.width < 2 {
        return;
    }

    // Compute left-side width up front so glyph truncation can use it.
    let marker = format!("{} ", icons.chevron);
    let marker_width = marker.width();
    let left_width = marker_width + card.id.width() + 1 + age_str.width();

    // Build icon candidates in display order: staleness (left, least important)
    // → priority (middle) → blocker (right, most important).
    // When space is tight, icons are dropped from the left first so that the
    // most important signal (blocker) survives as long as possible.
    let mut candidates: Vec<(&str, Style)> = Vec::new();

    match stale {
        Staleness::Stale => candidates.push((icons.stale, Style::default().fg(Theme::STALE))),
        Staleness::VeryStale => candidates.push((icons.very_stale, Style::default().fg(Theme::VERY_STALE))),
        Staleness::Fresh => {}
    }
    if let Some(sym) = icons.priority(card.priority) {
        candidates.push((sym, Style::default().fg(Theme::priority_color(card.priority))));
    }
    if is_overdue {
        candidates.push((icons.overdue, Style::default().fg(Theme::OVERDUE)));
    }
    if is_blocked {
        candidates.push((icons.blocker, Style::default().fg(Theme::BLOCKER)));
    }

    // Available width for icons. We reserve 1 char between the left content
    // (marker + id + age) and the icon cluster; padding_needed below fills it.
    let avail_glyph_width = (inner.width as usize).saturating_sub(left_width + 1);
    let candidates = fit_icons(&candidates, avail_glyph_width);

    // Build the final Span list with separators.
    let mut right_glyphs: Vec<Span> = Vec::new();
    for (i, (text, style)) in candidates.into_iter().enumerate() {
        if i > 0 {
            right_glyphs.push(Span::raw(" "));
        }
        right_glyphs.push(Span::styled(text, style));
    }

    // Calculate glyph width for right-alignment
    let glyphs_width: usize = right_glyphs.iter().map(|s| s.width()).sum();

    // Line 1: [marker] ID  age ... [glyphs on right]
    // marker/marker_width/left_width already computed above for glyph truncation.
    let marker_style = if is_selected {
        Style::default().fg(Theme::FG).add_modifier(selected_mod)
    } else {
        // Invisible: render as spaces matching the marker's display width
        Style::default()
    };
    let placeholder = " ".repeat(marker_width);
    let marker_display: &str = if is_selected { &marker } else { &placeholder };
    // Padding fills the gap between left content and right icons (minimum 0).
    let padding_needed =
        (inner.width as usize).saturating_sub(left_width + glyphs_width + 1);

    let age_style = if is_new {
        Style::default().fg(Theme::NEW_CARD_TITLE)
    } else {
        Theme::dim_style()
    };
    let mut line1_spans = vec![
        Span::styled(marker_display, marker_style),
        Span::styled(&card.id, Theme::dim_style().add_modifier(selected_mod)),
        Span::raw(" "),
        Span::styled(age_str, age_style.add_modifier(selected_mod)),
        Span::raw(" ".repeat(padding_needed)),
    ];
    line1_spans.extend(right_glyphs);

    // Line 2: title, indented by two, unicode-safe
    let title = format!(
        "  {}",
        truncate_to_width(&card.title, (inner.width as usize).saturating_sub(2))
    );
    let title_line = Line::from(Span::styled(
        title,
        Style::default()
            .fg(Theme::CARD_TITLE)
            .add_modifier(selected_mod),
    ));

    // Render lines
    if inner.height >= 1 {
        f.render_widget(
            Paragraph::new(Line::from(line1_spans)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    if inner.height >= 2 {
        f.render_widget(
            Paragraph::new(title_line),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    // Line 3: assignees + tags
    let has_metadata = !card.assignees.is_empty() || !card.tags.is_empty();
    if inner.height >= 3 && has_metadata {
        let mut spans = vec![Span::raw("  ")];
        let mut need_sep = false;
        for assignee in &card.assignees {
            if need_sep {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("@{assignee}"),
                Style::default()
                    .fg(Theme::ASSIGNEE)
                    .add_modifier(selected_mod),
            ));
            need_sep = true;
        }
        for tag in &card.tags {
            if need_sep {
                spans.push(Span::styled(" · ", Theme::dim_style()));
            }
            spans.push(Span::styled(
                tag.as_str(),
                Style::default()
                    .fg(Theme::tag_color(tag))
                    .add_modifier(selected_mod),
            ));
            need_sep = true;
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kando_core::board::{Board, Column, Policies};
    use ratatui::buffer::Buffer;

    fn s() -> Style { Style::default() }

    fn test_now() -> DateTime<Utc> {
        "2026-01-15T12:00:00Z".parse().unwrap()
    }

    /// One column of `count` plain, fresh cards.
    fn board_with(column: &str, count: usize) -> Board {
        Board {
            name: "Test".into(),
            next_card_id: 100,
            policies: Policies::default(),
            sync_branch: None,
            created_at: None,
            nerd_font: false,
            columns: vec![Column {
                slug: column.to_lowercase(),
                name: column.to_string(),
                order: 0,
                wip_limit: None,
                hidden: false,
                cards: (0..count)
                    .map(|i| Card::new(format!("01{i:02}"), format!("Card {i}")))
                    .collect(),
            }],
        }
    }

    // ── total_icon_width ──────────────────────────────────────────────────────

    #[test]
    fn total_icon_width_empty_is_zero() {
        assert_eq!(total_icon_width(&[]), 0);
    }

    #[test]
    fn total_icon_width_one_icon_no_separator() {
        assert_eq!(total_icon_width(&[("X", s())]), 1);
    }

    #[test]
    fn total_icon_width_two_icons_one_separator() {
        // "~" + " " + "X" = 3
        assert_eq!(total_icon_width(&[("~", s()), ("X", s())]), 3);
    }

    #[test]
    fn total_icon_width_three_icons_two_separators() {
        // "~" + " " + "!" + " " + "X" = 5
        assert_eq!(total_icon_width(&[("~", s()), ("!", s()), ("X", s())]), 5);
    }

    #[test]
    fn total_icon_width_two_char_icon_counts_correctly() {
        // "!!" (urgent, w=2) + " " + "X" = 4
        assert_eq!(total_icon_width(&[("!!", s()), ("X", s())]), 4);
    }

    // ── fit_icons ─────────────────────────────────────────────────────────────

    #[test]
    fn fit_icons_empty_input_returns_empty() {
        assert!(fit_icons(&[], 100).is_empty());
    }

    #[test]
    fn fit_icons_all_fit_returns_all() {
        // "~" + " " + "X" = 3; avail = 10
        let r = fit_icons(&[("~", s()), ("X", s())], 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "~");
        assert_eq!(r[1].0, "X");
    }

    #[test]
    fn fit_icons_exact_boundary_no_drop() {
        // "~" + " " + "X" = 3; avail = 3 → fits exactly
        let r = fit_icons(&[("~", s()), ("X", s())], 3);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn fit_icons_one_over_boundary_drops_leftmost() {
        // "~" + " " + "X" = 3; avail = 2 → drop "~", "X" = 1 fits
        let r = fit_icons(&[("~", s()), ("X", s())], 2);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "X");
    }

    #[test]
    fn fit_icons_drops_stale_when_only_blocker_fits() {
        // avail = 1: only "X" (width 1) fits
        let r = fit_icons(&[("~", s()), ("X", s())], 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "X");
    }

    #[test]
    fn fit_icons_drops_stale_and_priority_blocker_survives() {
        // 3 icons, avail = 1: drop stale + priority, keep blocker
        let r = fit_icons(&[("~", s()), ("!", s()), ("X", s())], 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "X");
    }

    #[test]
    fn fit_icons_clears_all_when_nothing_fits() {
        // "X" = 1; avail = 0 → clear
        assert!(fit_icons(&[("X", s())], 0).is_empty());
    }

    #[test]
    fn fit_icons_urgent_exact_fit() {
        // "!!" = 2; avail = 2 → fits
        let r = fit_icons(&[("!!", s())], 2);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "!!");
    }

    #[test]
    fn fit_icons_urgent_dropped_blocker_survives() {
        // "!!" + " " + "X" = 4; avail = 1 → drop "!!", "X" = 1 fits
        let r = fit_icons(&[("!!", s()), ("X", s())], 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "X");
    }

    #[test]
    fn fit_icons_preserves_input_order() {
        let r = fit_icons(&[("~", s()), ("!", s()), ("X", s())], 100);
        assert_eq!(r[0].0, "~");
        assert_eq!(r[1].0, "!");
        assert_eq!(r[2].0, "X");
    }

    #[test]
    fn fit_icons_after_left_drop_remaining_order_preserved() {
        // "~" + " " + "!" + " " + "X" = 5; avail = 3 → drop "~", "!" + " " + "X" = 3
        let r = fit_icons(&[("~", s()), ("!", s()), ("X", s())], 3);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "!");
        assert_eq!(r[1].0, "X");
    }

    // ── card_border_style ─────────────────────────────────────────────────────

    #[test]
    fn border_color_blocked_wins_over_all() {
        assert_eq!(card_border_style(true, false, true,  Staleness::Fresh, false).fg,     Some(Theme::BLOCKER));
        assert_eq!(card_border_style(true, false, false, Staleness::Fresh, false).fg,     Some(Theme::BLOCKER));
        assert_eq!(card_border_style(true, false, false, Staleness::VeryStale, false).fg, Some(Theme::BLOCKER));
        assert_eq!(card_border_style(true, false, true,  Staleness::Stale, false).fg,     Some(Theme::BLOCKER));
        // Blocker wins even for new cards
        assert_eq!(card_border_style(true, false, true,  Staleness::Fresh, true).fg,      Some(Theme::BLOCKER));
        // Blocker wins even for overdue cards
        assert_eq!(card_border_style(true, true, true,  Staleness::Fresh, false).fg,      Some(Theme::BLOCKER));
    }

    #[test]
    fn border_color_overdue_beats_staleness() {
        assert_eq!(card_border_style(false, true, true,  Staleness::Fresh, false).fg, Some(Theme::OVERDUE));
        assert_eq!(card_border_style(false, true, false, Staleness::Stale, false).fg, Some(Theme::OVERDUE));
        assert_eq!(card_border_style(false, true, true,  Staleness::VeryStale, false).fg, Some(Theme::OVERDUE));
    }

    #[test]
    fn border_color_unfocused_fresh_returns_dim() {
        // Receding is a modifier: asserting a colour here passed for free once
        // DIM and CARD_BORDER both resolved to Reset.
        let unfocused = card_border_style(false, false, false, Staleness::Fresh, false);
        let focused = card_border_style(false, false, true, Staleness::Fresh, false);
        assert!(unfocused.add_modifier.contains(Modifier::DIM));
        assert!(!focused.add_modifier.contains(Modifier::DIM));
        assert_ne!(unfocused, focused, "unfocused fresh cards must recede");
    }

    #[test]
    fn border_color_unfocused_stale_returns_stale_color() {
        assert_eq!(card_border_style(false, false, false, Staleness::Stale, false).fg, Some(Theme::STALE));
    }

    #[test]
    fn border_color_unfocused_very_stale_returns_very_stale_color() {
        assert_eq!(card_border_style(false, false, false, Staleness::VeryStale, false).fg, Some(Theme::VERY_STALE));
    }

    #[test]
    fn border_color_focused_fresh_is_card_border() {
        let focused = card_border_style(false, false, true, Staleness::Fresh, false);
        assert_eq!(focused.fg, Some(Theme::CARD_BORDER));
        assert!(!focused.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn border_color_focused_stale_is_stale_color() {
        assert_eq!(card_border_style(false, false, true, Staleness::Stale, false).fg, Some(Theme::STALE));
    }

    #[test]
    fn border_color_focused_very_stale_is_very_stale_color() {
        assert_eq!(card_border_style(false, false, true, Staleness::VeryStale, false).fg, Some(Theme::VERY_STALE));
    }

    #[test]
    fn border_color_stale_always_shows_semantic_color() {
        assert_eq!(card_border_style(false, false, true, Staleness::Stale, false).fg, Some(Theme::STALE));
        assert_eq!(card_border_style(false, false, false, Staleness::Stale, false).fg, Some(Theme::STALE));
    }

    #[test]
    fn border_color_very_stale_always_shows_semantic_color() {
        assert_eq!(card_border_style(false, false, true, Staleness::VeryStale, false).fg, Some(Theme::VERY_STALE));
        assert_eq!(card_border_style(false, false, false, Staleness::VeryStale, false).fg, Some(Theme::VERY_STALE));
    }

    #[test]
    fn border_color_new_card_is_green() {
        // New card border is green regardless of column focus
        assert_eq!(card_border_style(false, false, true, Staleness::Fresh, true).fg, Some(Theme::NEW_CARD_TITLE));
        assert_eq!(card_border_style(false, false, false, Staleness::Fresh, true).fg, Some(Theme::NEW_CARD_TITLE));
    }

    #[test]
    fn border_color_staleness_beats_new_card() {
        // Staleness takes priority over the new-green signal
        assert_eq!(card_border_style(false, false, false, Staleness::Stale, true).fg, Some(Theme::STALE));
        assert_eq!(card_border_style(false, false, false, Staleness::VeryStale, true).fg, Some(Theme::VERY_STALE));
    }

    #[test]
    fn border_color_overdue_beats_new_card() {
        assert_eq!(card_border_style(false, true, true, Staleness::Fresh, true).fg, Some(Theme::OVERDUE));
        assert_eq!(card_border_style(false, true, false, Staleness::Fresh, true).fg, Some(Theme::OVERDUE));
    }

    #[test]
    fn border_color_blocked_beats_overdue_with_stale() {
        assert_eq!(card_border_style(true, true, false, Staleness::Stale, false).fg, Some(Theme::BLOCKER));
        assert_eq!(card_border_style(true, true, true, Staleness::VeryStale, false).fg, Some(Theme::BLOCKER));
    }

    // ── is_new_card ─────────────────────────────────────────────────────────

    #[test]
    fn is_new_card_created_today_is_true() {
        use chrono::NaiveDate;
        let day = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let now = day.and_hms_opt(12, 0, 0).unwrap().and_utc();
        assert!(is_new_card(now, now));
    }

    #[test]
    fn is_new_card_created_yesterday_is_false() {
        use chrono::NaiveDate;
        let today = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let yesterday = today.pred_opt().unwrap();
        let created = yesterday.and_hms_opt(18, 0, 0).unwrap().and_utc();
        let now = today.and_hms_opt(9, 0, 0).unwrap().and_utc();
        assert!(!is_new_card(created, now));
    }

    #[test]
    fn is_new_card_same_day_start_and_end() {
        use chrono::NaiveDate;
        let day = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let start = day.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end = day.and_hms_opt(23, 59, 59).unwrap().and_utc();
        assert!(is_new_card(start, end));
    }

    #[test]
    fn is_new_card_one_second_across_midnight_is_false() {
        use chrono::NaiveDate;
        let day = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let before_midnight = day.and_hms_opt(23, 59, 59).unwrap().and_utc();
        let after_midnight = day.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc();
        assert!(!is_new_card(before_midnight, after_midnight));
    }

    #[test]
    fn card_height_drops_the_metadata_row_when_there_is_none() {
        let bare = Card::new("0001".to_string(), "Bare".to_string());
        assert_eq!(card_height(&bare), 4);

        let mut tagged = Card::new("0002".to_string(), "Tagged".to_string());
        tagged.tags.push("infra".to_string());
        assert_eq!(card_height(&tagged), 5);

        let mut assigned = Card::new("0003".to_string(), "Assigned".to_string());
        assigned.assignees.push("krfl".to_string());
        assert_eq!(card_height(&assigned), 5);
    }

    #[test]
    fn scrolling_keeps_the_selected_card_visible_at_mixed_heights() {
        let heights = [4, 5, 4, 5];
        // Everything fits: no scrolling.
        assert_eq!(scroll_offset_for(&heights, 3, 18), 0);
        // One row short of fitting all four, so the first scrolls away.
        assert_eq!(scroll_offset_for(&heights, 3, 17), 1);
        // Only the selected card fits.
        assert_eq!(scroll_offset_for(&heights, 3, 5), 3);
        // A stale selection index must not panic or run past the end.
        assert_eq!(scroll_offset_for(&heights, usize::MAX, 9), 2);
        assert_eq!(scroll_offset_for(&[], 0, 10), 0);
    }

    #[test]
    fn visible_count_measures_rows_not_cards() {
        let heights = [4, 5, 4];
        assert_eq!(visible_count(&heights, 0, 13), 3);
        assert_eq!(visible_count(&heights, 0, 9), 2);
        assert_eq!(visible_count(&heights, 0, 4), 1);
        // Never reports zero: a viewport too small for one card still shows it,
        // clipped, rather than rendering an empty column.
        assert_eq!(visible_count(&heights, 0, 1), 1);
    }

    #[test]
    fn truncation_respects_graphemes_and_display_width() {
        assert_eq!(truncate_to_width("short", 10), "short");
        assert_eq!(truncate_to_width("exactly-ten", 11), "exactly-ten");
        // A wide glyph counts as two columns, not one char.
        assert_eq!(truncate_to_width("日本語", 4).width(), 3);
        // A combining sequence is never split down the middle.
        let combined = "e\u{301}e\u{301}e\u{301}";
        let cut = truncate_to_width(combined, 2);
        assert!(cut.ends_with('…'));
        assert!(cut.width() <= 2);
        // A zero-cell budget yields zero cells: an ellipsis would overflow.
        assert_eq!(truncate_to_width("anything", 0), "");
    }

    // ── rendered output ───────────────────────────────────────────────────────
    //
    // These render through `render_board` and read the resulting cells. Asserting
    // on the helpers alone proved worthless: deleting both the scrollbar
    // reservation and the compact height branch left all 846 tests passing.

    fn render_to_buffer(board: &Board, state: &AppState, w: u16, h: u16) -> Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| render_board(f, f.area(), board, state, test_now()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// The cells of one row, as a string.
    fn row(buf: &Buffer, y: u16) -> String {
        (buf.area.x..buf.area.right())
            .map(|x| buf[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn compact_rows_are_one_line_each() {
        // Six cards, two of them carrying metadata that would push a full card
        // to five rows. In compact they must occupy one row apiece, so all six
        // appear in a viewport that could not hold two full cards.
        let mut board = board_with("Backlog", 6);
        board.columns[0].cards[1].tags.push("infra".to_string());
        board.columns[0].cards[3].assignees.push("ada".to_string());

        let mut state = AppState::new();
        state.compact = true;
        let buf = render_to_buffer(&board, &state, 40, 10);

        let text: String = (0..10).map(|y| row(&buf, y)).collect::<Vec<_>>().join("\n");
        for n in 0..6 {
            assert!(
                text.contains(&format!("Card {n}")),
                "compact dropped card {n}; rows are not one line each:\n{text}"
            );
        }
        // And no card drew a border: a box would mean full cards leaked through.
        assert!(
            !text.contains('╭') && !text.contains('┌'),
            "compact rows drew card borders:\n{text}"
        );
    }

    #[test]
    fn a_scrolling_column_keeps_the_bar_off_the_card_border() {
        // The column box used to absorb the scrollbar. Without it the bar landed
        // on the card's own right border and ate it. Check exact columns: bar at
        // the far right, a blank gap, then the card's border. Searching for "a
        // border somewhere to the left" is not enough — it finds the left border
        // and passes even with the reservation deleted.
        let board = board_with("Backlog", 12);
        let state = AppState::new();
        let width = 40u16;
        let buf = render_to_buffer(&board, &state, width, 14);
        let dump = || (0..14).map(|y| row(&buf, y)).collect::<Vec<_>>().join("\n");

        let mut bar_rows = 0;
        for y in 2..14 {
            let cells: Vec<char> = row(&buf, y).chars().collect();
            // ratatui paints the bar with half-block glyphs, not a box line.
            if !matches!(cells[usize::from(width - 1)], '▐' | '█' | '░' | '▌') {
                continue;
            }
            bar_rows += 1;
            assert_eq!(
                cells[usize::from(width - 2)],
                ' ',
                "row {y}: no gap between card and scrollbar:\n{}",
                dump()
            );
            assert!(
                matches!(cells[usize::from(width - 3)], '│' | '╮' | '╯' | '┃' | '┓' | '┛'),
                "row {y}: card's right border is missing at x={}; the bar ate it:\n{}",
                width - 3,
                dump()
            );
        }
        assert!(bar_rows > 0, "12 cards in 14 rows drew no scrollbar:\n{}", dump());
    }

    /// Cells painted for the first occurrence of `needle`, or None.
    ///
    /// Offsets are counted in display columns, not chars: one wide glyph
    /// occupies two cells, so a char count would drift on CJK text.
    fn cells_of(buf: &Buffer, needle: &str) -> Option<Vec<ratatui::buffer::Cell>> {
        for y in buf.area.y..buf.area.bottom() {
            let line = row(buf, y);
            if let Some(byte) = line.find(needle) {
                let col = line[..byte].width() as u16;
                return Some(
                    (0..needle.width() as u16)
                        .map(|i| buf[(buf.area.x + col + i, y)].clone())
                        .collect(),
                );
            }
        }
        None
    }

    /// Two columns side by side, the first focused, plus a hidden one.
    fn two_column_board() -> Board {
        let mut board = board_with("Backlog", 2);
        board.columns.push(Column {
            slug: "done".into(),
            name: "Done".into(),
            order: 1,
            wip_limit: None,
            hidden: false,
            cards: vec![Card::new("0200".into(), "Shipped".into())],
        });
        board
    }

    #[test]
    fn a_column_renders_its_name_and_card_count() {
        // The branch's headline change: columns get a heading and a rule instead
        // of a box. Deleting the heading render left all 850 tests passing, so
        // assert the text actually lands on screen.
        let board = two_column_board();
        let state = AppState::new();
        let buf = render_to_buffer(&board, &state, 80, 14);
        let dump = || (0..14).map(|y| row(&buf, y)).collect::<Vec<_>>().join("\n");

        let head = row(&buf, 0);
        assert!(head.contains("Backlog"), "no column name:\n{}", dump());
        assert!(head.contains("(2)"), "no card count for Backlog:\n{}", dump());
        assert!(head.contains("Done"), "second column has no name:\n{}", dump());
        assert!(head.contains("(1)"), "no card count for Done:\n{}", dump());
    }

    #[test]
    fn an_unfocused_column_rule_recedes() {
        // Same failure mode as the bug that started this review, one level up:
        // the rule is the column's only chrome, so it is the only thing that can
        // carry focus. Cards had two tests for this; the column rule had none.
        let board = two_column_board();
        let state = AppState::new();
        let buf = render_to_buffer(&board, &state, 80, 14);
        let rule_row = 1;

        // Column 0 is focused by default, column 1 is not. Find a rule cell in
        // each half, identified by the glyph rather than by a computed x.
        let mut focused_dim = None;
        let mut unfocused_dim = None;
        for x in 0..80u16 {
            let cell = &buf[(x, rule_row)];
            if cell.symbol() != "─" {
                continue;
            }
            if x < 40 {
                focused_dim.get_or_insert(cell.modifier.contains(Modifier::DIM));
            } else {
                unfocused_dim.get_or_insert(cell.modifier.contains(Modifier::DIM));
            }
        }
        assert_eq!(
            focused_dim,
            Some(false),
            "the focused column's rule should not recede"
        );
        assert_eq!(
            unfocused_dim,
            Some(true),
            "the unfocused column's rule should recede"
        );
    }

    #[test]
    fn a_hidden_column_keeps_a_dashed_rule() {
        // The dash is the only thing distinguishing a hidden column now that the
        // box is gone; swapping it for a solid rule broke no test.
        let mut board = two_column_board();
        board.columns[1].hidden = true;
        let mut state = AppState::new();
        // The dashed rule only renders when hidden columns are revealed; that is
        // the one situation in which a hidden column needs to be tellable apart.
        state.show_hidden_columns = true;
        let buf = render_to_buffer(&board, &state, 80, 14);
        let rule_row = row(&buf, 1);
        assert!(
            rule_row.contains('╌'),
            "a hidden column drew a solid rule:\n{rule_row}"
        );
        assert!(
            rule_row.contains('─'),
            "the visible column should still draw a solid rule:\n{rule_row}"
        );
    }

    #[test]
    fn adjacent_columns_do_not_touch() {
        // COLUMN_GAP is the whole replacement for the separation the column box
        // used to provide. At 0, adjacent cards' borders sit flush against each
        // other, which is the condition removing the box created.
        let board = two_column_board();
        let state = AppState::new();
        let buf = render_to_buffer(&board, &state, 80, 14);
        let dump = || (0..14).map(|y| row(&buf, y)).collect::<Vec<_>>().join("\n");

        // On every row that draws two cards, there must be a blank run between
        // the first card's right border and the second's left.
        let mut checked = 0;
        for y in 2..14 {
            let line = row(&buf, y);
            let chars: Vec<char> = line.chars().collect();
            let borders: Vec<usize> = chars
                .iter()
                .enumerate()
                .filter(|(_, c)| matches!(**c, '│' | '┃' | '╭' | '╮' | '┏' | '┓' | '╰' | '╯' | '┗' | '┛'))
                .map(|(i, _)| i)
                .collect();
            if borders.len() < 4 {
                continue; // not a row with two cards on it
            }
            // The gap sits between the first card's right border and the second
            // card's left border.
            let left_card_right = borders[1];
            let right_card_left = borders[2];
            assert!(
                right_card_left > left_card_right + 1,
                "row {y}: columns touch at {left_card_right}/{right_card_left}:\n{}",
                dump()
            );
            for (offset, painted) in chars[(left_card_right + 1)..right_card_left]
                .iter()
                .enumerate()
            {
                assert_eq!(
                    *painted,
                    ' ',
                    "row {y}: something painted in the column gap at {}:\n{}",
                    left_card_right + 1 + offset,
                    dump()
                );
            }
            checked += 1;
        }
        assert!(checked > 0, "found no row with two cards to check:\n{}", dump());
    }

    #[test]
    fn a_new_card_stays_bright_in_an_unfocused_compact_column() {
        // The exemption a comment claims and nothing checked: being new outranks
        // being out of focus, the same precedence card_border_style applies.
        let mut board = board_with("Backlog", 1);
        board.columns.push(Column {
            slug: "done".into(),
            name: "Done".into(),
            order: 1,
            wip_limit: None,
            hidden: false,
            cards: vec![
                {
                    let mut fresh = Card::new("0200".into(), "JustAdded".into());
                    fresh.created = test_now();
                    fresh
                },
                {
                    let mut old = Card::new("0201".into(), "LongSince".into());
                    old.created = test_now() - chrono::Duration::days(30);
                    old
                },
            ],
        });
        let mut state = AppState::new();
        state.compact = true;
        let buf = render_to_buffer(&board, &state, 80, 10);

        let dim = |needle: &str| {
            cells_of(&buf, needle)
                .unwrap_or_else(|| panic!("{needle:?} never reached the screen"))[0]
                .modifier
                .contains(Modifier::DIM)
        };
        assert!(
            !dim("JustAdded"),
            "a new card should stay bright even in an unfocused column"
        );
        assert!(
            dim("LongSince"),
            "an ordinary card in an unfocused column should recede"
        );
    }

    #[test]
    fn content_is_never_dim_on_screen() {
        // Titles, assignees and tags carry meaning, so they must render at full
        // strength. Only chrome recedes. This has to read painted cells: the two
        // previous attempts built a Style in the test body and asserted it had no
        // DIM modifier, which a locally-built style can never have. Rendering
        // every title with dim_style() left all 850 of those tests passing.
        let mut board = board_with("Backlog", 1);
        board.columns[0].cards[0].title = "Meaningful".into();
        board.columns[0].cards[0].assignees.push("ada".into());
        board.columns[0].cards[0].tags.push("infra".into());
        // A second column so the assertion covers unfocused cards too: in card
        // view the border carries focus, never the content.
        board.columns.push(Column {
            slug: "done".into(),
            name: "Done".into(),
            order: 1,
            wip_limit: None,
            hidden: false,
            cards: vec![{
                let mut c = Card::new("0200".into(), "Elsewhere".into());
                c.assignees.push("bob".into());
                c.tags.push("security".into());
                c
            }],
        });
        let state = AppState::new();
        let buf = render_to_buffer(&board, &state, 80, 14);
        let dump = || (0..14).map(|y| row(&buf, y)).collect::<Vec<_>>().join("\n");

        for needle in ["Meaningful", "@ada", "infra", "Elsewhere", "@bob", "security"] {
            let cells = cells_of(&buf, needle)
                .unwrap_or_else(|| panic!("{needle:?} never reached the screen:\n{}", dump()));
            for (i, cell) in cells.iter().enumerate() {
                assert!(
                    !cell.modifier.contains(Modifier::DIM),
                    "{needle:?} renders dim at offset {i}:\n{}",
                    dump()
                );
            }
        }

        // And a tag keeps its own palette slot rather than collapsing to the
        // default foreground, which is what made it distinguishable at all.
        let infra = cells_of(&buf, "infra").unwrap();
        let security = cells_of(&buf, "security").unwrap();
        assert_ne!(infra[0].fg, ratatui::style::Color::Reset, "tag lost its colour");
        assert_ne!(
            infra[0].fg, security[0].fg,
            "two different tags painted the same colour"
        );
    }

    #[test]
    fn compact_rows_in_an_unfocused_column_recede() {
        // Compact has no border to carry focus, so the title does. Render two
        // columns and read the titles' modifiers back off the screen.
        let mut board = board_with("Backlog", 1);
        board.columns[0].cards[0].title = "Focused".into();
        board.columns.push(Column {
            slug: "done".into(),
            name: "Done".into(),
            order: 1,
            wip_limit: None,
            hidden: false,
            cards: vec![Card::new("0200".into(), "Elsewhere".into())],
        });
        let mut state = AppState::new();
        state.compact = true;
        let buf = render_to_buffer(&board, &state, 80, 10);

        let dim_at = |needle: &str| {
            for y in 0..10 {
                let line = row(&buf, y);
                if let Some(col) = line.find(needle) {
                    return Some(buf[(col as u16, y)].modifier.contains(Modifier::DIM));
                }
            }
            None
        };
        assert_eq!(
            dim_at("Focused"),
            Some(false),
            "the focused column's row should not be dim"
        );
        assert_eq!(
            dim_at("Elsewhere"),
            Some(true),
            "the unfocused column's row should recede"
        );
    }

    #[test]
    fn an_unfocused_fresh_card_border_renders_dim_on_screen() {
        // The regression that started this: DIM became Color::Reset, so unfocused
        // cards rendered at full brightness. Read the painted cell, not the helper.
        let mut board = board_with("Backlog", 1);
        board.columns.push(kando_core::board::Column {
            slug: "done".into(),
            name: "Done".into(),
            order: 1,
            wip_limit: None,
            hidden: false,
            cards: vec![Card::new("0200".into(), "Elsewhere".into())],
        });
        let state = AppState::new();
        let buf = render_to_buffer(&board, &state, 80, 14);

        // Find a border cell belonging to the second (unfocused) column.
        let mut dim_seen = false;
        for y in 2..14 {
            for x in 40..80 {
                let cell = &buf[(x, y)];
                if matches!(cell.symbol(), "╭" | "╮" | "│" | "╰" | "╯") {
                    assert!(
                        cell.modifier.contains(Modifier::DIM),
                        "unfocused card border at ({x},{y}) is not dim"
                    );
                    dim_seen = true;
                }
            }
        }
        assert!(dim_seen, "found no unfocused card border to check");
    }
}

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::theme::Theme;
use kando_core::board::metrics::compute_metrics;
use kando_core::board::Board;

pub fn render_metrics(f: &mut Frame, area: Rect, board: &Board, scroll: &mut u16, _now: DateTime<Utc>, kando_dir: &std::path::Path) {
    let panel_area = super::overlay_rect(area);

    let title = Line::from(Span::styled(
        " Board Metrics ",
        Style::default().fg(Theme::FG).add_modifier(Modifier::BOLD),
    ));
    let inner = super::render_overlay_frame(
        f,
        panel_area,
        area,
        title,
        Style::default().fg(Theme::FG),
    );

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let metrics = compute_metrics(board, None, Some(kando_dir));

    let heading = Style::default()
        .fg(Theme::FG)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let label = Theme::dim_style();
    let value = Style::default().fg(Theme::FG).add_modifier(Modifier::BOLD);
    let dim = Theme::dim_style();

    // ── Summary ──
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "j/k scroll  |  Esc close",
            Style::default().fg(Theme::FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("Summary", heading)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Since:           ", label),
            Span::styled(metrics.since.format("%Y-%m-%d").to_string(), value),
        ]),
        Line::from(vec![
            Span::styled("  Total completed: ", label),
            Span::styled(metrics.total_completed.to_string(), value),
        ]),
        Line::from(vec![
            Span::styled("  Active WIP:      ", label),
            Span::styled(metrics.active_wip_total.to_string(), value),
        ]),
    ];

    if metrics.blocked_count > 0 {
        lines.push(Line::from(vec![
            Span::styled("  Blocked:         ", label),
            Span::styled(
                format!("{} ({:.1}%)", metrics.blocked_count, metrics.blocked_pct),
                Style::default().fg(Theme::BLOCKER).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    if let Some(ref ts) = metrics.time_stats {
        lines.push(Line::from(vec![
            Span::styled("  Cycle time P85:  ", label),
            Span::styled(format!("{:.1} days", ts.cycle_p85_days), value),
        ]));
    }
    lines.push(Line::from(""));

    // ── WIP per Column ──
    lines.push(Line::from(Span::styled(
        format!("WIP per Column (Active: {})", metrics.active_wip_total),
        heading,
    )));
    lines.push(Line::from(""));

    let max_wip = metrics.wip_per_column.iter().map(|w| w.count).max().unwrap_or(1).max(1);
    let wip_name_len = metrics
        .wip_per_column
        .iter()
        .map(|w| w.name.len())
        .max()
        .unwrap_or(0)
        .max(10);
    let wip_bar_max = (inner.width as usize).saturating_sub(wip_name_len + 19).min(40);
    for entry in &metrics.wip_per_column {
        let bar_len = if entry.count > 0 {
            (entry.count * wip_bar_max) / max_wip
        } else {
            0
        }
        .max(if entry.count > 0 { 1 } else { 0 });
        let bar: String = "\u{2588}".repeat(bar_len);
        let bar_pad = format!("{:<width$}", bar, width = wip_bar_max);

        let over_limit = entry
            .wip_limit
            .is_some_and(|l| entry.count as u32 >= l);
        let style = if over_limit {
            Style::default().fg(Theme::WIP_OVER)
        } else if entry.count == 0 {
            Theme::dim_style()
        } else {
            Style::default().fg(Theme::WIP_OK)
        };

        let count_str = if let Some(limit) = entry.wip_limit {
            format!(" {}/{}", entry.count, limit)
        } else {
            format!(" {}", entry.count)
        };

        let name_label = format!("  {:<width$} ", entry.name, width = wip_name_len);

        lines.push(Line::from(vec![
            Span::styled(name_label, label),
            Span::styled(bar_pad, style),
            Span::styled(count_str, if over_limit {
                Style::default().fg(Theme::WIP_OVER).add_modifier(Modifier::BOLD)
            } else {
                value
            }),
        ]));
    }
    lines.push(Line::from(""));

    // ── Throughput ──
    if !metrics.throughput_per_week.is_empty() {
        lines.push(Line::from(Span::styled("Throughput (cards/week)", heading)));
        lines.push(Line::from(""));

        let max_tp = metrics
            .throughput_per_week
            .iter()
            .map(|(_, c)| *c)
            .max()
            .unwrap_or(1)
            .max(1);
        let tp_bar_max = (inner.width as usize).saturating_sub(30).min(40);
        for (i, (week_label, count)) in metrics.throughput_per_week.iter().enumerate() {
            let bar_len = if *count > 0 {
                (*count as usize * tp_bar_max) / max_tp as usize
            } else {
                0
            }
            .max(if *count > 0 { 1 } else { 0 });
            let bar: String = "\u{2588}".repeat(bar_len);
            let bar_pad = format!("{:<width$}", bar, width = tp_bar_max);
            let style = if *count == 0 {
                Theme::dim_style()
            } else {
                Style::default().fg(Theme::WIP_OK)
            };
            let arrival = metrics.arrival_per_week.get(i).map_or(0, |(_, c)| *c);
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<10} ", week_label), label),
                Span::styled(bar_pad, style),
                Span::styled(format!(" {:>2}", count), value),
                Span::styled(format!("  ({arrival} arrived)"), dim),
            ]));
        }
        if let Some(stddev) = metrics.throughput_stddev {
            lines.push(Line::from(vec![
                Span::styled("  Variability: ", label),
                Span::styled(format!("stddev {:.1} cards/week", stddev), value),
            ]));
        }
        lines.push(Line::from(""));
    }

    // ── Lead Time ──
    if let Some(ref ts) = metrics.time_stats {
        lines.push(Line::from(Span::styled("Lead Time (created \u{2192} done)", heading)));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Average: ", label),
            Span::styled(format!("{:>6.1} days", ts.lead_avg_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Median:  ", label),
            Span::styled(format!("{:>6.1} days", ts.lead_median_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  P85:     ", label),
            Span::styled(format!("{:>6.1} days", ts.lead_p85_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Min:     ", label),
            Span::styled(format!("{:>6.1} days", ts.lead_min_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Max:     ", label),
            Span::styled(format!("{:>6.1} days", ts.lead_max_days), value),
        ]));
        lines.push(Line::from(""));

        // ── Cycle Time ──
        lines.push(Line::from(Span::styled("Cycle Time (started \u{2192} done)", heading)));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Average: ", label),
            Span::styled(format!("{:>6.1} days", ts.cycle_avg_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Median:  ", label),
            Span::styled(format!("{:>6.1} days", ts.cycle_median_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  P85:     ", label),
            Span::styled(format!("{:>6.1} days", ts.cycle_p85_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Min:     ", label),
            Span::styled(format!("{:>6.1} days", ts.cycle_min_days), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Max:     ", label),
            Span::styled(format!("{:>6.1} days", ts.cycle_max_days), value),
        ]));
        lines.push(Line::from(""));
    }

    // ── Stage Time ──
    if let Some(ref st) = metrics.stage_times {
        lines.push(Line::from(Span::styled(
            format!("Stage Time ({} cards with move data)", st.card_count),
            heading,
        )));
        lines.push(Line::from(""));

        let max_name_len = st
            .columns
            .iter()
            .map(|e| e.name.len())
            .max()
            .unwrap_or(0)
            .max(10);

        // Find the bottleneck (highest median among active columns)
        let max_active_median = st
            .columns
            .iter()
            .filter(|e| e.is_active)
            .map(|e| e.median_days)
            .fold(0.0f64, f64::max);

        let max_median = st
            .columns
            .iter()
            .map(|e| e.median_days)
            .fold(0.0f64, f64::max);
        let bar_max = (inner.width as usize).saturating_sub(max_name_len + 46).clamp(4, 30);

        // Column header: each data field is " %6.1fd" (8 chars), separated by "  "
        lines.push(Line::from(Span::styled(
            format!(
                "  {:<nw$} {:<bw$} {:>8}  {:>8}  {:>8}",
                "", "", "Avg", "Median", "P85",
                nw = max_name_len, bw = bar_max,
            ),
            dim,
        )));

        for entry in &st.columns {
            let bar_len = if max_median > 0.0 {
                ((entry.median_days / max_median) * bar_max as f64).round() as usize
            } else {
                0
            }
            .max(if entry.median_days > 0.0 { 1 } else { 0 });
            let bar: String = "\u{2588}".repeat(bar_len);
            let bar_pad = format!("{:<width$}", bar, width = bar_max);

            let is_bottleneck =
                entry.is_active && entry.median_days > 0.0 && entry.median_days >= max_active_median;
            let style = if is_bottleneck {
                Style::default().fg(Theme::STALE)
            } else if !entry.is_active {
                Theme::dim_style()
            } else {
                Style::default().fg(Theme::WIP_OK)
            };

            let name_label = format!("  {:<width$} ", entry.name, width = max_name_len);
            let stats_str = format!(
                " {:>6.1}d  {:>6.1}d  {:>6.1}d  (n={})",
                entry.avg_days, entry.median_days, entry.p85_days, entry.sample_count,
            );

            let entry_style = if !entry.is_active { dim } else { value };

            lines.push(Line::from(vec![
                Span::styled(name_label, if !entry.is_active { dim } else { label }),
                Span::styled(bar_pad, style),
                Span::styled(stats_str, entry_style),
            ]));
        }
        lines.push(Line::from(""));
    }

    // ── Work Item Age ──
    if let Some(ref wia) = metrics.work_item_age {
        lines.push(Line::from(Span::styled("Work Item Age", heading)));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  In-progress:  ", label),
            Span::styled(format!("{} items", wia.count), value),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Average age:  ", label),
            Span::styled(format!("{:.1} days", wia.avg_age_days), value),
        ]));
        if !wia.aging_cards.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  Aging (>P85): ", label),
                Span::styled(
                    format!("{} card(s)", wia.aging_cards.len()),
                    Style::default().fg(Theme::STALE).add_modifier(Modifier::BOLD),
                ),
            ]));
            for card in &wia.aging_cards {
                lines.push(Line::from(vec![
                    Span::styled("    ", dim),
                    Span::styled(
                        format!("#{} \"{}\" in {} ({:.1}d)", card.id, card.title, card.column, card.age_days),
                        Style::default().fg(Theme::STALE),
                    ),
                ]));
            }
        }
        lines.push(Line::from(""));
    }

    // ── Priority Breakdown ──
    if !metrics.priority_breakdown.is_empty() {
        lines.push(Line::from(Span::styled("Priority Breakdown", heading)));
        lines.push(Line::from(""));
        for (priority, count) in &metrics.priority_breakdown {
            let pcolor = Theme::priority_color(*priority);
            lines.push(Line::from(vec![
                Span::styled("  ", dim),
                Span::styled(format!("{:<10}", priority.as_str()), Style::default().fg(pcolor)),
                Span::styled(count.to_string(), value),
            ]));
        }
        lines.push(Line::from(""));
    }

    let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
    *scroll = (*scroll).min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((*scroll, 0));
    f.render_widget(paragraph, inner);
}

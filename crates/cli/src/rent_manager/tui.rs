use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table},
};
use super::state::AppState;


// --- UI Rendering Function ---
pub fn ui(f: &mut Frame, app: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), 
            Constraint::Length(8), 
            Constraint::Min(5),    
            Constraint::Length(3), 
        ])
        .split(f.area());

    let spinner = if app.is_working { ["|", "/", "-", "\\"][app.spinner_idx] } else { "✓" };
    let header_text = format!(" KORA RENT MANAGER v1.0 | {} ", spinner);
    let header = Paragraph::new(header_text)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    let stats_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    let (alert_color, alert_title) = if app.is_high_rent {
        (Color::Red, "⚠️ HIGH RENT ALERT")
    } else {
        (Color::Green, " Performance Metrics ")
    };

    let kpi_text = vec![
        Line::from(vec![Span::raw("Reclaimed SOL:   "), Span::styled(format!("{:.4}", app.total_reclaimed_sol), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("Current Locked:  "), Span::styled(format!("{:.4} SOL", app.current_locked_rent), Style::default().fg(alert_color).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("Accounts Closed: "), Span::styled(format!("{}", app.reclaimed_count), Style::default().fg(Color::Yellow))]),
    ];
    let kpi_block = Paragraph::new(kpi_text)
        .block(Block::default().title(alert_title).borders(Borders::ALL).border_style(Style::default().fg(alert_color)));
    f.render_widget(kpi_block, stats_chunks[0]);

    let gauge = Gauge::default()
        .block(Block::default().title(" Cycle Efficiency ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Magenta))
        .percent(if app.total_reclaimed_sol > 0.0 { 85 } else { 5 })
        .label(if app.total_reclaimed_sol > 0.0 { "OPTIMIZED" } else { "IDLE" });
    f.render_widget(gauge, stats_chunks[1]);

    let header_cells = ["Account", "Details"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let table_header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = app.logs.iter().rev().map(|(acc, details, color)| {
        let cells = vec![
            Cell::from(acc.clone()).style(Style::default().fg(*color).add_modifier(Modifier::BOLD)),
            Cell::from(details.clone()).style(Style::default().fg(*color)),
        ];
        Row::new(cells)
    });

    let t = Table::new(rows, [
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ])
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title(" Live Logs "))
        .column_spacing(1);
    f.render_widget(t, chunks[2]);

    let footer = Paragraph::new(format!(" {} | Press 'q' to quit ", app.status_msg))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(footer, chunks[3]);
}
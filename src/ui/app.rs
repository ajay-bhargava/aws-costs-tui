//! Application state and TUI rendering

use crate::aws::{CostData, CostExplorerClient};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Bar, BarChart, BarGroup, Block, Borders, Cell, Padding, Paragraph, Row, Table, Tabs,
    },
    Frame, Terminal,
};
use std::collections::HashMap;
use std::io;

/// Service colors for consistent coloring across views
const SERVICE_COLORS: [Color; 12] = [
    Color::Rgb(255, 107, 107),  // Coral Red
    Color::Rgb(78, 205, 196),   // Turquoise
    Color::Rgb(255, 230, 109),  // Yellow
    Color::Rgb(170, 128, 255),  // Purple
    Color::Rgb(255, 159, 243),  // Pink
    Color::Rgb(108, 255, 108),  // Lime Green
    Color::Rgb(255, 184, 77),   // Orange
    Color::Rgb(77, 182, 255),   // Sky Blue
    Color::Rgb(255, 138, 101),  // Salmon
    Color::Rgb(129, 236, 236),  // Cyan
    Color::Rgb(162, 155, 254),  // Lavender
    Color::Rgb(0, 184, 148),    // Teal
];

/// Application state
pub struct App {
    /// Current month costs
    current_month: Option<CostData>,
    /// Previous month costs
    previous_month: Option<CostData>,
    /// Monthly trend data (last 6 months)
    monthly_trend: Vec<CostData>,
    /// Selected tab index
    selected_tab: usize,
    /// Selected row in the table
    selected_row: usize,
    /// Error message if any
    error: Option<String>,
    /// Loading state
    loading: bool,
    /// Should quit
    should_quit: bool,
}

impl App {
    /// Create a new app
    pub fn new() -> Self {
        Self {
            current_month: None,
            previous_month: None,
            monthly_trend: Vec::new(),
            selected_tab: 0,
            selected_row: 0,
            error: None,
            loading: true,
            should_quit: false,
        }
    }

    /// Load data from AWS
    pub fn load_data(&mut self, client: &CostExplorerClient) {
        self.loading = true;
        self.error = None;

        // Load current month
        match client.get_current_month_costs() {
            Ok(data) => self.current_month = Some(data),
            Err(e) => {
                self.error = Some(format!("Failed to load current month: {}", e));
                self.loading = false;
                return;
            }
        }

        // Load previous month
        match client.get_previous_month_costs() {
            Ok(data) => self.previous_month = Some(data),
            Err(e) => {
                // Non-fatal, just log
                tracing::warn!("Failed to load previous month: {}", e);
            }
        }

        // Load monthly trend (last 6 months)
        match client.get_monthly_trend(6) {
            Ok(data) => self.monthly_trend = data,
            Err(e) => {
                tracing::warn!("Failed to load monthly trend: {}", e);
            }
        }

        self.loading = false;
    }

    /// Handle keyboard input
    fn handle_input(&mut self) -> Result<()> {
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                        KeyCode::Tab | KeyCode::Right => {
                            self.selected_tab = (self.selected_tab + 1) % 3;
                            self.selected_row = 0;
                        }
                        KeyCode::BackTab | KeyCode::Left => {
                            self.selected_tab = if self.selected_tab == 0 {
                                2
                            } else {
                                self.selected_tab - 1
                            };
                            self.selected_row = 0;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let max_rows = self.get_current_breakdown_len();
                            if self.selected_row < max_rows.saturating_sub(1) {
                                self.selected_row += 1;
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if self.selected_row > 0 {
                                self.selected_row -= 1;
                            }
                        }
                        KeyCode::Home | KeyCode::Char('g') => {
                            self.selected_row = 0;
                        }
                        KeyCode::End | KeyCode::Char('G') => {
                            self.selected_row = self.get_current_breakdown_len().saturating_sub(1);
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn get_current_breakdown_len(&self) -> usize {
        match self.selected_tab {
            0 => self.current_month.as_ref().map(|d| d.breakdown.len()).unwrap_or(0),
            1 => self.previous_month.as_ref().map(|d| d.breakdown.len()).unwrap_or(0),
            2 => self.get_top_services_across_months().len(),
            _ => 0,
        }
    }

    /// Get top services across all months for trend view
    fn get_top_services_across_months(&self) -> Vec<String> {
        let mut service_totals: HashMap<String, f64> = HashMap::new();
        
        for month in &self.monthly_trend {
            for service in &month.breakdown {
                *service_totals.entry(service.service.clone()).or_default() += service.cost;
            }
        }
        
        let mut services: Vec<_> = service_totals.into_iter().collect();
        services.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        services.into_iter().take(8).map(|(name, _)| name).collect()
    }

    /// Run the TUI
    pub fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Main loop
        while !self.should_quit {
            terminal.draw(|f| self.render(f))?;
            self.handle_input()?;
        }

        // Restore terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        Ok(())
    }

    /// Render the UI
    fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        // Main layout: header, tabs, content, footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Length(3), // Tabs
                Constraint::Min(10),   // Content
                Constraint::Length(3), // Footer
            ])
            .split(area);

        // Header
        self.render_header(frame, chunks[0]);

        // Tabs
        self.render_tabs(frame, chunks[1]);

        // Content based on selected tab
        match self.selected_tab {
            0 => self.render_current_month(frame, chunks[2]),
            1 => self.render_previous_month(frame, chunks[2]),
            2 => self.render_trend(frame, chunks[2]),
            _ => {}
        }

        // Footer
        self.render_footer(frame, chunks[3]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let title = Paragraph::new(vec![Line::from(vec![
            Span::styled("☁️  ", Style::default()),
            Span::styled("AWS", Style::default().fg(Color::Rgb(255, 153, 0)).bold()),
            Span::styled(" Cost Explorer ", Style::default().fg(Color::White).bold()),
            Span::styled("TUI", Style::default().fg(Color::Rgb(78, 205, 196)).bold()),
        ])])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(255, 153, 0))),
        );
        frame.render_widget(title, area);
    }

    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let titles = vec![
            Line::from(vec![
                Span::styled("📅 ", Style::default()),
                Span::styled("Current Month", Style::default().fg(Color::Rgb(108, 255, 108))),
            ]),
            Line::from(vec![
                Span::styled("📆 ", Style::default()),
                Span::styled("Previous Month", Style::default().fg(Color::Rgb(170, 128, 255))),
            ]),
            Line::from(vec![
                Span::styled("📊 ", Style::default()),
                Span::styled("6-Month Trend", Style::default().fg(Color::Rgb(255, 184, 77))),
            ]),
        ];
        let tabs = Tabs::new(titles)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Rgb(100, 100, 100)))
                    .title(Span::styled(" Views ", Style::default().fg(Color::White).bold())),
            )
            .select(self.selected_tab)
            .style(Style::default().fg(Color::DarkGray))
            .highlight_style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            )
            .divider(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        frame.render_widget(tabs, area);
    }

    fn render_current_month(&self, frame: &mut Frame, area: Rect) {
        if self.loading {
            self.render_loading(frame, area);
            return;
        }

        if let Some(ref error) = self.error {
            self.render_error(frame, area, error);
            return;
        }

        if let Some(ref data) = self.current_month {
            self.render_cost_breakdown(frame, area, data, Color::Rgb(108, 255, 108));
        } else {
            self.render_no_data(frame, area);
        }
    }

    fn render_previous_month(&self, frame: &mut Frame, area: Rect) {
        if let Some(ref data) = self.previous_month {
            self.render_cost_breakdown(frame, area, data, Color::Rgb(170, 128, 255));
        } else {
            self.render_no_data(frame, area);
        }
    }

    fn render_trend(&self, frame: &mut Frame, area: Rect) {
        if self.monthly_trend.is_empty() {
            self.render_no_data(frame, area);
            return;
        }

        // Split into chart and legend
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(area);

        // Get top services and assign colors
        let top_services = self.get_top_services_across_months();
        let service_colors: HashMap<String, Color> = top_services
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), SERVICE_COLORS[i % SERVICE_COLORS.len()]))
            .collect();

        // Create grouped bar chart - each month has bars for each service
        let bar_groups: Vec<BarGroup> = self
            .monthly_trend
            .iter()
            .map(|month| {
                let short_month = month.period.split(' ').next().unwrap_or(&month.period);
                let short_month = if short_month.len() > 3 {
                    &short_month[..3]
                } else {
                    short_month
                };

                let bars: Vec<Bar> = top_services
                    .iter()
                    .map(|service| {
                        let cost = month
                            .breakdown
                            .iter()
                            .find(|s| &s.service == service)
                            .map(|s| s.cost)
                            .unwrap_or(0.0);
                        
                        let color = service_colors.get(service).copied().unwrap_or(Color::Gray);
                        
                        Bar::default()
                            .value((cost * 100.0) as u64) // Scale for visibility
                            .style(Style::default().fg(color))
                    })
                    .collect();

                BarGroup::default()
                    .label(Line::from(Span::styled(
                        short_month.to_string(),
                        Style::default().fg(Color::White).bold(),
                    )))
                    .bars(&bars)
            })
            .collect();

        let mut bar_chart = BarChart::default()
            .block(
                Block::default()
                    .title(Span::styled(
                        " 📊 Monthly Cost Trend by Service ",
                        Style::default().fg(Color::Rgb(255, 184, 77)).bold(),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Rgb(255, 184, 77))),
            )
            .bar_width(2)
            .bar_gap(0)
            .group_gap(3)
            .value_style(Style::default().fg(Color::White));

        for group in &bar_groups {
            bar_chart = bar_chart.data(group.clone());
        }

        frame.render_widget(bar_chart, chunks[0]);

        // Legend and summary table side by side
        let bottom_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(chunks[1]);

        // Service legend
        let legend_items: Vec<Line> = top_services
            .iter()
            .enumerate()
            .map(|(i, service)| {
                let color = SERVICE_COLORS[i % SERVICE_COLORS.len()];
                let display_name = truncate_service_name(service, 25);
                Line::from(vec![
                    Span::styled("██ ", Style::default().fg(color)),
                    Span::styled(display_name, Style::default().fg(Color::White)),
                ])
            })
            .collect();

        let legend = Paragraph::new(legend_items)
            .block(
                Block::default()
                    .title(Span::styled(
                        " 🎨 Services ",
                        Style::default().fg(Color::Rgb(78, 205, 196)).bold(),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Rgb(78, 205, 196)))
                    .padding(Padding::horizontal(1)),
            );
        frame.render_widget(legend, bottom_chunks[0]);

        // Summary table with totals
        let rows: Vec<Row> = self
            .monthly_trend
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let is_current = i == self.monthly_trend.len() - 1;
                let row_style = if is_current {
                    Style::default().fg(Color::Rgb(108, 255, 108)).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                
                // Calculate month-over-month change
                let change = if i > 0 {
                    let prev = self.monthly_trend[i - 1].total_cost;
                    if prev > 0.0 {
                        ((d.total_cost - prev) / prev) * 100.0
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };

                let change_style = if change > 10.0 {
                    Style::default().fg(Color::Rgb(255, 107, 107)) // Red for increase
                } else if change < -10.0 {
                    Style::default().fg(Color::Rgb(108, 255, 108)) // Green for decrease
                } else {
                    Style::default().fg(Color::Yellow)
                };

                let change_str = if i > 0 {
                    format!("{:+.1}%", change)
                } else {
                    "—".to_string()
                };

                Row::new(vec![
                    Cell::from(Span::styled(&d.period, Style::default().fg(Color::White))),
                    Cell::from(Span::styled(
                        format!("${:.2}", d.total_cost),
                        Style::default().fg(get_cost_color(d.total_cost)).bold(),
                    )),
                    Cell::from(Span::styled(change_str, change_style)),
                ])
                .style(row_style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Percentage(45),
                Constraint::Percentage(30),
                Constraint::Percentage(25),
            ],
        )
        .header(
            Row::new(vec![
                Cell::from(Span::styled("Period", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
                Cell::from(Span::styled("Total", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
                Cell::from(Span::styled("Change", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
            ])
            .bottom_margin(1),
        )
        .block(
            Block::default()
                .title(Span::styled(
                    " 📋 Monthly Totals ",
                    Style::default().fg(Color::Rgb(255, 230, 109)).bold(),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(255, 230, 109))),
        );

        frame.render_widget(table, bottom_chunks[1]);
    }

    fn render_cost_breakdown(&self, frame: &mut Frame, area: Rect, data: &CostData, accent_color: Color) {
        // Split into summary and table (full width, no side chart)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(5)])
            .split(area);

        // Summary panel with more color
        let summary = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("📅 Period: ", Style::default().fg(Color::Gray)),
                Span::styled(&data.period, Style::default().fg(Color::White).bold()),
            ]),
            Line::from(vec![
                Span::styled("💰 Total Cost: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("${:.2}", data.total_cost),
                    Style::default().fg(get_cost_color(data.total_cost)).bold(),
                ),
                Span::styled(
                    format!(" {}", data.currency),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("  ({} services)", data.breakdown.len()),
                    Style::default().fg(Color::Rgb(170, 170, 170)),
                ),
            ]),
        ])
        .block(
            Block::default()
                .title(Span::styled(
                    " 💵 Cost Summary ",
                    Style::default().fg(accent_color).bold(),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent_color))
                .padding(Padding::horizontal(1)),
        );
        frame.render_widget(summary, chunks[0]);

        // Service breakdown table with colored bars - full width
        let rows: Vec<Row> = data
            .breakdown
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let is_selected = i == self.selected_row;
                let service_color = SERVICE_COLORS[i % SERVICE_COLORS.len()];
                
                let base_style = if is_selected {
                    Style::default().bg(Color::Rgb(60, 60, 80))
                } else {
                    Style::default()
                };

                Row::new(vec![
                    // Rank with color
                    Cell::from(Span::styled(
                        format!("#{}", i + 1),
                        Style::default().fg(Color::DarkGray),
                    )),
                    // Color indicator
                    Cell::from(Span::styled("██", Style::default().fg(service_color))),
                    // Service name
                    Cell::from(Span::styled(
                        truncate_service_name(&s.service, 40),
                        Style::default().fg(Color::White),
                    )),
                    // Cost with color based on amount
                    Cell::from(Span::styled(
                        format!("${:.2}", s.cost),
                        Style::default().fg(get_cost_color(s.cost)).bold(),
                    )),
                    // Percentage
                    Cell::from(Span::styled(
                        format!("{:.1}%", s.percentage),
                        Style::default().fg(Color::Rgb(170, 170, 170)),
                    )),
                    // Colored progress bar
                    Cell::from(Span::styled(
                        create_bar(s.percentage),
                        Style::default().fg(service_color),
                    )),
                ])
                .style(base_style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(4),     // Rank
                Constraint::Length(3),     // Color
                Constraint::Percentage(40), // Service
                Constraint::Length(12),    // Cost
                Constraint::Length(8),     // Percentage
                Constraint::Min(20),       // Bar
            ],
        )
        .header(
            Row::new(vec![
                Cell::from(Span::styled("#", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
                Cell::from(""),
                Cell::from(Span::styled("Service", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
                Cell::from(Span::styled("Cost", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
                Cell::from(Span::styled("%", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
                Cell::from(Span::styled("Distribution", Style::default().fg(Color::Rgb(255, 230, 109)).bold())),
            ])
            .bottom_margin(1),
        )
        .block(
            Block::default()
                .title(Span::styled(
                    " 📋 Service Breakdown ",
                    Style::default().fg(Color::Rgb(78, 205, 196)).bold(),
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(78, 205, 196))),
        );

        frame.render_widget(table, chunks[1]);
    }

    fn render_loading(&self, frame: &mut Frame, area: Rect) {
        let loading = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "⏳ Loading cost data from AWS...",
                Style::default().fg(Color::Rgb(255, 230, 109)).bold(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "This may take a few seconds",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(255, 230, 109))),
        );
        frame.render_widget(loading, area);
    }

    fn render_error(&self, frame: &mut Frame, area: Rect, error: &str) {
        let error_msg = Paragraph::new(vec![
            Line::from(Span::styled(
                "❌ Error",
                Style::default().fg(Color::Rgb(255, 107, 107)).bold(),
            )),
            Line::from(""),
            Line::from(Span::styled(error, Style::default().fg(Color::White))),
            Line::from(""),
            Line::from(Span::styled(
                "💡 Make sure you have:",
                Style::default().fg(Color::Rgb(255, 230, 109)),
            )),
            Line::from(Span::styled(
                "   • Valid AWS credentials configured",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "   • Cost Explorer API access (ce:GetCostAndUsage)",
                Style::default().fg(Color::Gray),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(255, 107, 107)))
                .title(Span::styled(
                    " Error ",
                    Style::default().fg(Color::Rgb(255, 107, 107)).bold(),
                )),
        );
        frame.render_widget(error_msg, area);
    }

    fn render_no_data(&self, frame: &mut Frame, area: Rect) {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "📭 No data available",
                Style::default().fg(Color::Gray),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        frame.render_widget(msg, area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let help = Paragraph::new(Line::from(vec![
            Span::styled(" q ", Style::default().fg(Color::Black).bg(Color::Rgb(255, 107, 107))),
            Span::styled(" Quit  ", Style::default().fg(Color::Gray)),
            Span::styled(" ←→ ", Style::default().fg(Color::Black).bg(Color::Rgb(78, 205, 196))),
            Span::styled(" Tab  ", Style::default().fg(Color::Gray)),
            Span::styled(" ↑↓ ", Style::default().fg(Color::Black).bg(Color::Rgb(255, 230, 109))),
            Span::styled(" Navigate  ", Style::default().fg(Color::Gray)),
            Span::styled(" g/G ", Style::default().fg(Color::Black).bg(Color::Rgb(170, 128, 255))),
            Span::styled(" Top/Bottom", Style::default().fg(Color::Gray)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(80, 80, 80)))
                .title(Span::styled(" Shortcuts ", Style::default().fg(Color::DarkGray))),
        );
        frame.render_widget(help, area);
    }
}

/// Get color based on cost value
fn get_cost_color(cost: f64) -> Color {
    if cost > 1000.0 {
        Color::Rgb(255, 107, 107) // Red
    } else if cost > 100.0 {
        Color::Rgb(255, 184, 77)  // Orange
    } else if cost > 10.0 {
        Color::Rgb(255, 230, 109) // Yellow
    } else {
        Color::Rgb(108, 255, 108) // Green
    }
}

/// Create a colorful progress bar
fn create_bar(percentage: f64) -> String {
    let width = 20;
    let filled = ((percentage / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

/// Truncate long service names
fn truncate_service_name(name: &str, max_len: usize) -> String {
    // Remove common prefixes
    let name = name
        .trim_start_matches("Amazon ")
        .trim_start_matches("AWS ")
        .trim_start_matches("Amazon");
    
    if name.len() > max_len {
        format!("{}…", &name[..max_len - 1])
    } else {
        name.to_string()
    }
}

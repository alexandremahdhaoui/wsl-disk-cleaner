use std::collections::HashSet;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use wdc_core::controller::{Cleaner, Planner, Scanner};
use wdc_core::types::{CleanOutcome, CleanupPlan, ScanResult, SizeNode, VolumeReport};
use wdc_core::util::human_bytes;

use super::present::{
    bold, commands_text, display_order, item_target, key_hints, path_spans, plan_groups,
    protected_entries, quiet, size_span, spinner_frame, strip, DELETE, PROTECTED, QUIET, RECLAIM,
    SEGMENT_COLORS,
};

const EXPAND_SHARE_OF_USED: u64 = 10;
const ROW_PREFIX_WIDTH: usize = 20;

pub fn run_scan(scanner: &dyn Scanner) -> Result<()> {
    with_terminal(|terminal| {
        let result = wait_with_spinner(terminal, "Scanning WSL and the Windows C drive", || {
            scanner.scan()
        })?;
        let mut view = ScanView::new(result);
        loop {
            terminal
                .draw(|frame| view.render(frame))
                .context("drawing the scan view")?;
            let Some(key) = next_key()? else {
                continue;
            };
            if view.handle(key) == Action::Quit {
                return Ok(());
            }
        }
    })
}

pub fn run_clean(planner: &dyn Planner, cleaner: &dyn Cleaner, home: &str) -> Result<()> {
    with_terminal(|terminal| {
        let plan = wait_with_spinner(
            terminal,
            "Looking for build caches in your home folder and in each tool",
            || planner.plan(),
        )?;
        let mut view = CleanView::new(plan, home.to_string());
        loop {
            terminal
                .draw(|frame| view.render(frame))
                .context("drawing the clean view")?;
            let Some(key) = next_key()? else {
                continue;
            };
            match view.handle(key) {
                Action::Quit => return Ok(()),
                Action::Continue => {}
                Action::Run => run_selected(terminal, &mut view, cleaner)?,
            }
        }
    })
}

fn run_selected(
    terminal: &mut DefaultTerminal,
    view: &mut CleanView,
    cleaner: &dyn Cleaner,
) -> Result<()> {
    let selected = view.selected_in_order();
    for (done, index) in selected.iter().enumerate() {
        let item = view.plan.entries[*index].item.clone();
        let title = format!(
            "Cleaning {} of {}: {} {}",
            done + 1,
            selected.len(),
            item.source,
            item_target(&item, &view.home, 60)
        );
        let outcome = wait_with_spinner(terminal, &title, || cleaner.clean(&item))?;
        view.outcomes.push((*index, outcome));
    }
    view.stage = Stage::Done;
    view.cursor = 0;
    view.offset = 0;
    Ok(())
}

fn with_terminal<T>(body: impl FnOnce(&mut DefaultTerminal) -> Result<T>) -> Result<T> {
    let mut terminal = ratatui::init();
    let result = body(&mut terminal);
    ratatui::restore();
    result
}

fn next_key() -> Result<Option<KeyCode>> {
    match event::read().context("reading a terminal event")? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Ok(Some(KeyCode::Char('q')));
            }
            Ok(Some(key.code))
        }
        _ => Ok(None),
    }
}

fn wait_with_spinner<T: Send>(
    terminal: &mut DefaultTerminal,
    title: &str,
    work: impl FnOnce() -> T + Send,
) -> Result<T> {
    let started = Instant::now();
    thread::scope(|scope| {
        let handle = scope.spawn(work);
        let mut tick = 0;
        while !handle.is_finished() {
            terminal
                .draw(|frame| draw_waiting(frame, title, tick, started.elapsed()))
                .context("drawing the progress view")?;
            if event::poll(Duration::from_millis(120)).context("polling terminal events")? {
                if let Some(KeyCode::Char('q')) = next_key()? {
                    ratatui::restore();
                    std::process::exit(130);
                }
            }
            tick += 1;
        }
        handle
            .join()
            .map_err(|_| anyhow!("{title}: the work panicked"))
    })
}

fn draw_waiting(frame: &mut Frame, title: &str, tick: usize, elapsed: Duration) {
    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(spinner_frame(tick), Style::default().fg(RECLAIM)),
            Span::raw("  "),
            Span::raw(title.to_string()),
        ]),
        Line::from(vec![
            quiet(format!("   {} s", elapsed.as_secs())),
            quiet("     q quits"),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), padded(frame.area()));
}

fn padded(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    }
}

fn render_scrolled(
    frame: &mut Frame,
    area: Rect,
    mut lines: Vec<Line<'static>>,
    cursor_line: Option<usize>,
    offset: &mut usize,
) {
    let height = usize::from(area.height).max(1);
    if let Some(cursor) = cursor_line {
        if cursor < *offset {
            *offset = cursor;
        }
        if cursor >= *offset + height {
            *offset = cursor + 1 - height;
        }
        if let Some(line) = lines.get_mut(cursor) {
            *line = line
                .clone()
                .patch_style(Style::default().add_modifier(Modifier::REVERSED));
        }
    }
    let scroll = u16::try_from(*offset).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

fn move_cursor(cursor: &mut usize, key: KeyCode, len: usize) {
    if len == 0 {
        return;
    }
    *cursor = match key {
        KeyCode::Up | KeyCode::Char('k') => cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => (*cursor + 1).min(len - 1),
        KeyCode::PageUp => cursor.saturating_sub(20),
        KeyCode::PageDown => (*cursor + 20).min(len - 1),
        KeyCode::Home => 0,
        KeyCode::End => len - 1,
        _ => *cursor,
    };
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Continue,
    Run,
    Quit,
}

struct ScanRow {
    depth: usize,
    key: String,
    name: String,
    bytes: u64,
    expandable: bool,
    open: bool,
    color: Color,
}

struct ScanView {
    result: ScanResult,
    tab: usize,
    expanded: HashSet<String>,
    cursor: usize,
    offset: usize,
}

impl ScanView {
    fn new(result: ScanResult) -> Self {
        let mut expanded = HashSet::new();
        for report in &result.reports {
            let threshold = report.usage.used_bytes / EXPAND_SHARE_OF_USED;
            collect_big(&report.usage.tree, threshold, &mut expanded);
        }
        Self {
            result,
            tab: 0,
            expanded,
            cursor: 0,
            offset: 0,
        }
    }

    fn tab_count(&self) -> usize {
        (self.result.reports.len() + self.result.failures.len()).max(1)
    }

    fn report(&self) -> Option<&VolumeReport> {
        self.result.reports.get(self.tab)
    }

    fn rows(&self) -> Vec<ScanRow> {
        let Some(report) = self.report() else {
            return vec![];
        };
        let mut rows = Vec::new();
        for (index, node) in report.usage.tree.iter().enumerate() {
            let color = SEGMENT_COLORS.get(index).copied().unwrap_or(QUIET);
            flatten(node, 0, color, &self.expanded, &mut rows);
        }
        rows
    }

    fn handle(&mut self, key: KeyCode) -> Action {
        let tabs = self.tab_count();
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.switch_tab((self.tab + 1) % tabs)
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.switch_tab((self.tab + tabs - 1) % tabs)
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle_selected(),
            other => {
                let len = self.rows().len();
                move_cursor(&mut self.cursor, other, len);
            }
        }
        Action::Continue
    }

    fn switch_tab(&mut self, tab: usize) {
        self.tab = tab;
        self.cursor = 0;
        self.offset = 0;
    }

    fn toggle_selected(&mut self) {
        let rows = self.rows();
        let Some(row) = rows.get(self.cursor) else {
            return;
        };
        if !row.expandable {
            return;
        }
        if !self.expanded.remove(&row.key) {
            self.expanded.insert(row.key.clone());
        }
    }

    fn tab_line(&self) -> Line<'static> {
        let mut spans = vec![bold("wdc scan"), Span::raw("     ")];
        let names = self
            .result
            .reports
            .iter()
            .map(|r| r.usage.volume.label().to_string())
            .chain(self.result.failures.iter().map(|f| f.source.clone()));
        for (index, name) in names.enumerate() {
            let style = if index == self.tab {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default().fg(QUIET)
            };
            spans.push(Span::styled(name, style));
            spans.push(Span::raw("   "));
        }
        Line::from(spans)
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = padded(frame.area());
        let note_lines = self.note_lines();
        let notes_height = wrapped_height(&note_lines, usize::from(area.width));
        let [tabs_area, _, summary_area, strip_area, legend_area, _, notes_area, body_area, _, detail_area, keys_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(u16::try_from(notes_height).unwrap_or(u16::MAX)),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);
        frame.render_widget(Paragraph::new(self.tab_line()), tabs_area);
        frame.render_widget(
            Paragraph::new(key_hints(&[
                ("↑↓", "move"),
                ("enter", "open or close"),
                ("tab", "switch disk"),
                ("q", "quit"),
            ])),
            keys_area,
        );
        let Some(report) = self.report() else {
            let failure = self
                .tab
                .checked_sub(self.result.reports.len())
                .and_then(|i| self.result.failures.get(i));
            let lines = match failure {
                Some(failure) => vec![
                    Line::from(Span::styled(
                        format!("The {} scan failed", failure.source),
                        Style::default().fg(DELETE).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(quiet(failure.reason.clone())),
                ],
                None => vec![Line::from(quiet("Nothing was scanned"))],
            };
            frame.render_widget(Paragraph::new(lines), body_area);
            return;
        };
        let usage = &report.usage;
        let width = usize::from(strip_area.width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                bold(human_bytes(usage.used_bytes)),
                quiet(format!(" used of {}", human_bytes(usage.capacity_bytes))),
            ])),
            summary_area,
        );
        let top: Vec<&SizeNode> = usage.tree.iter().take(SEGMENT_COLORS.len()).collect();
        let top_bytes: u64 = top.iter().map(|n| n.bytes).sum();
        let mut segments: Vec<(u64, Style)> = top
            .iter()
            .zip(SEGMENT_COLORS)
            .map(|(node, color)| (node.bytes, Style::default().fg(color)))
            .collect();
        segments.push((
            usage.used_bytes.saturating_sub(top_bytes),
            Style::default().fg(Color::Gray),
        ));
        frame.render_widget(
            Paragraph::new(strip(&segments, usage.capacity_bytes, width)),
            strip_area,
        );
        let mut legend = Vec::new();
        for (node, color) in top.iter().zip(SEGMENT_COLORS) {
            legend.push(Span::styled("■ ", Style::default().fg(color)));
            legend.push(Span::raw(node.name.clone()));
            legend.push(quiet(format!(" {}    ", human_bytes(node.bytes))));
        }
        frame.render_widget(Paragraph::new(Line::from(legend)), legend_area);
        frame.render_widget(
            Paragraph::new(note_lines).wrap(Wrap { trim: false }),
            notes_area,
        );
        let rows = self.rows();
        let lines = rows.iter().map(scan_row_line).collect();
        let detail = rows
            .get(self.cursor)
            .map(|row| row.key.clone())
            .unwrap_or_default();
        render_scrolled(frame, body_area, lines, Some(self.cursor), &mut self.offset);
        frame.render_widget(Paragraph::new(Line::from(quiet(detail))), detail_area);
    }

    fn note_lines(&self) -> Vec<Line<'static>> {
        let Some(report) = self.report() else {
            return vec![];
        };
        let mut lines = Vec::new();
        for note in &report.notes {
            lines.push(Line::from(vec![
                Span::styled(
                    note.label.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                quiet(format!("  {}", human_bytes(note.bytes))),
            ]));
            lines.push(Line::from(quiet(format!("  {}", note.hint))));
        }
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines
    }
}

fn wrapped_height(lines: &[Line], width: usize) -> usize {
    lines
        .iter()
        .map(|line| line.width().div_ceil(width.max(1)).max(1))
        .sum()
}

fn collect_big(nodes: &[SizeNode], threshold: u64, expanded: &mut HashSet<String>) {
    for node in nodes
        .iter()
        .filter(|n| n.bytes >= threshold && !n.children.is_empty())
    {
        expanded.insert(node.path.clone());
        collect_big(&node.children, threshold, expanded);
    }
}

fn flatten(
    node: &SizeNode,
    depth: usize,
    color: Color,
    expanded: &HashSet<String>,
    rows: &mut Vec<ScanRow>,
) {
    rows.push(ScanRow {
        depth,
        key: node.path.clone(),
        name: node.name.clone(),
        bytes: node.bytes,
        expandable: !node.children.is_empty(),
        open: expanded.contains(&node.path),
        color,
    });
    if expanded.contains(&node.path) {
        for child in &node.children {
            flatten(child, depth + 1, color, expanded, rows);
        }
    }
}

fn scan_row_line(row: &ScanRow) -> Line<'static> {
    let marker = match (row.expandable, row.open) {
        (false, _) => " ",
        (true, true) => "▾",
        (true, false) => "▸",
    };
    Line::from(vec![
        size_span(Some(row.bytes)),
        Span::raw("  "),
        Span::styled("▌", Style::default().fg(row.color)),
        Span::raw(" ".repeat(row.depth * 2 + 1)),
        quiet(marker),
        Span::raw(" "),
        Span::raw(row.name.clone()),
    ])
}

#[derive(Debug, PartialEq, Eq)]
enum Stage {
    Pick,
    Confirm,
    Done,
}

struct CleanView {
    plan: CleanupPlan,
    home: String,
    order: Vec<usize>,
    checked: Vec<bool>,
    cursor: usize,
    offset: usize,
    stage: Stage,
    outcomes: Vec<(usize, CleanOutcome)>,
}

impl CleanView {
    fn new(plan: CleanupPlan, home: String) -> Self {
        let checked = plan.entries.iter().map(|e| e.refusal.is_none()).collect();
        Self {
            order: display_order(&plan),
            plan,
            home,
            checked,
            cursor: 0,
            offset: 0,
            stage: Stage::Pick,
            outcomes: vec![],
        }
    }

    fn selected_in_order(&self) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .filter(|i| self.checked[*i])
            .collect()
    }

    fn selected_bytes(&self) -> u64 {
        self.selected_in_order()
            .iter()
            .map(|i| self.plan.entries[*i].item.known_bytes())
            .sum()
    }

    fn freed_bytes(&self) -> u64 {
        self.outcomes
            .iter()
            .map(|(_, outcome)| match outcome {
                CleanOutcome::Freed(bytes) => *bytes,
                _ => 0,
            })
            .sum()
    }

    fn toggle(&mut self, index: usize) {
        let Some(entry) = self.plan.entries.get(index) else {
            return;
        };
        if entry.refusal.is_none() {
            self.checked[index] = !self.checked[index];
        }
    }

    fn set_all(&mut self, value: bool) {
        for (index, entry) in self.plan.entries.iter().enumerate() {
            self.checked[index] = value && entry.refusal.is_none();
        }
    }

    fn row_count(&self) -> usize {
        match self.stage {
            Stage::Pick => self.order.len(),
            Stage::Confirm => self.selected_in_order().len(),
            Stage::Done => self.outcomes.len(),
        }
    }

    fn go_to(&mut self, stage: Stage) {
        self.stage = stage;
        self.cursor = 0;
        self.offset = 0;
    }

    fn handle(&mut self, key: KeyCode) -> Action {
        match (&self.stage, key) {
            (_, KeyCode::Char('q')) => Action::Quit,
            (Stage::Done, KeyCode::Esc | KeyCode::Enter) => Action::Quit,
            (Stage::Pick, KeyCode::Esc) => Action::Quit,
            (Stage::Pick, KeyCode::Char(' ')) => {
                if let Some(index) = self.order.get(self.cursor).copied() {
                    self.toggle(index);
                }
                Action::Continue
            }
            (Stage::Pick, KeyCode::Char('a')) => {
                self.set_all(true);
                Action::Continue
            }
            (Stage::Pick, KeyCode::Char('n')) => {
                self.set_all(false);
                Action::Continue
            }
            (Stage::Pick, KeyCode::Enter) => {
                if !self.selected_in_order().is_empty() {
                    self.go_to(Stage::Confirm);
                }
                Action::Continue
            }
            (Stage::Confirm, KeyCode::Char('y')) => Action::Run,
            (Stage::Confirm, KeyCode::Esc | KeyCode::Char('n')) => {
                self.go_to(Stage::Pick);
                Action::Continue
            }
            (_, other) => {
                let len = self.row_count();
                move_cursor(&mut self.cursor, other, len);
                Action::Continue
            }
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = padded(frame.area());
        let [title_area, _, strip_area, summary_area, _, body_area, _, detail_area, keys_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);
        let width = usize::from(body_area.width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                bold("wdc clean"),
                quiet(format!(
                    "     WSL uses {}",
                    human_bytes(self.plan.used_bytes)
                )),
            ])),
            title_area,
        );
        frame.render_widget(Paragraph::new(self.strip_line(width)), strip_area);
        frame.render_widget(Paragraph::new(self.summary_line()), summary_area);
        let (lines, cursor_lines) = match self.stage {
            Stage::Pick => self.pick_lines(width),
            Stage::Confirm => self.confirm_lines(width),
            Stage::Done => self.done_lines(width),
        };
        let cursor_line = cursor_lines.get(self.cursor).copied();
        render_scrolled(frame, body_area, lines, cursor_line, &mut self.offset);
        frame.render_widget(Paragraph::new(self.detail_line()), detail_area);
        let keys = match self.stage {
            Stage::Pick => key_hints(&[
                ("space", "check"),
                ("a", "all"),
                ("n", "none"),
                ("enter", "review"),
                ("q", "quit"),
            ]),
            Stage::Confirm => key_hints(&[
                ("y", "delete"),
                ("esc", "back"),
                ("q", "quit without deleting"),
            ]),
            Stage::Done => key_hints(&[("q", "quit")]),
        };
        frame.render_widget(Paragraph::new(keys), keys_area);
    }

    fn strip_line(&self, width: usize) -> Line<'static> {
        let highlighted = match self.stage {
            Stage::Done => self.freed_bytes(),
            _ => self.selected_bytes(),
        };
        let used = self.plan.used_bytes.max(highlighted);
        strip(
            &[
                (highlighted, Style::default().fg(RECLAIM)),
                (used - highlighted, Style::default().fg(Color::Gray)),
            ],
            used,
            width,
        )
    }

    fn summary_line(&self) -> Line<'static> {
        let selected = self.selected_bytes();
        match self.stage {
            Stage::Pick => Line::from(vec![
                Span::styled(
                    format!("{} will be freed", human_bytes(selected)),
                    Style::default().fg(RECLAIM).add_modifier(Modifier::BOLD),
                ),
                quiet(format!(
                    "     {} of {} items checked",
                    self.selected_in_order().len(),
                    self.order.len() - protected_entries(&self.plan).len()
                )),
            ]),
            Stage::Confirm => Line::from(vec![
                Span::styled(
                    format!(
                        "Delete {} items and free {}",
                        self.selected_in_order().len(),
                        human_bytes(selected)
                    ),
                    Style::default().fg(DELETE).add_modifier(Modifier::BOLD),
                ),
                quiet(format!(
                    "     WSL goes from {} to about {}",
                    human_bytes(self.plan.used_bytes),
                    human_bytes(self.plan.used_bytes.saturating_sub(selected))
                )),
            ]),
            Stage::Done => Line::from(vec![
                Span::styled(
                    format!("Freed {}", human_bytes(self.freed_bytes())),
                    Style::default().fg(RECLAIM).add_modifier(Modifier::BOLD),
                ),
                quiet("     Windows gets this space back after you compact the WSL disk. wdc scan shows how."),
            ]),
        }
    }

    fn detail_line(&self) -> Line<'static> {
        if self.stage != Stage::Pick {
            return Line::from("");
        }
        let Some(entry) = self.order.get(self.cursor).map(|i| &self.plan.entries[*i]) else {
            return Line::from("");
        };
        match &entry.refusal {
            Some(refusal) => Line::from(vec![
                Span::styled(refusal.rule.name(), Style::default().fg(PROTECTED)),
                quiet(format!(": {}", refusal.detail)),
            ]),
            None => Line::from(quiet(format!("runs {}", commands_text(&entry.item)))),
        }
    }

    fn target_spans(&self, index: usize, width: usize) -> Vec<Span<'static>> {
        let item = &self.plan.entries[index].item;
        let target = item_target(item, &self.home, width.saturating_sub(ROW_PREFIX_WIDTH));
        match item.path {
            Some(_) => path_spans(&target),
            None => vec![Span::raw(target)],
        }
    }

    fn pick_lines(&self, width: usize) -> (Vec<Line<'static>>, Vec<usize>) {
        let mut lines = Vec::new();
        let mut cursor_lines = Vec::new();
        for group in plan_groups(&self.plan) {
            let mut header = vec![bold(group.source.clone())];
            if group.entries.len() > 1 {
                header.push(quiet(format!(
                    "   {} in {} items",
                    human_bytes(group.bytes),
                    group.entries.len()
                )));
            }
            lines.push(Line::from(header));
            for index in group.entries {
                let checkbox = if self.checked[index] {
                    Span::styled(
                        "[x]",
                        Style::default().fg(RECLAIM).add_modifier(Modifier::BOLD),
                    )
                } else {
                    quiet("[ ]")
                };
                let mut spans = vec![
                    Span::raw("  "),
                    checkbox,
                    Span::raw(" "),
                    size_span(self.plan.entries[index].item.bytes),
                    Span::raw("  "),
                ];
                spans.extend(self.target_spans(index, width));
                cursor_lines.push(lines.len());
                lines.push(Line::from(spans));
            }
            lines.push(Line::from(""));
        }
        if !self.plan.empty.is_empty() {
            lines.push(Line::from(quiet(format!(
                "Nothing to clean in {}",
                self.plan.empty.join(", ")
            ))));
        }
        for unavailable in &self.plan.unavailable {
            lines.push(Line::from(vec![
                Span::styled(
                    unavailable.source.clone(),
                    Style::default().fg(Color::Yellow),
                ),
                quiet(format!(" could not be checked: {}", unavailable.reason)),
            ]));
        }
        let protected = protected_entries(&self.plan);
        if !protected.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "Protected",
                    Style::default().fg(PROTECTED).add_modifier(Modifier::BOLD),
                ),
                quiet("   the guard keeps these"),
            ]));
            for index in protected {
                let entry = &self.plan.entries[index];
                let mut spans = vec![
                    Span::raw("      "),
                    size_span(entry.item.bytes).patch_style(Style::default().fg(QUIET)),
                    Span::raw("  "),
                ];
                spans.extend(
                    self.target_spans(index, width)
                        .into_iter()
                        .map(|s| s.patch_style(Style::default().fg(QUIET))),
                );
                cursor_lines.push(lines.len());
                lines.push(Line::from(spans));
            }
        }
        (lines, cursor_lines)
    }

    fn confirm_lines(&self, width: usize) -> (Vec<Line<'static>>, Vec<usize>) {
        let mut lines = Vec::new();
        let mut cursor_lines = Vec::new();
        for index in self.selected_in_order() {
            let item = &self.plan.entries[index].item;
            let mut spans = vec![
                Span::styled("  - ", Style::default().fg(DELETE)),
                size_span(item.bytes),
                Span::raw("  "),
                Span::styled(format!("{}  ", item.source), Style::default().fg(DELETE)),
            ];
            spans.extend(self.target_spans(index, width));
            cursor_lines.push(lines.len());
            lines.push(Line::from(spans));
            lines.push(Line::from(quiet(format!(
                "               runs {}",
                commands_text(item)
            ))));
        }
        (lines, cursor_lines)
    }

    fn done_lines(&self, width: usize) -> (Vec<Line<'static>>, Vec<usize>) {
        let mut lines = Vec::new();
        let mut cursor_lines = Vec::new();
        for (index, outcome) in &self.outcomes {
            let item = &self.plan.entries[*index].item;
            let (word, color, bytes) = match outcome {
                CleanOutcome::Freed(bytes) => ("freed ", RECLAIM, Some(*bytes)),
                CleanOutcome::Refused(_) => ("kept  ", PROTECTED, item.bytes),
                CleanOutcome::Failed(_) => ("failed", DELETE, item.bytes),
            };
            let mut spans = vec![
                Span::styled(format!("  {word}"), Style::default().fg(color)),
                size_span(bytes),
                Span::raw("  "),
                bold(format!("{}  ", item.source)),
            ];
            spans.extend(self.target_spans(*index, width));
            cursor_lines.push(lines.len());
            lines.push(Line::from(spans));
            match outcome {
                CleanOutcome::Refused(refusal) => lines.push(Line::from(quiet(format!(
                    "                  {}: {}",
                    refusal.rule.name(),
                    refusal.detail
                )))),
                CleanOutcome::Failed(reason) => lines.push(Line::from(Span::styled(
                    format!("                  {reason}"),
                    Style::default().fg(DELETE),
                ))),
                CleanOutcome::Freed(_) => {}
            }
        }
        (lines, cursor_lines)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use wdc_core::types::{
        CleanupItem, PlanEntry, Refusal, Rule, ScanResult, Volume, VolumeReport, VolumeUsage,
    };

    fn entry(source: &str, bytes: u64, refused: bool) -> PlanEntry {
        PlanEntry {
            item: CleanupItem {
                source: source.into(),
                label: "build output".into(),
                path: Some(PathBuf::from(format!("/home/u/{source}/{bytes}/target"))),
                required_markers: vec![],
                bytes: Some(bytes),
                commands: vec![vec!["cargo".into(), "clean".into()]],
            },
            refusal: refused.then(|| Refusal {
                rule: Rule::TrackedByGit,
                detail: "tracked".into(),
            }),
        }
    }

    fn clean_view() -> CleanView {
        CleanView::new(
            CleanupPlan {
                used_bytes: 1000,
                entries: vec![
                    entry("Cargo", 10, false),
                    entry("Cargo", 20, true),
                    entry("npm cache", 30, false),
                ],
                empty: vec!["Podman".into()],
                unavailable: vec![],
            },
            "/home/u".into(),
        )
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_checklist_starts_with_every_allowed_item_checked_in_display_order() {
        let view = clean_view();
        assert_eq!(view.selected_in_order(), vec![2, 0]);
        assert_eq!(view.selected_bytes(), 40);
    }

    #[test]
    fn a_refused_item_can_never_be_checked() {
        let mut view = clean_view();
        view.toggle(1);
        view.set_all(true);
        assert_eq!(view.selected_in_order(), vec![2, 0]);
    }

    #[test]
    fn enter_with_nothing_checked_stays_on_the_checklist() {
        let mut view = clean_view();
        view.handle(KeyCode::Char('n'));
        view.handle(KeyCode::Enter);
        assert_eq!(view.stage, Stage::Pick);
    }

    #[test]
    fn only_y_on_the_review_screen_runs_the_cleanup() {
        let mut view = clean_view();
        assert_eq!(view.handle(KeyCode::Char('y')), Action::Continue);
        view.handle(KeyCode::Enter);
        assert_eq!(view.stage, Stage::Confirm);
        assert_eq!(view.handle(KeyCode::Esc), Action::Continue);
        assert_eq!(view.stage, Stage::Pick);
        view.handle(KeyCode::Enter);
        assert_eq!(view.handle(KeyCode::Char('y')), Action::Run);
    }

    #[test]
    fn space_unchecks_the_item_under_the_cursor_in_display_order() {
        let mut view = clean_view();
        view.handle(KeyCode::Down);
        view.handle(KeyCode::Char(' '));
        assert_eq!(view.selected_in_order(), vec![2]);
    }

    #[test]
    fn the_checklist_groups_by_tool_then_lists_empty_tools_then_protected_items() {
        let (lines, cursor_lines) = clean_view().pick_lines(100);
        let texts: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(texts[0], "npm cache");
        assert!(texts[1].ends_with("~/npm cache/30/target"));
        assert_eq!(texts[3], "Cargo");
        assert_eq!(texts[6], "Nothing to clean in Podman");
        assert_eq!(texts[8], "Protected   the guard keeps these");
        assert_eq!(cursor_lines, vec![1, 4, 9]);
    }

    fn node(path: &str, bytes: u64, children: Vec<SizeNode>) -> SizeNode {
        SizeNode {
            name: path.rsplit('/').next().unwrap().into(),
            path: path.into(),
            bytes,
            children,
        }
    }

    fn scan_view() -> ScanView {
        let tree = vec![
            node(
                "/home",
                80,
                vec![node("/home/u", 79, vec![node("/home/u/a", 5, vec![])])],
            ),
            node("/usr", 15, vec![node("/usr/lib", 9, vec![])]),
        ];
        ScanView::new(ScanResult {
            reports: vec![VolumeReport {
                usage: VolumeUsage {
                    volume: Volume::Wsl,
                    capacity_bytes: 200,
                    used_bytes: 100,
                    walked_bytes: 100,
                    tree,
                    disk_images: vec![],
                },
                notes: vec![],
            }],
            failures: vec![],
        })
    }

    #[test]
    fn the_scan_opens_folders_holding_a_tenth_of_used_space_or_more() {
        let names: Vec<_> = scan_view().rows().into_iter().map(|r| r.key).collect();
        assert_eq!(
            names,
            vec!["/home", "/home/u", "/home/u/a", "/usr", "/usr/lib"]
        );
    }

    #[test]
    fn every_row_keeps_the_color_of_its_top_level_folder() {
        let colors: Vec<_> = scan_view().rows().into_iter().map(|r| r.color).collect();
        assert_eq!(
            colors,
            vec![
                SEGMENT_COLORS[0],
                SEGMENT_COLORS[0],
                SEGMENT_COLORS[0],
                SEGMENT_COLORS[1],
                SEGMENT_COLORS[1]
            ]
        );
    }

    #[test]
    fn enter_closes_an_open_folder() {
        let mut view = scan_view();
        view.handle(KeyCode::Enter);
        let names: Vec<_> = view.rows().into_iter().map(|r| r.key).collect();
        assert_eq!(names, vec!["/home", "/usr", "/usr/lib"]);
    }
}

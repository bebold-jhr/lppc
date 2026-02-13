use anyhow::{bail, Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::collections::HashSet;
use std::io::{stdout, Stdout};

use crate::action::{compute_selected_actions, Action, ComputedActions, SelectedActions};
use crate::block_type::BlockType;
use crate::service::ServiceReference;

/// RAII guard to ensure terminal state is restored even on panic
struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode().context("Failed to enable raw mode")?;
        stdout()
            .execute(EnterAlternateScreen)
            .context("Failed to enter alternate screen")?;
        Ok(Self { active: true })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = stdout().execute(LeaveAlternateScreen);
            let _ = disable_raw_mode();
        }
    }
}

fn create_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    Terminal::new(CrosstermBackend::new(stdout())).context("Failed to create terminal")
}

fn render_controls_simple(frame: &mut Frame, area: Rect) {
    let controls = Line::from(vec![
        Span::styled("[ENTER]", Style::default().fg(Color::Green)),
        Span::raw(" Select  "),
        Span::styled("[↑↓]", Style::default().fg(Color::Cyan)),
        Span::raw(" Navigate  "),
        Span::styled("[q]", Style::default().fg(Color::Red)),
        Span::raw(" Quit"),
    ]);
    frame.render_widget(Paragraph::new(controls), area);
}

// ============================================================================
// Single Selection Component (for block type, terraform type, service prefix)
// ============================================================================

struct SingleSelector {
    all_items: Vec<String>,
    filtered_indices: Vec<usize>,
    cursor_position: usize,
    list_state: ListState,
    title: String,
    filter_text: String,
    filterable: bool,
}

impl SingleSelector {
    fn new(items: Vec<String>, title: &str, initial_position: usize, filterable: bool) -> Self {
        let filtered_indices: Vec<usize> = (0..items.len()).collect();
        let cursor_position = initial_position.min(items.len().saturating_sub(1));
        let mut list_state = ListState::default();
        list_state.select(Some(cursor_position));
        Self {
            all_items: items,
            filtered_indices,
            cursor_position,
            list_state,
            title: title.to_string(),
            filter_text: String::new(),
            filterable,
        }
    }

    fn update_filter(&mut self) {
        let filter_lower = self.filter_text.to_lowercase();
        self.filtered_indices = self
            .all_items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.to_lowercase().contains(&filter_lower))
            .map(|(i, _)| i)
            .collect();

        self.cursor_position = 0;
        if !self.filtered_indices.is_empty() {
            self.list_state.select(Some(0));
        } else {
            self.list_state.select(None);
        }
    }

    fn add_char(&mut self, c: char) {
        if self.filterable {
            self.filter_text.push(c);
            self.update_filter();
        }
    }

    fn remove_char(&mut self) {
        if self.filterable && !self.filter_text.is_empty() {
            self.filter_text.pop();
            self.update_filter();
        }
    }

    fn move_up(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.list_state.select(Some(self.cursor_position));
        }
    }

    fn move_down(&mut self) {
        if self.cursor_position < self.filtered_indices.len().saturating_sub(1) {
            self.cursor_position += 1;
            self.list_state.select(Some(self.cursor_position));
        }
    }

    fn selected_original_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.cursor_position).copied()
    }

    fn visible_items(&self) -> Vec<&str> {
        self.filtered_indices
            .iter()
            .map(|&i| self.all_items[i].as_str())
            .collect()
    }
}

fn render_single_selector(frame: &mut Frame, selector: &mut SingleSelector) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(frame.area());

    let visible_items = selector.visible_items();
    let items: Vec<ListItem> = visible_items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == selector.cursor_position {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("  {}", item)).style(style)
        })
        .collect();

    let title = if selector.filterable && !selector.filter_text.is_empty() {
        format!(" {} (filter: {}) ", selector.title, selector.filter_text)
    } else if selector.filterable {
        format!(" {} (type to filter) ", selector.title)
    } else {
        format!(" {} ", selector.title)
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, chunks[0], &mut selector.list_state);

    if selector.filterable {
        render_controls_filterable(frame, chunks[1]);
    } else {
        render_controls_simple(frame, chunks[1]);
    }
}

fn render_controls_filterable(frame: &mut Frame, area: Rect) {
    let controls = Line::from(vec![
        Span::styled("[ENTER]", Style::default().fg(Color::Green)),
        Span::raw(" Select  "),
        Span::styled("[↑↓]", Style::default().fg(Color::Cyan)),
        Span::raw(" Navigate  "),
        Span::styled("[type]", Style::default().fg(Color::Cyan)),
        Span::raw(" Filter  "),
        Span::styled("[ESC]", Style::default().fg(Color::Red)),
        Span::raw(" Quit"),
    ]);
    frame.render_widget(Paragraph::new(controls), area);
}

fn run_single_selector(
    items: Vec<String>,
    title: &str,
    initial_position: usize,
    filterable: bool,
) -> Result<usize> {
    let _guard = TerminalGuard::new()?;
    let mut terminal = create_terminal()?;
    let mut selector = SingleSelector::new(items, title, initial_position, filterable);

    loop {
        terminal
            .draw(|frame| render_single_selector(frame, &mut selector))
            .context("Failed to draw UI")?;

        if let Event::Key(key) = event::read().context("Failed to read event")? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Up => selector.move_up(),
                KeyCode::Down => selector.move_down(),
                KeyCode::Enter => {
                    if let Some(index) = selector.selected_original_index() {
                        return Ok(index);
                    }
                }
                KeyCode::Backspace => selector.remove_char(),
                KeyCode::Esc => {
                    bail!("Selection cancelled by user");
                }
                KeyCode::Char(c) => {
                    if selector.filterable {
                        selector.add_char(c);
                    } else if c == 'k' {
                        selector.move_up();
                    } else if c == 'j' {
                        selector.move_down();
                    } else if c == 'q' {
                        bail!("Selection cancelled by user");
                    }
                }
                _ => {}
            }
        }
    }
}

// ============================================================================
// Public Selection Functions
// ============================================================================

pub fn select_block_type() -> Result<BlockType> {
    let options: Vec<String> = BlockType::ALL
        .iter()
        .map(|bt| bt.as_str().to_string())
        .collect();
    let selected_index = run_single_selector(options, "Select a block type", 0, false)?;

    Ok(BlockType::ALL[selected_index])
}

pub fn select_terraform_type(types: Vec<String>) -> Result<String> {
    let selected_index = run_single_selector(types.clone(), "Select a Terraform type", 0, true)?;
    Ok(types[selected_index].clone())
}

pub fn select_service_prefix(
    services: Vec<ServiceReference>,
    preselected_index: Option<usize>,
) -> Result<ServiceReference> {
    let service_names: Vec<String> = services.iter().map(|s| s.service.clone()).collect();
    let initial_position = preselected_index.unwrap_or(0);
    let selected_index =
        run_single_selector(service_names, "Select a service prefix", initial_position, true)?;

    Ok(services[selected_index].clone())
}

// ============================================================================
// Multi-Selection Component (for actions)
// ============================================================================

const ACTION_WARNING: &str =
    "Warning: Do not select read actions on data (e.g., s3:GetObject). Only select actions for infrastructure management.";

struct ActionSelector<'a> {
    actions: &'a [Action],
    service_prefix: &'a str,
    allow_indices: HashSet<usize>,
    deny_indices: HashSet<usize>,
    filtered_indices: Vec<usize>,
    cursor_position: usize,
    list_state: ListState,
    filter_text: String,
}

impl<'a> ActionSelector<'a> {
    fn new(actions: &'a [Action], service_prefix: &'a str, preselected_indices: &[usize]) -> Self {
        let allow_indices: HashSet<usize> = preselected_indices.iter().copied().collect();
        let deny_indices: HashSet<usize> = HashSet::new();
        let filtered_indices: Vec<usize> = (0..actions.len()).collect();
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            actions,
            service_prefix,
            allow_indices,
            deny_indices,
            filtered_indices,
            cursor_position: 0,
            list_state,
            filter_text: String::new(),
        }
    }

    fn update_filter(&mut self) {
        let filter_lower = self.filter_text.to_lowercase();
        self.filtered_indices = self
            .actions
            .iter()
            .enumerate()
            .filter(|(_, action)| action.name.to_lowercase().contains(&filter_lower))
            .map(|(i, _)| i)
            .collect();

        self.cursor_position = 0;
        if !self.filtered_indices.is_empty() {
            self.list_state.select(Some(0));
        } else {
            self.list_state.select(None);
        }
    }

    fn add_char(&mut self, c: char) {
        self.filter_text.push(c);
        self.update_filter();
    }

    fn remove_char(&mut self) {
        if !self.filter_text.is_empty() {
            self.filter_text.pop();
            self.update_filter();
        }
    }

    fn current_original_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.cursor_position).copied()
    }

    fn cycle_current(&mut self) {
        if let Some(original_index) = self.current_original_index() {
            if self.allow_indices.contains(&original_index) {
                // allow -> deny
                self.allow_indices.remove(&original_index);
                self.deny_indices.insert(original_index);
            } else if self.deny_indices.contains(&original_index) {
                // deny -> deselected
                self.deny_indices.remove(&original_index);
            } else {
                // deselected -> allow
                self.allow_indices.insert(original_index);
            }
        }
    }

    fn toggle_all(&mut self) {
        let all_visible_deselected = self.filtered_indices.iter().all(|i| {
            !self.allow_indices.contains(i) && !self.deny_indices.contains(i)
        });

        if all_visible_deselected {
            // All visible are deselected -> add all to allow
            for &i in &self.filtered_indices {
                self.allow_indices.insert(i);
            }
        } else {
            // At least one visible is in allow or deny -> deselect all visible
            for &i in &self.filtered_indices {
                self.allow_indices.remove(&i);
                self.deny_indices.remove(&i);
            }
        }
    }

    fn move_up(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.list_state.select(Some(self.cursor_position));
        }
    }

    fn move_down(&mut self) {
        if self.cursor_position < self.filtered_indices.len().saturating_sub(1) {
            self.cursor_position += 1;
            self.list_state.select(Some(self.cursor_position));
        }
    }

    fn can_confirm(&self) -> bool {
        !self.allow_indices.is_empty() || !self.deny_indices.is_empty()
    }

    fn get_computed_actions(&self) -> ComputedActions {
        compute_selected_actions(
            self.service_prefix,
            self.actions,
            &self.allow_indices,
            &self.deny_indices,
        )
    }
}

fn render_action_selector(frame: &mut Frame, selector: &mut ActionSelector) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let pane_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_chunks[0]);

    render_available_actions(frame, pane_chunks[0], selector);
    render_selected_actions(frame, pane_chunks[1], selector);
    render_warning(frame, main_chunks[1]);
    render_action_controls(frame, main_chunks[2], selector.can_confirm());
}

fn render_available_actions(frame: &mut Frame, area: Rect, selector: &mut ActionSelector) {
    let items: Vec<ListItem> = selector
        .filtered_indices
        .iter()
        .enumerate()
        .map(|(display_index, &original_index)| {
            let action = &selector.actions[original_index];
            let (checkbox, state_style) =
                if selector.allow_indices.contains(&original_index) {
                    ("[✓]", Style::default().fg(Color::Green))
                } else if selector.deny_indices.contains(&original_index) {
                    ("[✗]", Style::default().fg(Color::Red))
                } else {
                    ("[ ]", Style::default())
                };
            let content = format!("{} {}", checkbox, action.name);
            let style = if display_index == selector.cursor_position {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                state_style
            };
            ListItem::new(content).style(style)
        })
        .collect();

    let title = if !selector.filter_text.is_empty() {
        format!(" Available Actions (filter: {}) ", selector.filter_text)
    } else {
        " Available Actions (type to filter) ".to_string()
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    frame.render_stateful_widget(list, area, &mut selector.list_state);
}

fn render_selected_actions(frame: &mut Frame, area: Rect, selector: &ActionSelector) {
    let computed = selector.get_computed_actions();
    let mut items: Vec<ListItem> = Vec::new();

    // UI shows Allow before Deny (per spec section 3.5).
    // Note: YAML output uses the opposite order (deny before allow) per AWS IAM convention.
    if !computed.allow.is_empty() {
        items.push(
            ListItem::new("Allow:")
                .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        );
        for action in &computed.allow {
            items.push(
                ListItem::new(format!("  {}", action))
                    .style(Style::default().fg(Color::Cyan)),
            );
        }
    }

    if !computed.deny.is_empty() {
        if !computed.allow.is_empty() {
            items.push(ListItem::new(""));
        }
        items.push(
            ListItem::new("Deny:")
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        );
        for action in &computed.deny {
            items.push(
                ListItem::new(format!("  {}", action))
                    .style(Style::default().fg(Color::Red)),
            );
        }
    }

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Selected Actions "),
    );

    frame.render_widget(list, area);
}

fn render_warning(frame: &mut Frame, area: Rect) {
    let warning = Paragraph::new(Line::from(vec![
        Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
        Span::styled(ACTION_WARNING, Style::default().fg(Color::Yellow)),
    ]))
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(warning, area);
}

fn render_action_controls(frame: &mut Frame, area: Rect, can_confirm: bool) {
    let confirm_style = if can_confirm {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let controls = Line::from(vec![
        Span::styled("[SPACE]", Style::default().fg(Color::Cyan)),
        Span::raw(" Cycle (allow/deny/none)  "),
        Span::styled("[TAB]", Style::default().fg(Color::Cyan)),
        Span::raw(" Toggle all  "),
        Span::styled("[ENTER]", confirm_style),
        Span::raw(" Confirm  "),
        Span::styled("[↑↓]", Style::default().fg(Color::Cyan)),
        Span::raw(" Navigate  "),
        Span::styled("[type]", Style::default().fg(Color::Cyan)),
        Span::raw(" Filter  "),
        Span::styled("[ESC]", Style::default().fg(Color::Red)),
        Span::raw(" Quit"),
    ]);

    frame.render_widget(Paragraph::new(controls), area);
}

pub fn select_actions(
    actions: &[Action],
    service_prefix: &str,
    preselected_indices: &[usize],
) -> Result<SelectedActions> {
    let _guard = TerminalGuard::new()?;
    let mut terminal = create_terminal()?;
    let mut selector = ActionSelector::new(actions, service_prefix, preselected_indices);

    loop {
        terminal
            .draw(|frame| render_action_selector(frame, &mut selector))
            .context("Failed to draw UI")?;

        if let Event::Key(key) = event::read().context("Failed to read event")? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Up => selector.move_up(),
                KeyCode::Down => selector.move_down(),
                KeyCode::Char(' ') => selector.cycle_current(),
                KeyCode::Tab => selector.toggle_all(),
                KeyCode::Enter => {
                    if selector.can_confirm() {
                        return Ok(SelectedActions {
                            allow_indices: selector.allow_indices,
                            deny_indices: selector.deny_indices,
                        });
                    }
                }
                KeyCode::Backspace => selector.remove_char(),
                KeyCode::Esc => {
                    bail!("Action selection cancelled by user");
                }
                KeyCode::Char(c) => {
                    // Filter by typing (all other characters)
                    selector.add_char(c);
                }
                _ => {}
            }
        }
    }
}

impl BlockType {
    fn as_str(&self) -> &'static str {
        match self {
            BlockType::Action => "action",
            BlockType::Data => "data",
            BlockType::Ephemeral => "ephemeral",
            BlockType::Resource => "resource",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{ActionAnnotations, ActionProperties};

    fn create_test_action(name: &str) -> Action {
        Action {
            name: name.to_string(),
            annotations: Some(ActionAnnotations {
                properties: ActionProperties {
                    is_list: name.starts_with("List"),
                    is_permission_management: false,
                    is_tagging_only: false,
                    is_write: false,
                },
            }),
        }
    }

    fn create_test_actions() -> Vec<Action> {
        vec![
            create_test_action("CreateSubnet"),
            create_test_action("DeleteSubnet"),
            create_test_action("DescribeSubnets"),
            create_test_action("GetSubnetCidr"),
            create_test_action("ListSubnets"),
            create_test_action("ModifySubnet"),
        ]
    }

    #[test]
    fn action_selector_filter_reduces_visible_items() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        assert_eq!(selector.filtered_indices.len(), 6);

        selector.add_char('S');
        selector.add_char('u');
        selector.add_char('b');

        // All actions contain "Sub" so all should be visible
        assert_eq!(selector.filtered_indices.len(), 6);

        selector.add_char('n');
        selector.add_char('e');
        selector.add_char('t');
        selector.add_char('s');

        // Only DescribeSubnets and ListSubnets contain "Subnets"
        assert_eq!(selector.filtered_indices.len(), 2);
        assert!(selector
            .filtered_indices
            .contains(&2)); // DescribeSubnets
        assert!(selector.filtered_indices.contains(&4)); // ListSubnets
    }

    #[test]
    fn action_selector_filter_is_case_insensitive() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        selector.add_char('c');
        selector.add_char('r');
        selector.add_char('e');
        selector.add_char('a');
        selector.add_char('t');
        selector.add_char('e');

        // Should match "CreateSubnet" despite lowercase input
        assert_eq!(selector.filtered_indices.len(), 1);
        assert!(selector.filtered_indices.contains(&0));
    }

    #[test]
    fn action_selector_remove_char_expands_filter() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        // Type "ListSu" which uniquely matches ListSubnets
        for c in "ListSu".chars() {
            selector.add_char(c);
        }

        // "ListSu" matches only ListSubnets
        assert_eq!(selector.filtered_indices.len(), 1);
        assert!(selector.filtered_indices.contains(&4)); // ListSubnets

        // Remove one char -> "ListS"
        selector.remove_char();
        // Still only ListSubnets contains "ListS"
        assert_eq!(selector.filtered_indices.len(), 1);

        // Remove more chars -> "List"
        selector.remove_char();
        // Only ListSubnets starts with "List"
        assert_eq!(selector.filtered_indices.len(), 1);

        // Remove all chars one by one
        selector.remove_char(); // -> "Lis"
        selector.remove_char(); // -> "Li"
        selector.remove_char(); // -> "L"
        // "L" matches ListSubnets and DeleteSubnet (has 'l')
        assert_eq!(selector.filtered_indices.len(), 2);

        selector.remove_char(); // -> ""
        // Empty filter shows all
        assert_eq!(selector.filtered_indices.len(), 6);
    }

    #[test]
    fn action_selector_selection_preserved_during_filter() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[0, 2, 4]);

        // Initially allowed: CreateSubnet (0), DescribeSubnets (2), ListSubnets (4)
        assert!(selector.allow_indices.contains(&0));
        assert!(selector.allow_indices.contains(&2));
        assert!(selector.allow_indices.contains(&4));

        // Apply filter
        selector.add_char('D');
        selector.add_char('e');
        selector.add_char('l');

        // Only DeleteSubnet visible
        assert_eq!(selector.filtered_indices.len(), 1);

        // But original selections still preserved
        assert!(selector.allow_indices.contains(&0));
        assert!(selector.allow_indices.contains(&2));
        assert!(selector.allow_indices.contains(&4));
    }

    #[test]
    fn action_selector_cycle_current_uses_original_index() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        // Apply filter to show only "Delete*"
        selector.add_char('D');
        selector.add_char('e');
        selector.add_char('l');

        // Only DeleteSubnet (index 1) should be visible
        assert_eq!(selector.filtered_indices.len(), 1);
        assert_eq!(selector.filtered_indices[0], 1);

        // Cycle current (cursor at 0 in filtered list = original index 1)
        // deselected -> allow
        selector.cycle_current();

        // Should add original index 1 (DeleteSubnet) to allow
        assert!(selector.allow_indices.contains(&1));
        assert!(!selector.allow_indices.contains(&0));
    }

    #[test]
    fn action_selector_toggle_all_selects_deselected_to_allow() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        // Apply filter to show only "Describe*" and "Delete*"
        selector.add_char('D');
        selector.add_char('e');

        // DescribeSubnets (2) and DeleteSubnet (1) should be visible
        assert_eq!(selector.filtered_indices.len(), 2);

        // Toggle all visible (all deselected -> allow)
        selector.toggle_all();

        // Only indices 1 and 2 should be in allow
        assert_eq!(selector.allow_indices.len(), 2);
        assert!(selector.allow_indices.contains(&1));
        assert!(selector.allow_indices.contains(&2));
        assert!(!selector.allow_indices.contains(&0));
        assert!(!selector.allow_indices.contains(&3));
        assert!(selector.deny_indices.is_empty());
    }

    #[test]
    fn action_selector_toggle_all_deselects_allow_and_deny() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[1, 2]);

        // Put index 2 into deny manually (cycle: allow -> deny)
        selector.allow_indices.remove(&2);
        selector.deny_indices.insert(2);

        // Apply filter to show only "Describe*" and "Delete*"
        selector.add_char('D');
        selector.add_char('e');

        // Visible: DeleteSubnet (1, in allow), DescribeSubnets (2, in deny)
        assert!(selector.allow_indices.contains(&1));
        assert!(selector.deny_indices.contains(&2));

        // Toggle all should deselect all visible (from allow or deny)
        selector.toggle_all();

        assert!(!selector.allow_indices.contains(&1));
        assert!(!selector.allow_indices.contains(&2));
        assert!(!selector.deny_indices.contains(&1));
        assert!(!selector.deny_indices.contains(&2));
    }

    #[test]
    fn action_selector_navigation_respects_filter() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        // Apply filter to show only "Describe*" and "Delete*"
        selector.add_char('D');
        selector.add_char('e');

        assert_eq!(selector.filtered_indices.len(), 2);
        assert_eq!(selector.cursor_position, 0);

        selector.move_down();
        assert_eq!(selector.cursor_position, 1);

        // Can't move beyond filtered list
        selector.move_down();
        assert_eq!(selector.cursor_position, 1);

        selector.move_up();
        assert_eq!(selector.cursor_position, 0);

        // Can't move before start
        selector.move_up();
        assert_eq!(selector.cursor_position, 0);
    }

    #[test]
    fn action_selector_cursor_resets_on_filter_change() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        // Move cursor down
        selector.move_down();
        selector.move_down();
        assert_eq!(selector.cursor_position, 2);

        // Apply filter - cursor should reset to 0
        selector.add_char('L');
        assert_eq!(selector.cursor_position, 0);
    }

    #[test]
    fn spacebar_cycles_deselected_to_allow_to_deny_to_deselected() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        // Initially deselected
        assert!(!selector.allow_indices.contains(&0));
        assert!(!selector.deny_indices.contains(&0));

        // Cycle 1: deselected -> allow
        selector.cycle_current();
        assert!(selector.allow_indices.contains(&0));
        assert!(!selector.deny_indices.contains(&0));

        // Cycle 2: allow -> deny
        selector.cycle_current();
        assert!(!selector.allow_indices.contains(&0));
        assert!(selector.deny_indices.contains(&0));

        // Cycle 3: deny -> deselected
        selector.cycle_current();
        assert!(!selector.allow_indices.contains(&0));
        assert!(!selector.deny_indices.contains(&0));
    }

    #[test]
    fn tab_never_puts_actions_into_deny() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        // Toggle all (all deselected -> allow)
        selector.toggle_all();
        assert!(selector.deny_indices.is_empty());

        // Toggle all again (all allowed -> deselected)
        selector.toggle_all();
        assert!(selector.deny_indices.is_empty());
        assert!(selector.allow_indices.is_empty());
    }

    #[test]
    fn tab_only_affects_filtered_visible_actions() {
        let actions = create_test_actions();
        // Pre-select index 0 (CreateSubnet) into allow
        let mut selector = ActionSelector::new(&actions, "ec2", &[0]);

        // Also put index 5 (ModifySubnet) into deny manually
        selector.deny_indices.insert(5);

        // Filter to show only "List*"
        selector.add_char('L');
        selector.add_char('i');
        selector.add_char('s');
        selector.add_char('t');

        // Only ListSubnets (4) visible
        assert_eq!(selector.filtered_indices.len(), 1);

        // Toggle all (all visible deselected -> allow)
        selector.toggle_all();

        // ListSubnets now in allow
        assert!(selector.allow_indices.contains(&4));
        // Previous selections untouched
        assert!(selector.allow_indices.contains(&0));
        assert!(selector.deny_indices.contains(&5));
    }

    #[test]
    fn can_confirm_true_with_only_deny_actions() {
        let actions = create_test_actions();
        let mut selector = ActionSelector::new(&actions, "ec2", &[]);

        selector.deny_indices.insert(0);

        assert!(selector.can_confirm());
    }

    #[test]
    fn can_confirm_false_when_both_sets_empty() {
        let actions = create_test_actions();
        let selector = ActionSelector::new(&actions, "ec2", &[]);

        assert!(!selector.can_confirm());
    }
}

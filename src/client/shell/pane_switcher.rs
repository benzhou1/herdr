//! Ctrl+Tab style switcher over panes in most-recently-used order.
//!
//! The MRU list and the hold session are client presentation state. Opening
//! reuses the Navigator overlay, and committing reuses its pane focus path, so
//! nothing here touches the server protocol.

use super::*;
use crossterm::event::{KeyEventKind, KeyModifiers, ModifierKeyCode};

const MRU_LIMIT: usize = 64;

/// Marks a Navigator overlay as the pane switcher.
#[derive(Debug)]
pub(super) struct PaneSwitcherSession {
    /// Panes by recency as of opening; frozen while the switcher is open.
    order: Vec<ClientNavigatorTarget>,
    /// Releasing Ctrl commits the selection. Cleared when the user starts
    /// searching, after which this is the regular Navigator.
    hold: bool,
}

impl PaneSwitcherSession {
    /// Flatten to one line per pane, prefixed with its workspace, in recency
    /// order. The focused pane always leads so the first step lands on the
    /// previous one.
    pub(super) fn order_rows(&self, rows: Vec<ClientNavigatorRow>) -> Vec<ClientNavigatorRow> {
        // Tree order puts each workspace row ahead of its panes.
        let mut workspace = String::new();
        let mut panes = Vec::new();
        for mut row in rows {
            match row.target {
                ClientNavigatorTarget::Workspace { .. } => workspace = row.label,
                ClientNavigatorTarget::Pane { .. } => {
                    if !workspace.is_empty() {
                        // A lone pane is labelled with its workspace; name the
                        // pane as the other rows do.
                        let pane = if row.label == workspace {
                            row.agent.as_deref().unwrap_or("terminal").to_owned()
                        } else {
                            std::mem::take(&mut row.label)
                        };
                        row.label = format!("{workspace} · {pane}");
                    }
                    row.depth = 0;
                    panes.push(row);
                }
                ClientNavigatorTarget::Machine { .. } => {}
            }
        }
        panes.sort_by_cached_key(|row| {
            if row.current {
                0
            } else {
                1 + self
                    .order
                    .iter()
                    .position(|target| *target == row.target)
                    .unwrap_or(usize::MAX - 1)
            }
        });
        panes
    }

    pub(super) fn holding(&self) -> bool {
        self.hold
    }
}

/// The workspace name leading a switcher row's label, for styling. Found from
/// the row's `workspace / tab / pane` detail and only returned when the label
/// really starts with it.
pub(super) fn row_workspace(row: &ClientNavigatorRow) -> Option<&str> {
    let ClientNavigatorTarget::Pane { pane_id, .. } = &row.target else {
        return None;
    };
    let (workspace, _tab) = row
        .detail
        .strip_suffix(pane_id.as_str())?
        .strip_suffix(" / ")?
        .rsplit_once(" / ")?;
    row.label
        .strip_prefix(workspace)?
        .starts_with(" · ")
        .then(|| &row.label[..workspace.len()])
}

impl ClientShellState {
    /// Record the active endpoint's focused pane as most recently used.
    pub(super) fn note_pane_focus(&mut self, pane_id: Option<&str>) {
        let Some(pane_id) = pane_id else {
            return;
        };
        if matches!(
            self.pane_mru.first(),
            Some(ClientNavigatorTarget::Pane { endpoint_id, pane_id: first })
                if *endpoint_id == self.active_endpoint_id && first == pane_id
        ) {
            return;
        }
        let target = ClientNavigatorTarget::Pane {
            endpoint_id: self.active_endpoint_id.clone(),
            pane_id: pane_id.to_owned(),
        };
        self.pane_mru.retain(|entry| *entry != target);
        self.pane_mru.insert(0, target);
        self.pane_mru.truncate(MRU_LIMIT);
    }

    /// True while the switcher waits for Ctrl to be released.
    pub(super) fn pane_switcher_holding(&self) -> bool {
        matches!(
            &self.overlay,
            Some(ClientShellOverlay::Navigator(ClientNavigatorOverlay {
                switcher: Some(session),
                ..
            })) if session.holding()
        )
    }

    /// Open the switcher, or step an open one, for a switcher keybind.
    pub(super) fn step_pane_switcher(&mut self, backwards: bool, outcome: &mut ClientShellInput) {
        match &self.overlay {
            None => self.open_pane_switcher(backwards),
            Some(ClientShellOverlay::Navigator(ClientNavigatorOverlay {
                switcher: Some(_),
                ..
            })) => self.move_pane_switcher_selection(backwards),
            Some(_) => return,
        }
        outcome.repaint = true;
    }

    fn open_pane_switcher(&mut self, backwards: bool) {
        let mut navigator = ClientNavigatorOverlay {
            query: TextEditor::default(),
            search_focused: false,
            selected: None,
            scroll: 0,
            filter: None,
            switcher: Some(PaneSwitcherSession {
                order: self.pane_mru.clone(),
                // Without key release reports there is no signal to commit
                // on, so stay a regular Navigator (Enter commits, Esc closes).
                hold: self.host_reports_key_releases,
            }),
        };
        let rows =
            render::client_navigator_rows(&self.endpoints, &self.active_endpoint_id, &navigator);
        let start = if backwards {
            rows.len().saturating_sub(1)
        } else {
            1.min(rows.len().saturating_sub(1))
        };
        navigator.selected = rows.get(start).map(|row| row.target.clone());
        self.overlay = Some(ClientShellOverlay::Navigator(navigator));
    }

    fn move_pane_switcher_selection(&mut self, backwards: bool) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let rows =
            render::client_navigator_rows(&self.endpoints, &self.active_endpoint_id, navigator);
        if rows.is_empty() {
            return;
        }
        let selected =
            super::aggregate_navigation::navigator_selected_index(&rows, navigator).unwrap_or(0);
        let next = if backwards {
            (selected + rows.len() - 1) % rows.len()
        } else {
            (selected + 1) % rows.len()
        };
        navigator.selected = Some(rows[next].target.clone());
    }

    fn commit_pane_switcher(&mut self, outcome: &mut ClientShellInput) {
        self.accept_navigator_selection(outcome);
        // If the target could not be focused, leave a regular Navigator behind
        // rather than a hold that nothing will end.
        if let Some(ClientShellOverlay::Navigator(ClientNavigatorOverlay {
            switcher: Some(session),
            ..
        })) = self.overlay.as_mut()
        {
            session.hold = false;
        }
    }

    /// Abandon a held switcher without switching, when its release can no
    /// longer be observed.
    pub(super) fn cancel_pane_switcher_hold(&mut self) -> bool {
        let holding = self.pane_switcher_holding();
        if holding {
            self.overlay = None;
        }
        holding
    }

    /// Handle a key while the switcher waits for Ctrl to be released. Returns
    /// true when the key is consumed; anything else falls through to the
    /// Navigator, which already covers arrows and search.
    pub(super) fn handle_pane_switcher_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.pane_switcher_holding() {
            return false;
        }
        let ctrl_held = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.kind == KeyEventKind::Release {
            let ctrl_key = matches!(
                key.code,
                KeyCode::Modifier(ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl)
            );
            if ctrl_key || !ctrl_held {
                self.commit_pane_switcher(outcome);
                return true;
            }
            return false;
        }
        if matches!(key.code, KeyCode::Modifier(_)) {
            return true;
        }
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
                outcome.repaint = true;
                return true;
            }
            KeyCode::Enter => {
                self.commit_pane_switcher(outcome);
                return true;
            }
            _ => {}
        }
        if !ctrl_held {
            // The host only reports the Ctrl release once it reports every
            // key, so a quick tap can miss it. A key without Ctrl means it is up.
            self.commit_pane_switcher(outcome);
            return true;
        }
        let press = key.clone().with_kind(KeyEventKind::Press);
        match crate::input::resolve_direct_binding(&self.config.keybinds.keybinds, &press) {
            Some(crate::input::KeybindMatch::Action(crate::input::KeybindAction::PaneSwitcher)) => {
                self.move_pane_switcher_selection(false);
                outcome.repaint = true;
                return true;
            }
            Some(crate::input::KeybindMatch::Action(
                crate::input::KeybindAction::PaneSwitcherPrevious,
            )) => {
                self.move_pane_switcher_selection(true);
                outcome.repaint = true;
                return true;
            }
            _ => {}
        }
        if key.code == KeyCode::Char('/') {
            if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
                navigator.search_focused = true;
                navigator.filter = None;
                if let Some(session) = navigator.switcher.as_mut() {
                    session.hold = false;
                }
            }
            outcome.repaint = true;
            return true;
        }
        false
    }
}

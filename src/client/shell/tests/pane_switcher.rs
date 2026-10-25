use super::*;
use crate::input::TerminalKey;
use crossterm::event::{KeyEventKind, ModifierKeyCode};

const CTRL: KeyModifiers = KeyModifiers::CONTROL;

fn snapshot_with(panes: &[&str], focused: &str, revision: u64) -> ClientShellSnapshot {
    let mut snapshot = snapshot();
    let template = snapshot.panes[0].clone();
    snapshot.revision = revision;
    snapshot.focused_pane_id = Some(focused.into());
    snapshot.panes = panes
        .iter()
        .map(|id| {
            let mut pane = template.clone();
            pane.pane_id = (*id).into();
            pane.focused = *id == focused;
            pane
        })
        .collect();
    snapshot
}

fn shell(panes: &[&str], focused: &str) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_host_reports_key_releases(true);
    state.set_snapshot(Box::new(snapshot_with(panes, focused, 1)));
    state
}

fn focus(state: &mut ClientShellState, panes: &[&str], focused: &str, revision: u64) {
    state.set_snapshot(Box::new(snapshot_with(panes, focused, revision)));
}

fn send(
    state: &mut ClientShellState,
    code: KeyCode,
    modifiers: KeyModifiers,
    kind: KeyEventKind,
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(
        TerminalKey::new(code, modifiers).with_kind(kind),
    )])
}

fn press(state: &mut ClientShellState, code: KeyCode, modifiers: KeyModifiers) -> ClientShellInput {
    send(state, code, modifiers, KeyEventKind::Press)
}

fn ctrl_tab(state: &mut ClientShellState) -> ClientShellInput {
    press(state, KeyCode::Tab, CTRL)
}

fn release_ctrl(state: &mut ClientShellState) -> ClientShellInput {
    send(
        state,
        KeyCode::Modifier(ModifierKeyCode::LeftControl),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    )
}

fn focus_request(outcome: &ClientShellInput) -> Option<String> {
    outcome.actions.iter().find_map(|action| match action {
        ClientShellAction::Endpoint { request, .. } => match &request.method {
            crate::api::schema::Method::PaneFocus(target) => Some(target.pane_id.clone()),
            _ => None,
        },
        _ => None,
    })
}

fn selected_pane(state: &ClientShellState) -> Option<String> {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        return None;
    };
    match &navigator.selected {
        Some(ClientNavigatorTarget::Pane { pane_id, .. }) => Some(pane_id.clone()),
        _ => None,
    }
}

fn row_panes(state: &ClientShellState) -> Vec<String> {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };
    render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator)
        .into_iter()
        .map(|row| match row.target {
            ClientNavigatorTarget::Pane { pane_id, .. } => pane_id,
            target => panic!("unexpected row {target:?}"),
        })
        .collect()
}

#[test]
fn rows_follow_focus_recency_with_the_focused_pane_first() {
    let panes = ["p1", "p2", "p3"];
    let mut state = shell(&panes, "p1");
    focus(&mut state, &panes, "p2", 2);
    focus(&mut state, &panes, "p3", 3);

    ctrl_tab(&mut state);

    assert_eq!(row_panes(&state), ["p3", "p2", "p1"]);
    assert_eq!(selected_pane(&state).as_deref(), Some("p2"));
}

#[test]
fn rows_are_single_lines_naming_the_workspace_before_the_pane() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    ctrl_tab(&mut state);
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };

    let rows =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);

    let labels = rows
        .iter()
        .map(|row| row.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        ["client-shell · terminal · 1", "client-shell · terminal · 2"]
    );
    assert!(rows.iter().all(|row| row.depth == 0));
}

#[test]
fn a_lone_pane_is_named_after_its_workspace_like_other_rows() {
    let mut state = shell(&["p1"], "p1");
    ctrl_tab(&mut state);
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator");
    };

    let rows =
        render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "client-shell · terminal");
}

#[test]
fn only_the_workspace_name_is_bold() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    state.set_pane_surface(surface());
    ctrl_tab(&mut state);

    let frame = state.compose(106, 30).expect("switcher");
    // The first row is the focused pane; the selection is on the second.
    let (rect, _) = state.hits.navigator_rows[0];
    let buffer = frame.to_ratatui_buffer().expect("buffer");
    let workspace = cell_symbol_position(&frame, rect, "client-shell");
    let pane = cell_symbol_position(&frame, rect, "terminal");

    assert!(buffer[workspace].modifier.contains(Modifier::BOLD));
    assert!(!buffer[pane].modifier.contains(Modifier::BOLD));
}

#[test]
fn tab_steps_and_ctrl_release_commits_the_selection() {
    let panes = ["p1", "p2", "p3"];
    let mut state = shell(&panes, "p1");
    ctrl_tab(&mut state);
    assert_eq!(selected_pane(&state).as_deref(), Some("p2"));
    assert!(state.pane_switcher_holding());

    ctrl_tab(&mut state);
    assert_eq!(selected_pane(&state).as_deref(), Some("p3"));
    ctrl_tab(&mut state);
    assert_eq!(selected_pane(&state).as_deref(), Some("p1"));
    ctrl_tab(&mut state);

    let outcome = release_ctrl(&mut state);
    assert_eq!(focus_request(&outcome).as_deref(), Some("p2"));
    assert!(state.overlay.is_none());
    assert!(!state.host_keyboard_report_all_requested());
}

#[test]
fn held_tab_repeats_advance_the_selection() {
    let panes = ["p1", "p2", "p3"];
    let mut state = shell(&panes, "p1");
    ctrl_tab(&mut state);

    send(&mut state, KeyCode::Tab, CTRL, KeyEventKind::Repeat);

    assert_eq!(selected_pane(&state).as_deref(), Some("p3"));
}

#[test]
fn ctrl_shift_tab_opens_on_the_oldest_pane_and_steps_back() {
    let panes = ["p1", "p2", "p3"];
    let mut state = shell(&panes, "p1");
    press(&mut state, KeyCode::Tab, CTRL | KeyModifiers::SHIFT);
    assert_eq!(selected_pane(&state).as_deref(), Some("p3"));

    press(&mut state, KeyCode::Tab, CTRL | KeyModifiers::SHIFT);
    assert_eq!(selected_pane(&state).as_deref(), Some("p2"));
}

#[test]
fn ctrl_slash_hands_over_to_search_and_release_no_longer_commits() {
    let panes = ["p1", "p2", "p3"];
    let mut state = shell(&panes, "p1");
    ctrl_tab(&mut state);

    press(&mut state, KeyCode::Char('/'), CTRL);
    assert!(!state.pane_switcher_holding());
    assert!(!state.host_keyboard_report_all_requested());

    let outcome = release_ctrl(&mut state);
    assert!(focus_request(&outcome).is_none());
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("navigator stays open");
    };
    assert!(navigator.search_focused);

    press(&mut state, KeyCode::Char('p'), KeyModifiers::NONE);
    press(&mut state, KeyCode::Char('3'), KeyModifiers::NONE);
    assert_eq!(row_panes(&state), ["p3"]);
    let outcome = press(&mut state, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(focus_request(&outcome).as_deref(), Some("p3"));
}

#[test]
fn escape_cancels_without_switching() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    ctrl_tab(&mut state);

    let outcome = press(&mut state, KeyCode::Esc, KeyModifiers::NONE);

    assert!(focus_request(&outcome).is_none());
    assert!(state.overlay.is_none());
    let outcome = release_ctrl(&mut state);
    assert!(focus_request(&outcome).is_none());
}

#[test]
fn a_key_without_ctrl_commits_when_the_release_was_missed() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    ctrl_tab(&mut state);

    let outcome = press(&mut state, KeyCode::Char('a'), KeyModifiers::NONE);

    assert_eq!(focus_request(&outcome).as_deref(), Some("p2"));
    assert!(state.overlay.is_none());
}

#[test]
fn hosts_without_release_reports_get_a_regular_navigator() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    state.set_host_reports_key_releases(false);

    ctrl_tab(&mut state);
    assert!(!state.pane_switcher_holding());
    assert_eq!(selected_pane(&state).as_deref(), Some("p2"));

    let outcome = release_ctrl(&mut state);
    assert!(focus_request(&outcome).is_none());
    assert!(state.overlay.is_some());

    let outcome = press(&mut state, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(focus_request(&outcome).as_deref(), Some("p2"));
}

#[test]
fn holding_requests_report_all_keys_to_observe_the_ctrl_release() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    assert!(!state.host_keyboard_report_all_requested());

    ctrl_tab(&mut state);

    assert!(state.host_keyboard_report_all_requested());
}

#[test]
fn losing_terminal_focus_cancels_a_held_switcher() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    ctrl_tab(&mut state);

    let outcome = state.handle_raw_events(vec![RawInputEvent::OuterFocusLost]);

    assert!(focus_request(&outcome).is_none());
    assert!(state.overlay.is_none());
}

#[test]
fn an_open_dialog_is_not_replaced_by_the_switcher() {
    let panes = ["p1", "p2"];
    let mut state = shell(&panes, "p1");
    state.open_navigator_overlay();

    ctrl_tab(&mut state);

    assert!(!state.pane_switcher_holding());
}

#[test]
fn recency_list_is_bounded_and_deduplicated() {
    let ids = (0..80).map(|n| format!("p{n}")).collect::<Vec<_>>();
    let panes = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let mut state = shell(&panes, "p0");
    let mut revision = 1;
    for id in &panes {
        for _ in 0..2 {
            revision += 1;
            focus(&mut state, &panes, id, revision);
        }
    }

    assert!(state.pane_mru.len() <= 64);
    let mut unique = state.pane_mru.clone();
    unique.dedup();
    assert_eq!(unique.len(), state.pane_mru.len());
}

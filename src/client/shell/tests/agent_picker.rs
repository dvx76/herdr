use super::*;

use crate::api::schema::AgentStatus;

/// The shared `agent()` helper hardcodes `pane_1`; the picker tests need
/// several agents per snapshot.
fn agent_on(
    pane_id: &str,
    name: &str,
    status: AgentStatus,
    state_change_seq: u64,
) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some(name.into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }
}

fn local_with_agents(agents: Vec<ClientShellAgent>) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.agents = agents;
    projected
}

fn remote_with_agents(agents: Vec<ClientShellAgent>) -> ClientShellSnapshot {
    let mut projected = local_with_agents(agents);
    projected.boot_id = "remote-boot".into();
    projected.workspaces[0].label = "remote-workspace".into();
    projected
}

fn pane_with_cwd(pane_id: &str, cwd: &str) -> ClientShellPane {
    ClientShellPane {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        label: None,
        cwd: Some(cwd.into()),
        foreground_cwd: Some(cwd.into()),
        focused: false,
        right_click_passthrough: false,
    }
}

/// A federated state that sorts its agent surfaces by priority, mirroring
/// `state_with_remote()`.
fn priority_state() -> (ClientShellState, ClientEndpointId) {
    let mut config = Config::default();
    config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let profile = remote_profile();
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    (state, endpoint_id)
}

fn preview_key(state: &mut ClientShellState, bytes: &[u8]) {
    let outcome = state.handle_input_bytes(bytes);
    assert!(outcome.actions.is_empty(), "{bytes:?}");
    assert!(outcome.requests.is_empty(), "{bytes:?}");
    assert!(outcome.repaint, "{bytes:?}");
}

fn navigator(state: &ClientShellState) -> &ClientNavigatorOverlay {
    let ClientShellOverlay::Navigator(navigator) = state.overlay.as_ref().expect("navigator")
    else {
        panic!("expected navigator");
    };
    navigator
}

fn agent_rows(state: &ClientShellState) -> Vec<ClientNavigatorRow> {
    render::client_navigator_rows(
        &state.endpoints,
        &state.active_endpoint_id,
        navigator(state),
        state.config.agent_panel_sort,
    )
}

fn agent_rows_with_query(state: &mut ClientShellState, query: &str) -> Vec<ClientNavigatorRow> {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() else {
        panic!("expected navigator");
    };
    navigator.query = query.into();
    navigator.selected = None;
    render::client_navigator_rows(
        &state.endpoints,
        &state.active_endpoint_id,
        navigator,
        state.config.agent_panel_sort,
    )
}

fn labels(rows: &[ClientNavigatorRow]) -> Vec<&str> {
    rows.iter().map(|row| row.label.as_str()).collect()
}

fn frame_text(state: &mut ClientShellState) -> String {
    let frame = state.compose(100, 28).expect("composed navigator frame");
    frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The composed frame text of every visible navigator hit row, in order.
fn navigator_row_texts(state: &mut ClientShellState) -> Vec<String> {
    let frame = state.compose(100, 28).expect("composed navigator frame");
    state
        .hits
        .navigator_rows
        .iter()
        .map(|(rect, _)| {
            frame.cells[rect.y as usize * frame.width as usize + rect.x as usize..]
                .iter()
                .take(rect.width as usize)
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect()
}

/// The display column of the machine `·` separator in every visible navigator
/// hit row. Reading frame cells (not the joined text) keeps wide glyphs at
/// their rendered width and skips the status dot that sits before it.
fn navigator_separator_columns(state: &mut ClientShellState) -> Vec<u16> {
    let frame = state.compose(100, 28).expect("composed navigator frame");
    state
        .hits
        .navigator_rows
        .iter()
        .map(|(rect, _)| {
            let base = rect.y as usize * frame.width as usize + rect.x as usize;
            (3..rect.width)
                .find(|offset| frame.cells[base + usize::from(*offset)].symbol.as_str() == "·")
                .expect("machine separator")
        })
        .collect()
}

#[test]
fn agent_picker_opens_on_prefix_a_and_navigator_on_prefix_g() {
    let (mut state, _) = state_with_remote();
    preview_key(&mut state, &[0x02, b'a']);
    assert_eq!(navigator(&state).mode, ClientNavigatorMode::AgentPicker);

    preview_key(&mut state, &[0x1b]);
    assert!(state.overlay.is_none());

    preview_key(&mut state, &[0x02, b'g']);
    assert_eq!(navigator(&state).mode, ClientNavigatorMode::Tree);

    preview_key(&mut state, &[0x1b]);
    assert!(state.overlay.is_none());
}

#[test]
fn agent_picker_rows_are_flat_agent_panes() {
    let (mut state, remote) = state_with_remote();
    state.set_snapshot(Box::new(local_with_agents(vec![agent_on(
        "pane_1",
        "local one",
        AgentStatus::Idle,
        1,
    )])));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![agent_on(
            "pane_1",
            "remote one",
            AgentStatus::Blocked,
            2,
        )])),
    );

    state.open_agent_picker_overlay();
    let rows = agent_rows(&state);
    assert!(rows.iter().all(|row| row.depth == 0));
    assert!(rows.iter().all(|row| row.agent.as_deref() == Some("pi")));
    assert!(rows
        .iter()
        .all(|row| matches!(row.target, ClientNavigatorTarget::Pane { .. })));
    assert_eq!(labels(&rows), ["Local · local one", "Build · remote one"]);
    assert_eq!(rows[0].detail, "Local / client-shell / 1 / pane_1");
    assert_eq!(rows[1].detail, "Build / remote-workspace / 1 / pane_1");
    assert_eq!(rows[0].status, Some(AgentStatus::Idle));
    assert_eq!(rows[1].status, Some(AgentStatus::Blocked));

    // A single-machine client keeps bare labels.
    let mut local = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    local.set_snapshot(Box::new(local_with_agents(vec![agent_on(
        "pane_1",
        "local one",
        AgentStatus::Idle,
        1,
    )])));
    local.open_agent_picker_overlay();
    assert_eq!(labels(&agent_rows(&local)), ["local one"]);
}

#[test]
fn agent_picker_rows_are_flat_with_aligned_machine_column() {
    let (mut state, remote) = state_with_remote();
    state.set_snapshot(Box::new(local_with_agents(vec![agent_on(
        "pane_1",
        "local one",
        AgentStatus::Idle,
        1,
    )])));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![agent_on(
            "pane_1",
            "remote one",
            AgentStatus::Idle,
            1,
        )])),
    );
    let mut long_profile = remote_profile();
    long_profile.id = ProfileId::parse("fedcba9876543210fedcba9876543210").unwrap();
    long_profile.label = "long machine".into();
    let long = ClientEndpointId::Ssh(long_profile.id.clone());
    let mut short_profile = remote_profile();
    short_profile.id = ProfileId::parse("00112233445566778899aabbccddeeff").unwrap();
    short_profile.label = "k2".into();
    let short = ClientEndpointId::Ssh(short_profile.id.clone());
    let mut wide_profile = remote_profile();
    wide_profile.id = ProfileId::parse("aabbccddeeff00112233445566778899").unwrap();
    wide_profile.label = "機械".into();
    let wide = ClientEndpointId::Ssh(wide_profile.id.clone());
    state.set_endpoint_catalog(&[remote_profile(), long_profile, short_profile, wide_profile]);
    for (endpoint, name) in [
        (&long, "long one"),
        (&short, "short one"),
        (&wide, "wide one"),
    ] {
        state.set_endpoint_status(endpoint, ClientEndpointStatus::Online);
        state.set_endpoint_snapshot(
            endpoint,
            Box::new(remote_with_agents(vec![agent_on(
                "pane_1",
                name,
                AgentStatus::Idle,
                1,
            )])),
        );
    }

    state.open_agent_picker_overlay();
    let rows = navigator_row_texts(&mut state);
    assert_eq!(rows.len(), 5);
    // Flat rows: no dangling tree connectors and no ◆ current marker, so the
    // status dot sits in the same column on every row, current pane included.
    for row in &rows {
        assert!(!row.contains("├─") && !row.contains("└─"), "{row:?}");
        assert!(!row.contains('◆'), "{row:?}");
        assert_eq!(row.chars().nth(1), Some('○'), "{row:?}");
    }
    // The machine prefix is padded to the widest machine: every `·` lands in
    // the same cell, wide labels included.
    let separators = navigator_separator_columns(&mut state);
    assert!(
        separators.windows(2).all(|pair| pair[0] == pair[1]),
        "{separators:?} in {rows:?}"
    );
    // The column width comes from the candidates before the status filter and
    // query, so hiding the widest machine with a query does not reflow it.
    let filtered = agent_rows_with_query(&mut state, "k2");
    assert_eq!(filtered.len(), 1);
    let machine_width = usize::from(render::display_width("long machine"));
    assert_eq!(
        filtered[0].label,
        format!("{:<machine_width$} · short one", "k2")
    );
    assert_eq!(navigator_separator_columns(&mut state), [separators[0]]);

    // The tree navigator keeps its connectors and current-pane marker.
    let (mut tree, _) = state_with_remote();
    tree.open_navigator_overlay();
    let tree_rows = navigator_row_texts(&mut tree);
    assert!(
        tree_rows
            .iter()
            .any(|row| row.contains("├─") || row.contains("└─")),
        "{tree_rows:?}"
    );
    assert!(
        tree_rows.iter().any(|row| row.contains('◆')),
        "{tree_rows:?}"
    );
}

#[test]
fn agent_picker_follows_agent_panel_sort() {
    fn seed(state: &mut ClientShellState, remote: &ClientEndpointId) {
        state.set_snapshot(Box::new(local_with_agents(vec![
            agent_on("pane_1", "local idle", AgentStatus::Idle, 1),
            agent_on("pane_2", "local blocked", AgentStatus::Blocked, 2),
        ])));
        state.set_endpoint_snapshot(
            remote,
            Box::new(remote_with_agents(vec![agent_on(
                "pane_1",
                "remote working",
                AgentStatus::Working,
                3,
            )])),
        );
        state.open_agent_picker_overlay();
    }

    // The default spaces order walks machines, then each snapshot in place.
    let (mut spaces, remote) = state_with_remote();
    seed(&mut spaces, &remote);
    assert_eq!(
        labels(&agent_rows(&spaces)),
        [
            "Local · local idle",
            "Local · local blocked",
            "Build · remote working",
        ]
    );

    // Priority order interleaves machines by status.
    let (mut priority, remote) = priority_state();
    seed(&mut priority, &remote);
    assert_eq!(
        labels(&agent_rows(&priority)),
        [
            "Local · local blocked",
            "Build · remote working",
            "Local · local idle",
        ]
    );

    // A stale machine's agents are dimmed and sort after every online agent.
    priority.set_endpoint_status(&remote, ClientEndpointStatus::Reconnecting);
    let rows = agent_rows(&priority);
    assert_eq!(
        labels(&rows),
        [
            "Local · local blocked",
            "Local · local idle",
            "Build · remote working",
        ]
    );
    assert_eq!(
        rows.iter().map(|row| row.stale).collect::<Vec<_>>(),
        [false, false, true]
    );
}

#[test]
fn agent_picker_status_filters_narrow_within_picker() {
    let (mut state, remote) = state_with_remote();
    state.set_snapshot(Box::new(local_with_agents(vec![
        agent_on("pane_1", "local idle", AgentStatus::Idle, 1),
        agent_on("pane_2", "local working", AgentStatus::Working, 2),
        agent_on("pane_3", "local blocked", AgentStatus::Blocked, 3),
    ])));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![
            agent_on("pane_1", "remote idle", AgentStatus::Idle, 1),
            agent_on("pane_2", "remote blocked", AgentStatus::Blocked, 2),
        ])),
    );
    state.open_agent_picker_overlay();

    // `b` narrows to exactly the blocked agents on every machine and names the
    // filter in the indicator.
    preview_key(&mut state, b"b");
    assert_eq!(
        navigator(&state).filter,
        Some(ClientNavigatorFilter::Blocked)
    );
    assert_eq!(
        labels(&agent_rows(&state)),
        ["Local · local blocked", "Build · remote blocked"]
    );
    let text = frame_text(&mut state);
    assert!(text.contains(" / agents · blocked"), "frame:\n{text}");

    // `i` narrows to exactly the idle agents.
    preview_key(&mut state, b"i");
    assert_eq!(
        labels(&agent_rows(&state)),
        ["Local · local idle", "Build · remote idle"]
    );

    // `a` clears the filter and restores every agent.
    preview_key(&mut state, b"a");
    assert_eq!(navigator(&state).filter, None);
    assert_eq!(agent_rows(&state).len(), 5);

    // `w` narrows to working, then `/` keeps the picker, clears the filter, and
    // focuses search.
    preview_key(&mut state, b"w");
    assert_eq!(labels(&agent_rows(&state)), ["Local · local working"]);
    preview_key(&mut state, b"/");
    assert!(navigator(&state).search_focused);
    assert_eq!(navigator(&state).filter, None);
    assert_eq!(navigator(&state).mode, ClientNavigatorMode::AgentPicker);
    assert_eq!(agent_rows(&state).len(), 5);
}

#[test]
fn agent_picker_search_matches_machine_location_and_agent_fields() {
    let (mut state, remote) = state_with_remote();
    let mut notes = agent_on("pane_2", "scratch notes", AgentStatus::Idle, 2);
    notes.title = Some("annual report".into());
    let mut local = local_with_agents(vec![
        agent_on("pane_1", "local one", AgentStatus::Idle, 1),
        notes,
    ]);
    local.workspaces[0].label = "alpha-space".into();
    local.workspaces[0].branch = Some("feature-x".into());
    local.tabs[0].label = "docs-tab".into();
    local.tabs[0].custom_label = true;
    let mut renamed = pane_with_cwd("pane_2", "/srv/secret-project");
    renamed.label = Some("renamed pane".into());
    local.panes.push(renamed);
    state.set_snapshot(Box::new(local));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![agent_on(
            "pane_1",
            "remote one",
            AgentStatus::Idle,
            1,
        )])),
    );
    state.open_agent_picker_overlay();

    for (query, expected) in [
        // The machine prefix.
        ("build", &["Build · remote one"][..]),
        // A custom pane name labels the row ahead of the agent name, and both
        // stay searchable.
        ("renamed", &["Local · renamed pane"]),
        ("scratch", &["Local · renamed pane"]),
        // The raw title, which appears in no other field.
        ("annual report", &["Local · renamed pane"]),
        // The pane cwd.
        ("secret-project", &["Local · renamed pane"]),
        // Workspace, branch, and tab names shown in the detail line.
        (
            "alpha-space",
            &["Local · local one", "Local · renamed pane"],
        ),
        ("feature-x", &["Local · local one", "Local · renamed pane"]),
        ("docs-tab", &["Local · local one", "Local · renamed pane"]),
        ("remote-workspace", &["Build · remote one"]),
        // The agent kind.
        (
            "pi",
            &[
                "Local · local one",
                "Local · renamed pane",
                "Build · remote one",
            ],
        ),
        ("zzznothing", &[]),
    ] {
        let rows = agent_rows_with_query(&mut state, query);
        assert_eq!(labels(&rows), expected, "query {query:?}");
    }
    let rows = agent_rows_with_query(&mut state, "secret-project");
    assert_eq!(rows[0].meta, "/srv/secret-project");
    assert_eq!(rows[0].detail, "Local / alpha-space / docs-tab / pane_2");
}

#[test]
fn agent_picker_projection_filters_rows_and_labels_indicator() {
    let (mut state, remote) = state_with_remote();
    let mut local = local_with_agents(vec![
        agent_on("pane_1", "local one", AgentStatus::Idle, 1),
        agent_on("pane_2", "local two", AgentStatus::Idle, 2),
    ]);
    local.agent_view_label = Some("current space".into());
    local.agent_order = vec!["pane_1".into(), "pane_2".into()];
    state.set_snapshot(Box::new(local));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![agent_on(
            "pane_1",
            "remote one",
            AgentStatus::Idle,
            1,
        )])),
    );
    state.set_test_endpoint_agent_view(&ClientEndpointId::Local, Some(current_workspace_view()));
    state.open_agent_picker_overlay();

    // The active machine's view scopes the picker, so the same workspace ID on
    // another machine stays out.
    assert_eq!(
        labels(&agent_rows(&state)),
        ["Local · local one", "Local · local two"]
    );
    // The indicator shows the view label the sidebar shows.
    let text = frame_text(&mut state);
    assert!(text.contains(" / agents · current space"), "frame:\n{text}");
}

#[test]
fn agent_picker_enter_on_remote_row_switches_machine() {
    let (mut state, remote) = state_with_remote();
    state.set_snapshot(Box::new(local_with_agents(vec![agent_on(
        "pane_1",
        "local one",
        AgentStatus::Idle,
        1,
    )])));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![agent_on(
            "pane_1",
            "remote blocked",
            AgentStatus::Blocked,
            2,
        )])),
    );

    state.open_agent_picker_overlay();
    preview_key(&mut state, b"j");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::ActivateEndpoint {
            endpoint_id: activated,
            target: Some(ClientEndpointFocusTarget::Pane(pane_id)),
        }] if activated == &remote && pane_id == "pane_1"
    ));
    assert!(outcome.repaint, "accepting a row must request a frame");
    assert!(state.overlay.is_none());
}

#[test]
fn agent_picker_open_preselects_first_row() {
    // The focused local agent is not at the top of the priority sort, so the
    // picker starts on the first row instead of the pane the user came from.
    let (mut state, remote) = priority_state();
    state.set_snapshot(Box::new(local_with_agents(vec![agent_on(
        "pane_1",
        "local idle",
        AgentStatus::Idle,
        1,
    )])));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![agent_on(
            "pane_1",
            "remote blocked",
            AgentStatus::Blocked,
            1,
        )])),
    );

    state.open_agent_picker_overlay();
    let rows = agent_rows(&state);
    assert_eq!(rows[0].label, "Build · remote blocked");
    assert!(rows.iter().any(|row| row.current), "focused local row");
    assert_eq!(
        navigator(&state).selected,
        Some(ClientNavigatorTarget::Pane {
            endpoint_id: remote,
            pane_id: "pane_1".into(),
        })
    );
}

#[test]
fn agent_picker_render_count_footer_and_empty_states() {
    let (mut state, remote) = state_with_remote();
    state.set_snapshot(Box::new(local_with_agents(vec![
        agent_on("pane_1", "local one", AgentStatus::Idle, 1),
        agent_on("pane_2", "local two", AgentStatus::Idle, 2),
    ])));
    state.set_endpoint_snapshot(
        &remote,
        Box::new(remote_with_agents(vec![agent_on(
            "pane_1",
            "remote one",
            AgentStatus::Blocked,
            3,
        )])),
    );

    state.open_agent_picker_overlay();
    let text = frame_text(&mut state);
    assert!(text.contains(" / search agents"), "frame:\n{text}");
    assert!(text.contains("3 agents"), "frame:\n{text}");
    assert!(!text.contains("terminals"), "frame:\n{text}");
    // The flat picker has no workspace sections for ←→ to jump between.
    assert!(text.contains("a/b/w/i/d filter"), "frame:\n{text}");
    assert!(!text.contains("←→ workspace"), "frame:\n{text}");
    let rows = agent_rows(&state);
    assert_eq!(
        state
            .hits
            .navigator_rows
            .iter()
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>(),
        rows.iter()
            .map(|row| row.target.clone())
            .collect::<Vec<_>>()
    );

    // A filter or query that hides every agent says nothing matches.
    preview_key(&mut state, b"w");
    let text = frame_text(&mut state);
    assert!(text.contains("No matching agents"), "frame:\n{text}");
    assert!(state.hits.navigator_rows.is_empty());
    preview_key(&mut state, b"a");
    agent_rows_with_query(&mut state, "zzznothing");
    let text = frame_text(&mut state);
    assert!(text.contains("No matching agents"), "frame:\n{text}");

    // A picker with no agents at all says so.
    let (mut empty, _) = state_with_remote();
    empty.open_agent_picker_overlay();
    let text = frame_text(&mut empty);
    assert!(text.contains("No agents"), "frame:\n{text}");
    assert!(!text.contains("No matching agents"), "frame:\n{text}");

    // The tree navigator keeps its terminal count, prompt, and workspace hint.
    let (mut tree, _) = state_with_remote();
    tree.open_navigator_overlay();
    let text = frame_text(&mut tree);
    assert!(text.contains("2 terminals"), "frame:\n{text}");
    assert!(
        text.contains(" / search agents and terminals"),
        "frame:\n{text}"
    );
    assert!(text.contains("←→ workspace"), "frame:\n{text}");
}

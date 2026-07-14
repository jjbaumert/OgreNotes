// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pure navigation model for the shared menu primitive.
//!
//! Holds no `leptos` or `web_sys` types so every rule about how the
//! highlight moves — wrapping, skipping separators and disabled items,
//! entering and leaving submenus — is natively unit-testable. The
//! component layer (`components/menu` in the binary) derives a
//! `Vec<NavNode>` shape from its `MenuEntry` tree and drives these
//! functions from keyboard and pointer events.
//!
//! Lives at the lib crate root (not under `components/`) so CI's
//! `cargo test --lib` exercises the tests — the `components` tree is
//! binary-only.

/// Structural shadow of one menu entry, carrying only what navigation
/// needs to know.
#[derive(Debug, Clone)]
pub enum NavNode {
    Leaf { disabled: bool },
    Parent { disabled: bool, children: Vec<NavNode> },
    Separator,
}

/// Where the keyboard highlight currently is.
///
/// `path` is the chain of open submenus (entry index at each depth);
/// `active` is the highlighted index *within* the deepest open panel.
/// An item can therefore be "highlighted" either because it is on the
/// open-submenu chain or because it is the active item — see
/// [`is_highlighted`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MenuNavState {
    pub path: Vec<usize>,
    pub active: Option<usize>,
}

/// Resolve the panel (slice of siblings) that `path` points into.
/// `None` when the path is stale (indexes a leaf or out of bounds) —
/// callers reset to the root panel in that case.
pub fn panel_at<'a>(nodes: &'a [NavNode], path: &[usize]) -> Option<&'a [NavNode]> {
    let mut cur = nodes;
    for &i in path {
        match cur.get(i) {
            Some(NavNode::Parent { children, .. }) => cur = children,
            _ => return None,
        }
    }
    Some(cur)
}

fn selectable(node: &NavNode) -> bool {
    matches!(
        node,
        NavNode::Leaf { disabled: false } | NavNode::Parent { disabled: false, .. }
    )
}

/// Move the highlight by `delta` (±1) within one panel, wrapping and
/// skipping separators and disabled items. `from: None` enters the
/// panel: delta +1 lands on the first selectable, −1 on the last.
/// Returns `None` when the panel has nothing selectable.
pub fn step_active(panel: &[NavNode], from: Option<usize>, delta: isize) -> Option<usize> {
    let len = panel.len() as isize;
    if len == 0 || !panel.iter().any(selectable) {
        return None;
    }
    let mut idx = match from {
        Some(i) => i as isize,
        None if delta > 0 => -1,
        None => len,
    };
    for _ in 0..len {
        idx = (idx + delta).rem_euclid(len);
        if selectable(&panel[idx as usize]) {
            return Some(idx as usize);
        }
    }
    None
}

pub fn first_selectable(panel: &[NavNode]) -> Option<usize> {
    step_active(panel, None, 1)
}

pub fn last_selectable(panel: &[NavNode]) -> Option<usize> {
    step_active(panel, None, -1)
}

/// Arrow up/down: move within the deepest open panel. A stale path
/// (entries changed under us) resets to the root panel's edge item.
pub fn nav_move(nodes: &[NavNode], state: &MenuNavState, delta: isize) -> MenuNavState {
    match panel_at(nodes, &state.path) {
        Some(panel) => MenuNavState {
            path: state.path.clone(),
            active: step_active(panel, state.active, delta),
        },
        None => MenuNavState {
            path: Vec::new(),
            active: step_active(nodes, None, delta),
        },
    }
}

pub fn nav_home(nodes: &[NavNode], state: &MenuNavState) -> MenuNavState {
    let panel = panel_at(nodes, &state.path).unwrap_or(nodes);
    MenuNavState {
        path: panel_at(nodes, &state.path).map(|_| state.path.clone()).unwrap_or_default(),
        active: first_selectable(panel),
    }
}

pub fn nav_end(nodes: &[NavNode], state: &MenuNavState) -> MenuNavState {
    let panel = panel_at(nodes, &state.path).unwrap_or(nodes);
    MenuNavState {
        path: panel_at(nodes, &state.path).map(|_| state.path.clone()).unwrap_or_default(),
        active: last_selectable(panel),
    }
}

/// ArrowRight / Enter on a submenu parent: descend into it, landing on
/// its first selectable child. Unchanged state when the active item is
/// not an enabled parent — callers compare with `!=` to detect that.
pub fn nav_enter_submenu(nodes: &[NavNode], state: &MenuNavState) -> MenuNavState {
    let Some(panel) = panel_at(nodes, &state.path) else {
        return state.clone();
    };
    if let Some(i) = state.active
        && let Some(NavNode::Parent { disabled: false, children }) = panel.get(i)
    {
        let mut path = state.path.clone();
        path.push(i);
        return MenuNavState {
            path,
            active: first_selectable(children),
        };
    }
    state.clone()
}

/// ArrowLeft: close the deepest submenu, re-highlighting its parent
/// row. No-op at the root panel.
pub fn nav_exit_submenu(state: &MenuNavState) -> MenuNavState {
    let mut path = state.path.clone();
    match path.pop() {
        Some(popped) => MenuNavState {
            path,
            active: Some(popped),
        },
        None => state.clone(),
    }
}

/// Whether the item at (`depth`, `index`) should render highlighted:
/// either it is a parent on the open-submenu chain, or it is the
/// active item of the deepest panel.
pub fn is_highlighted(state: &MenuNavState, depth: usize, index: usize) -> bool {
    state.path.get(depth) == Some(&index)
        || (state.path.len() == depth && state.active == Some(index))
}

/// Estimated rendered width of a menu panel — used for viewport
/// clamping before the DOM exists. Matches the CSS `min-width` plus
/// typical label overshoot; an over-estimate only pulls the menu a few
/// px further from the edge, so precision is not critical.
pub const MENU_EST_WIDTH: f64 = 240.0;
const ITEM_EST_HEIGHT: f64 = 26.0;
const SEP_EST_HEIGHT: f64 = 9.0;
const PANEL_V_PADDING: f64 = 10.0;

/// (width, height) estimate for clamping a panel to the viewport.
pub fn estimate_menu_size(panel: &[NavNode]) -> (f64, f64) {
    let (items, seps) = panel.iter().fold((0usize, 0usize), |(items, seps), n| match n {
        NavNode::Separator => (items, seps + 1),
        _ => (items + 1, seps),
    });
    (
        MENU_EST_WIDTH,
        PANEL_V_PADDING + items as f64 * ITEM_EST_HEIGHT + seps as f64 * SEP_EST_HEIGHT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf() -> NavNode {
        NavNode::Leaf { disabled: false }
    }
    fn dis() -> NavNode {
        NavNode::Leaf { disabled: true }
    }
    fn parent(children: Vec<NavNode>) -> NavNode {
        NavNode::Parent { disabled: false, children }
    }

    // ─── step_active ───────────────────────────────────────────

    #[test]
    fn step_enters_at_first_and_last() {
        let panel = vec![NavNode::Separator, leaf(), leaf()];
        assert_eq!(step_active(&panel, None, 1), Some(1));
        assert_eq!(step_active(&panel, None, -1), Some(2));
    }

    #[test]
    fn step_wraps_both_directions() {
        let panel = vec![leaf(), leaf(), leaf()];
        assert_eq!(step_active(&panel, Some(2), 1), Some(0));
        assert_eq!(step_active(&panel, Some(0), -1), Some(2));
    }

    #[test]
    fn step_skips_disabled_and_separators() {
        let panel = vec![leaf(), dis(), NavNode::Separator, leaf()];
        assert_eq!(step_active(&panel, Some(0), 1), Some(3));
        assert_eq!(step_active(&panel, Some(3), -1), Some(0));
    }

    #[test]
    fn step_all_disabled_is_none() {
        let panel = vec![dis(), NavNode::Separator, dis()];
        assert_eq!(step_active(&panel, None, 1), None);
    }

    #[test]
    fn step_empty_is_none() {
        assert_eq!(step_active(&[], None, 1), None);
    }

    #[test]
    fn disabled_parent_not_selectable() {
        let panel = vec![NavNode::Parent { disabled: true, children: vec![leaf()] }, leaf()];
        assert_eq!(step_active(&panel, None, 1), Some(1));
    }

    // ─── submenu enter/exit ────────────────────────────────────

    #[test]
    fn enter_on_leaf_is_unchanged() {
        let nodes = vec![leaf()];
        let st = MenuNavState { path: vec![], active: Some(0) };
        assert_eq!(nav_enter_submenu(&nodes, &st), st);
    }

    #[test]
    fn enter_on_disabled_parent_is_unchanged() {
        let nodes = vec![NavNode::Parent { disabled: true, children: vec![leaf()] }];
        let st = MenuNavState { path: vec![], active: Some(0) };
        assert_eq!(nav_enter_submenu(&nodes, &st), st);
    }

    #[test]
    fn enter_descends_to_first_enabled_child() {
        let nodes = vec![leaf(), parent(vec![dis(), leaf()])];
        let st = MenuNavState { path: vec![], active: Some(1) };
        let next = nav_enter_submenu(&nodes, &st);
        assert_eq!(next, MenuNavState { path: vec![1], active: Some(1) });
    }

    #[test]
    fn enter_supports_nested_parents() {
        let nodes = vec![parent(vec![parent(vec![leaf()])])];
        let st = MenuNavState { path: vec![0], active: Some(0) };
        let next = nav_enter_submenu(&nodes, &st);
        assert_eq!(next, MenuNavState { path: vec![0, 0], active: Some(0) });
    }

    #[test]
    fn exit_pops_and_reactivates_parent() {
        let st = MenuNavState { path: vec![2, 1], active: Some(0) };
        assert_eq!(
            nav_exit_submenu(&st),
            MenuNavState { path: vec![2], active: Some(1) }
        );
    }

    #[test]
    fn exit_at_root_is_noop() {
        let st = MenuNavState { path: vec![], active: Some(3) };
        assert_eq!(nav_exit_submenu(&st), st);
    }

    // ─── nav_move / home / end ─────────────────────────────────

    #[test]
    fn move_operates_on_deepest_panel() {
        let nodes = vec![parent(vec![leaf(), leaf()])];
        let st = MenuNavState { path: vec![0], active: Some(0) };
        assert_eq!(
            nav_move(&nodes, &st, 1),
            MenuNavState { path: vec![0], active: Some(1) }
        );
    }

    #[test]
    fn move_with_stale_path_resets_to_root() {
        let nodes = vec![leaf(), leaf()];
        let st = MenuNavState { path: vec![7], active: Some(0) };
        assert_eq!(
            nav_move(&nodes, &st, 1),
            MenuNavState { path: vec![], active: Some(0) }
        );
    }

    #[test]
    fn home_and_end_land_on_edges() {
        let nodes = vec![NavNode::Separator, leaf(), dis(), leaf()];
        let st = MenuNavState { path: vec![], active: Some(3) };
        assert_eq!(nav_home(&nodes, &st).active, Some(1));
        assert_eq!(nav_end(&nodes, &st).active, Some(3));
    }

    // ─── is_highlighted ────────────────────────────────────────

    #[test]
    fn highlight_covers_open_chain_and_active() {
        let st = MenuNavState { path: vec![2], active: Some(4) };
        assert!(is_highlighted(&st, 0, 2)); // parent on the chain
        assert!(is_highlighted(&st, 1, 4)); // active in deepest panel
        assert!(!is_highlighted(&st, 0, 4)); // active index, wrong depth
        assert!(!is_highlighted(&st, 1, 2));
    }

    // ─── estimate_menu_size ────────────────────────────────────

    #[test]
    fn size_estimate_counts_items_and_separators() {
        let panel = vec![leaf(), NavNode::Separator, leaf(), parent(vec![leaf()])];
        let (w, h) = estimate_menu_size(&panel);
        assert_eq!(w, MENU_EST_WIDTH);
        assert_eq!(h, 10.0 + 3.0 * 26.0 + 9.0);
    }
}

//! Éditeur de tâche, par de vrais clics.

mod common;

use common::*;

/// Une session Claude travaille d'office dans un worktree dédié.
#[test]
fn claude_session_gets_a_worktree_by_default() {
    let dir = temp("editeur-claude");
    let mut h = app(&dir);
    click(&mut h, "+ Nouvelle tâche");
    click(&mut h, "Session Claude non interactive");
    let draft = h.state().draft().expect("éditeur ouvert");
    assert!(draft.action.claude.is_some());
    assert_eq!(draft.action.worktree.map(|w| w.branche).as_deref(), Some("main"));
}

//! Ephemeral live selections. Encoded Loro cursors follow edits without entering the document.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::Id;
use crate::sync::{CURSOR_TIMEOUT, Selection};

struct Peer {
    name: String,
    seen: Instant,
    selections: Vec<Selection>,
}

/// A resolved selection, ready for a platform to draw without revealing the owner's hidden syntax.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteSelection {
    pub machine: Id,
    pub name: String,
    pub selection: super::Selection,
}

/// Sessions outlive expired cursors: the next cursor message can show that machine again.
#[derive(Default)]
pub struct Presence {
    peers: HashMap<Id, Peer>,
    sessions: HashSet<Id>,
}

impl Presence {
    pub fn started(&mut self, machine: Id) {
        self.sessions.insert(machine);
    }

    pub fn ended(&mut self, machine: Id) {
        self.sessions.remove(&machine);
        self.peers.remove(&machine);
    }

    /// Ignores messages after their session ended. Positions resolve only when they're drawn, so
    /// messages can arrive before the text update that defines their cursors.
    pub fn received(&mut self, machine: Id, selections: Vec<Selection>) {
        if !self.sessions.contains(&machine) {
            return;
        }
        let peer = self.peers.entry(machine).or_insert_with(|| Peer {
            name: machine.to_string(),
            seen: Instant::now(),
            selections: Vec::new(),
        });
        peer.seen = Instant::now();
        peer.selections = selections;
    }

    /// A delayed name lookup can't bring back an ended or expired cursor.
    pub fn set_name(&mut self, machine: Id, name: String) {
        if let Some(peer) = self.peers.get_mut(&machine) {
            peer.name = name;
        }
    }

    /// Returns whether any visible cursors expired.
    pub fn expire(&mut self) -> bool {
        self.expire_at(Instant::now())
    }

    fn expire_at(&mut self, now: Instant) -> bool {
        let before = self.peers.len();
        self.peers.retain(|_, peer| now.duration_since(peer.seen) < CURSOR_TIMEOUT);
        self.peers.len() != before
    }

    pub(super) fn resolve(
        &self,
        mut position: impl FnMut(&[u8]) -> Option<usize>,
    ) -> Vec<RemoteSelection> {
        let mut peers: Vec<_> = self.peers.iter().collect();
        peers.sort_by_key(|(machine, _)| **machine);
        let mut resolved = Vec::new();
        for (machine, peer) in peers {
            for selection in &peer.selections {
                if let (Some(anchor), Some(head)) =
                    (position(&selection.anchor), position(&selection.head))
                {
                    resolved.push(RemoteSelection {
                        machine: *machine,
                        name: peer.name.clone(),
                        selection: super::Selection { anchor, head },
                    });
                }
            }
        }
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_and_ended_cursors_disappear_without_reviving_from_a_name_lookup() {
        let machine = Id::generate();
        let selection = Selection { anchor: vec![1], head: vec![2] };
        let mut presence = Presence::default();
        presence.received(machine, vec![selection.clone()]);
        assert!(presence.peers.is_empty());
        presence.started(machine);
        presence.received(machine, vec![selection.clone()]);
        let seen = presence.peers[&machine].seen;
        assert!(!presence.expire_at(seen + CURSOR_TIMEOUT / 2));
        assert!(presence.expire_at(seen + CURSOR_TIMEOUT));
        presence.set_name(machine, "late name".to_owned());
        assert!(presence.peers.is_empty());
        presence.received(machine, vec![selection.clone()]);
        assert_eq!(presence.peers.len(), 1);
        presence.ended(machine);
        presence.received(machine, vec![selection]);
        presence.set_name(machine, "late name".to_owned());
        assert!(presence.peers.is_empty());
    }
}

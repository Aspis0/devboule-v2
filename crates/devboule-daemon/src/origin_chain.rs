//! The hops behind a piece of untrusted content, and whether any of them was
//! data (a page, a screen, a CI log, a restore whose provenance is gone)
//! rather than an agent.
//!
//! One responsibility: the daemon's own record of how content reached a session,
//! so a message that session relays names where its input came from. Hops are
//! only ever built from ids the daemon holds; nothing here parses a body. Data
//! hops are sticky — they stay on a session until the person next types to it —
//! because an agent that read a page may relay that page's words later, whatever
//! another agent says to it in between.

use crate::untrusted_frame::fact_line;

/// The most hops a chain names; an older one is dropped and the cut is marked,
/// so a chain is always bounded and never silently shortened.
pub(crate) const MAX_CHAIN_HOPS: usize = 4;
const MAX_FACT_CHARS: usize = 96;

/// One hop of an origin chain: `local:<session>`, `peer:<device>/<session>`,
/// `browser:<host>`; the id is bounded, single-line and visible.
pub(crate) fn hop(kind: &str, id: &str) -> String {
    format!("{kind}:{}", fact_line(id, MAX_FACT_CHARS))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chain {
    /// Agent hops, oldest first, sender last.
    agents: Vec<String>,
    /// Data hops (`browser:<host>`, `terminal`, `ci`) and the `restored` marker
    /// of a rebuild, distinct, oldest first.
    data: Vec<String>,
    /// Whether data ever reached this chain, even if its hop was since dropped
    /// to keep the chain bounded.
    tainted: bool,
    /// Whether any hop was dropped to keep the chain bounded.
    cut: bool,
}

impl Chain {
    /// A rebuild whose provenance is gone: the session comes back as data may
    /// have reached it — `restored` is the daemon's own marker, never a
    /// body's — and only the person's own message takes it off.
    pub(crate) fn restored() -> Chain {
        Chain {
            data: vec!["restored".to_string()],
            tainted: true,
            ..Chain::default()
        }
    }

    /// Whether any hop read untrusted data. No sentence reads this since
    /// agent text stopped carrying the taint line; the ingress, restore
    /// and relay machinery that sets it is untouched.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_tainted(&self) -> bool {
        self.tainted
    }

    /// The hops as a message names them: data first, then agents, with the
    /// oldest replaced by one `…` when the chain was cut or is longer than
    /// [`MAX_CHAIN_HOPS`].
    pub(crate) fn hops(&self) -> Vec<String> {
        let mut hops: Vec<String> = self.data.iter().chain(&self.agents).cloned().collect();
        if hops.len() > MAX_CHAIN_HOPS || self.cut {
            let room = hops.len().min(MAX_CHAIN_HOPS - 1);
            hops.drain(..hops.len() - room);
            hops.insert(0, "…".to_string());
        }
        hops
    }

    /// What a session passes on when it sends: its own chain, then itself.
    pub(crate) fn extend(&self, sender: String) -> Chain {
        let mut next = self.clone();
        next.agents.push(sender);
        next.bound();
        next
    }

    /// Data came back to this session: a page it read, a screen, a CI verdict.
    pub(crate) fn tainted_by(&self, data_hop: String) -> Chain {
        let mut next = self.clone();
        next.push_data(data_hop);
        next.tainted = true;
        next.bound();
        next
    }

    /// This session received `incoming`: its agent hops are the newest delivery's,
    /// its data hops are everything it has been tainted by so far.
    pub(crate) fn receive(&self, incoming: &Chain) -> Chain {
        let mut next = Chain {
            agents: incoming.agents.clone(),
            data: self.data.clone(),
            tainted: self.tainted || incoming.tainted,
            cut: self.cut || incoming.cut,
        };
        for data_hop in &incoming.data {
            next.push_data(data_hop.clone());
        }
        next.bound();
        next
    }

    fn push_data(&mut self, data_hop: String) {
        if !self.data.contains(&data_hop) {
            self.data.push(data_hop);
        }
    }

    /// Keep the newest [`MAX_CHAIN_HOPS`] of each list; the cut is remembered.
    fn bound(&mut self) {
        for list in [&mut self.agents, &mut self.data] {
            if list.len() > MAX_CHAIN_HOPS {
                list.drain(..list.len() - MAX_CHAIN_HOPS);
                self.cut = true;
            }
        }
    }
}

#[cfg(test)]
#[path = "origin_chain_tests.rs"]
mod tests;

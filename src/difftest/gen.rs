//! Deterministic random command streams for the differential test.
//! The distributions and their rationale are documented in README §5.

use crate::command::Command;
use crate::event::Event;
use crate::loadgen::SplitMix64;
use crate::types::{AccountId, OrderId, Qty, Side};

const ACCOUNTS: u64 = 4;
/// Ids from here up are never used by NEW, so they are "never seen".
const NEVER_SEEN_BASE: OrderId = 1 << 40;

/// Generates commands; distributions are documented in the README.
pub struct Generator {
    rng: SplitMix64,
    next_id: OrderId,
    accepted: Vec<OrderId>,
    rejected: Vec<OrderId>,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SplitMix64::new(seed),
            next_id: 1,
            accepted: Vec::new(),
            rejected: Vec::new(),
        }
    }

    /// Learns which ids were accepted or rejected, to reuse them later.
    pub fn observe(&mut self, events: &[Event]) {
        for e in events {
            match *e {
                Event::Ack { id } => self.accepted.push(id),
                Event::Reject { id, .. } => self.rejected.push(id),
                _ => {}
            }
        }
    }

    /// Next command. `live` is (id, remaining) of every live order, from the reference.
    pub fn next(&mut self, live: &[(OrderId, Qty)]) -> Command {
        let roll = self.rng.below(100);
        if roll < 55 {
            self.new_order()
        } else if roll < 85 {
            Command::Cancel {
                id: self.target(live).0,
            }
        } else {
            let (id, remaining) = self.target(live);
            Command::Modify {
                id,
                new_qty: self.modify_qty(remaining),
            }
        }
    }

    fn new_order(&mut self) -> Command {
        let roll = self.rng.below(100);
        let id = if roll < 8 && !self.accepted.is_empty() {
            self.rng.pick(&self.accepted).unwrap_or(0)
        } else if roll < 15 && !self.rejected.is_empty() {
            self.rng.pick(&self.rejected).unwrap_or(0)
        } else {
            let id = self.next_id;
            self.next_id += 1;
            id
        };
        let price = match self.rng.below(100) {
            0..=89 => self.rng.range(95, 105),
            90..=95 => self.rng.range(50, 200),
            _ => 0,
        };
        let qty = if self.rng.percent(95) {
            self.rng.range(1, 10)
        } else {
            0
        };
        Command::New {
            id,
            account: self.rng.below(ACCOUNTS) as AccountId,
            side: if self.rng.percent(50) {
                Side::Buy
            } else {
                Side::Sell
            },
            price,
            qty,
        }
    }

    /// Target for CANCEL/MODIFY: 80% live, 10% any accepted (often dead), 10% never seen.
    /// Returns the live remaining quantity, or `None` if not known to be live.
    fn target(&mut self, live: &[(OrderId, Qty)]) -> (OrderId, Option<Qty>) {
        let roll = self.rng.below(100);
        if roll < 80 {
            if let Some((id, rem)) = self.rng.pick(live) {
                return (id, Some(rem));
            }
        }
        if roll < 90 {
            if let Some(id) = self.rng.pick(&self.accepted) {
                return (id, None);
            }
        }
        (NEVER_SEEN_BASE + self.rng.below(1000), None)
    }

    /// For a live order: 25% decrease, 25% equal, 35% increase, 15% zero.
    fn modify_qty(&mut self, remaining: Option<Qty>) -> Qty {
        let Some(rem) = remaining else {
            return self.rng.range(0, 10);
        };
        match self.rng.below(100) {
            0..=24 if rem > 1 => self.rng.range(1, rem - 1),
            0..=49 => rem,
            50..=84 => rem + self.rng.range(1, 10),
            _ => 0,
        }
    }
}

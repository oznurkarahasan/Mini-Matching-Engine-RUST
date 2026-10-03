//! Baseline book: per side `BTreeMap<price, VecDeque<id>>` plus a hash map of
//! live orders. Simple and allocation-heavy; the reference point for Part 3.

use std::collections::{BTreeMap, HashMap, VecDeque};

use super::{Book, Resting};
use crate::types::{OrderId, Price, Qty, Side};

type Levels = BTreeMap<Price, VecDeque<OrderId>>;

#[derive(Debug, Default)]
pub struct BaselineBook {
    /// Best bid = highest key.
    bids: Levels,
    /// Best ask = lowest key.
    asks: Levels,
    /// Live orders only. Invariant: exactly the ids queued in `bids` and `asks`.
    orders: HashMap<OrderId, Resting>,
}

impl BaselineBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn best_queue(&self, resting_side: Side) -> Option<&VecDeque<OrderId>> {
        match resting_side {
            Side::Buy => self.bids.last_key_value(),
            Side::Sell => self.asks.first_key_value(),
        }
        .map(|(_, q)| q)
    }
}

impl Book for BaselineBook {
    fn front(&self, resting_side: Side) -> Option<Resting> {
        let id = self.best_queue(resting_side)?.front()?;
        self.orders.get(id).copied()
    }

    fn fill_front(&mut self, resting_side: Side, qty: Qty) {
        let entry = match resting_side {
            Side::Buy => self.bids.last_entry(),
            Side::Sell => self.asks.first_entry(),
        };
        debug_assert!(entry.is_some(), "fill_front on empty side");
        let Some(mut level) = entry else {
            return;
        };
        let Some(order) = level.get().front().and_then(|id| self.orders.get_mut(id)) else {
            debug_assert!(false, "front id not live");
            return;
        };
        debug_assert!(
            qty > 0 && qty <= order.remaining,
            "fill_front: qty {qty} not in 1..={}",
            order.remaining
        );
        let Some(left) = order.remaining.checked_sub(qty) else {
            return;
        };
        if left > 0 {
            order.remaining = left;
            return;
        }
        let id = order.id;
        self.orders.remove(&id);
        level.get_mut().pop_front();
        if level.get().is_empty() {
            level.remove();
        }
    }

    fn insert(&mut self, resting_side: Side, order: Resting) {
        debug_assert!(order.remaining > 0, "insert with zero quantity");
        debug_assert!(!self.orders.contains_key(&order.id), "insert of live id");
        let levels = match resting_side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        levels.entry(order.price).or_default().push_back(order.id);
        self.orders.insert(order.id, order);
    }

    #[cfg(test)]
    fn check_invariants(&self) {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        for (side, levels) in [("bid", &self.bids), ("ask", &self.asks)] {
            for (&price, queue) in levels {
                assert!(!queue.is_empty(), "empty {side} level at {price}");
                for id in queue {
                    assert!(seen.insert(*id), "id {id} queued twice");
                    let o = self.orders.get(id).expect("queued id must be live");
                    assert_eq!(o.id, *id);
                    assert_eq!(o.price, price, "id {id} queued at wrong price");
                    assert!(o.remaining > 0, "id {id} has zero remaining");
                }
            }
        }
        assert_eq!(seen.len(), self.orders.len(), "live order not queued");
        if let (Some((bid, _)), Some((ask, _))) =
            (self.bids.last_key_value(), self.asks.first_key_value())
        {
            assert!(bid < ask, "crossed book: bid {bid} >= ask {ask}");
        }
    }

    #[cfg(test)]
    fn live_ids(&self) -> Vec<OrderId> {
        self.orders.keys().copied().collect()
    }
}

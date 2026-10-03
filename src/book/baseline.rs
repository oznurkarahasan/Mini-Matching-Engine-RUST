//! Baseline book: per side `BTreeMap<price, VecDeque<id>>` plus a hash map of
//! live orders. Simple and allocation-heavy; the reference point for Part 3.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap, VecDeque};

use super::{Book, Resting};
use crate::types::{OrderId, Price, Qty, Side};

type Levels = BTreeMap<Price, VecDeque<OrderId>>;

/// A live order plus the side it rests on (needed to find its level on cancel).
#[derive(Debug, Clone, Copy)]
struct Order {
    resting: Resting,
    side: Side,
}

#[derive(Debug, Default)]
pub struct BaselineBook {
    /// Best bid = highest key.
    bids: Levels,
    /// Best ask = lowest key.
    asks: Levels,
    /// Live orders only. Invariant: exactly the ids queued in `bids` and `asks`.
    orders: HashMap<OrderId, Order>,
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

    fn levels_mut(&mut self, resting_side: Side) -> &mut Levels {
        match resting_side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }
}

impl Book for BaselineBook {
    fn front(&self, resting_side: Side) -> Option<Resting> {
        let id = self.best_queue(resting_side)?.front()?;
        self.orders.get(id).map(|o| o.resting)
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
        let order = &mut order.resting;
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
        self.levels_mut(resting_side)
            .entry(order.price)
            .or_default()
            .push_back(order.id);
        self.orders.insert(
            order.id,
            Order {
                resting: order,
                side: resting_side,
            },
        );
    }

    fn remove(&mut self, id: OrderId) -> Option<Qty> {
        let Order { resting, side } = self.orders.remove(&id)?;
        match self.levels_mut(side).entry(resting.price) {
            Entry::Occupied(mut level) => {
                // Linear search within the level: O(level length). Deliberate
                // baseline cost, replaced in Part 3 (see DECISIONS.md #4, #5).
                let pos = level.get().iter().position(|&q| q == id);
                debug_assert!(pos.is_some(), "live id {id} not in its level");
                if let Some(pos) = pos {
                    level.get_mut().remove(pos);
                }
                if level.get().is_empty() {
                    level.remove();
                }
            }
            Entry::Vacant(_) => debug_assert!(false, "live id {id} has no level"),
        }
        Some(resting.remaining)
    }

    fn get_remaining(&self, id: OrderId) -> Option<Qty> {
        self.orders.get(&id).map(|o| o.resting.remaining)
    }

    fn set_qty(&mut self, id: OrderId, new_qty: Qty) {
        let Some(order) = self.orders.get_mut(&id) else {
            debug_assert!(false, "set_qty on non-live id {id}");
            return;
        };
        debug_assert!(
            0 < new_qty && new_qty <= order.resting.remaining,
            "set_qty: {new_qty} not in 1..={}",
            order.resting.remaining
        );
        order.resting.remaining = new_qty;
    }

    fn move_to_back(&mut self, id: OrderId, new_qty: Qty) {
        debug_assert!(new_qty > 0, "move_to_back with zero quantity");
        let Some(order) = self.orders.get_mut(&id) else {
            debug_assert!(false, "move_to_back on non-live id {id}");
            return;
        };
        order.resting.remaining = new_qty;
        let (side, price) = (order.side, order.resting.price);
        let Some(queue) = self.levels_mut(side).get_mut(&price) else {
            debug_assert!(false, "live id {id} has no level");
            return;
        };
        // Same O(level length) linear search as `remove`; see DECISIONS.md #4, #5.
        let pos = queue.iter().position(|&q| q == id);
        debug_assert!(pos.is_some(), "live id {id} not in its level");
        if let Some(pos) = pos {
            queue.remove(pos);
            queue.push_back(id);
        }
    }

    #[cfg(test)]
    fn check_invariants(&self) {
        use std::collections::HashSet;

        let mut seen = HashSet::new();
        for (side, levels) in [(Side::Buy, &self.bids), (Side::Sell, &self.asks)] {
            for (&price, queue) in levels {
                assert!(!queue.is_empty(), "empty {side:?} level at {price}");
                for id in queue {
                    assert!(seen.insert(*id), "id {id} queued twice");
                    let o = self.orders.get(id).expect("queued id must be live");
                    assert_eq!(o.side, side, "id {id} stored with wrong side");
                    assert_eq!(o.resting.id, *id);
                    assert_eq!(o.resting.price, price, "id {id} queued at wrong price");
                    assert!(o.resting.remaining > 0, "id {id} has zero remaining");
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

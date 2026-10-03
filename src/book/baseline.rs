//! Baseline book: per side `BTreeMap<price, VecDeque<id>>` plus a hash map of
//! live orders. Simple and allocation-heavy; the reference point for Part 3.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap, VecDeque};

use super::{Book, Resting};
use crate::hash::IdBuildHasher;
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
    orders: HashMap<OrderId, Order, IdBuildHasher>,
}

impl BaselineBook {
    /// Bytes per bucket entry of the `orders` map (key + value), for memory estimates.
    pub const ORDER_ENTRY_BYTES: usize = std::mem::size_of::<(OrderId, Order)>();

    pub fn new() -> Self {
        Self::default()
    }

    /// (length of the level holding live order `id`, its 0-based position in
    /// the FIFO). Reporting only: shows the linear-search cost of cancel.
    pub fn level_len_and_pos_of(&self, id: OrderId) -> Option<(usize, usize)> {
        let o = self.orders.get(&id)?;
        let levels = match o.side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        };
        let q = levels.get(&o.resting.price)?;
        Some((q.len(), q.iter().position(|&x| x == id)?))
    }

    /// Current capacity of the live-order map (for memory reporting).
    pub fn orders_capacity(&self) -> usize {
        self.orders.capacity()
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
    fn with_capacity(expected_live_orders: usize) -> Self {
        Self {
            orders: HashMap::with_capacity_and_hasher(
                expected_live_orders,
                IdBuildHasher::default(),
            ),
            ..Self::default()
        }
    }

    fn front(&self, resting_side: Side) -> Option<Resting> {
        let id = self.best_queue(resting_side)?.front()?;
        match self.orders.get(id) {
            Some(o) => Some(o.resting),
            None => panic!("baseline invariant violated: front id {id} is not live"),
        }
    }

    fn fill_front(&mut self, resting_side: Side, qty: Qty) {
        let entry = match resting_side {
            Side::Buy => self.bids.last_entry(),
            Side::Sell => self.asks.first_entry(),
        };
        let Some(mut level) = entry else {
            panic!("fill_front on empty {resting_side:?} side");
        };
        let Some(order) = level.get().front().and_then(|id| self.orders.get_mut(id)) else {
            panic!("baseline invariant violated: front id of a level is not live");
        };
        let order = &mut order.resting;
        assert!(
            qty > 0 && qty <= order.remaining,
            "fill_front: qty {qty} not in 1..={}",
            order.remaining
        );
        let left = order.remaining - qty;
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
        assert!(
            order.remaining > 0,
            "insert of id {} with zero quantity",
            order.id
        );
        // The map's own insert detects a live duplicate: no extra lookup.
        let previous = self.orders.insert(
            order.id,
            Order {
                resting: order,
                side: resting_side,
            },
        );
        assert!(previous.is_none(), "insert of already-live id {}", order.id);
        self.levels_mut(resting_side)
            .entry(order.price)
            .or_default()
            .push_back(order.id);
    }

    fn remove(&mut self, id: OrderId) -> Option<Qty> {
        let Order { resting, side } = self.orders.remove(&id)?;
        match self.levels_mut(side).entry(resting.price) {
            Entry::Occupied(mut level) => {
                // Linear search within the level: O(level length). Deliberate
                // baseline cost, replaced in Part 3 (see DECISIONS.md #4, #5).
                let Some(pos) = level.get().iter().position(|&q| q == id) else {
                    panic!("baseline invariant violated: live id {id} not in its level");
                };
                level.get_mut().remove(pos);
                if level.get().is_empty() {
                    level.remove();
                }
            }
            Entry::Vacant(_) => panic!("baseline invariant violated: live id {id} has no level"),
        }
        Some(resting.remaining)
    }

    fn get_remaining(&self, id: OrderId) -> Option<Qty> {
        self.orders.get(&id).map(|o| o.resting.remaining)
    }

    fn set_qty(&mut self, id: OrderId, new_qty: Qty) {
        let Some(order) = self.orders.get_mut(&id) else {
            panic!("set_qty on non-live id {id}");
        };
        assert!(
            0 < new_qty && new_qty <= order.resting.remaining,
            "set_qty: {new_qty} not in 1..={}",
            order.resting.remaining
        );
        order.resting.remaining = new_qty;
    }

    fn move_to_back(&mut self, id: OrderId, new_qty: Qty) {
        assert!(new_qty > 0, "move_to_back of id {id} with zero quantity");
        let Some(order) = self.orders.get_mut(&id) else {
            panic!("move_to_back on non-live id {id}");
        };
        order.resting.remaining = new_qty;
        let (side, price) = (order.side, order.resting.price);
        let Some(queue) = self.levels_mut(side).get_mut(&price) else {
            panic!("baseline invariant violated: live id {id} has no level");
        };
        // Same O(level length) linear search as `remove`; see DECISIONS.md #4, #5.
        let Some(pos) = queue.iter().position(|&q| q == id) else {
            panic!("baseline invariant violated: live id {id} not in its level");
        };
        queue.remove(pos);
        queue.push_back(id);
    }

    fn order_count(&self) -> usize {
        self.orders.len()
    }

    fn level_count(&self) -> usize {
        self.bids.len() + self.asks.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "fill_front: qty 6 not in 1..=5")]
    fn fill_front_over_remaining_panics() {
        let mut b = BaselineBook::new();
        b.insert(
            Side::Sell,
            Resting {
                id: 1,
                account: 0,
                price: 100,
                remaining: 5,
            },
        );
        b.fill_front(Side::Sell, 6);
    }

    fn one_order(id: OrderId) -> BaselineBook {
        let mut b = BaselineBook::new();
        b.insert(
            Side::Sell,
            Resting {
                id,
                account: 0,
                price: 100,
                remaining: 5,
            },
        );
        b
    }

    #[test]
    #[should_panic(expected = "insert of already-live id 1")]
    fn insert_of_live_id_panics() {
        let mut b = one_order(1);
        b.insert(
            Side::Buy,
            Resting {
                id: 1,
                account: 0,
                price: 90,
                remaining: 1,
            },
        );
    }

    #[test]
    #[should_panic(expected = "set_qty on non-live id 7")]
    fn set_qty_on_non_live_id_panics() {
        one_order(1).set_qty(7, 1);
    }

    #[test]
    #[should_panic(expected = "move_to_back on non-live id 7")]
    fn move_to_back_on_non_live_id_panics() {
        one_order(1).move_to_back(7, 1);
    }

    #[test]
    #[should_panic(expected = "fill_front on empty Buy side")]
    fn fill_front_on_empty_side_panics() {
        BaselineBook::new().fill_front(Side::Buy, 1);
    }
}

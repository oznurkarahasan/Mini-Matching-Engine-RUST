//! Slab book: price levels in a `BTreeMap<Price, Level>` per side; orders in a
//! contiguous `Vec<Slot>` linked per level by `u32` indices (FIFO = head to
//! tail); an id -> slot map. Cancel and modify unlink in O(1) (plus one B-tree
//! access only when the order is at either end of its level); matching reads
//! the head slot directly, without hashing.
//!
//! Broken internal invariants (a slot index that should exist but does not,
//! a violated `fill_front` precondition) panic with a clear message instead of
//! being silently ignored. Input errors never reach this code.
//!
//! Limitations (documented, see DECISIONS.md #14):
//! - At most `u32::MAX - 1` simultaneously live orders. Beyond that `insert`
//!   panics.
//! - The slab never shrinks: memory stays at the peak live-order count. Freed
//!   slots are reused through a free list threaded through `next`.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};

use super::{Book, Resting};
use crate::hash::IdBuildHasher;
use crate::types::{AccountId, OrderId, Price, Qty, Side};

/// "No slot" marker for `head`/`tail`/`prev`/`next`/`free_head`.
const NONE: u32 = u32::MAX;

#[derive(Debug, Clone, Copy)]
struct Level {
    head: u32,
    tail: u32,
}

/// A live order, or a free slot (`remaining == 0`, `next` = next free slot).
#[derive(Debug, Clone, Copy)]
struct Slot {
    id: OrderId,
    price: Price,
    remaining: Qty,
    account: AccountId,
    prev: u32,
    next: u32,
    side: Side,
}

type Levels = BTreeMap<Price, Level>;

#[derive(Debug)]
pub struct SlabBook {
    /// Best bid = highest key.
    bids: Levels,
    /// Best ask = lowest key.
    asks: Levels,
    slots: Vec<Slot>,
    free_head: u32,
    /// Live orders only: id -> slot index.
    index: HashMap<OrderId, u32, IdBuildHasher>,
}

impl Default for SlabBook {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl SlabBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Slots ever allocated (peak live count; the slab never shrinks).
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    #[cfg(test)]
    fn levels(&self, side: Side) -> &Levels {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }
}

/// Slot `i`, which an internal invariant says must exist. Panics otherwise:
/// a corrupted slab must fail loudly, never be silently skipped.
fn slot_mut(slots: &mut [Slot], i: u32) -> &mut Slot {
    let len = slots.len();
    match slots.get_mut(i as usize) {
        Some(s) => s,
        None => panic!("slab invariant violated: slot {i} does not exist (len {len})"),
    }
}

/// Shared-reference counterpart of [`slot_mut`]; panics on an invalid index.
fn slot(slots: &[Slot], i: u32) -> &Slot {
    match slots.get(i as usize) {
        Some(s) => s,
        None => panic!(
            "slab invariant violated: slot {i} does not exist (len {})",
            slots.len()
        ),
    }
}

/// Takes a slot from the free list or appends one. `None` if the index space
/// is exhausted.
fn alloc(slots: &mut Vec<Slot>, free_head: &mut u32, slot: Slot) -> Option<u32> {
    if *free_head != NONE {
        let i = *free_head;
        let s = slot_mut(slots, i);
        *free_head = s.next;
        *s = slot;
        return Some(i);
    }
    let i = u32::try_from(slots.len()).ok().filter(|&i| i != NONE)?;
    slots.push(slot);
    Some(i)
}

/// Returns slot `i` to the free list.
fn release(slots: &mut [Slot], free_head: &mut u32, i: u32) {
    let s = slot_mut(slots, i);
    s.remaining = 0;
    s.prev = NONE;
    s.next = *free_head;
    *free_head = i;
}

/// Removes slot `i` from its level's list; drops the level if it becomes empty.
/// Touches the B-tree only if `i` is the head or tail.
fn unlink(levels: &mut Levels, slots: &mut [Slot], i: u32) {
    let Slot {
        prev, next, price, ..
    } = *slot_mut(slots, i);
    if prev != NONE {
        slot_mut(slots, prev).next = next;
    }
    if next != NONE {
        slot_mut(slots, next).prev = prev;
    }
    if prev == NONE || next == NONE {
        match levels.entry(price) {
            Entry::Occupied(mut e) => {
                let level = e.get_mut();
                if prev == NONE {
                    level.head = next;
                }
                if next == NONE {
                    level.tail = prev;
                }
                if level.head == NONE {
                    e.remove();
                }
            }
            Entry::Vacant(_) => {
                panic!("slab invariant violated: slot {i} at price {price} has no level")
            }
        }
    }
    let s = slot_mut(slots, i);
    s.prev = NONE;
    s.next = NONE;
}

/// Appends slot `i` at the tail of its price level, creating the level if needed.
fn push_back(levels: &mut Levels, slots: &mut [Slot], i: u32, price: Price) {
    match levels.entry(price) {
        Entry::Vacant(e) => {
            e.insert(Level { head: i, tail: i });
            let s = slot_mut(slots, i);
            s.prev = NONE;
            s.next = NONE;
        }
        Entry::Occupied(mut e) => {
            let level = e.get_mut();
            let tail = level.tail;
            level.tail = i;
            slot_mut(slots, tail).next = i;
            let s = slot_mut(slots, i);
            s.prev = tail;
            s.next = NONE;
        }
    }
}

impl Book for SlabBook {
    fn with_capacity(expected_live_orders: usize) -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            slots: Vec::with_capacity(expected_live_orders),
            free_head: NONE,
            index: HashMap::with_capacity_and_hasher(
                expected_live_orders,
                IdBuildHasher::default(),
            ),
        }
    }

    fn front(&self, resting_side: Side) -> Option<Resting> {
        let (_, level) = match resting_side {
            Side::Buy => self.bids.last_key_value(),
            Side::Sell => self.asks.first_key_value(),
        }?;
        let s = slot(&self.slots, level.head);
        Some(Resting {
            id: s.id,
            account: s.account,
            price: s.price,
            remaining: s.remaining,
        })
    }

    fn fill_front(&mut self, resting_side: Side, qty: Qty) {
        let (entry, slots) = match resting_side {
            Side::Buy => (self.bids.last_entry(), &mut self.slots),
            Side::Sell => (self.asks.first_entry(), &mut self.slots),
        };
        let Some(mut level) = entry else {
            panic!("fill_front on empty {resting_side:?} side");
        };
        let head = level.get().head;
        let slot = slot_mut(slots, head);
        assert!(
            qty > 0 && qty <= slot.remaining,
            "fill_front: qty {qty} not in 1..={}",
            slot.remaining
        );
        let left = slot.remaining - qty;
        if left > 0 {
            slot.remaining = left;
            return;
        }
        // Fully filled: unlink the head directly through the entry we hold.
        let (id, next) = (slot.id, slot.next);
        if next == NONE {
            level.remove();
        } else {
            level.get_mut().head = next;
            slot_mut(slots, next).prev = NONE;
        }
        self.index.remove(&id);
        release(&mut self.slots, &mut self.free_head, head);
    }

    fn insert(&mut self, resting_side: Side, order: Resting) {
        assert!(
            order.remaining > 0,
            "insert of id {} with zero quantity",
            order.id
        );
        let new_slot = Slot {
            id: order.id,
            price: order.price,
            remaining: order.remaining,
            account: order.account,
            prev: NONE,
            next: NONE,
            side: resting_side,
        };
        let Some(i) = alloc(&mut self.slots, &mut self.free_head, new_slot) else {
            panic!("slab index space exhausted: at most u32::MAX - 1 live orders");
        };
        // The map's own insert detects a live duplicate: no extra lookup.
        let previous = self.index.insert(order.id, i);
        assert!(previous.is_none(), "insert of already-live id {}", order.id);
        let levels = match resting_side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        push_back(levels, &mut self.slots, i, order.price);
    }

    fn remove(&mut self, id: OrderId) -> Option<Qty> {
        let i = self.index.remove(&id)?;
        let s = *slot_mut(&mut self.slots, i);
        let levels = match s.side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        unlink(levels, &mut self.slots, i);
        release(&mut self.slots, &mut self.free_head, i);
        Some(s.remaining)
    }

    fn get_remaining(&self, id: OrderId) -> Option<Qty> {
        let &i = self.index.get(&id)?;
        Some(slot(&self.slots, i).remaining)
    }

    fn set_qty(&mut self, id: OrderId, new_qty: Qty) {
        let Some(&i) = self.index.get(&id) else {
            panic!("set_qty on non-live id {id}");
        };
        let s = slot_mut(&mut self.slots, i);
        assert!(
            0 < new_qty && new_qty <= s.remaining,
            "set_qty: {new_qty} not in 1..={}",
            s.remaining
        );
        s.remaining = new_qty;
    }

    fn move_to_back(&mut self, id: OrderId, new_qty: Qty) {
        assert!(new_qty > 0, "move_to_back of id {id} with zero quantity");
        let Some(&i) = self.index.get(&id) else {
            panic!("move_to_back on non-live id {id}");
        };
        let s = slot_mut(&mut self.slots, i);
        s.remaining = new_qty;
        if s.next == NONE {
            return; // already last in its level
        }
        let (side, price) = (s.side, s.price);
        let levels = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        unlink(levels, &mut self.slots, i);
        push_back(levels, &mut self.slots, i, price);
    }

    fn order_count(&self) -> usize {
        self.index.len()
    }

    fn level_count(&self) -> usize {
        self.bids.len() + self.asks.len()
    }

    #[cfg(test)]
    fn check_invariants(&self) {
        let n = self.slots.len();
        let mut walked = 0usize;
        for side in [Side::Buy, Side::Sell] {
            for (&price, level) in self.levels(side) {
                assert_ne!(level.head, NONE, "empty {side:?} level at {price}");
                let mut prev = NONE;
                let mut cur = level.head;
                let mut steps = 0usize;
                while cur != NONE {
                    steps += 1;
                    assert!(steps <= n, "cycle in {side:?} level {price}");
                    let s = &self.slots[cur as usize];
                    assert_eq!(s.prev, prev, "prev/next asymmetric at slot {cur}");
                    assert_eq!(s.price, price, "slot {cur} at wrong price");
                    assert_eq!(s.side, side, "slot {cur} on wrong side");
                    assert!(s.remaining > 0, "slot {cur} in level with zero qty");
                    assert_eq!(
                        self.index.get(&s.id),
                        Some(&cur),
                        "index mismatch for id {}",
                        s.id
                    );
                    prev = cur;
                    cur = s.next;
                }
                assert_eq!(level.tail, prev, "wrong tail for {side:?} level {price}");
                walked += steps;
            }
        }
        assert_eq!(
            walked,
            self.index.len(),
            "index has orders not in any level"
        );

        let mut free = 0usize;
        let mut cur = self.free_head;
        while cur != NONE {
            free += 1;
            assert!(free <= n, "cycle in free list");
            let s = &self.slots[cur as usize];
            assert_eq!(s.remaining, 0, "free slot {cur} looks live");
            cur = s.next;
        }
        assert_eq!(walked + free, n, "live and free slots overlap or leak");

        if let (Some((bid, _)), Some((ask, _))) =
            (self.bids.last_key_value(), self.asks.first_key_value())
        {
            assert!(bid < ask, "crossed book: bid {bid} >= ask {ask}");
        }
    }

    #[cfg(test)]
    fn live_ids(&self) -> Vec<OrderId> {
        self.index.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: OrderId, price: Price, qty: Qty) -> Resting {
        Resting {
            id,
            account: 0,
            price,
            remaining: qty,
        }
    }

    /// Ids of the ask level at `price`, head to tail.
    fn level_ids(b: &SlabBook, price: Price) -> Vec<OrderId> {
        let mut ids = Vec::new();
        let mut cur = b.asks.get(&price).map_or(NONE, |l| l.head);
        while cur != NONE {
            ids.push(b.slots[cur as usize].id);
            cur = b.slots[cur as usize].next;
        }
        ids
    }

    fn book_with(ids: &[OrderId]) -> SlabBook {
        let mut b = SlabBook::new();
        for &id in ids {
            b.insert(Side::Sell, order(id, 100, 5));
        }
        b.check_invariants();
        b
    }

    #[test]
    #[should_panic(expected = "fill_front: qty 6 not in 1..=5")]
    fn fill_front_over_remaining_panics() {
        let mut b = book_with(&[1]);
        b.fill_front(Side::Sell, 6);
    }

    #[test]
    #[should_panic(expected = "fill_front on empty Sell side")]
    fn fill_front_on_empty_side_panics() {
        let mut b = SlabBook::new();
        b.fill_front(Side::Sell, 1);
    }

    #[test]
    #[should_panic(expected = "slab invariant violated: slot 99 does not exist")]
    fn unlink_of_invalid_index_panics() {
        let mut b = book_with(&[1]);
        unlink(&mut b.asks, &mut b.slots, 99);
    }

    #[test]
    #[should_panic(expected = "insert of already-live id 1")]
    fn insert_of_live_id_panics() {
        let mut b = book_with(&[1]);
        b.insert(Side::Buy, order(1, 90, 1));
    }

    #[test]
    #[should_panic(expected = "insert of id 2 with zero quantity")]
    fn insert_with_zero_qty_panics() {
        let mut b = SlabBook::new();
        b.insert(Side::Sell, order(2, 100, 0));
    }

    #[test]
    #[should_panic(expected = "set_qty on non-live id 7")]
    fn set_qty_on_non_live_id_panics() {
        book_with(&[1]).set_qty(7, 1);
    }

    #[test]
    #[should_panic(expected = "move_to_back on non-live id 7")]
    fn move_to_back_on_non_live_id_panics() {
        book_with(&[1]).move_to_back(7, 1);
    }

    #[test]
    #[should_panic(expected = "slab invariant violated: slot 99 does not exist")]
    fn front_with_corrupt_head_panics() {
        let mut b = book_with(&[1]);
        if let Some(level) = b.asks.get_mut(&100) {
            level.head = 99;
        }
        b.front(Side::Sell);
    }

    #[test]
    fn slot_is_40_bytes() {
        // 3 x u64 + 3 x u32 + Side (1 B) = 37 B, padded to 8-byte alignment.
        assert_eq!(std::mem::size_of::<Slot>(), 40);
    }

    #[test]
    fn freed_slot_is_reused() {
        let mut b = book_with(&[1]);
        assert_eq!(b.remove(1), Some(5));
        b.insert(Side::Sell, order(2, 100, 5));
        b.check_invariants();
        assert_eq!(b.slot_count(), 1);
    }

    #[test]
    fn unlink_head_middle_tail() {
        for (victim, rest) in [(1, [2, 3]), (2, [1, 3]), (3, [1, 2])] {
            let mut b = book_with(&[1, 2, 3]);
            assert_eq!(b.remove(victim), Some(5));
            b.check_invariants();
            assert_eq!(level_ids(&b, 100), rest, "removed {victim}");
        }
    }

    #[test]
    fn removing_last_order_drops_level() {
        let mut b = book_with(&[1]);
        b.remove(1);
        b.check_invariants();
        assert_eq!(b.level_count(), 0);
        assert_eq!(b.front(Side::Sell), None);
    }

    #[test]
    fn fill_front_partial_then_full_advances_head() {
        let mut b = book_with(&[1, 2]);
        b.fill_front(Side::Sell, 3);
        assert_eq!(
            b.front(Side::Sell).map(|r| (r.id, r.remaining)),
            Some((1, 2))
        );
        b.fill_front(Side::Sell, 2);
        b.check_invariants();
        assert_eq!(level_ids(&b, 100), [2]);
        b.fill_front(Side::Sell, 5);
        b.check_invariants();
        assert_eq!(b.level_count(), 0);
    }

    #[test]
    fn move_to_back_reorders_and_handles_single_and_tail() {
        let mut b = book_with(&[1]);
        b.move_to_back(1, 9);
        b.check_invariants();
        assert_eq!(level_ids(&b, 100), [1]);
        assert_eq!(b.get_remaining(1), Some(9));

        let mut b = book_with(&[1, 2, 3]);
        b.move_to_back(1, 7);
        b.check_invariants();
        assert_eq!(level_ids(&b, 100), [2, 3, 1]);
        b.move_to_back(1, 8); // already the tail
        b.check_invariants();
        assert_eq!(level_ids(&b, 100), [2, 3, 1]);
        b.move_to_back(3, 6); // middle
        b.check_invariants();
        assert_eq!(level_ids(&b, 100), [2, 1, 3]);
    }

    #[test]
    fn set_qty_keeps_position() {
        let mut b = book_with(&[1, 2]);
        b.set_qty(1, 2);
        b.check_invariants();
        assert_eq!(level_ids(&b, 100), [1, 2]);
        assert_eq!(b.get_remaining(1), Some(2));
    }
}

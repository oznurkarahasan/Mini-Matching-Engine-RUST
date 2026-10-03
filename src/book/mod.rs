//! Order storage. A `Book` holds resting orders in price-time order and knows
//! nothing about matching rules; those live in [`crate::engine::Engine`].

mod baseline;
mod slab;

pub use baseline::BaselineBook;
pub use slab::SlabBook;

use crate::types::{AccountId, OrderId, Price, Qty, Side};

/// A resting order as seen by the engine, returned by value in one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resting {
    pub id: OrderId,
    pub account: AccountId,
    pub price: Price,
    pub remaining: Qty,
}

/// Preconditions are internal invariants guaranteed by `Engine`; a violation
/// panics with a clear message (in release too) instead of being ignored.
/// Input errors never reach the book.
pub trait Book {
    /// Empty book with live-order storage pre-sized for `expected_live_orders`.
    fn with_capacity(expected_live_orders: usize) -> Self
    where
        Self: Sized;

    /// Oldest order at the best price on `resting_side`, if any.
    fn front(&self, resting_side: Side) -> Option<Resting>;

    /// Fills `qty` of the front order on `resting_side`. If its remaining
    /// quantity reaches zero, the order is removed, and its level too if empty.
    /// Precondition: side non-empty and `0 < qty <= front.remaining`.
    fn fill_front(&mut self, resting_side: Side, qty: Qty);

    /// Appends `order` at the back of its price level on `resting_side`.
    /// Precondition: `order.id` is not live and `order.remaining > 0`.
    fn insert(&mut self, resting_side: Side, order: Resting);

    /// Removes the live order `id`, dropping its level if it becomes empty.
    /// Returns its remaining quantity, or `None` if `id` is not live.
    fn remove(&mut self, id: OrderId) -> Option<Qty>;

    /// Remaining quantity of the live order `id`, or `None` if not live.
    fn get_remaining(&self, id: OrderId) -> Option<Qty>;

    /// Sets the remaining quantity of `id`, keeping its queue position.
    /// Precondition: `id` is live and `0 < new_qty <= remaining`.
    fn set_qty(&mut self, id: OrderId, new_qty: Qty);

    /// Sets the remaining quantity of `id` and moves it to the back of its level.
    /// Precondition: `id` is live and `new_qty > 0`.
    fn move_to_back(&mut self, id: OrderId, new_qty: Qty);

    /// Number of live orders (for reporting book depth).
    fn order_count(&self) -> usize;

    /// Number of non-empty price levels, both sides (for reporting book depth).
    fn level_count(&self) -> usize;

    /// Panics if the book's internal structure is inconsistent.
    #[cfg(test)]
    fn check_invariants(&self);

    /// Ids of all live orders, in no particular order.
    #[cfg(test)]
    fn live_ids(&self) -> Vec<OrderId>;
}

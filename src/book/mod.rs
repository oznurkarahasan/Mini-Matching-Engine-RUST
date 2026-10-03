//! Order storage. A `Book` holds resting orders in price-time order and knows
//! nothing about matching rules; those live in [`crate::engine::Engine`].

mod baseline;

pub use baseline::BaselineBook;

use crate::types::{AccountId, OrderId, Price, Qty, Side};

/// A resting order as seen by the engine, returned by value in one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resting {
    pub id: OrderId,
    pub account: AccountId,
    pub price: Price,
    pub remaining: Qty,
}

/// Preconditions are checked with `debug_assert!` only: zero cost and no
/// panics in release builds.
pub trait Book {
    /// Oldest order at the best price on `resting_side`, if any.
    fn front(&self, resting_side: Side) -> Option<Resting>;

    /// Fills `qty` of the front order on `resting_side`. If its remaining
    /// quantity reaches zero, the order is removed, and its level too if empty.
    /// Precondition: side non-empty and `0 < qty <= front.remaining`; if
    /// violated in release the book is left unchanged.
    fn fill_front(&mut self, resting_side: Side, qty: Qty);

    /// Appends `order` at the back of its price level on `resting_side`.
    /// Precondition: `order.id` is not live and `order.remaining > 0`.
    fn insert(&mut self, resting_side: Side, order: Resting);

    /// Panics if the book's internal structure is inconsistent.
    #[cfg(test)]
    fn check_invariants(&self);

    /// Ids of all live orders, in no particular order.
    #[cfg(test)]
    fn live_ids(&self) -> Vec<OrderId>;
}

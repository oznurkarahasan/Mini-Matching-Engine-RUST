//! Primitive domain types shared by the parser, engine and book.

pub type OrderId = u64;
/// Price in integer ticks.
pub type Price = u64;
pub type Qty = u64;
/// Interned account name, see [`crate::parse::AccountInterner`].
pub type AccountId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

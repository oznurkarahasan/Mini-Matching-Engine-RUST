pub mod command;
pub mod parse;
pub mod types;

pub use command::Command;
pub use parse::{parse_line, AccountInterner, ParseError};
pub use types::{AccountId, OrderId, Price, Qty, Side};

// Placeholder still used by benches/bench.rs; removed when the real bench lands.
pub fn sum(v: &[u64]) -> u64 {
    v.iter().sum()
}

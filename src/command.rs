use crate::types::{AccountId, OrderId, Price, Qty, Side};

/// One parsed input command. Values are not validated here (e.g. zero price);
/// that is the engine's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    New {
        id: OrderId,
        account: AccountId,
        side: Side,
        price: Price,
        qty: Qty,
    },
    Cancel {
        id: OrderId,
    },
    Modify {
        id: OrderId,
        new_qty: Qty,
    },
}

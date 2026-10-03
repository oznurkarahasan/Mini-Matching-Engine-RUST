//! Deliberately simple reference engine. Shares only `Command`/`Event` types
//! with the real engine; each rule maps to a sentence of the spec (quoted).
//! Speed does not matter: every lookup is a linear scan.

use crate::command::Command;
use crate::event::{Event, Reason};
use crate::types::{AccountId, OrderId, Price, Qty, Side};

#[derive(Clone, Copy)]
struct RefOrder {
    id: OrderId,
    account: AccountId,
    side: Side,
    price: Price,
    remaining: Qty,
    /// Arrival sequence: lower = older = higher priority within a price.
    seq: u64,
}

#[derive(Default)]
pub struct RefEngine {
    /// Live (resting) orders, in no particular order.
    orders: Vec<RefOrder>,
    /// Every accepted id, ever.
    accepted: Vec<OrderId>,
    last_trade: Option<Price>,
    next_seq: u64,
}

impl RefEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn live_orders(&self) -> Vec<(OrderId, Qty)> {
        self.orders.iter().map(|o| (o.id, o.remaining)).collect()
    }

    pub fn process(&mut self, cmd: &Command) -> Vec<Event> {
        let mut out = Vec::new();
        match *cmd {
            Command::New {
                id,
                account,
                side,
                price,
                qty,
            } => self.new_order(id, account, side, price, qty, &mut out),
            Command::Cancel { id } => self.cancel(id, &mut out),
            Command::Modify { id, new_qty } => self.modify(id, new_qty, &mut out),
        }
        out
    }

    fn find_live(&self, id: OrderId) -> Option<usize> {
        self.orders.iter().position(|o| o.id == id)
    }

    fn new_order(
        &mut self,
        id: OrderId,
        account: AccountId,
        side: Side,
        price: Price,
        qty: Qty,
        out: &mut Vec<Event>,
    ) {
        // "DUPLICATE_ID (id of any previously accepted order, even if already filled/cancelled)"
        if self.accepted.contains(&id) {
            out.push(Event::Reject {
                id,
                reason: Reason::DuplicateId,
            });
            return;
        }
        // "INVALID_VALUE (price == 0 or qty == 0)"
        if price == 0 || qty == 0 {
            out.push(Event::Reject {
                id,
                reason: Reason::InvalidValue,
            });
            return;
        }
        // "if at least one trade has occurred, reject when |price - last_trade_price| * 10 > last_trade_price"
        // Signed i128 here (the engine uses abs_diff + u128) to keep the two independent.
        if let Some(last) = self.last_trade {
            let diff = (i128::from(price) - i128::from(last)).abs();
            if diff * 10 > i128::from(last) {
                out.push(Event::Reject {
                    id,
                    reason: Reason::PriceBand,
                });
                return;
            }
        }
        // "Accepted orders emit ACK first, then any TRADE events, in match order."
        self.accepted.push(id);
        out.push(Event::Ack { id });

        let mut remaining = qty;
        while remaining > 0 {
            // "A buy matches asks priced <= its limit, a sell matches bids >= its limit,
            //  best price first, oldest first within a price level."
            let Some(i) = self.best_match(side, price) else {
                break;
            };
            let maker = self.orders[i];
            // "If the next resting order to match belongs to the same account as the incoming
            //  order, do not trade; emit CANCELED,<incoming_id>,<remaining_qty>, discard the
            //  incoming order's remainder and stop matching."
            if maker.account == account {
                out.push(Event::Canceled { id, remaining });
                return;
            }
            // "Trades execute at the resting (maker) order's price."
            let qty = remaining.min(maker.remaining);
            out.push(Event::Trade {
                taker: id,
                maker: maker.id,
                price: maker.price,
                qty,
            });
            self.last_trade = Some(maker.price);
            remaining -= qty;
            self.orders[i].remaining -= qty;
            if self.orders[i].remaining == 0 {
                self.orders.remove(i);
            }
        }
        // "Unfilled remainder rests in the book."
        if remaining > 0 {
            let seq = self.take_seq();
            self.orders.push(RefOrder {
                id,
                account,
                side,
                price,
                remaining,
                seq,
            });
        }
    }

    /// Index of the best resting order on the other side that `side`/`limit` can trade with.
    fn best_match(&self, side: Side, limit: Price) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, o) in self.orders.iter().enumerate() {
            let crosses = match side {
                Side::Buy => o.side == Side::Sell && o.price <= limit,
                Side::Sell => o.side == Side::Buy && o.price >= limit,
            };
            if !crosses {
                continue;
            }
            let better = match best {
                None => true,
                Some(b) => {
                    let b = &self.orders[b];
                    let better_price = match side {
                        Side::Buy => o.price < b.price,
                        Side::Sell => o.price > b.price,
                    };
                    better_price || (o.price == b.price && o.seq < b.seq)
                }
            };
            if better {
                best = Some(i);
            }
        }
        best
    }

    fn take_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    fn cancel(&mut self, id: OrderId, out: &mut Vec<Event>) {
        // "if the order is live: remove it, emit CANCELED,<id>,<remaining_qty>.
        //  Otherwise CANCEL_REJECT,<id>,UNKNOWN_ORDER."
        match self.find_live(id) {
            Some(i) => {
                let o = self.orders.remove(i);
                out.push(Event::Canceled {
                    id,
                    remaining: o.remaining,
                });
            }
            None => out.push(Event::CancelReject {
                id,
                reason: Reason::UnknownOrder,
            }),
        }
    }

    fn modify(&mut self, id: OrderId, new_qty: Qty, out: &mut Vec<Event>) {
        // "Unknown/non-live id -> MODIFY_REJECT,<id>,UNKNOWN_ORDER; new_qty == 0 ->
        //  MODIFY_REJECT,<id>,INVALID_VALUE (check unknown first)."
        let Some(i) = self.find_live(id) else {
            out.push(Event::ModifyReject {
                id,
                reason: Reason::UnknownOrder,
            });
            return;
        };
        if new_qty == 0 {
            out.push(Event::ModifyReject {
                id,
                reason: Reason::InvalidValue,
            });
            return;
        }
        // "if new_qty is <= current remaining, the order keeps queue priority; if it is
        //  greater, it loses priority (moves to the back of its price level)."
        if new_qty > self.orders[i].remaining {
            self.orders[i].seq = self.take_seq();
        }
        self.orders[i].remaining = new_qty;
        // "A modify never triggers matching."
        out.push(Event::Modified { id, new_qty });
    }
}

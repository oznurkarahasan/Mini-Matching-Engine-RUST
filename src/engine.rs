//! Order validation, matching and self-trade prevention. The book is storage
//! only; all rules live here so every `Book` implementation behaves the same.

use std::collections::HashSet;

use crate::book::{BaselineBook, Book, Resting};
use crate::command::Command;
use crate::event::{Event, Reason};
use crate::types::{AccountId, OrderId, Price, Qty, Side};

#[derive(Debug)]
pub struct Engine<B: Book> {
    book: B,
    /// Every id ever accepted (ACKed), including filled and canceled orders.
    /// Grows without bound; see README limitations.
    accepted: HashSet<OrderId>,
    /// Price of the most recent trade; `None` until the first trade.
    last_trade: Option<Price>,
}

impl Engine<BaselineBook> {
    pub fn new() -> Self {
        Self::with_book(BaselineBook::new())
    }
}

impl Default for Engine<BaselineBook> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Book> Engine<B> {
    pub fn with_book(book: B) -> Self {
        Self {
            book,
            accepted: HashSet::new(),
            last_trade: None,
        }
    }

    /// Processes one command and appends the resulting events to `out`.
    /// The caller is responsible for clearing `out`.
    pub fn process(&mut self, cmd: &Command, out: &mut Vec<Event>) {
        match *cmd {
            Command::New {
                id,
                account,
                side,
                price,
                qty,
            } => self.process_new(id, account, side, price, qty, out),
            // CANCEL and MODIFY are implemented in steps 6 and 7.
            Command::Cancel { .. } | Command::Modify { .. } => {}
        }
    }

    fn process_new(
        &mut self,
        id: OrderId,
        account: AccountId,
        side: Side,
        price: Price,
        qty: Qty,
        out: &mut Vec<Event>,
    ) {
        let reject = if self.accepted.contains(&id) {
            Some(Reason::DuplicateId)
        } else if price == 0 || qty == 0 {
            Some(Reason::InvalidValue)
        } else if self
            .last_trade
            .is_some_and(|last| outside_band(price, last))
        {
            Some(Reason::PriceBand)
        } else {
            None
        };
        if let Some(reason) = reject {
            // A rejected id is not consumed: it never enters `accepted`.
            out.push(Event::Reject { id, reason });
            return;
        }
        self.accepted.insert(id);
        out.push(Event::Ack { id });

        let resting_side = side.opposite();
        let mut remaining = qty;
        while remaining > 0 {
            let Some(maker) = self.book.front(resting_side) else {
                break;
            };
            if !crosses(side, price, maker.price) {
                break;
            }
            // Self-trade prevention is added in step 5.
            let fill = remaining.min(maker.remaining);
            out.push(Event::Trade {
                taker: id,
                maker: maker.id,
                price: maker.price,
                qty: fill,
            });
            self.last_trade = Some(maker.price);
            remaining -= fill;
            if fill == maker.remaining {
                self.book.pop_front(resting_side);
            } else {
                self.book.reduce_front(resting_side, fill);
            }
        }

        if remaining > 0 {
            self.book.insert(
                side,
                Resting {
                    id,
                    account,
                    price,
                    remaining,
                },
            );
        }
    }

    /// Book invariants plus: every live order was accepted.
    #[cfg(test)]
    fn check_invariants(&self) {
        self.book.check_invariants();
        for id in self.book.live_ids() {
            assert!(self.accepted.contains(&id), "live id {id} not accepted");
        }
    }
}

/// True if an incoming order on `taker_side` with limit `limit` can trade
/// against a resting order at `maker_price`.
fn crosses(taker_side: Side, limit: Price, maker_price: Price) -> bool {
    match taker_side {
        Side::Buy => maker_price <= limit,
        Side::Sell => maker_price >= limit,
    }
}

/// True if `price` is more than 10% away from `last`: `|price - last| * 10 > last`.
/// Computed in u128 so the multiplication cannot overflow for any u64 inputs.
fn outside_band(price: Price, last: Price) -> bool {
    u128::from(price.abs_diff(last)) * 10 > u128::from(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each order gets its own account (derived from the id), so STP never fires
    /// in these tests unless an account is given explicitly.
    fn order(id: OrderId, side: Side, price: Price, qty: Qty) -> Command {
        Command::New {
            id,
            account: id as AccountId,
            side,
            price,
            qty,
        }
    }

    fn buy(id: OrderId, price: Price, qty: Qty) -> Command {
        order(id, Side::Buy, price, qty)
    }

    fn sell(id: OrderId, price: Price, qty: Qty) -> Command {
        order(id, Side::Sell, price, qty)
    }

    /// Runs `cmds`, checking all invariants after every command.
    fn run(engine: &mut Engine<BaselineBook>, cmds: &[Command]) -> Vec<Event> {
        let mut out = Vec::new();
        for c in cmds {
            engine.process(c, &mut out);
            engine.check_invariants();
        }
        out
    }

    fn ack(id: OrderId) -> Event {
        Event::Ack { id }
    }

    fn reject(id: OrderId, reason: Reason) -> Event {
        Event::Reject { id, reason }
    }

    fn trade(taker: OrderId, maker: OrderId, price: Price, qty: Qty) -> Event {
        Event::Trade {
            taker,
            maker,
            price,
            qty,
        }
    }

    fn resting(id: OrderId, price: Price, remaining: Qty) -> Option<Resting> {
        Some(Resting {
            id,
            account: id as AccountId,
            price,
            remaining,
        })
    }

    /// Engine whose last trade price is `last`, with an empty book.
    fn engine_with_last_trade(last: Price) -> Engine<BaselineBook> {
        let mut e = Engine::new();
        run(&mut e, &[sell(900, last, 1), buy(901, last, 1)]);
        assert_eq!(e.last_trade, Some(last));
        assert_eq!(e.book.front(Side::Buy), None);
        assert_eq!(e.book.front(Side::Sell), None);
        e
    }

    // ---- NEW checks ----

    #[test]
    fn valid_new_is_acked_and_rests() {
        let mut e = Engine::new();
        assert_eq!(run(&mut e, &[buy(1, 100, 10)]), [ack(1)]);
        assert_eq!(e.book.front(Side::Buy), resting(1, 100, 10));
    }

    #[test]
    fn zero_price_or_qty_is_invalid_and_does_not_rest() {
        let mut e = Engine::new();
        let out = run(&mut e, &[buy(1, 0, 10), buy(2, 100, 0)]);
        assert_eq!(
            out,
            [
                reject(1, Reason::InvalidValue),
                reject(2, Reason::InvalidValue)
            ]
        );
        assert_eq!(e.book.front(Side::Buy), None);
    }

    #[test]
    fn accepted_id_is_duplicate() {
        let out = run(&mut Engine::new(), &[buy(1, 100, 10), buy(1, 101, 5)]);
        assert_eq!(out, [ack(1), reject(1, Reason::DuplicateId)]);
    }

    #[test]
    fn filled_id_is_still_duplicate() {
        let out = run(
            &mut Engine::new(),
            &[sell(1, 100, 5), buy(2, 100, 5), buy(1, 100, 5)],
        );
        assert_eq!(out.last(), Some(&reject(1, Reason::DuplicateId)));
    }

    #[test]
    fn duplicate_is_checked_before_invalid_value() {
        let out = run(&mut Engine::new(), &[buy(1, 100, 10), buy(1, 0, 0)]);
        assert_eq!(out[1], reject(1, Reason::DuplicateId));
    }

    #[test]
    fn rejected_id_is_reusable() {
        let out = run(&mut Engine::new(), &[buy(8, 0, 5), buy(8, 100, 5)]);
        assert_eq!(out, [reject(8, Reason::InvalidValue), ack(8)]);
    }

    #[test]
    fn process_appends_without_clearing() {
        let mut out = vec![ack(99)];
        Engine::new().process(&buy(1, 100, 10), &mut out);
        assert_eq!(out, [ack(99), ack(1)]);
    }

    // ---- Matching ----

    #[test]
    fn non_crossing_orders_rest() {
        let mut e = Engine::new();
        let out = run(&mut e, &[buy(1, 99, 5), sell(2, 100, 5)]);
        assert_eq!(out, [ack(1), ack(2)]);
        assert_eq!(e.book.front(Side::Buy), resting(1, 99, 5));
        assert_eq!(e.book.front(Side::Sell), resting(2, 100, 5));
    }

    #[test]
    fn exact_fill_empties_both_sides() {
        let mut e = Engine::new();
        let out = run(&mut e, &[sell(1, 100, 5), buy(2, 100, 5)]);
        assert_eq!(out, [ack(1), ack(2), trade(2, 1, 100, 5)]);
        assert_eq!(e.book.front(Side::Buy), None);
        assert_eq!(e.book.front(Side::Sell), None);
    }

    #[test]
    fn taker_remainder_rests_at_its_limit() {
        let mut e = Engine::new();
        let out = run(&mut e, &[sell(1, 100, 5), buy(2, 101, 8)]);
        assert_eq!(out, [ack(1), ack(2), trade(2, 1, 100, 5)]);
        assert_eq!(e.book.front(Side::Buy), resting(2, 101, 3));
        assert_eq!(e.book.front(Side::Sell), None);
    }

    #[test]
    fn maker_partially_filled_keeps_its_place() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[sell(1, 100, 10), sell(2, 100, 10), buy(3, 100, 4)],
        );
        assert_eq!(out.last(), Some(&trade(3, 1, 100, 4)));
        assert_eq!(e.book.front(Side::Sell), resting(1, 100, 6));
    }

    #[test]
    fn trade_executes_at_maker_price() {
        let out = run(&mut Engine::new(), &[sell(1, 100, 5), buy(2, 105, 5)]);
        assert_eq!(out.last(), Some(&trade(2, 1, 100, 5)));
        let out = run(&mut Engine::new(), &[buy(1, 100, 5), sell(2, 95, 5)]);
        assert_eq!(out.last(), Some(&trade(2, 1, 100, 5)));
    }

    #[test]
    fn fifo_within_a_level() {
        let out = run(
            &mut Engine::new(),
            &[
                sell(1, 100, 2),
                sell(2, 100, 2),
                sell(3, 100, 2),
                buy(4, 100, 5),
            ],
        );
        assert_eq!(
            out[4..],
            [
                trade(4, 1, 100, 2),
                trade(4, 2, 100, 2),
                trade(4, 3, 100, 1)
            ]
        );
    }

    #[test]
    fn buy_sweeps_asks_best_price_first_and_stops_at_limit() {
        let mut e = Engine::new();
        // Inserted out of price order on purpose.
        let out = run(
            &mut e,
            &[
                sell(1, 102, 2),
                sell(2, 100, 2),
                sell(3, 101, 2),
                buy(4, 101, 5),
            ],
        );
        assert_eq!(out[3..], [ack(4), trade(4, 2, 100, 2), trade(4, 3, 101, 2)]);
        assert_eq!(e.book.front(Side::Buy), resting(4, 101, 1));
        assert_eq!(e.book.front(Side::Sell), resting(1, 102, 2));
        assert_eq!(e.last_trade, Some(101));
    }

    #[test]
    fn sell_sweeps_bids_best_price_first_and_stops_at_limit() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[buy(1, 98, 2), buy(2, 100, 2), buy(3, 99, 2), sell(4, 99, 5)],
        );
        assert_eq!(out[3..], [ack(4), trade(4, 2, 100, 2), trade(4, 3, 99, 2)]);
        assert_eq!(e.book.front(Side::Sell), resting(4, 99, 1));
        assert_eq!(e.book.front(Side::Buy), resting(1, 98, 2));
        assert_eq!(e.last_trade, Some(99));
    }

    #[test]
    fn sweep_spanning_several_orders_and_levels() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[
                buy(1, 100, 3),
                buy(2, 100, 3),
                buy(3, 99, 3),
                sell(4, 99, 8),
            ],
        );
        assert_eq!(
            out[4..],
            [trade(4, 1, 100, 3), trade(4, 2, 100, 3), trade(4, 3, 99, 2)]
        );
        assert_eq!(e.book.front(Side::Buy), resting(3, 99, 1));
        assert_eq!(e.book.front(Side::Sell), None);
    }

    // ---- Price band ----

    fn band_case(last: Price, accepted: &[Price], rejected: &[Price]) {
        let mut e = engine_with_last_trade(last);
        // All buys on an empty ask side: nothing trades, so last_trade stays fixed.
        let mut id = 1;
        for &p in accepted {
            assert_eq!(run(&mut e, &[buy(id, p, 1)]), [ack(id)], "price {p}");
            id += 1;
        }
        for &p in rejected {
            assert_eq!(
                run(&mut e, &[buy(id, p, 1)]),
                [reject(id, Reason::PriceBand)],
                "price {p}"
            );
            id += 1;
        }
        assert_eq!(e.last_trade, Some(last));
    }

    #[test]
    fn band_last_100_boundaries() {
        band_case(100, &[100, 110, 90], &[111, 89]);
    }

    #[test]
    fn band_last_not_multiple_of_10() {
        band_case(105, &[115, 95], &[116, 94]);
    }

    #[test]
    fn band_last_u64_max_no_overflow() {
        let max = u64::MAX;
        let edge = max - max / 10;
        band_case(max, &[max, edge], &[edge - 1]);
    }

    #[test]
    fn band_far_price_does_not_overflow() {
        band_case(100, &[], &[u64::MAX, 1]);
    }

    #[test]
    fn band_rejected_order_does_not_rest_or_trade() {
        let mut e = engine_with_last_trade(100);
        let out = run(&mut e, &[sell(1, 100, 5), buy(2, 111, 5)]);
        assert_eq!(out, [ack(1), reject(2, Reason::PriceBand)]);
        assert_eq!(e.book.front(Side::Sell), resting(1, 100, 5));
        assert_eq!(e.book.front(Side::Buy), None);
    }

    #[test]
    fn band_uses_the_latest_trade_price() {
        // last = 100. Buy at 110 is in band and sweeps 100 then 110; the next
        // order is checked against the new last trade price, 110.
        let mut e = engine_with_last_trade(100);
        let out = run(
            &mut e,
            &[
                sell(1, 100, 1),
                sell(2, 110, 1),
                buy(3, 110, 2),
                buy(4, 121, 1),
            ],
        );
        assert_eq!(out[3..5], [trade(3, 1, 100, 1), trade(3, 2, 110, 1)]);
        assert_eq!(e.last_trade, Some(110));
        assert_eq!(out[5], ack(4));
    }

    #[test]
    fn no_price_band_before_first_trade() {
        let out = run(&mut Engine::new(), &[buy(1, 1, 1), buy(2, u64::MAX, 1)]);
        assert_eq!(out, [ack(1), ack(2)]);
    }
}

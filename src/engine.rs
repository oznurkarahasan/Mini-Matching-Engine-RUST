//! Order validation, matching and self-trade prevention. The book is storage
//! only; all rules live here so every `Book` implementation behaves the same.

use std::collections::HashSet;

use crate::hash::IdBuildHasher;

use crate::book::{BaselineBook, Book, Resting};
use crate::command::Command;
use crate::event::{Event, Reason};
use crate::types::{AccountId, OrderId, Price, Qty, Side};

#[derive(Debug)]
pub struct Engine<B: Book> {
    book: B,
    /// Every id ever accepted (ACKed), including filled and canceled orders.
    /// Grows without bound; see README limitations.
    accepted: HashSet<OrderId, IdBuildHasher>,
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
            accepted: HashSet::default(),
            last_trade: None,
        }
    }

    /// Read-only access to the book (used for reporting depth in benchmarks).
    pub fn book(&self) -> &B {
        &self.book
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
            Command::Cancel { id } => out.push(match self.book.remove(id) {
                Some(remaining) => Event::Canceled { id, remaining },
                None => Event::CancelReject {
                    id,
                    reason: Reason::UnknownOrder,
                },
            }),
            Command::Modify { id, new_qty } => self.process_modify(id, new_qty, out),
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
        // Insert first so the accepted path hashes once; `insert` returning
        // false is exactly the DUPLICATE_ID check.
        let reject = if !self.accepted.insert(id) {
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
            // A rejected id is not consumed: undo the insert (not for a
            // duplicate, whose id belongs to the earlier accepted order).
            if reason != Reason::DuplicateId {
                self.accepted.remove(&id);
            }
            out.push(Event::Reject { id, reason });
            return;
        }
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
            // Self-trade prevention, cancel-newest: drop the incoming remainder
            // and stop. Earlier trades stay; last_trade and the book are untouched.
            if maker.account == account {
                out.push(Event::Canceled { id, remaining });
                remaining = 0;
                break;
            }
            let fill = remaining.min(maker.remaining);
            out.push(Event::Trade {
                taker: id,
                maker: maker.id,
                price: maker.price,
                qty: fill,
            });
            self.last_trade = Some(maker.price);
            remaining -= fill;
            self.book.fill_front(resting_side, fill);
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

    /// `new_qty` is the new remaining quantity. Not larger than the current
    /// remaining keeps queue priority; larger loses it. Never triggers matching.
    fn process_modify(&mut self, id: OrderId, new_qty: Qty, out: &mut Vec<Event>) {
        let Some(remaining) = self.book.get_remaining(id) else {
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
        if new_qty <= remaining {
            self.book.set_qty(id, new_qty);
        } else {
            self.book.move_to_back(id, new_qty);
        }
        out.push(Event::Modified { id, new_qty });
    }

    /// Book invariants plus: every live order was accepted.
    #[cfg(test)]
    pub(crate) fn check_invariants(&self) {
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

    /// Shared account for STP tests; far above any test order id, so it never
    /// collides with the id-derived default accounts.
    const SHARED: AccountId = 1_000_000;

    fn buy_as(id: OrderId, account: AccountId, price: Price, qty: Qty) -> Command {
        Command::New {
            id,
            account,
            side: Side::Buy,
            price,
            qty,
        }
    }

    fn sell_as(id: OrderId, account: AccountId, price: Price, qty: Qty) -> Command {
        Command::New {
            id,
            account,
            side: Side::Sell,
            price,
            qty,
        }
    }

    /// Compares the full event list; on mismatch prints both as output lines.
    #[track_caller]
    fn assert_events(out: &[Event], expected: &[Event]) {
        fn lines(events: &[Event]) -> String {
            let mut buf = Vec::new();
            for e in events {
                crate::event::write_event(&mut buf, e).expect("writing to a Vec cannot fail");
            }
            String::from_utf8_lossy(&buf).into_owned()
        }
        assert!(
            out == expected,
            "events differ\n--- got ---\n{}--- expected ---\n{}",
            lines(out),
            lines(expected)
        );
    }

    fn canceled(id: OrderId, remaining: Qty) -> Event {
        Event::Canceled { id, remaining }
    }

    /// (id, remaining) of the front order on `side`, ignoring account and price.
    fn front_of(e: &Engine<BaselineBook>, side: Side) -> Option<(OrderId, Qty)> {
        e.book.front(side).map(|r| (r.id, r.remaining))
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

    // ---- Self-trade prevention ----

    #[test]
    fn stp_on_first_match_cancels_whole_order() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[sell_as(1, SHARED, 100, 5), buy_as(2, SHARED, 100, 3)],
        );
        assert_events(&out, &[ack(1), ack(2), canceled(2, 3)]);
        assert_eq!(front_of(&e, Side::Sell), Some((1, 5)));
        assert_eq!(front_of(&e, Side::Buy), None);
        assert_eq!(e.last_trade, None);
    }

    #[test]
    fn stp_on_sell_side() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[buy_as(1, SHARED, 100, 5), sell_as(2, SHARED, 99, 4)],
        );
        assert_events(&out, &[ack(1), ack(2), canceled(2, 4)]);
        assert_eq!(front_of(&e, Side::Buy), Some((1, 5)));
        assert_eq!(front_of(&e, Side::Sell), None);
    }

    #[test]
    fn stp_mid_sweep_keeps_earlier_trades_and_last_trade() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[
                sell(1, 100, 2),
                sell_as(2, SHARED, 101, 5),
                sell(3, 101, 5),
                buy_as(4, SHARED, 102, 10),
            ],
        );
        assert_events(
            &out,
            &[
                ack(1),
                ack(2),
                ack(3),
                ack(4),
                trade(4, 1, 100, 2),
                canceled(4, 8),
            ],
        );
        // The own order and the one behind it are untouched; nothing rests for 4.
        assert_eq!(front_of(&e, Side::Sell), Some((2, 5)));
        assert_eq!(front_of(&e, Side::Buy), None);
        assert_eq!(e.last_trade, Some(100));
    }

    #[test]
    fn stp_stops_even_if_other_accounts_wait_behind_own_order() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[
                sell_as(1, SHARED, 100, 5),
                sell(2, 100, 5),
                buy_as(3, SHARED, 100, 5),
            ],
        );
        assert_events(&out, &[ack(1), ack(2), ack(3), canceled(3, 5)]);
        assert_eq!(front_of(&e, Side::Sell), Some((1, 5)));
    }

    #[test]
    fn non_crossing_own_order_does_not_trigger_stp() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[sell_as(1, SHARED, 105, 5), buy_as(2, SHARED, 100, 5)],
        );
        assert_events(&out, &[ack(1), ack(2)]);
        assert_eq!(front_of(&e, Side::Buy), Some((2, 5)));
        assert_eq!(front_of(&e, Side::Sell), Some((1, 5)));
    }

    #[test]
    fn fully_filled_before_own_order_emits_no_cancel() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[
                sell(1, 100, 5),
                sell_as(2, SHARED, 100, 5),
                buy_as(3, SHARED, 100, 5),
            ],
        );
        assert_events(&out, &[ack(1), ack(2), ack(3), trade(3, 1, 100, 5)]);
        assert_eq!(front_of(&e, Side::Sell), Some((2, 5)));
        assert_eq!(front_of(&e, Side::Buy), None);
    }

    #[test]
    fn stp_canceled_id_is_consumed() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[
                sell_as(1, SHARED, 100, 5),
                buy_as(2, SHARED, 100, 3),
                buy(2, 90, 1),
            ],
        );
        assert_events(
            &out,
            &[
                ack(1),
                ack(2),
                canceled(2, 3),
                reject(2, Reason::DuplicateId),
            ],
        );
    }

    #[test]
    fn stp_does_not_change_existing_last_trade() {
        let mut e = engine_with_last_trade(100);
        run(
            &mut e,
            &[sell_as(1, SHARED, 105, 5), buy_as(2, SHARED, 105, 5)],
        );
        assert_eq!(e.last_trade, Some(100));
    }

    // ---- CANCEL ----

    fn cancel(id: OrderId) -> Command {
        Command::Cancel { id }
    }

    fn cancel_unknown(id: OrderId) -> Event {
        Event::CancelReject {
            id,
            reason: Reason::UnknownOrder,
        }
    }

    #[test]
    fn cancel_live_order() {
        let mut e = Engine::new();
        let out = run(&mut e, &[buy(1, 100, 10), cancel(1)]);
        assert_events(&out, &[ack(1), canceled(1, 10)]);
        assert_eq!(front_of(&e, Side::Buy), None);
    }

    #[test]
    fn cancel_reports_remaining_after_partial_fill() {
        let out = run(
            &mut Engine::new(),
            &[sell(1, 100, 10), buy(2, 100, 4), cancel(1)],
        );
        assert_events(&out, &[ack(1), ack(2), trade(2, 1, 100, 4), canceled(1, 6)]);
    }

    #[test]
    fn cancel_unknown_id() {
        let out = run(&mut Engine::new(), &[cancel(42)]);
        assert_events(&out, &[cancel_unknown(42)]);
    }

    #[test]
    fn cancel_filled_maker_and_filled_taker() {
        let out = run(
            &mut Engine::new(),
            &[sell(1, 100, 5), buy(2, 100, 5), cancel(1), cancel(2)],
        );
        assert_events(
            &out,
            &[
                ack(1),
                ack(2),
                trade(2, 1, 100, 5),
                cancel_unknown(1),
                cancel_unknown(2),
            ],
        );
    }

    #[test]
    fn cancel_twice() {
        let out = run(&mut Engine::new(), &[buy(1, 100, 5), cancel(1), cancel(1)]);
        assert_events(&out, &[ack(1), canceled(1, 5), cancel_unknown(1)]);
    }

    #[test]
    fn cancel_stp_canceled_order() {
        let out = run(
            &mut Engine::new(),
            &[
                sell_as(1, SHARED, 100, 5),
                buy_as(2, SHARED, 100, 3),
                cancel(2),
            ],
        );
        assert_events(&out, &[ack(1), ack(2), canceled(2, 3), cancel_unknown(2)]);
    }

    #[test]
    fn cancel_rejected_id() {
        let out = run(&mut Engine::new(), &[buy(1, 0, 5), cancel(1)]);
        assert_events(&out, &[reject(1, Reason::InvalidValue), cancel_unknown(1)]);
    }

    #[test]
    fn rejected_duplicate_does_not_affect_live_original() {
        let out = run(
            &mut Engine::new(),
            &[buy(1, 100, 5), buy(1, 101, 3), cancel(1)],
        );
        assert_events(
            &out,
            &[ack(1), reject(1, Reason::DuplicateId), canceled(1, 5)],
        );
    }

    #[test]
    fn cancel_last_order_in_level_next_best_takes_over() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[sell(1, 100, 5), sell(2, 101, 5), cancel(1), buy(3, 101, 5)],
        );
        assert_events(
            &out,
            &[ack(1), ack(2), canceled(1, 5), ack(3), trade(3, 2, 101, 5)],
        );
        assert_eq!(front_of(&e, Side::Sell), None);
    }

    #[test]
    fn cancel_from_middle_of_level_keeps_fifo() {
        let out = run(
            &mut Engine::new(),
            &[
                sell(1, 100, 2),
                sell(2, 100, 2),
                sell(3, 100, 2),
                cancel(2),
                buy(4, 100, 4),
            ],
        );
        assert_events(
            &out,
            &[
                ack(1),
                ack(2),
                ack(3),
                canceled(2, 2),
                ack(4),
                trade(4, 1, 100, 2),
                trade(4, 3, 100, 2),
            ],
        );
    }

    // ---- MODIFY ----

    fn modify(id: OrderId, new_qty: Qty) -> Command {
        Command::Modify { id, new_qty }
    }

    fn modified(id: OrderId, new_qty: Qty) -> Event {
        Event::Modified { id, new_qty }
    }

    fn modify_reject(id: OrderId, reason: Reason) -> Event {
        Event::ModifyReject { id, reason }
    }

    #[test]
    fn modify_unknown_id() {
        let out = run(&mut Engine::new(), &[modify(42, 3)]);
        assert_events(&out, &[modify_reject(42, Reason::UnknownOrder)]);
    }

    #[test]
    fn modify_unknown_is_checked_before_invalid_value() {
        let out = run(&mut Engine::new(), &[modify(42, 0)]);
        assert_events(&out, &[modify_reject(42, Reason::UnknownOrder)]);
    }

    #[test]
    fn modify_to_zero_is_invalid_and_leaves_order_unchanged() {
        let mut e = Engine::new();
        let out = run(&mut e, &[sell(1, 100, 5), modify(1, 0)]);
        assert_events(&out, &[ack(1), modify_reject(1, Reason::InvalidValue)]);
        assert_eq!(front_of(&e, Side::Sell), Some((1, 5)));
    }

    #[test]
    fn modify_decrease_keeps_priority() {
        let out = run(
            &mut Engine::new(),
            &[
                sell(1, 100, 5),
                sell(2, 100, 5),
                modify(1, 3),
                buy(3, 100, 4),
            ],
        );
        assert_events(
            &out,
            &[
                ack(1),
                ack(2),
                modified(1, 3),
                ack(3),
                trade(3, 1, 100, 3),
                trade(3, 2, 100, 1),
            ],
        );
    }

    #[test]
    fn modify_equal_keeps_priority() {
        let out = run(
            &mut Engine::new(),
            &[
                sell(1, 100, 5),
                sell(2, 100, 5),
                modify(1, 5),
                buy(3, 100, 5),
            ],
        );
        assert_events(
            &out,
            &[ack(1), ack(2), modified(1, 5), ack(3), trade(3, 1, 100, 5)],
        );
    }

    #[test]
    fn modify_increase_loses_priority() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[
                sell(1, 100, 5),
                sell(2, 100, 5),
                modify(1, 6),
                buy(3, 100, 5),
            ],
        );
        assert_events(
            &out,
            &[ack(1), ack(2), modified(1, 6), ack(3), trade(3, 2, 100, 5)],
        );
        assert_eq!(front_of(&e, Side::Sell), Some((1, 6)));
    }

    #[test]
    fn modify_compares_with_remaining_not_original_qty() {
        // Order 1: original 10, remaining 4 after a fill. 8 < 10 but > 4, so it
        // is an increase and loses priority to order 3.
        let out = run(
            &mut Engine::new(),
            &[
                sell(1, 100, 10),
                buy(2, 100, 6),
                sell(3, 100, 5),
                modify(1, 8),
                buy(4, 100, 5),
            ],
        );
        assert_events(
            &out,
            &[
                ack(1),
                ack(2),
                trade(2, 1, 100, 6),
                ack(3),
                modified(1, 8),
                ack(4),
                trade(4, 3, 100, 5),
            ],
        );
    }

    #[test]
    fn modify_increase_on_only_order_in_level() {
        let mut e = Engine::new();
        let out = run(
            &mut e,
            &[
                sell(1, 100, 5),
                sell(2, 101, 1),
                modify(1, 9),
                buy(3, 100, 9),
            ],
        );
        assert_events(
            &out,
            &[ack(1), ack(2), modified(1, 9), ack(3), trade(3, 1, 100, 9)],
        );
        assert_eq!(front_of(&e, Side::Sell), Some((2, 1)));
    }

    #[test]
    fn modify_never_matches() {
        let mut e = Engine::new();
        let out = run(&mut e, &[buy(1, 99, 5), sell(2, 100, 5), modify(1, 50)]);
        assert_events(&out, &[ack(1), ack(2), modified(1, 50)]);
        assert_eq!(front_of(&e, Side::Buy), Some((1, 50)));
        assert_eq!(front_of(&e, Side::Sell), Some((2, 5)));
        assert_eq!(e.last_trade, None);
    }

    #[test]
    fn modify_filled_canceled_and_stp_canceled_orders() {
        let out = run(
            &mut Engine::new(),
            &[
                sell(1, 100, 5),
                buy(2, 100, 5),
                buy(3, 90, 5),
                cancel(3),
                sell_as(4, SHARED, 110, 5),
                buy_as(5, SHARED, 110, 5),
                modify(1, 1),
                modify(2, 1),
                modify(3, 1),
                modify(5, 1),
            ],
        );
        assert_events(
            &out,
            &[
                ack(1),
                ack(2),
                trade(2, 1, 100, 5),
                ack(3),
                canceled(3, 5),
                ack(4),
                ack(5),
                canceled(5, 5),
                modify_reject(1, Reason::UnknownOrder),
                modify_reject(2, Reason::UnknownOrder),
                modify_reject(3, Reason::UnknownOrder),
                modify_reject(5, Reason::UnknownOrder),
            ],
        );
    }
}

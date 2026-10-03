//! Order validation, matching and self-trade prevention. The book (added in
//! step 4) is storage only; all rules live here.

use std::collections::HashSet;

use crate::command::Command;
use crate::event::{Event, Reason};
use crate::types::{OrderId, Price, Qty};

#[derive(Debug, Default)]
pub struct Engine {
    /// Every id ever accepted (ACKed), including filled and canceled orders.
    /// Grows without bound; see README limitations.
    accepted: HashSet<OrderId>,
    /// Price of the most recent trade; `None` until the first trade.
    last_trade: Option<Price>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Processes one command and appends the resulting events to `out`.
    /// The caller is responsible for clearing `out`.
    pub fn process(&mut self, cmd: &Command, out: &mut Vec<Event>) {
        match *cmd {
            Command::New { id, price, qty, .. } => self.process_new(id, price, qty, out),
            // CANCEL and MODIFY need the book; implemented in steps 6 and 7.
            Command::Cancel { .. } | Command::Modify { .. } => {}
        }
    }

    fn process_new(&mut self, id: OrderId, price: Price, qty: Qty, out: &mut Vec<Event>) {
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
        // Matching and resting are added in step 4.
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
    use crate::types::Side;

    fn new(id: OrderId, price: Price, qty: Qty) -> Command {
        Command::New {
            id,
            account: 0,
            side: Side::Buy,
            price,
            qty,
        }
    }

    fn run(engine: &mut Engine, cmds: &[Command]) -> Vec<Event> {
        let mut out = Vec::new();
        for c in cmds {
            engine.process(c, &mut out);
        }
        out
    }

    fn reject(id: OrderId, reason: Reason) -> Event {
        Event::Reject { id, reason }
    }

    #[test]
    fn valid_new_is_acked() {
        let out = run(&mut Engine::new(), &[new(1, 100, 10)]);
        assert_eq!(out, [Event::Ack { id: 1 }]);
    }

    #[test]
    fn zero_price_or_qty_is_invalid() {
        let out = run(&mut Engine::new(), &[new(1, 0, 10), new(2, 100, 0)]);
        assert_eq!(
            out,
            [
                reject(1, Reason::InvalidValue),
                reject(2, Reason::InvalidValue)
            ]
        );
    }

    #[test]
    fn accepted_id_is_duplicate() {
        let out = run(&mut Engine::new(), &[new(1, 100, 10), new(1, 101, 5)]);
        assert_eq!(out, [Event::Ack { id: 1 }, reject(1, Reason::DuplicateId)]);
    }

    #[test]
    fn duplicate_is_checked_before_invalid_value() {
        let out = run(&mut Engine::new(), &[new(1, 100, 10), new(1, 0, 0)]);
        assert_eq!(out[1], reject(1, Reason::DuplicateId));
    }

    #[test]
    fn rejected_id_is_reusable() {
        let out = run(&mut Engine::new(), &[new(8, 0, 5), new(8, 100, 5)]);
        assert_eq!(out, [reject(8, Reason::InvalidValue), Event::Ack { id: 8 }]);
    }

    #[test]
    fn no_price_band_before_first_trade() {
        let out = run(&mut Engine::new(), &[new(1, 1, 1), new(2, u64::MAX, 1)]);
        assert_eq!(out, [Event::Ack { id: 1 }, Event::Ack { id: 2 }]);
    }

    #[test]
    fn process_appends_without_clearing() {
        let mut out = vec![Event::Ack { id: 99 }];
        Engine::new().process(&new(1, 100, 10), &mut out);
        assert_eq!(out, [Event::Ack { id: 99 }, Event::Ack { id: 1 }]);
    }
}

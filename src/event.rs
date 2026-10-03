//! Output events and their text format (one line each, `\n`-terminated).

use std::io::{self, Write};

use crate::types::{OrderId, Price, Qty};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    DuplicateId,
    InvalidValue,
    PriceBand,
    UnknownOrder,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::DuplicateId => "DUPLICATE_ID",
            Reason::InvalidValue => "INVALID_VALUE",
            Reason::PriceBand => "PRICE_BAND",
            Reason::UnknownOrder => "UNKNOWN_ORDER",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Ack {
        id: OrderId,
    },
    Reject {
        id: OrderId,
        reason: Reason,
    },
    Trade {
        taker: OrderId,
        maker: OrderId,
        price: Price,
        qty: Qty,
    },
    Canceled {
        id: OrderId,
        remaining: Qty,
    },
    CancelReject {
        id: OrderId,
        reason: Reason,
    },
    Modified {
        id: OrderId,
        new_qty: Qty,
    },
    ModifyReject {
        id: OrderId,
        reason: Reason,
    },
}

/// Writes one event as a text line. `writeln!` always emits `\n`, never `\r\n`.
pub fn write_event(w: &mut impl Write, e: &Event) -> io::Result<()> {
    match *e {
        Event::Ack { id } => writeln!(w, "ACK,{id}"),
        Event::Reject { id, reason } => writeln!(w, "REJECT,{id},{}", reason.as_str()),
        Event::Trade {
            taker,
            maker,
            price,
            qty,
        } => writeln!(w, "TRADE,{taker},{maker},{price},{qty}"),
        Event::Canceled { id, remaining } => writeln!(w, "CANCELED,{id},{remaining}"),
        Event::CancelReject { id, reason } => {
            writeln!(w, "CANCEL_REJECT,{id},{}", reason.as_str())
        }
        Event::Modified { id, new_qty } => writeln!(w, "MODIFIED,{id},{new_qty}"),
        Event::ModifyReject { id, reason } => {
            writeln!(w, "MODIFY_REJECT,{id},{}", reason.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(e: Event) -> String {
        let mut buf = Vec::new();
        write_event(&mut buf, &e).expect("writing to a Vec cannot fail");
        String::from_utf8(buf).expect("output is ASCII")
    }

    #[test]
    fn formats_every_event() {
        assert_eq!(line(Event::Ack { id: 1 }), "ACK,1\n");
        assert_eq!(
            line(Event::Reject {
                id: 7,
                reason: Reason::PriceBand
            }),
            "REJECT,7,PRICE_BAND\n"
        );
        assert_eq!(
            line(Event::Trade {
                taker: 4,
                maker: 1,
                price: 100,
                qty: 10
            }),
            "TRADE,4,1,100,10\n"
        );
        assert_eq!(
            line(Event::Canceled {
                id: 2,
                remaining: 3
            }),
            "CANCELED,2,3\n"
        );
        assert_eq!(
            line(Event::CancelReject {
                id: 2,
                reason: Reason::UnknownOrder
            }),
            "CANCEL_REJECT,2,UNKNOWN_ORDER\n"
        );
        assert_eq!(
            line(Event::Modified { id: 3, new_qty: 4 }),
            "MODIFIED,3,4\n"
        );
        assert_eq!(
            line(Event::ModifyReject {
                id: 5,
                reason: Reason::InvalidValue
            }),
            "MODIFY_REJECT,5,INVALID_VALUE\n"
        );
    }

    #[test]
    fn formats_every_reason() {
        assert_eq!(Reason::DuplicateId.as_str(), "DUPLICATE_ID");
        assert_eq!(Reason::InvalidValue.as_str(), "INVALID_VALUE");
        assert_eq!(Reason::PriceBand.as_str(), "PRICE_BAND");
        assert_eq!(Reason::UnknownOrder.as_str(), "UNKNOWN_ORDER");
    }

    #[test]
    fn formats_u64_max() {
        assert_eq!(
            line(Event::Ack { id: u64::MAX }),
            "ACK,18446744073709551615\n"
        );
    }
}

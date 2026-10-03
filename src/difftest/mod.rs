//! Randomized differential test: the real engine against a simple reference,
//! on many seeded random command streams. Test-only (see DECISIONS.md #10).

mod gen;
mod reference;

use std::fmt::Write as _;
use std::panic::{self, AssertUnwindSafe};

use crate::command::Command;
use crate::engine::Engine;
use crate::event::{write_event, Event, Reason};
use crate::types::{OrderId, Qty, Side};
use gen::Generator;
use reference::RefEngine;

/// How often each interesting path was hit, summed over all streams.
#[derive(Debug, Default)]
struct Coverage {
    trades: u64,
    stp_cancels: u64,
    reject_duplicate: u64,
    reject_invalid: u64,
    reject_band: u64,
    cancel_live: u64,
    cancel_unknown: u64,
    modify_keep_priority: u64,
    modify_lose_priority: u64,
    modify_unknown: u64,
    modify_invalid: u64,
}

impl Coverage {
    fn record(&mut self, cmd: &Command, live: &[(OrderId, Qty)], events: &[Event]) {
        for e in events {
            match *e {
                Event::Trade { .. } => self.trades += 1,
                Event::Canceled { .. } if matches!(cmd, Command::New { .. }) => {
                    self.stp_cancels += 1
                }
                Event::Canceled { .. } => self.cancel_live += 1,
                Event::Reject { reason, .. } => match reason {
                    Reason::DuplicateId => self.reject_duplicate += 1,
                    Reason::InvalidValue => self.reject_invalid += 1,
                    Reason::PriceBand => self.reject_band += 1,
                    Reason::UnknownOrder => {}
                },
                Event::CancelReject { .. } => self.cancel_unknown += 1,
                Event::ModifyReject { reason, .. } => match reason {
                    Reason::InvalidValue => self.modify_invalid += 1,
                    _ => self.modify_unknown += 1,
                },
                Event::Modified { id, new_qty } => {
                    let before = live.iter().find(|(l, _)| *l == id).map(|&(_, q)| q);
                    match before {
                        Some(rem) if new_qty <= rem => self.modify_keep_priority += 1,
                        Some(_) => self.modify_lose_priority += 1,
                        None => {}
                    }
                }
                Event::Ack { .. } => {}
            }
        }
    }

    fn assert_all_hit(&self) {
        let counters = [
            ("trades", self.trades),
            ("stp_cancels", self.stp_cancels),
            ("reject_duplicate", self.reject_duplicate),
            ("reject_invalid", self.reject_invalid),
            ("reject_band", self.reject_band),
            ("cancel_live", self.cancel_live),
            ("cancel_unknown", self.cancel_unknown),
            ("modify_keep_priority", self.modify_keep_priority),
            ("modify_lose_priority", self.modify_lose_priority),
            ("modify_unknown", self.modify_unknown),
            ("modify_invalid", self.modify_invalid),
        ];
        for (name, n) in counters {
            assert!(n > 0, "coverage counter {name} is zero: {self:?}");
        }
    }
}

/// Command in input-file format, so a failing stream can be replayed with the CLI.
fn command_line(cmd: &Command) -> String {
    match *cmd {
        Command::New {
            id,
            account,
            side,
            price,
            qty,
        } => {
            let s = match side {
                Side::Buy => 'B',
                Side::Sell => 'S',
            };
            format!("NEW,{id},A{account},{s},{price},{qty}")
        }
        Command::Cancel { id } => format!("CANCEL,{id}"),
        Command::Modify { id, new_qty } => format!("MODIFY,{id},{new_qty}"),
    }
}

fn event_lines(events: &[Event]) -> String {
    let mut buf = Vec::new();
    for e in events {
        write_event(&mut buf, e).expect("writing to a Vec cannot fail");
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn failure_report(seed: u64, index: usize, history: &[Command], problem: &str) -> String {
    let mut r =
        format!("differential test failed: seed {seed}, command index {index}\n{problem}\n");
    r.push_str("--- commands up to and including the failing one ---\n");
    for c in history {
        let _ = writeln!(r, "{}", command_line(c));
    }
    r
}

/// Runs one stream through both engines, comparing events and checking
/// invariants after every command. Panics with a replayable report on failure.
fn run_stream(seed: u64, len: usize, cov: &mut Coverage) {
    let mut gen = Generator::new(seed);
    let mut engine = Engine::new();
    let mut reference = RefEngine::new();
    let mut history = Vec::with_capacity(len);
    let mut got = Vec::new();

    for index in 0..len {
        let live = reference.live_orders();
        let cmd = gen.next(&live);
        history.push(cmd);

        got.clear();
        // Catch panics from invariant checks / debug asserts so the report still prints.
        let step = panic::catch_unwind(AssertUnwindSafe(|| {
            engine.process(&cmd, &mut got);
            engine.check_invariants();
        }));
        if let Err(payload) = step {
            let msg = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic>");
            panic!(
                "{}",
                failure_report(seed, index, &history, &format!("engine panicked: {msg}"))
            );
        }

        let expected = reference.process(&cmd);
        if got != expected {
            let problem = format!(
                "--- engine ---\n{}--- reference ---\n{}",
                event_lines(&got),
                event_lines(&expected)
            );
            panic!("{}", failure_report(seed, index, &history, &problem));
        }

        cov.record(&cmd, &live, &expected);
        gen.observe(&expected);
    }
}

#[test]
fn differential_many_short_streams() {
    let mut cov = Coverage::default();
    for seed in 0..2000 {
        run_stream(0xD1FF_0000 + seed, 200, &mut cov);
    }
    eprintln!("{cov:?}");
    cov.assert_all_hit();
}

#[test]
fn differential_few_long_streams() {
    let mut cov = Coverage::default();
    for seed in 0..5 {
        run_stream(0x10_0000 + seed, 10_000, &mut cov);
    }
    eprintln!("{cov:?}");
    cov.assert_all_hit();
}

/// The reference itself must reproduce the provided sample.
#[test]
fn reference_reproduces_sample() {
    use crate::parse::{parse_line, AccountInterner};

    let mut accounts = AccountInterner::new();
    let mut reference = RefEngine::new();
    let mut out = Vec::new();
    for line in include_str!("../../sample_input.txt").lines() {
        if let Ok(Some(cmd)) = parse_line(line, &mut accounts) {
            out.extend(reference.process(&cmd));
        }
    }
    assert_eq!(event_lines(&out), include_str!("../../sample_expected.txt"));
}

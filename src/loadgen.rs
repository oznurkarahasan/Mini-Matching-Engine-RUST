//! Benchmark and test support: a seeded PRNG and a synthetic load generator.
//! Not part of the engine API; the engine never depends on this module.
//!
//! Load model (see README for the rationale):
//! - Mid price random-walks (±1 tick with `MID_STEP_PCT` = 1% probability per command).
//! - NEW: 80% passive, 1-20 ticks behind the mid (mostly 1-5); 20% marketable,
//!   0-5 ticks through the mid. Qty skewed small (1-100). 50 accounts.
//!   Ids are fresh and increasing; ~0.5% deliberate rejects (duplicate id / zero qty).
//! - A shadow `Engine<BaselineBook>` processes every generated command, so the
//!   generator knows which orders are really live (it sees fills, like a real
//!   participant). The shadow is always the baseline book, so the stream is
//!   identical whichever book is benchmarked.
//! - CANCEL target: 70% one of the ~1000 most recent live orders, 10% any live
//!   order, 10% the oldest live order (stale, usually far from the mid),
//!   10% an id never used by NEW. MODIFY: 80% recent, 10% any, 10% unknown.
//! - Depth bounded by design: the cancel share scales with the real live count
//!   relative to `TARGET_LIVE` (clamped to 15%-45%); stale cancels bound levels.

use std::io::{self, Write};

use crate::book::{BaselineBook, Book};
use crate::command::Command;
use crate::engine::Engine;
use crate::event::Event;
use crate::types::{AccountId, OrderId, Price, Side};

/// SplitMix64 (Steele, Lea, Flood 2014): tiny, fast, good enough for test data.
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Value in `0..n` (0 if `n == 0`). Modulo bias is negligible for the small
    /// `n` used here.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64().checked_rem(n).unwrap_or(0)
    }

    /// Value in `lo..=hi`. Precondition: `lo <= hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi.saturating_sub(lo).saturating_add(1))
    }

    /// True with probability `p` percent.
    pub fn percent(&mut self, p: u64) -> bool {
        self.below(100) < p
    }

    pub fn pick<T: Copy>(&mut self, xs: &[T]) -> Option<T> {
        let i = self.below(xs.len() as u64) as usize;
        xs.get(i).copied()
    }
}

/// Live-order count around which the cancel share is centred.
pub const TARGET_LIVE: usize = 10_000;
const ACCOUNTS: u64 = 50;
const START_MID: Price = 10_000;
/// Ids from here up are never used by NEW.
const UNKNOWN_BASE: OrderId = 1 << 40;
/// "Recent" window for cancel/modify targets.
const RECENT: u64 = 1_000;
/// Probability (percent) that the mid moves one tick per command. This is the
/// mix knob: most fills come from resting orders becoming crossable as the mid
/// drifts, so lower volatility -> fewer fills -> live count nearer
/// `TARGET_LIVE` -> cancel share nearer 30%.
pub const MID_STEP_PCT: u64 = 1;

/// Which population a CANCEL/MODIFY target is drawn from.
#[derive(Clone, Copy)]
enum Target {
    Recent,
    Any,
    Oldest,
    Unknown,
}

pub struct LoadGen {
    rng: SplitMix64,
    mid: Price,
    next_id: OrderId,
    shadow: Engine<BaselineBook>,
    /// Events the shadow emitted for the last generated command.
    events: Vec<Event>,
    /// Ids that rested when submitted, in submission order (most recent last).
    /// Entries die lazily: an id is dropped when the shadow says it is not live.
    rested: Vec<OrderId>,
    /// Index into `rested` below which every entry is known dead.
    oldest: usize,
}

impl LoadGen {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SplitMix64::new(seed),
            mid: START_MID,
            next_id: 1,
            shadow: Engine::new(),
            events: Vec::new(),
            rested: Vec::new(),
            oldest: 0,
        }
    }

    /// (live orders, price levels) in the shadow book.
    pub fn depth(&self) -> (usize, usize) {
        let book = self.shadow.book();
        (book.order_count(), book.level_count())
    }

    /// Events the shadow engine emitted for the last command.
    pub fn last_events(&self) -> &[Event] {
        &self.events
    }

    pub fn next_command(&mut self) -> Command {
        self.walk_mid();
        let live = self.shadow.book().order_count();
        let cancel_pct = (30 * live / TARGET_LIVE).clamp(15, 45) as u64;
        let roll = self.rng.below(100);
        let cmd = if roll < cancel_pct {
            self.cancel()
        } else if roll < cancel_pct + 10 {
            self.modify()
        } else {
            self.new_order()
        };
        self.events.clear();
        self.shadow.process(&cmd, &mut self.events);
        if let Command::New { id, .. } = cmd {
            if self.is_live(id) {
                self.rested.push(id);
            }
        }
        self.maybe_compact();
        cmd
    }

    fn is_live(&self, id: OrderId) -> bool {
        self.shadow.book().get_remaining(id).is_some()
    }

    fn walk_mid(&mut self) {
        if self.rng.percent(MID_STEP_PCT) {
            self.mid = if self.rng.percent(50) {
                self.mid + 1
            } else {
                (self.mid - 1).max(1_000)
            };
        }
    }

    fn new_order(&mut self) -> Command {
        let side = if self.rng.percent(50) {
            Side::Buy
        } else {
            Side::Sell
        };
        let price = if self.rng.percent(80) {
            let offset = if self.rng.percent(70) {
                self.rng.range(1, 5)
            } else {
                self.rng.range(6, 20)
            };
            match side {
                Side::Buy => self.mid - offset,
                Side::Sell => self.mid + offset,
            }
        } else {
            let through = self.rng.range(0, 5);
            match side {
                Side::Buy => self.mid + through,
                Side::Sell => self.mid - through,
            }
        };
        let mut qty = match self.rng.below(100) {
            0..=69 => self.rng.range(1, 10),
            70..=94 => self.rng.range(11, 50),
            _ => self.rng.range(51, 100),
        };
        let account = self.rng.below(ACCOUNTS) as AccountId;

        // ~0.5% deliberate rejects: 0.3% duplicate id, 0.2% zero qty.
        let id = match self.rng.below(1000) {
            0..=2 if self.next_id > 1 => self.rng.range(1, self.next_id - 1),
            3..=4 => {
                qty = 0;
                self.fresh_id()
            }
            _ => self.fresh_id(),
        };
        Command::New {
            id,
            account,
            side,
            price,
            qty,
        }
    }

    fn fresh_id(&mut self) -> OrderId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn cancel(&mut self) -> Command {
        let target = match self.rng.below(100) {
            0..=69 => Target::Recent,
            70..=79 => Target::Any,
            80..=89 => Target::Oldest,
            _ => Target::Unknown,
        };
        let id = self.pick(target).unwrap_or_else(|| self.unknown_id());
        Command::Cancel { id }
    }

    fn modify(&mut self) -> Command {
        let target = match self.rng.below(100) {
            0..=79 => Target::Recent,
            80..=89 => Target::Any,
            _ => Target::Unknown,
        };
        let Some(id) = self.pick(target) else {
            return Command::Modify {
                id: self.unknown_id(),
                new_qty: self.rng.range(1, 10),
            };
        };
        let rem = self.shadow.book().get_remaining(id).unwrap_or(1);
        let new_qty = if self.rng.percent(50) {
            self.rng.range(1, rem)
        } else {
            rem + self.rng.range(1, 20)
        };
        Command::Modify { id, new_qty }
    }

    fn unknown_id(&mut self) -> OrderId {
        UNKNOWN_BASE + self.rng.below(1000)
    }

    /// A currently live id from the given population, or `None` (use an unknown id).
    fn pick(&mut self, target: Target) -> Option<OrderId> {
        if self.shadow.book().order_count() == 0 {
            return None;
        }
        match target {
            Target::Unknown => None,
            Target::Oldest => self.oldest_live(),
            Target::Recent | Target::Any => {
                let len = self.rested.len() as u64;
                for _ in 0..8 {
                    let i = match target {
                        Target::Recent => len - 1 - self.rng.below(RECENT.min(len)),
                        _ => self.rng.below(len),
                    } as usize;
                    let id = self.rested[i];
                    if self.is_live(id) {
                        return Some(id);
                    }
                }
                // Window mostly dead: newest live order.
                let shadow = &self.shadow;
                self.rested
                    .iter()
                    .rev()
                    .copied()
                    .find(|&id| shadow.book().get_remaining(id).is_some())
            }
        }
    }

    fn oldest_live(&mut self) -> Option<OrderId> {
        while let Some(&id) = self.rested.get(self.oldest) {
            if self.is_live(id) {
                return Some(id);
            }
            self.oldest += 1;
        }
        None
    }

    /// Drops dead entries once `rested` is more than twice the live count
    /// (amortized O(1) per command).
    fn maybe_compact(&mut self) {
        if self.rested.len() > 2 * self.shadow.book().order_count() + 1024 {
            let shadow = &self.shadow;
            self.rested
                .retain(|&id| shadow.book().get_remaining(id).is_some());
            self.oldest = 0;
        }
    }
}

/// `n` commands from a fresh generator with `seed`.
pub fn generate(seed: u64, n: usize) -> Vec<Command> {
    let mut g = LoadGen::new(seed);
    (0..n).map(|_| g.next_command()).collect()
}

/// FNV-1a over every field of every command: a fingerprint to confirm two runs
/// used the identical stream.
pub fn fingerprint(cmds: &[Command]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |x: u64| {
        for b in x.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for c in cmds {
        match *c {
            Command::New {
                id,
                account,
                side,
                price,
                qty,
            } => {
                mix(1);
                mix(id);
                mix(u64::from(account));
                mix(side as u64);
                mix(price);
                mix(qty);
            }
            Command::Cancel { id } => {
                mix(2);
                mix(id);
            }
            Command::Modify { id, new_qty } => {
                mix(3);
                mix(id);
                mix(new_qty);
            }
        }
    }
    h
}

/// Writes one command as an input line (accounts as `A<n>`).
pub fn write_command(w: &mut impl Write, cmd: &Command) -> io::Result<()> {
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
            writeln!(w, "NEW,{id},A{account},{s},{price},{qty}")
        }
        Command::Cancel { id } => writeln!(w, "CANCEL,{id}"),
        Command::Modify { id, new_qty } => writeln!(w, "MODIFY,{id},{new_qty}"),
    }
}

pub fn write_commands(w: &mut impl Write, cmds: &[Command]) -> io::Result<()> {
    for c in cmds {
        write_command(w, c)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_deterministic() {
        let a = generate(7, 10_000);
        assert_eq!(a, generate(7, 10_000));
        assert_eq!(fingerprint(&a), fingerprint(&generate(7, 10_000)));
        assert_ne!(fingerprint(&a), fingerprint(&generate(8, 10_000)));
    }

    #[test]
    fn written_commands_parse_back() {
        use crate::parse::{parse_line, AccountInterner};

        let cmds = generate(3, 2_000);
        let mut buf = Vec::new();
        write_commands(&mut buf, &cmds).expect("Vec write");
        let text = String::from_utf8(buf).expect("ASCII");
        let mut acc = AccountInterner::new();
        let parsed: Vec<Command> = text
            .lines()
            .map(|l| parse_line(l, &mut acc).expect("parses").expect("not blank"))
            .collect();
        assert_eq!(parsed.len(), cmds.len());
        // Account ids are re-interned, so compare everything except accounts.
        for (a, b) in parsed.iter().zip(&cmds) {
            match (a, b) {
                (Command::New { id: i1, .. }, Command::New { id: i2, .. }) => assert_eq!(i1, i2),
                _ => assert_eq!(a, b),
            }
        }
    }
}

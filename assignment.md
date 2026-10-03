# Mini Matching Engine — Take-Home Assignment

**Time box: 5 hours, one sitting.** Language: Rust (std + `criterion` only; no order-book or matching crates).

## 1. Background

You are building the core of a simplified exchange venue simulator that our strategies test against. It consumes a stream of order commands, maintains a limit order book for **one instrument**, and emits events. Correctness is essential; speed matters because we replay billions of messages.

## 2. Input format

Plain text, one command per line, comma separated. Empty lines and lines starting with `#` are ignored. Syntax is always well-formed; *values* may be invalid.

```
NEW,<order_id>,<account>,<B|S>,<price>,<qty>
CANCEL,<order_id>
MODIFY,<order_id>,<new_qty>
```

- `order_id`: u64, `account`: short string, `price`: u64 in integer ticks, `qty`: u64.
- All orders are **limit, good-till-cancel**.

## 3. Output events (one per line, in processing order)

```
ACK,<id>
REJECT,<id>,<reason>
TRADE,<taker_id>,<maker_id>,<price>,<qty>
CANCELED,<id>,<remaining_qty>
CANCEL_REJECT,<id>,<reason>
MODIFIED,<id>,<new_qty>
MODIFY_REJECT,<id>,<reason>
```

## 4. Rules

**NEW**
1. Checks, in this order: `DUPLICATE_ID` (id of any previously *accepted* order, even if already filled/cancelled), `INVALID_VALUE` (price == 0 or qty == 0), `PRICE_BAND`. On failure emit `REJECT,<id>,<reason>`; a rejected id is not consumed.
2. `PRICE_BAND`: if at least one trade has occurred, reject when `|price - last_trade_price| * 10 > last_trade_price` (more than 10% away). Use integer arithmetic only.
3. Accepted orders emit `ACK` first, then any `TRADE` events, in match order.
4. **Price-time priority.** A buy matches asks priced `<=` its limit, a sell matches bids `>=` its limit, best price first, oldest first within a price level. Trades execute at the **resting (maker) order's price**. Unfilled remainder rests in the book.
5. **Self-trade prevention (cancel-newest).** If the next resting order to match belongs to the same `account` as the incoming order, do not trade; emit `CANCELED,<incoming_id>,<remaining_qty>`, discard the incoming order's remainder and stop matching. Trades already executed stay.

**CANCEL** — if the order is live: remove it, emit `CANCELED,<id>,<remaining_qty>`. Otherwise `CANCEL_REJECT,<id>,UNKNOWN_ORDER`.

**MODIFY** — `new_qty` is the new *remaining* quantity. Unknown/non-live id → `MODIFY_REJECT,<id>,UNKNOWN_ORDER`; `new_qty == 0` → `MODIFY_REJECT,<id>,INVALID_VALUE` (check unknown first). Otherwise emit `MODIFIED,<id>,<new_qty>`: if `new_qty` is **≤ current remaining**, the order **keeps** queue priority; if it is greater, it **loses** priority (moves to the back of its price level). Price cannot be modified. A modify never triggers matching.

A sample input and its exact expected output are in `sample_input.txt` / `sample_expected.txt`. Your program must reproduce it byte for byte. Usage: `engine <input_file>` prints events to stdout.

## 5. Tasks

**Part 1 — Correctness (core).** Implement all rules above as a library crate plus a thin CLI. Unit tests for every rule and edge case you can think of (partial fills, multiple price levels, FIFO within a level, STP in the middle of a sweep, modify priority, duplicate ids, price band before and after the first trade, cancel of a filled order...).

**Part 2 — Verification.** Write a deliberately simple, obviously-correct reference implementation (inside the test code) and a randomized **differential test** that compares it against your real engine on many random command streams. Explain in the README what random distributions you used and why.

**Part 3 — Performance.**
1. Write a generator for a realistic load: e.g. 1M–10M commands, roughly 60% NEW / 30% CANCEL / 10% MODIFY, prices clustered around a moving mid, several dozen accounts.
2. Measure and report throughput (commands/s) and latency percentiles (p50 / p99 / p99.9) in `--release`.
3. Profile, find the top bottleneck, fix it, and report **before/after numbers**. Justify your data-structure choices (price levels, per-level queues, id lookup, cancel cost). Avoid allocations on the hot path where you can, and say where you chose not to.

**Part 4 — Write-up (`README.md`, max ~2 pages).** Design overview, invariants, complexity of each operation, known limitations, what you would do with more time, and how you would extend the engine to multiple instruments or to a multi-threaded setup. Add a short, honest note on **how you used tools/AI** and how you verified their output.

## 6. Deliverables

- Cargo project that builds with `cargo build --release` with no warnings, `cargo clippy` clean, `cargo fmt` applied.
- `cargo test` passes (including the sample test).
- Benchmark/load generator and instructions to run it.
- `README.md` as described.

## 7. What we look at

Correctness on hidden tests · quality and breadth of your tests · whether your performance claims are backed by measurements · clarity of design decisions and documentation · scope judgement (a finished, simple solution beats an unfinished clever one).

## 8. Suggested time plan (not binding)

| Time | Activity |
|---|---|
| 0:00 – 0:30 | Read carefully, list ambiguities and assumptions, choose data structures |
| 0:30 – 2:15 | Part 1 |
| 2:15 – 3:00 | Part 2 |
| 3:00 – 4:15 | Part 3 |
| 4:15 – 4:50 | Part 4 |
| 4:50 – 5:00 | Final checks (`fmt`, `clippy`, `test`, clean build) |

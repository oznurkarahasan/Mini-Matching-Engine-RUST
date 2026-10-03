# NOTES

## 1. The problem in my own words
I am building a small exchange matching engine for one instrument. It reads
commands (NEW, CANCEL, MODIFY) line by line, keeps a limit order book, and prints
an event for everything that happens (ACK, REJECT, TRADE, CANCELED, MODIFIED...).
Orders match by best price first and arrival time second, and trades happen at the
resting order's price. It will be replayed on billions of messages, so correctness
comes first and speed second.

## 2. Inputs / outputs / constraints
- Input: text file, one command per line; blank lines and `#` comments ignored;
  syntax is valid but values can be invalid (zero price or qty).
- Output: events on stdout, in processing order. Must match sample_expected.txt
  byte for byte. Usage: `engine <input_file>`.
- Constraints: std + criterion only, no matching crates, fmt/clippy clean, no warnings.
- Benchmark: none provided, so I write my own generator (1M-10M commands,
  about 60% NEW / 30% CANCEL / 10% MODIFY) and report throughput and p50/p99/p99.9.

## 3. Ambiguities and the assumption I chose
| # | Ambiguity | Assumption | Risk if wrong |
|---|---|---|---|
| 1 | `price * 10` can overflow u64 in the band check | u128 (or checked_mul, overflow = reject); exactly 10% is accepted | wrong reject for huge prices |
| 2 | Is an order canceled by self-trade prevention "accepted"? | yes: ACK first, id consumed; keep a set of all accepted ids (unbounded growth -> limitations) | duplicate-id behaviour differs |
| 3 | MODIFY with new_qty == remaining | keeps priority, still emits MODIFIED; compared to *remaining*, not original qty | missing/extra event |
| 4 | Self-trade check scope | only a resting order that would cross triggers STP; it stops all matching (even if other accounts are behind it); STP before any trade -> ACK then CANCELED with full qty | missed or extra cancels |
| 5 | Id of a rejected order | not consumed, can be reused | accepts a reused id wrongly |
| 6 | Total/sum of quantities may overflow | use checked/wide arithmetic where sums exist | silent wraparound |
| 7 | When is the price band evaluated? | once per NEW, against the last trade price before that order; rejected orders never change last_trade_price | wrong rejects during/after sweeps |
| 8 | Account comparison | raw bytes, case-sensitive; interned to u32 at parse time | wrong STP decisions |
| 9 | Line endings / comments | trim trailing `\r`; ignore blank lines and lines starting with `#` | CRLF input mis-parsed |
| 10 | Unparsable values (overflow, negative) | never panic; skip the line, count skipped lines on stderr | hidden test expects other handling |
| 11 | Order ids are arbitrary, sparse u64 | hash map lookup, not a Vec indexed by id | n/a (design constraint) |
| 12 | Numbers with a leading `+` (e.g. `+5`) | accepted, as `str::parse::<u64>` does; harmless since syntax is well-formed | a hidden test expects such a line to be skipped |

## 4. Approach candidates
- A: ordered map per side (price -> FIFO queue) plus a hash map from id to order.
  Simple and quick to get correct; allocation-heavy.
- B: preallocated order slab with index-based linked lists per price level, flat
  price-indexed levels, best-price tracking. More code; no allocation per order.
- Start with A to get correct and tested. Move toward B only if the profile shows
  allocation / pointer chasing as the bottleneck.
- Chosen baseline: per side `BTreeMap<price, VecDeque<order_id>>`, `HashMap<id, Order>`
  for live orders, separate set of accepted ids; cancel = linear search in the level.
- Planned Part 3 step: keep BTreeMap levels, move orders into a preallocated slab with
  index-based doubly linked lists per level (O(1) cancel), custom hasher instead of
  SipHash. Sorted Vec levels only if time remains. No price-indexed ladder.
- Engine pushes into a reused `Vec<Event>`; CLI formats with `BufWriter`.
- Book behind a small internal interface so baseline and optimized book are tested
  against the same reference.

## 5. Plan (total <= 5h)
- [ ] (0:00-0:30) Understand, fill sections 1-4
- [ ] (0:30-2:15) Part 1: engine, CLI, sample test, unit tests
- [ ] (2:15-3:00) Part 2: naive reference + differential test
- [ ] (3:00-4:15) Part 3: generator, baseline, profile, one optimization at a time
- [ ] (4:15-4:50) Part 4: README from NOTES/DECISIONS/BENCH
- [ ] (4:50-5:00) fmt, clippy, test, clean build

## 6. Before each commit
- [ ] cargo fmt  [ ] cargo clippy  [ ] cargo test  [ ] DECISIONS/BENCH updated

## 7. TODO
- [ ] Set up repo layout: `engine` binary, lib crate, sample files in place

Part 1 — Correctness
- [x] 1. Types, Command, parser (`parse_line`, `AccountInterner`) + tests
- [x] 2. Events + `write_event`, CLI skeleton (`run`, `main`), sample test (ignored)
- [x] 3. Engine NEW checks: DUPLICATE_ID, INVALID_VALUE, PRICE_BAND; ACK; accepted-id set
- [x] 4. `Book` trait + `BaselineBook`, matching without STP, `check_invariants` (test only); price band tests
- [x] 5. Self-trade prevention (cancel-newest) + tests
- [x] 6. CANCEL + tests
- [x] 7. MODIFY + tests; un-ignore sample test; `cargo run --release -- sample_input.txt | diff - sample_expected.txt`

Part 2 — Verification
- [x] Simple reference engine in test code
- [x] Randomized differential test (reference vs engine), distributions documented in README

Part 3 — Performance
- [x] Load generator (shadow-engine feedback, live target 10k, mix knob `MID_STEP_PCT` = 1 -> 60.8/29.3/10.0, 50 accounts, fingerprinted streams)
- [x] Bench harness: Criterion engine_only + end_to_end (1M), latency example (p50..p99.99, max, timer overhead); placeholder bench and `sum` removed
- [x] Baseline numbers -> BENCH.md row 0 (pinned, 5 runs, Criterion baseline `baseline0`)
- [x] Profile (perf / flamegraph), identify top bottleneck: SipHash 32.9% of engine_only, `accepted` rehash 10.8%, cancel linear search ~20%
- [x] Custom hasher instead of SipHash (change 1, BENCH.md row 1: engine_only -35%, end_to_end -16%)
  - Order changed vs the original plan: hasher came before the slab because the profile showed hashing as the top bottleneck.
- [ ] (in progress) EngineConfig pre-sizing (change 2): opt-in, default bench un-sized; max latency 27 ms -> ~176 µs in the latency example
- [ ] Open item: remaining ~176 µs max-latency outlier with pre-sizing (command 5,300,890). Unverified hypothesis: in-place rehash of the `orders` map due to tombstones from cancel churn.
- [ ] Slab book with index-based linked lists (O(1) cancel, no repeated lookups in front/fill_front), behind the same `Book` trait (change 3)
- [ ] (partial) Before/after numbers in BENCH.md, one change per measurement: row 1 done; slab pending; the differential test must also run on the slab book

Part 4 — Write-up
- [ ] README: design, invariants, complexity, limitations, more time, multi-instrument / multi-thread, AI use

Final checks
- [ ] `cargo fmt`, `cargo clippy --all-targets`, `cargo test`, clean `cargo build --release` with no warnings

## 8. Log
- 

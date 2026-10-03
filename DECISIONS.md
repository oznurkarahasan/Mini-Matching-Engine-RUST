# DECISIONS (2-4 lines each: what, why, alternative, result)

## 1. STP-canceled orders consume their id
- Decision: an order canceled by self-trade prevention counts as accepted; its id is consumed. All accepted ids live in a separate set.
- Why: the order was ACKed, so it was accepted; the duplicate rule covers any previously accepted order.
- Alternative considered: only track live ids (smaller memory, but violates DUPLICATE_ID for filled/canceled orders).
- Result / evidence: test TBD. Set grows without bound -> README limitations.

## 2. Price band arithmetic and timing
- Decision: `|price - last| * 10 > last` computed in u128 (or checked_mul, overflow = reject); exactly 10% is accepted. Checked once per NEW against the last trade price before that order; rejected orders never change last_trade_price.
- Why: integer-only per spec; u64 multiplication can overflow for large prices.
- Alternative considered: re-check during the sweep (not supported by the spec text).
- Result / evidence: tests TBD (±10% boundary, 10% + 1 tick, before/after first trade).

## 3. Input handling
- Decision: trim trailing `\r`; ignore blank lines and lines starting with `#`. Unparsable lines are skipped (never panic) and the count is printed on stderr. Accounts are compared as raw bytes, case-sensitive, interned to u32 at parse time.
- Why: robustness on CRLF files without changing stdout; interning makes the STP check an integer compare.
- Alternative considered: abort on a bad line; inline fixed-size account bytes.
- Result / evidence: tests TBD.

## 4. Baseline book data structures
- Decision: per side `BTreeMap<price, VecDeque<order_id>>`, `HashMap<id, Order>` for live orders, separate accepted-id set. Cancel removes by linear search in the level.
- Why: simplest to get correct quickly; serves as the measured baseline.
- Alternative considered: slab + intrusive lists (planned optimization); price-indexed ladder (rejected: unbounded u64 prices need a fallback path).
- Result / evidence: BENCH.md row 0 (TBD).

## 5. Planned optimization (Part 3)
- Decision: keep BTreeMap levels; move orders to a preallocated slab with index-based doubly linked lists per level (O(1) cancel); custom hasher instead of SipHash. Sorted Vec levels only if time remains.
- Why: cancel is 30% of the load and linear in level length in the baseline; SipHash is a known cost on every lookup.
- Alternative considered: sorted Vec levels, price ladder (not now).
- Result / evidence: before/after in BENCH.md (TBD); must be confirmed by profile first.

## 6. Event output and testable book interface
- Decision: the engine pushes events into a reused `Vec<Event>`; the CLI formats them with `BufWriter`. The book sits behind a small internal interface so both baseline and optimized versions run against the same reference test.
- Why: no allocation per command in steady state; formatting kept out of the engine; one differential test covers both books.
- Alternative considered: generic `FnMut(Event)` sink; writing text directly from the engine.
- Result / evidence: TBD.

## 7. Matching lives in Engine; Book is storage only
- Decision: matching, STP and price band are implemented once in `Engine`; the `Book` trait only stores orders. `front(side)` returns the resting order's id, account and remaining in one call.
- Why: one implementation of the tricky logic, so swapping in the slab book in Part 3 cannot change behaviour; the one-call `front` means the slab version needs no extra lookups.
- Alternative considered: trait = whole engine, each book does its own matching (more freedom, duplicated logic).
- Result / evidence: TBD (same unit + differential tests run on both books).

## 8. Sample test added early, ignored until the engine is complete
- Decision: add `tests/sample.rs` in step 2 with `#[ignore]`, remove the attribute in step 7; the test body never changes. Step 7 also checks the binary: `cargo run --release -- sample_input.txt | diff - sample_expected.txt`.
- Why: the target is visible from the start without a red test suite.
- Alternative considered: add the test only in step 7.
- Result / evidence: TBD.

## 9. One `Reason` enum for all reject events
- Decision: a single `Reason { DuplicateId, InvalidValue, PriceBand, UnknownOrder }` shared by REJECT, CANCEL_REJECT and MODIFY_REJECT.
- Why: the engine never builds wrong combinations and tests cover it; per-event reason types are not worth it in a 5h budget.
- Alternative considered: one reason type per event (wrong combinations unrepresentable).
- Result / evidence: rule tests.

<!-- Copy the block for each decision. Write it when you decide, not at the end. -->

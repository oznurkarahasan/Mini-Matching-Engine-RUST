# DECISIONS

## 1. STP-canceled orders consume their id
- Decision: an order canceled by self-trade prevention counts as accepted; its id is consumed. All accepted ids live in a separate set.
- Why: the order was ACKed, so it was accepted; the duplicate rule covers any previously accepted order.
- Alternative considered: only track live ids (smaller memory, but violates DUPLICATE_ID for filled/canceled orders).
- Result / evidence: tests `stp_canceled_id_is_consumed`, `filled_id_is_still_duplicate`, `cancel_stp_canceled_order` (both books); differential test. Set grows without bound -> README limitations.

## 2. Price band arithmetic and timing
- Decision: `|price - last| * 10 > last` computed in u128 (or checked_mul, overflow = reject); exactly 10% is accepted. Checked once per NEW against the last trade price before that order; rejected orders never change last_trade_price.
- Why: integer-only per spec; u64 multiplication can overflow for large prices.
- Alternative considered: re-check during the sweep (not supported by the spec text).
- Result / evidence: tests `band_last_100_boundaries`, `band_last_not_multiple_of_10`, `band_last_u64_max_no_overflow`, `band_far_price_does_not_overflow`, `band_rejected_order_does_not_rest_or_trade`, `band_uses_the_latest_trade_price`, `no_price_band_before_first_trade` (both books); mutation `>` -> `>=` caught by both differential tests.

## 3. Input handling
- Decision: trim trailing `\r`; ignore blank lines and lines starting with `#`. Unparsable lines are skipped (never panic) and the count is printed on stderr. Accounts are compared as raw bytes, case-sensitive, interned to u32 at parse time.
- Why: robustness on CRLF files without changing stdout; interning makes the STP check an integer compare.
- Alternative considered: abort on a bad line; inline fixed-size account bytes.
- Result / evidence: tests `blank_and_comment_lines_are_ignored`, `trailing_cr_is_stripped`, `interner_is_stable_and_case_sensitive`, `malformed_lines_are_errors`, `counts_only_unparsable_lines`, `invalid_utf8_line_is_skipped_and_processing_continues`; `tools/verify.py` (CRLF, no final newline, comments, `a` vs `A`) byte-identical on 2.0M lines (BENCH.md).

## 4. Baseline book data structures
- Decision: per side `BTreeMap<price, VecDeque<order_id>>`, `HashMap<id, Order>` for live orders, separate accepted-id set. Cancel removes by linear search in the level.
- Why: simplest to get correct quickly; serves as the measured baseline.
- Alternative considered: slab + intrusive lists (planned optimization); price-indexed ladder (rejected: unbounded u64 prices need a fallback path).
- Result / evidence: BENCH.md row 0 (4.51 M cmd/s engine_only). Kept as `BaselineBook` for comparison and as the simple book; replaced as CLI default by #14.

## 5. Planned optimization (Part 3)
- Decision: keep BTreeMap levels; move orders to a preallocated slab with index-based doubly linked lists per level (O(1) cancel); custom hasher instead of SipHash. Sorted Vec levels only if time remains.
- Why: cancel is 30% of the load and linear in level length in the baseline; SipHash is a known cost on every lookup.
- Alternative considered: sorted Vec levels, price ladder (not now).
- Result / evidence: the profile changed the order: hasher first (BENCH.md row 1, #13), then the slab (row 2, #14). Sorted `Vec` levels not done.

## 6. Event output and testable book interface
- Decision: the engine pushes events into a reused `Vec<Event>`; the CLI formats them with `BufWriter`. The book sits behind a small internal interface so both baseline and optimized versions run against the same reference test.
- Why: no allocation per command in steady state; formatting kept out of the engine; one differential test covers both books.
- Alternative considered: generic `FnMut(Event)` sink; writing text directly from the engine.
- Result / evidence: tests `formats_every_event`, `process_appends_without_clearing`; one differential test covers both books (#10, #14).

## 7. Matching lives in Engine; Book is storage only
- Decision: matching, STP and price band are implemented once in `Engine`; the `Book` trait only stores orders. `front(side)` returns the resting order's id, account and remaining in one call.
- Why: one implementation of the tricky logic, so swapping in the slab book in Part 3 cannot change behaviour; the one-call `front` means the slab version needs no extra lookups.
- Alternative considered: trait = whole engine, each book does its own matching (more freedom, duplicated logic).
- Result / evidence: all engine tests are instantiated for both books (`engine_tests!` macro) and the differential test runs both; switching books left the 5M-command CLI output byte-identical (sha256 `770b6476…`, BENCH.md).

## 8. Sample test added early, ignored until the engine is complete
- Decision: add `tests/sample.rs` in step 2 with `#[ignore]`, remove the attribute in step 7; the test body never changes. Step 7 also checks the binary: `cargo run --release -- sample_input.txt | diff - sample_expected.txt`.
- Why: the target is visible from the start without a red test suite.
- Alternative considered: add the test only in step 7.
- Result / evidence: `tests/sample.rs::sample_matches_expected_byte_for_byte` passes; release binary diff against `sample_expected.txt` is empty.

## 9. One `Reason` enum for all reject events
- Decision: a single `Reason { DuplicateId, InvalidValue, PriceBand, UnknownOrder }` shared by REJECT, CANCEL_REJECT and MODIFY_REJECT.
- Why: the engine never builds wrong combinations and tests cover it; per-event reason types are not worth it in a 5h budget.
- Alternative considered: one reason type per event (wrong combinations unrepresentable).
- Result / evidence: rule tests.

## 10. Reference engine and differential test live under src/
- Decision: the reference engine and the randomized differential test go in a `#[cfg(test)]` module under `src/`, not in `tests/`.
- Why: `check_invariants` is `#[cfg(test)]` only, which is not set for integration tests; a unit-test module can call it after every command, and release/bench builds stay free of it.
- Alternative considered: a cargo feature (e.g. `invariants`) enabled by `tests/`.
- Result / evidence: `difftest::differential_many_short_streams`, `differential_few_long_streams` (reference + both books, `check_invariants` after every command), `reference_reproduces_sample`; mutations caught: band `>`->`>=`, MODIFY `<=`->`<`, slab unlink `prev`.

## 11. `fill_front` replaces `reduce_front` + `pop_front`
- Decision: one `Book::fill_front(resting_side, qty)` subtracts with `checked_sub` and removes the order (and its level if empty) when it reaches zero; `debug_assert!(qty <= remaining)`.
- Why: the engine no longer chooses between reduce and pop, so the silent-wrap path in release disappears and the trait is smaller.
- Alternative considered: separate reduce/pop methods (previous version).
- Result / evidence: matching + STP tests.

## 12. MODIFY priority rule lives in Engine; Book has `set_qty` and `move_to_back`
- Decision: the engine decides priority (`new_qty <= remaining` keeps it, larger loses it) and calls either `Book::set_qty(id, q)` (position kept) or `Book::move_to_back(id, q)`.
- Why: the priority rule is a spec rule; consistent with #7, the book only stores. A single `modify(id, q, keep_priority)` would push the rule into every book implementation.
- Alternative considered: one `modify` method with a boolean flag.
- Result / evidence: MODIFY tests (priority shown by which maker trades next).

## 13. SplitMix64-finalizer hasher for id maps; `accepted` inserts first
- Decision: every engine/book map keyed by order id (`accepted`, `orders`) uses `IdHasher` (SplitMix64 finalizer, xor-shift-multiply). `accepted.insert(id)` is the duplicate check; on a later INVALID_VALUE / PRICE_BAND reject the id is removed again, so the accepted path hashes once.
- Why: profile showed SipHash in 32.9% of engine_only samples. A plain multiply leaves low bits (hashbrown's bucket index) poorly mixed for strided ids; the finalizer mixes all bits. No HashDoS protection is needed: this is a replay simulator on trusted input. The account interner (string keys, parse path) keeps SipHash.
- Alternative considered: plain multiplicative hash; keeping `contains` + `insert`.
- Known trade-off of insert-first: removing a rejected id can leave a hashbrown tombstone, which still consumes the table's growth budget. The `accepted` set therefore grows slightly earlier than its length predicts (~100 inserts earlier at 1.8M ids in the latency stream, verified with an exact mirror of the set). Negligible for throughput; it moves, but does not add, growth spikes.
- Result / evidence: behaviour unchanged (differential test green; release CLI output on the 5M file byte-identical to the previous commit, sha256 770b6476…; stream fingerprint unchanged). Numbers: BENCH.md row 1.

## 14. SlabBook: slab + index-linked FIFO per level, behind the same `Book` trait
- Decision: `SlabBook` keeps `BTreeMap<Price, Level { head, tail }>` per side, orders in a `Vec<Slot>` (40 B) linked by `u32` prev/next (`NONE = u32::MAX`), a free list threaded through `next`, and `HashMap<OrderId, u32, IdBuildHasher>`. `front` reads the head slot (no hashing); `fill_front` decrements or unlinks the head through the B-tree entry it already holds; `remove`/`move_to_back` unlink in O(1) and touch the B-tree only at the ends of a level. `BaselineBook` stays; `Book::with_capacity` moved into the trait so `Engine::with_config` is generic.
- Why: profile showed cancel (B-tree + linear search over the level) at ~20% beyond hashing; histogram: half of cancels of live orders hit levels of >= 128 orders, mean position scanned 122.9.
- Limitations: at most `u32::MAX - 1` simultaneously live orders (`debug_assert!`; in release the order is not rested). The slab never shrinks: memory stays at the peak live count (12,112 slots = ~0.48 MB on the latency stream).
- Alternative considered: caching the best level to avoid the second B-tree descent in `fill_front` (not done: extra state to keep consistent; revisit only if profiled); sorted `Vec` levels; price ladder.
- Result / evidence: all 52 engine tests and the differential test run on both books; mutation (drop neighbour `prev` update in unlink) caught by `check_invariants` in the differential test, 4 engine tests and 2 slab unit tests. Numbers: BENCH.md row 2. CLI default (`run()` / `engine`) is now `SlabBook`, un-sized; output on the 5M file unchanged (BENCH.md).

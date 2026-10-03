# Mini Matching Engine

Single-instrument limit order book in Rust (std + criterion only): price-time priority, trades at the maker's price, cancel-newest self-trade prevention, a ±10% price band, and quantity-only MODIFY (an increase loses priority). `engine <input_file>` reproduces `sample_expected.txt` byte for byte. Decisions: [DECISIONS.md](DECISIONS.md). Measurements and conditions: [BENCH.md](BENCH.md).

## 1. Build, run, test, bench

| Task | Command |
|---|---|
| Build | `cargo build --release` |
| Run | `target/release/engine sample_input.txt` |
| Test (unit, differential, sample) | `cargo test` |
| Throughput (Criterion, both books) | `cargo bench --bench bench` (optional: prefix `taskset -c 2`; append `-- --save-baseline <name>` / `-- --baseline <name>` to save or compare) |
| Latency (p50…p99.99, max, timer overhead) | `cargo build --release --examples && taskset -c 2 target/release/examples/latency [--slab] [--presized]` (book: baseline unless `--slab`; sizing: un-sized unless `--presized`) |
| Generate a load file + depth/mix report | `cargo run --release --example gen_input -- 5000000 42` (writes `data/load_5000000_s42.txt`) |
| Black-box check against a Python reference | `python3 tools/verify.py target/release/engine --seed 1` |
| All pre-submission checks (fmt, clippy, build, test, sample diff, verify.py with 100 streams); stops at the first failure | `bash tools/check.sh` |

## 2. Design

**Engine vs book.** `Engine<B: Book>` owns every rule: validation order, the price band, the matching loop, STP, and the MODIFY priority rule. `Book` is storage only. That keeps one implementation of the tricky logic, so swapping the book cannot change behaviour (DECISIONS #7, #12).

`Book` methods: `front`, `fill_front`, `insert`, `remove`, `get_remaining`, `set_qty`, `move_to_back` (storage operations; the engine decides when to call which), `with_capacity`, `order_count`, `level_count` (sizing and reporting), and the test-only `check_invariants`, `live_ids`.

| | `BaselineBook` (reference point) | `SlabBook` (CLI default) |
|---|---|---|
| Levels | `BTreeMap<Price, VecDeque<OrderId>>` per side | `BTreeMap<Price, Level{head, tail}>` per side |
| Orders | `HashMap<OrderId, Order>` | `Vec<Slot>` (40 B) linked by `u32` prev/next, a free list, and `HashMap<OrderId, u32>` |

- **Hasher:** id-keyed maps use the SplitMix64 finalizer, which mixes all bits for any id pattern. There is no HashDoS protection, because input is trusted replay data (#13).
- **`accepted` set:** inserted first, so the accepted path hashes once (#13).
- **`EngineConfig`:** opt-in pre-sizing of `accepted` and of live-order storage. The default is un-sized.

Complexity per operation. L = price levels on a side, k = orders in the level; hash operations are expected O(1).

| Operation | BaselineBook | SlabBook |
|---|---|---|
| NEW that rests | O(log L) | O(log L) |
| Each match step (`front` + `fill_front`) | O(log L) + 2 hash lookups | O(log L), no hashing (one hash removal on a full fill) |
| CANCEL | O(log L + k): linear search, then `VecDeque::remove` | O(1) unlink; + O(log L) only at a level's head or tail |
| MODIFY that keeps priority | O(1) | O(1) |
| MODIFY that loses priority | O(log L + k) | O(log L) |
| `accepted` growth | amortized O(1); a single growth rehashes the whole set (the max-latency outlier) | same |

## 3. Invariants (checked by `check_invariants` after every command in tests)
- The book is never crossed (best bid < best ask), and no price level is empty.
- Queued orders and the id map match exactly: each live order is queued once, at its own price and side, with `remaining > 0`.
- Slab only: `prev`/`next` are symmetric, each level's `head`/`tail` are correct, lists have no cycles, and free slots (remaining 0) plus live slots account for every slot exactly once.
- Every live order id is in `accepted`.

## 4. Assumptions (spec ambiguities)
| # | Decision |
|---|---|
| A1 | An order cancelled by STP was ACKed, so it counts as accepted and its id is consumed |
| A2 | The band check is computed in u128, so it cannot overflow; exactly 10% away is accepted |
| A3 | The band is checked once per NEW, against the last trade price before that order; rejected orders never change it |
| A4 | STP fires only for a resting order that would actually cross |
| A5 | STP stops matching, even if other accounts' orders wait behind the own order |
| A6 | STP before any trade gives `ACK`, then `CANCELED,id,full_qty` |
| A7 | Accounts are compared as raw bytes (case-sensitive) and interned to `u32` |
| A8 | A trailing `\r` is stripped; blank lines and lines starting with `#` are ignored |
| A9 | Unparsable or non-UTF-8 lines are skipped (never a panic) and counted on stderr |
| A10 | Order ids are arbitrary sparse `u64`, so lookup is hashed, not indexed |
| A11 | MODIFY compares against *remaining* qty; equal keeps priority and still emits `MODIFIED` |
| A12 | A leading `+` on a number is accepted (`str::parse::<u64>`) |

## 5. Testing
- **Rule tests:** every engine test is instantiated once per book by a macro, so every rule is tested on both books. The slab also has its own unit tests: slot reuse, unlinking at head, middle and tail, and `move_to_back`.
- **Differential test** (`src/difftest`): a deliberately simple reference engine (`Vec` scans, arrival sequence numbers, each rule commented with the spec sentence it implements) runs against both books on 2,000 × 200-command streams plus 5 × 10,000. After every command it compares events and checks invariants, and on failure it prints the seed and a replayable input.
  - **Distributions:** NEW/CANCEL/MODIFY 55/30/15. NEW prices are 95–105 in 90% of cases, 50–200 in 6% (mostly band rejects once a trade exists) and 0 in 4%; 4 accounts; qty 1–10, or 0 in 5%. About 8% of NEW ids reuse an accepted id and 7% a rejected one; the rest are fresh. CANCEL/MODIFY target a live order 80% of the time, any accepted (usually dead) id 10%, and a never-seen id 10%. MODIFY on a live order: 25% decrease, 25% equal, 35% increase, 15% zero.
  - **Why:** a narrow price range and few accounts make crossing, multi-level sweeps, FIFO ties and STP frequent, while the rare cases (duplicates, reused rejected ids, far prices, zeros, dead targets) are injected at fixed rates so every rule fires many times. Coverage counters for every rule path must all be non-zero.
- **Mutation checks:** each mutation was applied temporarily, then reverted.

| Mutation | Caught by |
|---|---|
| band `>` → `>=` | both differential tests |
| MODIFY `<=` → `<` | both differential tests |
| slab `unlink` skips the neighbour's `prev` update | `check_invariants` in the differential test, engine tests on the slab, slab unit tests |

- **Independent black-box check** (`tools/verify.py`): a Python reference; an independent black-box reference written directly from the spec by the second AI used for design discussions, not by the coding assistant. It shares no code with the engine but follows the same spec interpretations listed in §4, so it cannot catch a misreading that both share. It generates text-level edge cases (CRLF, a missing final newline, comments, ids and prices near `u64::MAX`, `a` vs `A` accounts) and diffs the release binary's stdout. Result: byte-identical on seeds 1 and 2 (500 streams × 2,000 lines, 1.0M input lines per seed).

## 6. Performance
**Method.** The load comes from `loadgen`, with a seeded stream and a printed FNV fingerprint:
- a random-walk mid and 50 accounts;
- a mix of 60.8/29.3/10.0 NEW/CANCEL/MODIFY, with book depth bounded by design (about 9–11k live orders);
- a shadow engine, so cancels target orders that are really live.

Runs are pinned with `taskset -c 2`, 5 per configuration, and the median is reported. Observed min–max per row (engine_only / end_to_end, ms per 1M): row 0 199–226 / 326–360, row 1 131–152 / 262–288, row 2 76–104 / 196–248. Effects inside these spreads are not claimed. `engine_only` = pre-generated commands, engine only. `end_to_end` = parse + engine + format, in-memory input, output to `io::sink`; excludes file and stdout I/O. Conditions and per-run values are in BENCH.md.

| Row | Change | engine_only (1M) | end_to_end (1M) | p50 | p99 | p99.9 | max |
|---|---|---|---|---|---|---|---|
| 0 | Baseline: BTreeMap + VecDeque + SipHash | 4.51 M cmd/s | 3.03 M cmd/s | 190 ns | 644 ns | 1348 ns¹ | 37.2 ms |
| 1 | Fast id hasher + insert-first `accepted`³ | 6.95 M cmd/s | 3.60 M cmd/s | 155 ns | 503 ns | 2762 ns¹ | 27.7 ms |
| 2 | SlabBook (CLI default) | 12.14 M cmd/s | 4.74 M cmd/s | 90 ns | 361 ns | 650 ns | 26.8 ms² |

¹ p99.9 was bimodal across runs in rows 0–1 and followed machine load, so no claim is made for it. ² This max is the `accepted` rehash stall; pre-sizing removes the ~27 ms rehash stall; no exact max is claimed; the remaining ~176 µs sample repeats at the same command in every run (open item). ³ Two changes in one row, which breaks the one-change rule; the profile attributes most of the gain to the hasher (SipHash was 33% of engine samples).

**What the profile showed.**
- **Hashing was the top bottleneck, not cancel.** The plan was to build the slab first. The baseline profile showed SipHash in 33% of engine samples and `accepted` rehashing in 11%, against about 20% for cancel's linear search beyond hashing. So the hasher came first (row 1).
- **The max latency is the `accepted` set growing.** The two largest samples are exactly the inserts at which `accepted` grows; this was verified with an exact mirror of the set. Pre-sizing through `EngineConfig` removes the ~27 ms rehash stall with no throughput change; no exact max is claimed; the remaining ~176 µs sample repeats at the same command in every run (open item). On this stream, pre-sizing spends the memory *earlier*, not *more*: about 37.7 MB for `accepted`, the same table the un-sized engine reaches anyway.
- **The slab removes cancel's linear search.** Half of all cancels of live orders hit levels with 128 or more orders (mean position scanned: 122.9). The slab makes cancel O(1), and p99.9 stays tight across runs (629–695 ns).

## 7. Trade-offs
| Choice | Reverse it if |
|---|---|
| Rules in `Engine`, `Book` is storage only (each match step walks the B-tree twice: `front`, then `fill_front`) | a profile shows that boundary cost dominating and a cursor-style trait method cannot remove it |
| `BTreeMap` price levels instead of a price-indexed ladder | prices are bounded around the mid and the level lookup shows up in a profile (a ladder is O(1) but needs a fallback for outliers) |
| Exact `accepted` set of every id ever accepted (required by DUPLICATE_ID) | ids are guaranteed increasing or dense (then a range set or bitmap), or memory must stay bounded over billions of messages |

## 8. Limitations and next steps
- **`accepted` grows without bound.** It costs about 9 B per slot (BENCH.md); per billion ids that is ≥ 9 GB, an estimate not measured.
- **The slab:** at most `u32::MAX − 1` live orders (guarded by `debug_assert`; in release the order is not rested), and it never shrinks: memory stays at the peak live count.
- **No HashDoS protection** in the id hasher.
- **The CLI runs un-sized,** so the ~27 ms rehash stall is still present in the shipped binary; `EngineConfig` pre-sizing is opt-in.
- **Internal preconditions between Engine and Book are checked with `debug_assert!`;** the rule and differential tests show they are unreachable from input. In production I would make them fail loudly; an experiment doing so with `assert!` cost ~14% on engine_only (3 runs), likely from inlined panic paths, so it was reverted.
- **Open:** with pre-sizing, the remaining ~176 µs sample repeats at the same command in every run. Hypothesis, unverified: an in-place rehash of the live-order map caused by tombstones from cancel churn.
- **Next: parsing and formatting now dominate end-to-end.** With the slab, the engine-only time is 82.4 ms per 1M commands and the end-to-end time 210.8 ms, so about 60% goes to reading, parsing and formatting (by subtraction, not profiled). The next step is a byte-level parser (no `from_utf8`, no `split`) and integer formatting without `fmt`.

## 9. Extensions
- **Multiple instruments:** one `Engine` + book per instrument, routed by symbol from a symbol → index table. DUPLICATE_ID must then be checked against a global accepted-id set, or ids must be namespaced by instrument.
- **Multi-threading:** a single writer per instrument (the engine stays single-threaded and lock-free); SPSC ring buffers carry commands in and events out, with a parser/router thread in front and a formatter/sink thread behind. No locks on the hot path; instruments scale across cores.

## 10. Use of tools
Development used an AI coding assistant under the rules in AGENTS.md; it wrote most of the code and the documentation drafts. Design options were discussed with a second AI; I chose among them and recorded the reasoning in DECISIONS.md. Every change was checked with the rule tests, the differential test, mutation checks and byte-identical output comparisons, and I reviewed all code.

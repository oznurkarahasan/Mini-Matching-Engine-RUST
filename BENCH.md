# BENCH (every number needs its conditions)

Machine: Intel Core i7-11390H (4C/8T, L2 5 MiB, L3 12 MiB), Ubuntu kernel 6.8.0-138, rustc 1.92.0 | Power mode: platform profile `balanced`, governor `powersave` (intel_pstate; not changeable without root) | On AC: yes | Pinning: `taskset -c 2` | Build: release profile (opt-level 3, fat LTO, codegen-units 1, panic=abort) | Repeats: 5, median reported | Background: desktop session with VS Code open, load average 0.6-1.0 before runs

**Noise policy.** Observed min-max of the 5 pinned runs per row (engine_only / end_to_end, ms per 1M): row 0 199.1-226.5 / 325.8-360.1; row 1 131.0-152.4 / 262.4-288.5; row 2 (slab) 75.7-103.6 / 195.7-248.4. A change whose effect is inside the observed spread is not claimed as an improvement without repeated runs (at least 5 each, medians compared).

**Benchmark definitions.** `engine_only`: pre-generated `Vec<Command>` through `Engine::process`, no parsing or formatting. `end_to_end`: parse + engine + format, in-memory input, output to `io::sink`; excludes file and stdout I/O (bench id unchanged).

## Load stream

Generator: `assignment::loadgen` (shadow `Engine<BaselineBook>`, live target 10,000, seed 42).
Mix knob: `loadgen::MID_STEP_PCT` = **1** (probability the mid moves one tick per command; was 10).
Rationale: most fills come from resting orders becoming crossable as the mid drifts, so lower volatility -> fewer fills -> live count nearer target -> cancel share nearer 30%. (The marketable share was tried first and barely moved the mix: 20%->5% gave 66/24/10 -> 65/25/10.)

| MID_STEP_PCT | NEW | CANCEL | MODIFY | trades/cmd |
|---|---|---|---|---|
| 10 (old) | 67.3% | 22.6% | 10.0% | 0.441 |
| 5 | 65.7% | 24.2% | 10.0% | 0.412 |
| 2 | 62.1% | 27.9% | 10.0% | 0.346 |
| **1 (chosen)** | **60.8%** | **29.3%** | **10.0%** | 0.322 |

Achieved (5M commands, MID_STEP_PCT=1): NEW 60.8% / CANCEL 29.3% / MODIFY 10.0%; cancels of non-live ids 10.0% (= deliberate unknown-id share); modifies of non-live ids 10.0%; STP 0.0066/cmd; NEW rejects 0.0031/cmd.
Depth (live orders / levels): 0.5M 9,363 / 88; 2.5M 10,487 / 117; 5M 11,103 / 89 (all 10 checkpoints: live 8.6k-11.1k, levels 82-143).

Stream fingerprints (FNV-1a over all command fields, printed by each tool):
- Throughput: `generate(42, 1_000_000)` -> `41a93dfbb17908b7`
- Latency: `generate(42, 6_000_000)` (1M warm-up + 5M measured) -> `2ba0ffeb86d5ead2`
- File: `data/load_5000000_s42.txt` (5M) -> `b69ba24b8287f5f6`

How to reproduce:
- Throughput: `taskset -c 2 cargo bench --bench bench -- --save-baseline baseline0` (Criterion, sample_size 10, measurement_time 10 s, `iter_batched` with a fresh engine per iteration; setup and drop outside the timed region, checked in criterion-0.5 `bencher.rs`)
- Latency: `cargo build --release --examples && taskset -c 2 target/release/examples/latency`

## Results

| # | Change | Stream | engine_only | end_to_end | p50 | p99 | p99.9 | Notes |
|---|---|---|---|---|---|---|---|---|
| 0 | Baseline (BTreeMap + VecDeque + std HashMap/SipHash) | `41a93dfb…` / `2ba0ffeb…` | 221.9 ms / 1M = 4.51 M cmd/s | 329.6 ms / 1M = 3.03 M cmd/s | 190 ns | 644 ns | 1348 ns | Criterion baseline saved as `baseline0` (from run 5). max 37.2 ms = `accepted` set growth (see below) |
| 1 | SplitMix64-finalizer hasher for `accepted` + `orders`; `accepted` insert-first (DECISIONS #13). Two changes in one row (breaks the one-change rule); the profile attributes most of the gain to the hasher | same | 143.9 ms / 1M = 6.95 M cmd/s (**-35%**) | 277.8 ms / 1M = 3.60 M cmd/s (**-16%**) | 155 ns | 503 ns | 2762 ns (noisy, see details) | max 27.7 ms (was 37.2), still `accepted` growth |
| 2 | SlabBook (slab + index-linked FIFO per level; DECISIONS #14) | same | 82.4 ms / 1M = 12.14 M cmd/s (**-38%** vs baseline book in the same session) | 210.8 ms / 1M = 4.74 M cmd/s (**-24%**) | 90 ns | 361 ns | 650 ns | max 26.8 ms (`accepted` growth, book-independent). CLI default since this row; Criterion baseline `row2` |
| 3 | | | | | | | | |

Superseded (old stream, MID_STEP_PCT=10, unpinned, fingerprints `f6d131f3…` / `ee234d5f…`): engine_only 189.7 ms, end_to_end 338.6 ms, p50/p99/p99.9 167/651/3223 ns. Not comparable with row 0: different stream (fewer cancels, shallower book).

### Row 0 details
- engine_only point estimates (ms), 5 pinned runs: 199.08, 220.45, 221.91, 226.01, 226.49 -> median 221.91.
- end_to_end point estimates (ms): 328.39, 360.10, 325.77, 347.59, 329.56 -> median 329.56.
- Latency (ns), engine, 5 pinned runs: p50 205, 190, 190, 189, 189 -> 190; p99 729, 644, 644, 645, 640 -> 644; p99.9 3723, 1348, 1286, 1369, 1316 -> 1348; p99.99 9948, 4427, 4463, 4502, 4406 -> 4463; max 53.2, 37.2, 37.2, 37.2, 38.2 ms.
- Timer overhead (two `Instant::now()`, empty body): p50 20 ns, p99 22-31 ns, p99.9 28-45 ns. Reported alongside, not subtracted.
- Final book after latency run: 10,403 live orders, 94 levels.

### Row 1 details
- Behaviour check: differential test green; release CLI output on `data/load_5000000_s42.txt` byte-identical to row 0's commit (sha256 `770b6476…` both); stream fingerprints unchanged.
- engine_only point estimates (ms), 5 pinned runs vs `baseline0`: 130.95, 143.88, 152.37, 147.29, 142.93 -> median 143.88. Criterion change vs baseline0 in every run: -37% to -41% (p = 0.00). Well outside the observed spread (row 0: 199.1-226.5 ms; row 1: 131.0-152.4 ms).
- end_to_end (ms): 279.69, 262.37, 276.62, 288.45, 277.76 -> median 277.76. Criterion change: -13% to -23% (p = 0.00).
- Load average during these runs was 1.8-2.6 (higher than row 0's 0.6-1.6): the comparison is conservative if anything.
- This row combines two changes (hasher and insert-first), which breaks the one-change rule. The profile attributes most of the gain to the hasher (SipHash was 33% of engine_only samples); the share of insert-first alone was not measured.
- Latency (ns), 5 pinned runs: p50 155, 155, 153, 156, 154 -> 155; p99 514, 508, 501, 503, 500 -> 503; p99.9 3445, 3268, 2762, 1026, 2755 -> 2762; p99.99 9492, 9405, 4413, 4372, 4415 -> 4415; max 31.8, 31.6, 27.2, 26.8, 27.7 ms -> 27.7.
- p99.9 is bimodal across runs in both rows (~1.0-1.4 µs vs ~2.7-3.7 µs) and moves with machine load; no claim is made for p99.9. p50 (-18%) and p99 (-22%) moved consistently in all runs.
- Outlier: the top two samples are still the `accepted` set growth (verified with an exact mirror of the set: same hasher, same insert/remove sequence): commands 3,032,489 (26.8-31.8 ms) and 1,514,810 (13.3-15.7 ms). Growth now happens ~100 inserts earlier than the plain count predicts, because removing a rejected id can leave a hashbrown tombstone that still consumes growth budget.

### Row 2 details (SlabBook)
- Same session, same stream (`41a93dfb…` / `2ba0ffeb…`), 5 pinned runs, each Criterion run measures both books (`*/baseline` = BaselineBook with the row 1 code path, `*/slab` = SlabBook). Load average 1.4-1.8.
- engine_only (ms): baseline 143.26, 139.37, 131.78, 133.52, 127.52 -> median 133.52 (row 1 was 143.9: session within noise); slab 82.35, 78.37, 103.55, 75.66, 85.09 -> median 82.35. Slab faster in every run; -38% vs same-session baseline, -43% vs row 1.
- end_to_end (ms): baseline 277.59, 276.23, 269.29, 363.26, 287.86 -> 277.59; slab 212.62, 248.37, 206.80, 210.78, 195.73 -> 210.78 (-24%).
- Latency (ns), interleaved baseline/slab, 5 runs each, default sizing:

| Book | Throughput (6M) | p50 | p99 | p99.9 | p99.99 | max |
|---|---|---|---|---|---|---|
| baseline | 7.07 M cmd/s | 154 | 504 | 2604 | 4372 | 27.2 ms |
| slab | **11.67 M cmd/s** | **90** | **361** | **650** | 4157 | 26.8 ms |

- Per-run slab: p50 90, 88, 90, 94, 88; p99 361, 347, 361, 392, 348; p99.9 695, 650, 629, 633, 677; p99.99 4230, 4157, 4122, 4153, 4199; throughput 11.50, 11.78, 11.67, 11.73, 11.34.
- p99.9: the slab is tight across runs (629-695 ns), unlike the bimodal baseline (974-2800 ns). Consistent with the baseline tail being the linear search on long levels, but not separately verified.
- max is unchanged: it is the `accepted` set growth (same command 3,032,489), which `EngineConfig` pre-sizing removes (change 2).
- Level length at cancel of a live order (baseline book, 5M stream, n = 1,316,328): 1: 0.1%, 2-3: 0.3%, 4-7: 1.0%, 8-15: 3.1%, 16-31: 7.1%, 32-63: 14.5%, 64-127: 23.5%, >=128: 50.3%; mean length 158.6, mean position 122.9.
- Memory: slab 12,112 slots x 40 B (`size_of::<Slot>()`, asserted in a test) = ~0.48 MB at peak; never shrinks.

### Independent black-box check
- `python3 tools/verify.py target/release/engine --seed 1` and `--seed 2` (defaults: 500 streams x 2,000 lines each) on the SlabBook-default release binary: ALL OK, byte-identical, 1,000,000 input lines per seed (2.0M total), ~7 s per seed.

### Default switched to SlabBook (after row 2)
- `run()` / the `engine` CLI now use `Engine::with_book(SlabBook::new())`, un-sized (`EngineConfig` stays opt-in). `Engine::new()` still builds the baseline book for comparison.
- Bench ids name books explicitly: `*/baseline` = BaselineBook (end_to_end now calls `run_with(.., Engine::new())`), `*/slab` = SlabBook = the CLI default.
- Behaviour check: sample test passes; `cargo run --release -- sample_input.txt | diff - sample_expected.txt` identical; release CLI output on `data/load_5000000_s42.txt` sha256 `770b6476…`, identical to the baseline-book output of rows 0-1.
- **Criterion baseline for later changes: `row2`** (one pinned run, load average 1.2, saved with `taskset -c 2 cargo bench --bench bench -- --save-baseline row2`): engine_only/slab 76.0 ms (13.16 M cmd/s), end_to_end/slab 196.0 ms (5.10 M cmd/s); engine_only/baseline 127.6 ms, end_to_end/baseline 254.4 ms. This single run is at the fast end of the row 2 five-run range (slab engine_only 75.7-103.6 ms), so compare later changes with repeated runs (`--baseline row2`), per the noise policy.

### Change 2: `EngineConfig` pre-sizing (opt-in; default benchmark stays un-sized)
`Engine::with_config(EngineConfig { expected_accepted_ids, expected_live_orders })` sizes `accepted` and the book's `orders` map separately; `Engine::new()` is unchanged. Measured only in the latency example (`--presized`): `expected_accepted_ids` = number of NEW commands in the stream (3.65M; an upper bound on inserts, so `accepted` can never grow), `expected_live_orders` = 2 x TARGET_LIVE = 20,000. 5 pinned runs per mode, interleaved default/presized, same 6M stream (`2ba0ffeb…`), load average 1.2-1.5.

| Mode | Throughput (6M, whole stream) | p50 | p99 | p99.99 | max | mean |
|---|---|---|---|---|---|---|
| default (un-sized) | 7.01 M cmd/s | 152 ns | 498 ns | 4442 ns | **27.2 ms** | 175.4 ns |
| presized | 7.02 M cmd/s | 158 ns | 503 ns | 4482 ns | no exact max claimed | 169.4 ns |

- Per-run values: throughput default 7.03, 6.04, 6.87, 7.01, 7.02 / presized 4.98, 7.12, 7.02, 7.11, 6.68 M cmd/s; max default 26.9, 31.6, 27.0, 27.2, 27.4 ms / presized 1041, 172, 173, 176, 181 µs; p99.99 default 4508, 9415, 4433, 4395, 4442 / presized 9571, 4482, 4473, 4502, 4482 ns.
- Throughput, p50, p99 and p99.99: no difference beyond noise. Mean drops ~6 ns/command, consistent with the ~45 ms of rehash work (27 + 14 ms + smaller growths) no longer spread over 5M commands.
- Max: pre-sizing removes the ~27 ms rehash stall; no exact max is claimed; the remaining ~176 µs sample repeats at the same command (5,300,890) in every presized run (open item); the mirror shows the `accepted` capacity does not change there and live count is not at a new peak. Unverified hypothesis: an in-place rehash of the `orders` map (cancel churn leaves tombstones; hashbrown rehashes in place when growth budget is exhausted). Not investigated further.
- Memory: both modes end with the same tables on this stream (default grows to the same bucket count), so pre-sizing costs memory *earlier*, not *more*, here: `accepted` 4,194,304 buckets x (8 B entry + 1 control byte) + 16 = ~37.7 MB; `orders` 32,768 buckets x (48 B entry + 1) + 16 = ~1.61 MB. Computed from `capacity()` with hashbrown's layout (capacity = 7/8 of a power-of-two bucket count, one control byte per bucket, 16 trailing control bytes); allocator overhead ignored. Entry size from `size_of::<(OrderId, Order)>()` = 48 B. An over-estimate of `expected_accepted_ids` costs 9 B per extra slot, rounded up to the next power of two of buckets.

### Experiment: `assert!` instead of `debug_assert!` for Engine/Book preconditions (reverted)
- Commit ac6736f turned every internal precondition check in both books into `assert!`/`panic!` (fail loudly in release). Same session, engine_only/slab, 3 pinned runs each, interleaved with the previous commit: before 77.6 / 77.8 / 77.7 ms (median 77.7), after 79.9 / 93.3 / 88.3 ms (median 88.3), +14% at the median; ranges do not overlap.
- Likely cause (unverified): panic formatting code inlined into hot functions. Reverted in 7458ed6; the code is back to the row 2 state, so rows 0-2 still describe it.

### Max-latency outlier (verified, not fixed)
In all 5 latency runs the two largest samples are the same two commands, and both are exactly the insert at which the `accepted: HashSet<OrderId>` grows (growth sizes found by simulating a std `HashSet<u64>` with the public `capacity()`):
- command 3,032,648 (NEW id 1838765): accepted 1,835,008 -> 1,835,009; std grows at 1,835,009 -> 37-53 ms
- command 1,514,898 (NEW id 919411): accepted 917,504 -> 917,505; std grows at 917,505 -> 19-22 ms
- Neither coincides with a new live-order peak, so the `orders` map is not involved. The third-largest sample (356 µs) is not at a growth point.
- The superseded stream reached the next growth point (3,670,017 accepted) inside the measured window, which explains its ~78 ms max.

## Profiling notes
- Command used: `CARGO_TARGET_DIR=target-prof RUSTFLAGS="-C force-frame-pointers=yes" cargo flamegraph -o target-prof/fg_<bench>.svg --no-inline -c "record -F 499 --call-graph fp" --bench bench -- --bench --profile-time 20 <bench_fn>`
- Flamegraphs: `target-prof/fg_engine_only.svg`, `target-prof/fg_end_to_end.svg` (not committed).
- Attribution: `perf script -F ip,sym` aggregated by nearest engine/book frame (script kept outside the repo). Generator setup is 2.3% (engine_only) / 1.6% (end_to_end) of samples.

engine_only (9,424 samples), inclusive by component:
| Component | Incl. | of which hashing | of which rehash |
|---|---|---|---|
| Engine::process own code (incl. `accepted` set, inlined matching) | 38.2% | 16.8% | 10.8% |
| BaselineBook::remove (cancel) | 22.6% | 2.3% | 0 |
| BaselineBook::insert | 11.9% | 3.2% | 0.7% |
| BaselineBook::front | 11.2% | 5.3% | 0 |
| BaselineBook::fill_front | 7.3% | 3.8% | 0 |
| BaselineBook::move_to_back | 3.6% | 0.4% | 0 |
- SipHash (`hash_one` + `sip::Hasher::write`) anywhere in the stack: 32.9%. `reserve_rehash` anywhere: 11.8%.
- perf stat (pinned, 20 s): IPC 2.45, branch-miss 1.76%, L1d miss 4.34% of loads, LLC miss 14.0% of LLC loads, avg clock 2.89 GHz.

end_to_end (10,840 samples):
- Engine + book: ~58% (same pattern as above, scaled). Parse/format/run code (inlined into the Criterion closure): ~42%.
- Top self symbols: `Bencher::iter` (inlined run loop: read_until, from_utf8, integer parse, `write!` formatting) 14.5%, `str::split` iterator 13.6%, SipHash 20.6% total, `memcmp` 3.5%, `AccountInterner::intern` 3.1%.
- perf stat (pinned, 20 s): IPC 2.69, branch-miss 1.28%, L1d miss 2.38%, LLC miss 19.9% of LLC loads.

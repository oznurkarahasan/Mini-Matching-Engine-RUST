# BENCH (every number needs its conditions)

Machine: Intel Core i7-11390H (4C/8T, L2 5 MiB, L3 12 MiB), Ubuntu kernel 6.8.0-138, rustc 1.92.0 | Power mode: platform profile `balanced`, governor `powersave` (intel_pstate; not changeable without root) | On AC: yes | Pinning: `taskset -c 2` | Build: release profile (opt-level 3, fat LTO, codegen-units 1, panic=abort) | Repeats: 5, median reported | Background: desktop session with VS Code open, load average 0.6-1.0 before runs

**Noise policy.** Run-to-run spread of `engine_only` is ~13% even pinned (row 0: 199-226 ms). A change whose effect is inside this band is not claimed as an improvement without repeated runs (at least 5 each, medians compared, Criterion comparison against the saved baseline `baseline0`).

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
| 1 | SplitMix64-finalizer hasher for `accepted` + `orders`; `accepted` insert-first (DECISIONS #13) | same | 143.9 ms / 1M = 6.95 M cmd/s (**-35.2%**) | 277.8 ms / 1M = 3.60 M cmd/s (**-15.7%**) | 155 ns | 503 ns | 2762 ns (noisy, see details) | max 27.7 ms (was 37.2), still `accepted` growth |
| 2 | | | | | | | | |

Superseded (old stream, MID_STEP_PCT=10, unpinned, fingerprints `f6d131f3…` / `ee234d5f…`): engine_only 189.7 ms, end_to_end 338.6 ms, p50/p99/p99.9 167/651/3223 ns. Not comparable with row 0: different stream (fewer cancels, shallower book).

### Row 0 details
- engine_only point estimates (ms), 5 pinned runs: 199.08, 220.45, 221.91, 226.01, 226.49 -> median 221.91.
- end_to_end point estimates (ms): 328.39, 360.10, 325.77, 347.59, 329.56 -> median 329.56.
- Latency (ns), engine, 5 pinned runs: p50 205, 190, 190, 189, 189 -> 190; p99 729, 644, 644, 645, 640 -> 644; p99.9 3723, 1348, 1286, 1369, 1316 -> 1348; p99.99 9948, 4427, 4463, 4502, 4406 -> 4463; max 53.2, 37.2, 37.2, 37.2, 38.2 ms.
- Timer overhead (two `Instant::now()`, empty body): p50 20 ns, p99 22-31 ns, p99.9 28-45 ns. Reported alongside, not subtracted.
- Final book after latency run: 10,403 live orders, 94 levels.

### Row 1 details
- Behaviour check: differential test green; release CLI output on `data/load_5000000_s42.txt` byte-identical to row 0's commit (sha256 `770b6476…` both); stream fingerprints unchanged.
- engine_only point estimates (ms), 5 pinned runs vs `baseline0`: 130.95, 143.88, 152.37, 147.29, 142.93 -> median 143.88. Criterion change vs baseline0 in every run: -37% to -41% (p = 0.00). Well outside the ~13% noise band.
- end_to_end (ms): 279.69, 262.37, 276.62, 288.45, 277.76 -> median 277.76. Criterion change: -13% to -23% (p = 0.00).
- Load average during these runs was 1.8-2.6 (higher than row 0's 0.6-1.6): the comparison is conservative if anything.
- Latency (ns), 5 pinned runs: p50 155, 155, 153, 156, 154 -> 155; p99 514, 508, 501, 503, 500 -> 503; p99.9 3445, 3268, 2762, 1026, 2755 -> 2762; p99.99 9492, 9405, 4413, 4372, 4415 -> 4415; max 31.8, 31.6, 27.2, 26.8, 27.7 ms -> 27.7.
- p99.9 is bimodal across runs in both rows (~1.0-1.4 µs vs ~2.7-3.7 µs) and moves with machine load; no claim is made for p99.9. p50 (-18%) and p99 (-22%) moved consistently in all runs.
- Outlier: the top two samples are still the `accepted` set growth (verified with an exact mirror of the set: same hasher, same insert/remove sequence): commands 3,032,489 (26.8-31.8 ms) and 1,514,810 (13.3-15.7 ms). Growth now happens ~100 inserts earlier than the plain count predicts, because removing a rejected id can leave a hashbrown tombstone that still consumes growth budget.

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

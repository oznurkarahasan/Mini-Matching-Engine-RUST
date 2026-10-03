# BENCH (every number needs its conditions)

Machine: Intel Core i7-11390H (4C/8T, L2 5 MiB, L3 12 MiB), Ubuntu kernel 6.8.0-138, rustc 1.92.0 | Power mode: platform profile `balanced`, governor `powersave` (intel_pstate) | On AC: yes | Build: release profile (opt-level 3, fat LTO, codegen-units 1, panic=abort) | Repeats: 5, median reported | Background: VS Code open, load average ~1.0 (not an idle machine)

Streams (fingerprint = FNV-1a over all command fields, printed by each tool):
- Throughput: `loadgen::generate(42, 1_000_000)` -> fnv `f6d131f3106a2bca`
- Latency: `loadgen::generate(42, 6_000_000)` (1M warm-up + 5M measured) -> fnv `ee234d5fd3ad8f8c`
- File (perf / CLI): `data/load_5000000_s42.txt`, 5M commands -> fnv `25232587ff27906e`

How to reproduce:
- Throughput: `cargo bench --bench bench` (Criterion, sample_size 10, measurement_time 10 s, `iter_batched` with fresh engine per iteration; setup and drop outside the timed region)
- Latency: `cargo build --release --examples && taskset -c 2 target/release/examples/latency`

| # | Change | engine_only | end_to_end | p50 | p99 | p99.9 | Notes |
|---|---|---|---|---|---|---|---|
| 0 | Baseline (BTreeMap + VecDeque + std HashMap/SipHash) | 189.7 ms / 1M = 5.27 M cmd/s | 338.6 ms / 1M = 2.95 M cmd/s | 167 ns | 651 ns | 3223 ns | max ~78 ms in every latency run (see below) |
| 1 | | | | | | | |

### Row 0 details
- engine_only point estimates (ms), 5 runs: 194.15, 204.90, 182.68, 182.21, 189.73 -> median 189.73. Run-to-run spread ~12%.
- end_to_end point estimates (ms), 5 runs: 355.83, 323.54, 338.59, 324.99, 345.66 -> median 338.59.
- Latency (ns), engine, 5 runs pinned to core 2:
  - p50: 168, 165, 196, 167, 164 -> 167
  - p99: 684, 649, 761, 651, 640 -> 651
  - p99.9: 2894, 3223, 1837, 3349, 3289 -> 3223
  - p99.99: 4572, 4561, 9263, 4699, 4589
  - max: 79.2 ms, 78.5 ms, 82.3 ms, 77.9 ms, 77.4 ms
- Timer overhead (two `Instant::now()` calls, empty body): p50 20 ns, p99 22-26 ns, p99.9 25-33 ns. Reported alongside, not subtracted.
- Book depth at end of latency run: 9370 live orders, 202 levels.
- Load stream (5M, seed 42): NEW 67.3% / CANCEL 22.6% / MODIFY 10.0%; cancels of non-live ids 10.0% (= the deliberate unknown-id share); trades 0.441 per command; STP 0.0090 per command. Live orders settle at ~6.7k-8.5k, levels ~150-260 (checkpoints every 0.5M).

## Profiling notes
- Command used: `CARGO_TARGET_DIR=target-prof RUSTFLAGS="-C force-frame-pointers=yes" cargo flamegraph --no-inline -c "record -F 499 --call-graph fp" --bench <name> -- --bench --profile-time 5 <bench_fn>`
- perf stat (IPC, cache-miss rate) before/after:

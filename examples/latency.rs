//! Per-command latency of the engine on the synthetic stream.
//!
//! Usage: cargo run --release --example latency -- [measured] [warmup] [seed] [--presized] [--slab]
//! Defaults: 5_000_000 measured commands after 1_000_000 untimed warm-up
//! commands, seed 42, un-sized engine. Suggested: pin with `taskset -c 2`.
//!
//! Each command is timed with two `Instant::now()` calls; the same loop with an
//! empty body is measured first and reported as the timer overhead (it is
//! reported alongside, not subtracted). A second, untimed-per-command pass over
//! the whole stream on a fresh engine gives throughput for the same mode.
//!
//! `--presized` uses `EngineConfig`: `expected_accepted_ids` = number of NEW
//! commands in the stream (an upper bound on inserts into `accepted`, so it can
//! never grow) and `expected_live_orders` = 2 x `loadgen::TARGET_LIVE`.

use std::env;
use std::hint::black_box;
use std::time::Instant;

use assignment::book::BaselineBook;
use assignment::loadgen::{fingerprint, generate, TARGET_LIVE};
use assignment::{Book, Command, Engine, EngineConfig, Event, SlabBook};

fn nanos_since(t0: Instant, t1: Instant) -> u64 {
    u64::try_from(t1.duration_since(t0).as_nanos()).unwrap_or(u64::MAX)
}

/// Nearest-rank percentile of sorted data: smallest value with at least
/// `num/den` of the samples at or below it.
fn percentile(sorted: &[u64], num: u64, den: u64) -> u64 {
    let n = sorted.len() as u64;
    let rank = (n * num).div_ceil(den).max(1);
    sorted[(rank - 1) as usize]
}

fn report(label: &str, samples: &mut [u64]) {
    samples.sort_unstable();
    let mean = samples.iter().map(|&x| x as f64).sum::<f64>() / samples.len() as f64;
    println!(
        "{label:<14} p50 {:>6}  p90 {:>6}  p99 {:>6}  p99.9 {:>7}  p99.99 {:>7}  max {:>9}  mean {:>8.1}  (ns, n={})",
        percentile(samples, 50, 100),
        percentile(samples, 90, 100),
        percentile(samples, 99, 100),
        percentile(samples, 999, 1000),
        percentile(samples, 9999, 10000),
        samples[samples.len() - 1],
        mean,
        samples.len()
    );
}

/// Estimated heap bytes of a std (hashbrown) table with `capacity` and
/// `entry` bytes per bucket. hashbrown keeps capacity = buckets * 7/8 for
/// tables of 8+ buckets, plus one control byte per bucket and 16 trailing
/// control bytes (SSE2 group width). Allocator overhead is ignored.
fn table_bytes(capacity: usize, entry: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    let buckets = (capacity / 7 * 8).max(capacity + 1).next_power_of_two();
    buckets * entry + buckets + 16
}

fn make_engine<B: Book>(config: Option<EngineConfig>) -> Engine<B> {
    match config {
        Some(c) => Engine::with_config(c),
        None => Engine::with_book(B::with_capacity(0)),
    }
}

fn main() {
    let presized = env::args().any(|a| a == "--presized");
    let slab = env::args().any(|a| a == "--slab");
    let args: Vec<u64> = env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let measured = args.first().copied().unwrap_or(5_000_000) as usize;
    let warmup = args.get(1).copied().unwrap_or(1_000_000) as usize;
    let seed = args.get(2).copied().unwrap_or(42);

    let cmds = generate(seed, warmup + measured);
    println!(
        "stream fingerprint: n={} seed={seed} fnv={:016x}",
        cmds.len(),
        fingerprint(&cmds)
    );
    let config = presized.then(|| EngineConfig {
        expected_accepted_ids: cmds
            .iter()
            .filter(|c| matches!(c, Command::New { .. }))
            .count(),
        expected_live_orders: 2 * TARGET_LIVE,
    });
    println!(
        "mode: book={} {}",
        if slab { "slab" } else { "baseline" },
        match config {
            Some(c) => format!("presized {c:?}"),
            None => "default (un-sized)".to_string(),
        }
    );

    let top = if slab {
        measure::<SlabBook>(&cmds, warmup, measured, config, |e| {
            println!(
                "slab slots: {} (peak live; never shrinks)",
                e.book().slot_count()
            );
        })
    } else {
        measure::<BaselineBook>(&cmds, warmup, measured, config, |e| {
            let acc_cap = e.accepted_capacity();
            let ord_cap = e.book().orders_capacity();
            let acc_mb = table_bytes(acc_cap, 8) as f64 / 1e6;
            let ord_mb = table_bytes(ord_cap, BaselineBook::ORDER_ENTRY_BYTES) as f64 / 1e6;
            println!(
                "tables at end: accepted cap {acc_cap} (~{acc_mb:.1} MB, 8 B/entry), \
                 orders cap {ord_cap} (~{ord_mb:.2} MB, {} B/entry)",
                BaselineBook::ORDER_ENTRY_BYTES
            );
        })
    };
    // Outlier analysis replays on the baseline book: events are identical for
    // both books (differential test), and only the `accepted` mirror matters.
    explain_outliers(&cmds, warmup, &top, config);
}

/// Timer overhead, per-command latency after warm-up, and whole-stream
/// throughput, for book `B`. Returns the top samples for outlier analysis.
fn measure<B: Book>(
    cmds: &[Command],
    warmup: usize,
    measured: usize,
    config: Option<EngineConfig>,
    report_memory: impl Fn(&Engine<B>),
) -> Vec<(u64, usize)> {
    // Non-zero fill writes every page now, so no page faults while timing
    // (vec![0; n] may be lazily mapped zero pages).
    let mut samples = vec![1u64; measured];

    // Timer overhead: same loop, empty body.
    for s in samples.iter_mut() {
        let t0 = Instant::now();
        let t1 = Instant::now();
        *s = nanos_since(t0, t1);
    }
    report("timer_overhead", &mut samples);

    let mut engine: Engine<B> = make_engine(config);
    let mut out: Vec<Event> = Vec::with_capacity(64);
    for cmd in &cmds[..warmup] {
        out.clear();
        engine.process(cmd, &mut out);
        black_box(&out);
    }
    for (s, cmd) in samples.iter_mut().zip(&cmds[warmup..]) {
        out.clear();
        let t0 = Instant::now();
        engine.process(cmd, &mut out);
        let t1 = Instant::now();
        black_box(&out);
        *s = nanos_since(t0, t1);
    }
    let top = top_samples(&samples, 5);
    report("engine", &mut samples);

    println!(
        "final book: {} live orders, {} levels",
        engine.book().order_count(),
        engine.book().level_count()
    );
    report_memory(&engine);
    drop(engine);

    // Throughput: whole stream, fresh engine, no per-command timing.
    let mut engine: Engine<B> = make_engine(config);
    let t0 = Instant::now();
    for cmd in cmds {
        out.clear();
        engine.process(cmd, &mut out);
        black_box(&out);
    }
    let secs = t0.elapsed().as_secs_f64();
    println!(
        "throughput: {} commands in {:.3} s = {:.2} M cmd/s (includes ramp-up from empty book)",
        cmds.len(),
        secs,
        cmds.len() as f64 / secs / 1e6
    );
    top
}

/// The `k` largest samples as (value_ns, measured_index), largest first.
fn top_samples(samples: &[u64], k: usize) -> Vec<(u64, usize)> {
    let mut idx: Vec<(u64, usize)> = samples.iter().copied().zip(0..).collect();
    idx.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    idx.truncate(k);
    idx
}

/// For each outlier: the accepted-set size after that command, the live count,
/// and whether the `accepted` set's capacity changed during that command. The
/// set is mirrored exactly: same hasher, same operation sequence as the engine
/// (insert on every non-duplicate NEW, remove again on INVALID_VALUE /
/// PRICE_BAND), so tombstones and growth points match the real set.
fn explain_outliers(
    cmds: &[assignment::Command],
    warmup: usize,
    top: &[(u64, usize)],
    config: Option<EngineConfig>,
) {
    use assignment::hash::IdBuildHasher;
    use assignment::Reason;
    use std::collections::HashSet;

    let last = top.iter().map(|&(_, i)| warmup + i).max().unwrap_or(0);
    let mut engine: Engine<BaselineBook> = make_engine(config);
    let mut out = Vec::new();
    let mut mirror: HashSet<u64, IdBuildHasher> = HashSet::with_capacity_and_hasher(
        config.map_or(0, |c| c.expected_accepted_ids),
        IdBuildHasher::default(),
    );
    let mut live_peak = 0usize;
    // (accepted len, live, live peak, capacity changed during this command)
    let mut per_cmd = Vec::with_capacity(last + 1);
    for cmd in &cmds[..=last] {
        out.clear();
        engine.process(cmd, &mut out);
        let cap_before = mirror.capacity();
        if let Command::New { id, .. } = *cmd {
            let dup = out.iter().any(|e| {
                matches!(
                    e,
                    Event::Reject {
                        reason: Reason::DuplicateId,
                        ..
                    }
                )
            });
            if !dup {
                mirror.insert(id);
                if out.iter().any(|e| matches!(e, Event::Reject { .. })) {
                    mirror.remove(&id);
                }
            }
        }
        live_peak = live_peak.max(engine.book().order_count());
        per_cmd.push((
            mirror.len(),
            engine.book().order_count(),
            live_peak,
            mirror.capacity() != cap_before,
        ));
    }

    for &(ns, i) in top {
        let ci = warmup + i;
        let (acc, live, peak, grew) = per_cmd[ci];
        let (acc_before, _, peak_before, _) = if ci > 0 {
            per_cmd[ci - 1]
        } else {
            (0, 0, 0, false)
        };
        println!(
            "outlier {ns:>10} ns at command {ci} ({:?}): accepted {acc_before}->{acc}, \
             accepted-set capacity changed: {grew}; live {live}, new live peak: {}",
            cmds[ci],
            peak > peak_before
        );
    }
}

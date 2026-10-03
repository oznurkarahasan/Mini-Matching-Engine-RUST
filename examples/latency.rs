//! Per-command latency of the engine on the synthetic stream.
//!
//! Usage: cargo run --release --example latency -- [measured] [warmup] [seed]
//! Defaults: 5_000_000 measured commands after 1_000_000 untimed warm-up
//! commands, seed 42. Suggested: pin to one core with `taskset -c 2`.
//!
//! Each command is timed with two `Instant::now()` calls; the same loop with an
//! empty body is measured first and reported as the timer overhead (it is
//! reported alongside, not subtracted).

use std::env;
use std::hint::black_box;
use std::time::Instant;

use assignment::loadgen::{fingerprint, generate};
use assignment::{Book, Engine, Event};

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

fn main() {
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

    let mut engine = Engine::new();
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

    let book = engine.book();
    println!(
        "final book: {} live orders, {} levels",
        book.order_count(),
        book.level_count()
    );

    explain_outliers(&cmds, warmup, &top);
}

/// The `k` largest samples as (value_ns, measured_index), largest first.
fn top_samples(samples: &[u64], k: usize) -> Vec<(u64, usize)> {
    let mut idx: Vec<(u64, usize)> = samples.iter().copied().zip(0..).collect();
    idx.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    idx.truncate(k);
    idx
}

/// For each outlier: how many ids had been accepted (= `accepted` set size)
/// right after that command, the live count, and whether that command's insert
/// is exactly where a std `HashSet<u64>` grows (found by simulation with the
/// public `capacity()`, not by assuming a load factor).
fn explain_outliers(cmds: &[assignment::Command], warmup: usize, top: &[(u64, usize)]) {
    use std::collections::HashSet;

    let last = top.iter().map(|&(_, i)| warmup + i).max().unwrap_or(0);
    // Replay on a fresh engine, recording accepted count and live count per command.
    let mut engine = Engine::new();
    let mut out = Vec::new();
    let mut accepted = 0usize;
    let mut live_peak = 0usize;
    let mut per_cmd = Vec::with_capacity(last + 1);
    for cmd in &cmds[..=last] {
        out.clear();
        engine.process(cmd, &mut out);
        accepted += out
            .iter()
            .filter(|e| matches!(e, Event::Ack { .. }))
            .count();
        live_peak = live_peak.max(engine.book().order_count());
        per_cmd.push((accepted, engine.book().order_count(), live_peak));
    }

    // Sizes at which std HashSet<u64> reallocates (len after the insert that grew it).
    let max_accepted = per_cmd.last().map(|p| p.0).unwrap_or(0);
    let mut growth_at = Vec::new();
    let mut sim: HashSet<u64> = HashSet::new();
    for k in 0..max_accepted as u64 {
        let before = sim.capacity();
        sim.insert(k);
        if sim.capacity() != before {
            growth_at.push(sim.len());
        }
    }
    let big: Vec<usize> = growth_at.iter().copied().filter(|&g| g > 100_000).collect();
    println!("std HashSet<u64> grows when len reaches: {big:?} (sizes > 100k shown)");

    for &(ns, i) in top {
        let ci = warmup + i;
        let (acc, live, peak) = per_cmd[ci];
        let (acc_before, _, peak_before) = if ci > 0 { per_cmd[ci - 1] } else { (0, 0, 0) };
        let accepted_grew = acc > acc_before && growth_at.contains(&acc);
        let orders_new_peak = peak > peak_before;
        println!(
            "outlier {ns:>10} ns at command {ci} ({:?}): accepted {acc_before}->{acc}, \
             accepted-set growth point: {accepted_grew}; live {live}, new live peak: {orders_new_peak}",
            cmds[ci]
        );
    }
}

//! Throughput benchmarks on a fixed synthetic stream (see `assignment::loadgen`).
//!
//! Each group measures both books: `baseline` (BaselineBook) and `slab` (SlabBook).
//! - engine_only: pre-generated `Vec<Command>`, no parsing or formatting.
//! - end_to_end:  the same stream as text in memory -> parse -> engine -> format
//!   into `io::sink()`.

use std::hint::black_box;
use std::io;
use std::time::Duration;

use assignment::loadgen::{fingerprint, generate, write_commands};
use assignment::{run, run_with, Engine, Event, SlabBook};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};

const N: usize = 1_000_000;
const SEED: u64 = 42;

fn engine_only(c: &mut Criterion) {
    let cmds = generate(SEED, N);
    eprintln!(
        "stream fingerprint: n={N} seed={SEED} fnv={:016x}",
        fingerprint(&cmds)
    );
    let mut g = c.benchmark_group("engine_only");
    g.throughput(Throughput::Elements(N as u64));
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(10));
    g.bench_function("baseline", |b| {
        b.iter_batched(
            || (Engine::new(), Vec::<Event>::with_capacity(64)),
            |(mut engine, mut out)| {
                for cmd in &cmds {
                    out.clear();
                    engine.process(cmd, &mut out);
                    black_box(&out);
                }
                // Returned so that dropping them happens outside the timed region.
                (engine, out)
            },
            BatchSize::LargeInput,
        )
    });
    g.bench_function("slab", |b| {
        b.iter_batched(
            || {
                (
                    Engine::with_book(SlabBook::new()),
                    Vec::<Event>::with_capacity(64),
                )
            },
            |(mut engine, mut out)| {
                for cmd in &cmds {
                    out.clear();
                    engine.process(cmd, &mut out);
                    black_box(&out);
                }
                (engine, out)
            },
            BatchSize::LargeInput,
        )
    });
    g.finish();
}

fn end_to_end(c: &mut Criterion) {
    let mut input = Vec::new();
    write_commands(&mut input, &generate(SEED, N)).expect("writing to a Vec cannot fail");
    let mut g = c.benchmark_group("end_to_end");
    g.throughput(Throughput::Elements(N as u64));
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(10));
    g.bench_function("baseline", |b| {
        b.iter(|| run(&mut black_box(&input[..]), &mut io::sink()).expect("in-memory I/O"))
    });
    g.bench_function("slab", |b| {
        b.iter(|| {
            let mut engine = Engine::with_book(SlabBook::new());
            run_with(&mut black_box(&input[..]), &mut io::sink(), &mut engine)
                .expect("in-memory I/O")
        })
    });
    g.finish();
}

criterion_group!(benches, engine_only, end_to_end);
criterion_main!(benches);

use assignment::sum;
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn bench(c: &mut Criterion) {
    let v: Vec<u64> = (0..1_000_000).collect();
    c.bench_function("sum_vec", |b| b.iter(|| sum(black_box(&v))));
}

criterion_group!(benches, bench);
criterion_main!(benches);

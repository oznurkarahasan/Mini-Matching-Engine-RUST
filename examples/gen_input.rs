//! Writes a synthetic command file and reports book depth, command mix and
//! the stream fingerprint.
//!
//! Usage: cargo run --release --example gen_input -- [n_commands] [seed] [out_path]
//! Defaults: 5_000_000 commands, seed 42, data/load_<n>_s<seed>.txt

use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::process::ExitCode;

use assignment::loadgen::{fingerprint, write_commands, LoadGen};
use assignment::{Command, Event};

#[derive(Default)]
struct Mix {
    new: u64,
    cancel: u64,
    modify: u64,
    trades: u64,
    stp: u64,
    rejects: u64,
    cancel_rejects: u64,
    modify_rejects: u64,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let n: usize = args
        .first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000_000);
    let seed: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(42);
    let path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("data/load_{n}_s{seed}.txt"));

    let mut gen = LoadGen::new(seed);
    let mut cmds = Vec::with_capacity(n);
    let mut mix = Mix::default();
    let checkpoint = (n / 10).max(1);

    println!("{:>10} {:>10} {:>8}", "commands", "live", "levels");
    for i in 1..=n {
        let cmd = gen.next_command();
        match cmd {
            Command::New { .. } => mix.new += 1,
            Command::Cancel { .. } => mix.cancel += 1,
            Command::Modify { .. } => mix.modify += 1,
        }
        for e in gen.last_events() {
            match e {
                Event::Trade { .. } => mix.trades += 1,
                Event::Canceled { .. } if matches!(cmd, Command::New { .. }) => mix.stp += 1,
                Event::Reject { .. } => mix.rejects += 1,
                Event::CancelReject { .. } => mix.cancel_rejects += 1,
                Event::ModifyReject { .. } => mix.modify_rejects += 1,
                _ => {}
            }
        }
        cmds.push(cmd);
        if i % checkpoint == 0 {
            let (live, levels) = gen.depth();
            println!("{i:>10} {live:>10} {levels:>8}");
        }
    }

    let pct = |x: u64, of: u64| 100.0 * x as f64 / of.max(1) as f64;
    let total = n as u64;
    println!(
        "mix: NEW {:.1}%  CANCEL {:.1}%  MODIFY {:.1}%",
        pct(mix.new, total),
        pct(mix.cancel, total),
        pct(mix.modify, total)
    );
    println!(
        "cancels of non-live ids: {:.1}% of cancels (10% are deliberate unknown ids)",
        pct(mix.cancel_rejects, mix.cancel)
    );
    println!(
        "modifies of non-live ids: {:.1}% of modifies",
        pct(mix.modify_rejects, mix.modify)
    );
    println!(
        "per command: trades {:.3}  stp {:.4}  new_rejects {:.4}",
        mix.trades as f64 / n as f64,
        mix.stp as f64 / n as f64,
        mix.rejects as f64 / n as f64
    );
    println!(
        "stream fingerprint: n={n} seed={seed} fnv={:016x}",
        fingerprint(&cmds)
    );

    if let Some(dir) = std::path::Path::new(&path).parent() {
        if !dir.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(dir) {
                eprintln!("gen_input: cannot create {}: {e}", dir.display());
                return ExitCode::FAILURE;
            }
        }
    }
    let result = File::create(&path).and_then(|f| {
        let mut w = BufWriter::new(f);
        write_commands(&mut w, &cmds)?;
        w.flush()
    });
    match result {
        Ok(()) => {
            println!("wrote {n} commands to {path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("gen_input: cannot write {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

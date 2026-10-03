use std::env;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::process::ExitCode;

use assignment::run;

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        eprintln!("usage: engine <input_file>");
        return ExitCode::from(2);
    };

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("engine: cannot open {}: {e}", path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };

    let mut out = BufWriter::new(io::stdout().lock());
    let result = run(&mut BufReader::new(file), &mut out);
    // Flush even after a run error so already-produced events are not lost.
    let flushed = out.flush();

    match (result, flushed) {
        (Ok(skipped), Ok(())) => {
            if skipped > 0 {
                eprintln!("engine: skipped {skipped} unparsable line(s)");
            }
            ExitCode::SUCCESS
        }
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("engine: {e}");
            ExitCode::FAILURE
        }
    }
}

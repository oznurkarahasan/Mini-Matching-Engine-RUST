//! Text-in, text-out driver used by the CLI and the sample test.

use std::io::{self, BufRead, Write};

use crate::event::{write_event, Event};
use crate::parse::{parse_line, AccountInterner};

/// Reads commands line by line from `input` and writes events to `out`.
/// Returns the number of unparsable lines that were skipped. Does not flush `out`.
/// I/O errors (including invalid UTF-8 in the input) are returned, not skipped.
pub fn run(input: &mut impl BufRead, out: &mut impl Write) -> io::Result<u64> {
    let mut accounts = AccountInterner::new();
    let mut line = String::new();
    let events: Vec<Event> = Vec::new();
    let mut skipped = 0u64;

    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let text = line.strip_suffix('\n').unwrap_or(&line);
        match parse_line(text, &mut accounts) {
            Ok(Some(_cmd)) => {
                // Engine is wired in here from step 3; it will append to `events`.
            }
            Ok(None) => {}
            Err(_) => skipped += 1,
        }
        for e in &events {
            write_event(out, e)?;
        }
    }
    Ok(skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_unparsable_lines() {
        let input = "# comment\n\nNEW,1,A,B,100,10\r\nGARBAGE\nCANCEL,1\nCANCEL,x";
        let mut out = Vec::new();
        let skipped = run(&mut input.as_bytes(), &mut out).expect("in-memory I/O");
        assert_eq!(skipped, 2);
    }
}

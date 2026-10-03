//! Text-in, text-out driver used by the CLI and the sample test.

use std::io::{self, BufRead, Write};

use crate::engine::Engine;
use crate::event::{write_event, Event};
use crate::parse::{parse_line, AccountInterner};

/// Reads commands line by line from `input` and writes events to `out`.
/// Returns the number of skipped lines (unparsable or invalid UTF-8).
/// Does not flush `out`. I/O errors are returned, not skipped.
pub fn run(input: &mut impl BufRead, out: &mut impl Write) -> io::Result<u64> {
    let mut accounts = AccountInterner::new();
    let mut engine = Engine::new();
    let mut line: Vec<u8> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let mut skipped = 0u64;

    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let bytes = line.strip_suffix(b"\n").unwrap_or(&line);
        let Ok(text) = std::str::from_utf8(bytes) else {
            skipped += 1;
            continue;
        };
        match parse_line(text, &mut accounts) {
            Ok(Some(cmd)) => engine.process(&cmd, &mut events),
            Ok(None) => {}
            Err(_) => skipped += 1,
        }
        for e in &events {
            write_event(out, e)?;
        }
        events.clear();
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

    #[test]
    fn invalid_utf8_line_is_skipped_and_processing_continues() {
        let input: &[u8] = b"NEW,1,\xff,B,100,10\nNEW,2,A,B,100,10\n";
        let mut out = Vec::new();
        let skipped = run(&mut &input[..], &mut out).expect("in-memory I/O");
        assert_eq!(skipped, 1);
        assert_eq!(out, b"ACK,2\n");
    }
}

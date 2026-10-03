//! Line parser: `&str` -> [`Command`]. Never panics; bad lines return `Err`
//! and the caller decides what to do (the CLI skips and counts them).

use std::collections::HashMap;

use crate::command::Command;
use crate::types::{AccountId, Side};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    UnknownCommand,
    MissingField,
    ExtraField,
    BadNumber,
    BadSide,
    TooManyAccounts,
}

/// Maps account names to dense `u32` ids. Names are compared as raw bytes,
/// case-sensitive. Allocates only the first time a name is seen.
#[derive(Debug, Default)]
pub struct AccountInterner {
    map: HashMap<Box<str>, AccountId>,
}

impl AccountInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, name: &str) -> Result<AccountId, ParseError> {
        if let Some(&id) = self.map.get(name) {
            return Ok(id);
        }
        let id = AccountId::try_from(self.map.len()).map_err(|_| ParseError::TooManyAccounts)?;
        self.map.insert(name.into(), id);
        Ok(id)
    }
}

/// Parses one input line. Returns `Ok(None)` for blank lines and `#` comments.
/// A trailing `\r` is stripped so CRLF files parse the same as LF files.
pub fn parse_line(
    line: &str,
    accounts: &mut AccountInterner,
) -> Result<Option<Command>, ParseError> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }

    let mut fields = line.split(',');
    let cmd = match fields.next() {
        Some("NEW") => {
            let id = parse_u64(fields.next())?;
            let account = fields.next().ok_or(ParseError::MissingField)?;
            let side = parse_side(fields.next())?;
            let price = parse_u64(fields.next())?;
            let qty = parse_u64(fields.next())?;
            if fields.next().is_some() {
                return Err(ParseError::ExtraField);
            }
            // Intern last so a malformed line never registers an account.
            let account = accounts.intern(account)?;
            Command::New {
                id,
                account,
                side,
                price,
                qty,
            }
        }
        Some("CANCEL") => Command::Cancel {
            id: parse_u64(fields.next())?,
        },
        Some("MODIFY") => Command::Modify {
            id: parse_u64(fields.next())?,
            new_qty: parse_u64(fields.next())?,
        },
        _ => return Err(ParseError::UnknownCommand),
    };
    if fields.next().is_some() {
        return Err(ParseError::ExtraField);
    }
    Ok(Some(cmd))
}

fn parse_u64(field: Option<&str>) -> Result<u64, ParseError> {
    field
        .ok_or(ParseError::MissingField)?
        .parse()
        .map_err(|_| ParseError::BadNumber)
}

fn parse_side(field: Option<&str>) -> Result<Side, ParseError> {
    match field {
        Some("B") => Ok(Side::Buy),
        Some("S") => Ok(Side::Sell),
        Some(_) => Err(ParseError::BadSide),
        None => Err(ParseError::MissingField),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Option<Command>, ParseError> {
        parse_line(line, &mut AccountInterner::new())
    }

    #[test]
    fn parses_new_buy_and_sell() {
        let mut acc = AccountInterner::new();
        assert_eq!(
            parse_line("NEW,1,A,B,100,10", &mut acc),
            Ok(Some(Command::New {
                id: 1,
                account: 0,
                side: Side::Buy,
                price: 100,
                qty: 10
            }))
        );
        assert_eq!(
            parse_line("NEW,2,B,S,101,5", &mut acc),
            Ok(Some(Command::New {
                id: 2,
                account: 1,
                side: Side::Sell,
                price: 101,
                qty: 5
            }))
        );
    }

    #[test]
    fn parses_cancel_and_modify() {
        assert_eq!(parse("CANCEL,7"), Ok(Some(Command::Cancel { id: 7 })));
        assert_eq!(
            parse("MODIFY,7,3"),
            Ok(Some(Command::Modify { id: 7, new_qty: 3 }))
        );
    }

    #[test]
    fn zero_values_parse_engine_rejects_them() {
        assert!(matches!(
            parse("NEW,8,H,B,0,0"),
            Ok(Some(Command::New {
                price: 0,
                qty: 0,
                ..
            }))
        ));
        assert_eq!(
            parse("MODIFY,5,0"),
            Ok(Some(Command::Modify { id: 5, new_qty: 0 }))
        );
    }

    #[test]
    fn blank_and_comment_lines_are_ignored() {
        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("\r"), Ok(None));
        assert_eq!(parse("# NEW,1,A,B,100,10"), Ok(None));
    }

    #[test]
    fn trailing_cr_is_stripped() {
        assert_eq!(parse("CANCEL,7\r"), Ok(Some(Command::Cancel { id: 7 })));
    }

    #[test]
    fn interner_is_stable_and_case_sensitive() {
        let mut acc = AccountInterner::new();
        assert_eq!(acc.intern("a"), Ok(0));
        assert_eq!(acc.intern("A"), Ok(1));
        assert_eq!(acc.intern("a"), Ok(0));
    }

    #[test]
    fn malformed_line_does_not_intern_account() {
        let mut acc = AccountInterner::new();
        assert_eq!(
            parse_line("NEW,1,X,Q,100,10", &mut acc),
            Err(ParseError::BadSide)
        );
        assert_eq!(acc.intern("Y"), Ok(0));
    }

    #[test]
    fn malformed_lines_are_errors() {
        assert_eq!(parse("FOO,1"), Err(ParseError::UnknownCommand));
        assert_eq!(parse("CANCEL"), Err(ParseError::MissingField));
        assert_eq!(parse("CANCEL,1,2"), Err(ParseError::ExtraField));
        assert_eq!(parse("NEW,1,A,B,100,10,9"), Err(ParseError::ExtraField));
        assert_eq!(parse("NEW,1,A,B,-5,10"), Err(ParseError::BadNumber));
        assert_eq!(
            parse("CANCEL,18446744073709551616"),
            Err(ParseError::BadNumber)
        );
    }
}

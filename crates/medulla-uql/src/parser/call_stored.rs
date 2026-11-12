//! `CALL STORED name [ @ version ] ( [ name = literal { , name = literal } ] )`
//! (MEDULLA-FEDERATED-QUERY-R708.3): the UQL form of a stored-query call.
//!
//! The statement is not a pipeline. It names a stored query and binds its parameters as typed
//! literals; the engine resolves it to the same handler as `StoredQueryCall`.

use std::collections::BTreeMap;

use super::Parser;
use crate::diag::UqlError;
use crate::lexer::Tok;

/// One bound argument: a typed literal, never spliced into any query text.
#[derive(Clone, Debug, PartialEq)]
pub enum CallArg {
    /// A string literal.
    Str(String),
    /// A number literal.
    Num(f64),
    /// `TRUE` or `FALSE`.
    Bool(bool),
}

/// A parsed `CALL STORED` statement.
#[derive(Clone, Debug, PartialEq)]
pub struct CallStored {
    /// The stored query's name.
    pub name: String,
    /// The version asked for; the highest published one when absent.
    pub version: Option<u32>,
    /// The bound arguments by parameter name.
    pub args: BTreeMap<String, CallArg>,
}

impl Parser<'_> {
    /// Parse the whole statement; the cursor is at `CALL`.
    pub(crate) fn call_stored(&mut self) -> Result<CallStored, UqlError> {
        self.expect_kw("CALL")?;
        self.expect_kw("STORED")?;
        let name = self.name("a stored query name")?;
        let version = if self.eat(&Tok::At) {
            Some(self.stored_version()?)
        } else {
            None
        };
        self.expect(&Tok::LParen, "`(`")?;
        let mut args = BTreeMap::new();
        if !self.peek(&Tok::RParen) {
            loop {
                let key = self.name("a parameter name")?;
                self.expect(&Tok::Eq, "`=`")?;
                let value = self.call_arg()?;
                if args.insert(key.clone(), value).is_some() {
                    return Err(self.err_here(&format!("parameter `{key}` is bound twice")));
                }
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "`)`")?;
        self.eat(&Tok::Semi);
        if !self.at_end() {
            return Err(self.err_here("expected the end of the statement"));
        }
        Ok(CallStored {
            name,
            version,
            args,
        })
    }

    fn stored_version(&mut self) -> Result<u32, UqlError> {
        match self.peek_kind() {
            Some(Tok::Num(n)) if n.fract() == 0.0 && *n >= 1.0 && *n <= f64::from(u32::MAX) => {
                let version = *n as u32;
                self.bump();
                Ok(version)
            }
            _ => Err(self.err_here("expected a version number of at least 1")),
        }
    }

    fn call_arg(&mut self) -> Result<CallArg, UqlError> {
        if self.eat_kw("TRUE") {
            return Ok(CallArg::Bool(true));
        }
        if self.eat_kw("FALSE") {
            return Ok(CallArg::Bool(false));
        }
        match self.peek_kind() {
            Some(Tok::Str(s)) => {
                let s = s.clone();
                self.bump();
                Ok(CallArg::Str(s))
            }
            Some(Tok::Num(n)) => {
                let n = *n;
                self.bump();
                Ok(CallArg::Num(n))
            }
            Some(Tok::Dash) => {
                self.bump();
                match self.peek_kind() {
                    Some(Tok::Num(n)) => {
                        let n = -*n;
                        self.bump();
                        Ok(CallArg::Num(n))
                    }
                    _ => Err(self.err_here("expected a number after `-`")),
                }
            }
            _ => Err(self.err_here("expected a string, number, TRUE or FALSE")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_call_stored;

    // spec: MEDULLA-FEDERATED-QUERY-R708.3
    #[test]
    fn call_stored_parses_name_version_and_typed_arguments() {
        let call =
            parse_call_stored("call stored open_incidents@2(status = 'open', n = -3, on = TRUE)")
                .expect("it is a CALL STORED statement")
                .expect("it parses");
        assert_eq!(call.name, "open_incidents");
        assert_eq!(call.version, Some(2));
        assert_eq!(call.args["status"], CallArg::Str("open".into()));
        assert_eq!(call.args["n"], CallArg::Num(-3.0));
        assert_eq!(call.args["on"], CallArg::Bool(true));
        let bare = parse_call_stored("CALL STORED q()").unwrap().unwrap();
        assert_eq!((bare.version, bare.args.len()), (None, 0));
    }

    #[test]
    fn other_statements_are_not_call_stored_and_malformed_ones_are_errors() {
        assert!(parse_call_stored("MATCH (:Doc)").is_none());
        assert!(parse_call_stored("CALL other()").is_none());
        for bad in [
            "CALL STORED q(",
            "CALL STORED q(a = 1, a = 2)",
            "CALL STORED q(a = b)",
            "CALL STORED q() extra",
            "CALL STORED q@0()",
        ] {
            assert!(parse_call_stored(bad).unwrap().is_err(), "{bad}");
        }
    }
}

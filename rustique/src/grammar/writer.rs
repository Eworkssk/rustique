//! [`Writer`]: the SQL buffer every statement compiler writes into.

use std::fmt::Write as _;

use super::{Compiled, Dialect, MAX_BINDS, mysql, oracle};
use crate::query::Raw;
use crate::{Error, Result, Value};

/// Accumulates SQL text and bindings together, so placeholders always line up
/// with their values.
///
/// The first error is kept and returned by [`finish`](Self::finish); writing
/// carries on, so compilers need no early returns.
pub(super) struct Writer {
    pub dialect: Dialect,
    sql: String,
    binds: Vec<Value>,
    batch: Option<usize>,
    returning: usize,
    error: Option<Error>,
}

impl Writer {
    pub fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            sql: String::with_capacity(128),
            binds: Vec::new(),
            batch: None,
            returning: 0,
            error: None,
        }
    }

    /// Appends more rows for the statement written so far, for array binding.
    pub fn batch(&mut self, rows: Vec<Vec<Value>>) {
        self.batch = Some(rows.len() + 1);
        self.binds.extend(rows.into_iter().flatten());
    }

    pub fn push(&mut self, sql: &str) {
        self.sql.push_str(sql);
    }

    pub fn push_u64(&mut self, n: u64) {
        let _ = write!(self.sql, "{n}"); // writing to a String cannot fail
    }

    /// Column reference: `name`, `table.name`, `table.*`, each optionally `... as alias`.
    pub fn column(&mut self, name: &str) {
        match split_alias(name) {
            Some((column, alias)) => {
                self.dotted(column);
                self.push(" AS ");
                self.quote(alias);
            }
            None => self.dotted(name),
        }
    }

    /// Table reference: `table` or `schema.table`, optionally `... as alias`.
    pub fn table(&mut self, name: &str) {
        match split_alias(name) {
            Some((table, alias)) => {
                self.dotted(table);
                self.push(match self.dialect {
                    Dialect::MySql => mysql::TABLE_ALIAS,
                    Dialect::Oracle => oracle::TABLE_ALIAS,
                });
                self.quote(alias);
            }
            None => self.dotted(name),
        }
    }

    fn dotted(&mut self, name: &str) {
        for (i, part) in name.split('.').enumerate() {
            if i > 0 {
                self.sql.push('.');
            }
            if part == "*" {
                self.sql.push('*');
            } else {
                self.quote(part);
            }
        }
    }

    fn quote(&mut self, ident: &str) {
        let ident = ident.trim();
        let result = match self.dialect {
            Dialect::MySql => mysql::quote(&mut self.sql, ident),
            Dialect::Oracle => oracle::quote(&mut self.sql, ident),
        };
        if let Err(error) = result {
            self.fail(error);
        }
    }

    /// Writes a placeholder and records its value.
    pub fn bind(&mut self, value: Value) {
        self.binds.push(value);
        match self.dialect {
            Dialect::MySql => self.sql.push('?'),
            Dialect::Oracle => {
                let _ = write!(self.sql, ":{}", self.binds.len());
            }
        }
    }

    /// Writes the placeholder of an out bind (Oracle). Out binds must come
    /// after every other placeholder of the statement.
    pub fn out_bind(&mut self) {
        self.returning += 1;
        let _ = write!(self.sql, ":{}", self.binds.len() + self.returning);
    }

    /// Writes a raw fragment, turning each `?` outside quotes and comments
    /// into a placeholder. On MySQL a backslash escapes the next character
    /// inside quotes (unless `NO_BACKSLASH_ESCAPES` is set, when a `\'`
    /// in a string makes the count fail rather than misplace bindings).
    /// Returns whether the fragment ends inside a line comment, which
    /// SQL written after it on the same line would join.
    pub fn raw(&mut self, raw: &Raw) -> bool {
        let mysql = self.dialect == Dialect::MySql;
        let sql = raw.sql.as_str();
        let mut binds = raw.binds.iter().cloned();
        let mut state = Lexeme::Code;
        let mut chars = sql.char_indices();
        while let Some((i, c)) = chars.next() {
            let ahead = &sql[i + c.len_utf8()..];
            match state {
                Lexeme::Code => match c {
                    // Oracle's alternative quoting: `q'[it's]'` ends at `]'`.
                    'q' | 'Q' if !mysql && ahead.starts_with('\'') => {
                        if let Some(open) = ahead[1..].chars().next() {
                            let close = match open {
                                '[' => ']',
                                '{' => '}',
                                '(' => ')',
                                '<' => '>',
                                other => other,
                            };
                            self.sql.push(c);
                            self.sql.push('\'');
                            self.sql.push(open);
                            chars.next();
                            chars.next();
                            state = Lexeme::AltQuoted(close);
                            continue;
                        }
                    }
                    '\'' | '"' | '`' => state = Lexeme::Quoted(c),
                    '#' if mysql => state = Lexeme::LineComment,
                    // MySQL needs a space after `--`: `5--1` is arithmetic there.
                    '-' if ahead.starts_with('-')
                        && (!mysql
                            || ahead[1..]
                                .chars()
                                .next()
                                .is_none_or(|n| n.is_whitespace() || n.is_control())) =>
                    {
                        state = Lexeme::LineComment;
                    }
                    // `/*! ... */` is code that MySQL runs, so its `?` count.
                    '/' if ahead.starts_with('*') && !(mysql && ahead[1..].starts_with('!')) => {
                        state = Lexeme::BlockComment;
                    }
                    '?' => {
                        if let Some(value) = binds.next() {
                            self.bind(value);
                            continue;
                        }
                        self.fail(Error::InvalidQuery(
                            "raw SQL has more `?` placeholders than bindings".to_owned(),
                        ));
                    }
                    _ => {}
                },
                Lexeme::Quoted(quote) if c == quote => state = Lexeme::Code,
                Lexeme::AltQuoted(close) if c == close && ahead.starts_with('\'') => {
                    chars.next();
                    self.sql.push(c);
                    self.sql.push('\'');
                    state = Lexeme::Code;
                    continue;
                }
                Lexeme::Quoted(quote) if mysql && quote != '`' && c == '\\' => {
                    self.sql.push(c);
                    if let Some((_, escaped)) = chars.next() {
                        self.sql.push(escaped);
                    }
                    continue;
                }
                Lexeme::LineComment if c == '\n' => state = Lexeme::Code,
                Lexeme::BlockComment if c == '*' && ahead.starts_with('/') => {
                    chars.next();
                    self.sql.push_str("*/");
                    state = Lexeme::Code;
                    continue;
                }
                Lexeme::Quoted(_)
                | Lexeme::AltQuoted(_)
                | Lexeme::LineComment
                | Lexeme::BlockComment => {}
            }
            self.sql.push(c);
        }
        if binds.next().is_some() {
            self.fail(Error::InvalidQuery(
                "raw SQL has fewer `?` placeholders than bindings".to_owned(),
            ));
        }
        matches!(state, Lexeme::LineComment)
    }

    /// Writes `items` separated by `, `.
    pub fn list<T>(
        &mut self,
        items: impl IntoIterator<Item = T>,
        mut each: impl FnMut(&mut Self, T),
    ) {
        for (i, item) in items.into_iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            each(self, item);
        }
    }

    pub fn fail(&mut self, error: Error) {
        self.error.get_or_insert(error);
    }

    pub fn finish(self) -> Result<Compiled> {
        // Array binding sends the binds of one row per execution.
        let binds = self.binds.len() / self.batch.unwrap_or(1);
        match self.error {
            Some(error) => Err(error),
            None if binds > MAX_BINDS => Err(Error::InvalidQuery(format!(
                "statement has {binds} bindings; MySQL and Oracle allow at most {MAX_BINDS}"
            ))),
            None => Ok(Compiled {
                sql: self.sql,
                binds: self.binds,
                batch: self.batch,
                returning: self.returning,
            }),
        }
    }
}

/// Where [`Writer::raw`] is in a raw fragment.
#[derive(Clone, Copy)]
enum Lexeme {
    Code,
    /// Inside a string or quoted identifier opened by this character.
    Quoted(char),
    /// Inside an Oracle `q'[...]'` string that this character, then `'`, closes.
    AltQuoted(char),
    LineComment,
    BlockComment,
}

/// Splits `expr as alias` (case-insensitive `as`).
fn split_alias(name: &str) -> Option<(&str, &str)> {
    let at = name.to_ascii_lowercase().find(" as ")?;
    Some((&name[..at], &name[at + 4..]))
}

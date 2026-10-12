//! Syntax tree of a query: built by [`QueryBuilder`], read by the grammar.

use super::{Operator, QueryBuilder};
use crate::Value;

/// How a clause joins the one before it. Ignored for the first clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Conjunction {
    And,
    Or,
}

/// One entry of a WHERE, HAVING or ON list.
#[derive(Debug, Clone)]
pub(crate) struct Clause {
    pub conjunction: Conjunction,
    pub condition: Condition,
}

#[derive(Debug, Clone)]
pub(crate) enum Condition {
    /// `column op ?`
    Basic {
        column: String,
        op: Operator,
        value: Value,
    },
    /// `first op second`, both columns.
    Column {
        first: String,
        op: Operator,
        second: String,
    },
    /// `column [NOT] IN (?, ...)`
    In {
        column: String,
        values: Vec<Value>,
        negated: bool,
    },
    /// `(a, b) IN ((?, ?), ...)`
    InRows {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },
    /// `column [NOT] IN (SELECT ...)`
    InSub {
        column: String,
        query: Box<QueryBuilder>,
        negated: bool,
    },
    /// `column IS [NOT] NULL`
    Null { column: String, negated: bool },
    /// `column [NOT] BETWEEN ? AND ?`
    Between {
        column: String,
        low: Value,
        high: Value,
        negated: bool,
    },
    /// `[NOT] EXISTS (SELECT ...)`
    Exists {
        query: Box<QueryBuilder>,
        negated: bool,
    },
    /// `JSON_VALUE(column, path) op ?`: `path` is checked, so it can be
    /// written into the SQL as a literal.
    Json {
        column: String,
        path: String,
        op: Operator,
        value: Value,
    },
    /// `( ... )`
    Group(Vec<Clause>),
    /// Raw SQL.
    Raw(Raw),
}

/// Raw SQL fragment; each `?` outside quotes takes the next value of `binds`.
#[derive(Debug, Clone)]
pub(crate) struct Raw {
    pub sql: String,
    pub binds: Vec<Value>,
}

impl Raw {
    pub fn new(sql: impl Into<String>, binds: impl IntoIterator<Item = Value>) -> Self {
        Self {
            sql: sql.into(),
            binds: binds.into_iter().collect(),
        }
    }
}

/// One item of the SELECT list.
#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Column(String),
    Raw(Raw),
    /// `(SELECT ...) AS alias`
    Sub {
        query: Box<QueryBuilder>,
        alias: String,
    },
    /// `CASE WHEN EXISTS (SELECT ...) THEN 1 ELSE 0 END AS alias`: Oracle
    /// 19c has no boolean expressions in the select list.
    Exists {
        query: Box<QueryBuilder>,
        alias: String,
    },
    /// `ROW_NUMBER() OVER (PARTITION BY ... ORDER BY ...) AS alias`
    RowNumber {
        partition: Vec<String>,
        order: Vec<Order>,
        alias: String,
    },
    /// `FUNCTION(column | *) [AS alias]`
    Aggregate {
        function: Aggregate,
        column: Option<String>,
        alias: Option<&'static str>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Aggregate {
    Count,
    Max,
    Min,
    Sum,
    Avg,
}

impl Aggregate {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Count => "COUNT",
            Self::Max => "MAX",
            Self::Min => "MIN",
            Self::Sum => "SUM",
            Self::Avg => "AVG",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Order {
    Column { column: String, descending: bool },
    Raw(Raw),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JoinKind {
    Inner,
    Left,
    Right,
    Cross,
}

#[derive(Debug, Clone)]
pub(crate) struct Join {
    pub kind: JoinKind,
    /// The table, or the alias of `sub`.
    pub table: String,
    /// `JOIN (SELECT ...) table`
    pub sub: Option<Box<QueryBuilder>>,
    pub on: Vec<Clause>,
}

/// `UNION [ALL] (SELECT ...)` after the main query.
#[derive(Debug, Clone)]
pub(crate) struct Union {
    pub query: QueryBuilder,
    pub all: bool,
}

/// `FOR UPDATE | FOR SHARE [SKIP LOCKED]`
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Lock {
    pub shared: bool,
    pub skip_locked: bool,
}

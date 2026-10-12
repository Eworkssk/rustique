//! JOIN support: [`JoinClause`] and the join methods of [`QueryBuilder`].

use super::{Clause, Condition, Conjunction, Join, JoinKind, QueryBuilder, Raw, wheres};
use crate::Value;

/// `ON` conditions of a join, built inside [`QueryBuilder::join_with`].
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct JoinClause {
    on: Vec<Clause>,
    error: Option<String>,
}

impl JoinClause {
    /// `ON first op second`, comparing two columns.
    pub fn on(mut self, first: impl Into<String>, op: &str, second: impl Into<String>) -> Self {
        let condition = wheres::columns(first.into(), op, second.into());
        wheres::push(&mut self.on, &mut self.error, Conjunction::And, condition);
        self
    }

    /// `OR first op second`.
    pub fn or_on(mut self, first: impl Into<String>, op: &str, second: impl Into<String>) -> Self {
        let condition = wheres::columns(first.into(), op, second.into());
        wheres::push(&mut self.on, &mut self.error, Conjunction::Or, condition);
        self
    }

    /// `AND column op value`, comparing a column with a bound value.
    pub fn where_(mut self, column: impl Into<String>, op: &str, value: impl Into<Value>) -> Self {
        let condition = wheres::basic(column.into(), op, value.into());
        wheres::push(&mut self.on, &mut self.error, Conjunction::And, condition);
        self
    }

    /// `OR column op value`.
    pub fn or_where(
        mut self,
        column: impl Into<String>,
        op: &str,
        value: impl Into<Value>,
    ) -> Self {
        let condition = wheres::basic(column.into(), op, value.into());
        wheres::push(&mut self.on, &mut self.error, Conjunction::Or, condition);
        self
    }

    /// `AND ( ... )`: conditions built by `build`, in parentheses.
    pub fn where_group(self, build: impl FnOnce(Self) -> Self) -> Self {
        self.group(Conjunction::And, build)
    }

    /// `OR ( ... )`.
    pub fn or_where_group(self, build: impl FnOnce(Self) -> Self) -> Self {
        self.group(Conjunction::Or, build)
    }

    /// Raw `AND` SQL; each `?` outside quotes takes the next value of `binds`.
    pub fn where_raw(self, sql: impl Into<String>, binds: impl IntoIterator<Item = Value>) -> Self {
        self.raw(Conjunction::And, Raw::new(sql, binds))
    }

    /// Raw `OR` SQL.
    pub fn or_where_raw(
        self,
        sql: impl Into<String>,
        binds: impl IntoIterator<Item = Value>,
    ) -> Self {
        self.raw(Conjunction::Or, Raw::new(sql, binds))
    }

    fn raw(mut self, conjunction: Conjunction, raw: Raw) -> Self {
        let condition = Ok(Condition::Raw(raw));
        wheres::push(&mut self.on, &mut self.error, conjunction, condition);
        self
    }

    fn group(mut self, conjunction: Conjunction, build: impl FnOnce(Self) -> Self) -> Self {
        let nested = build(Self::default());
        if let Some(message) = nested.error {
            self.error.get_or_insert(message);
        }
        if !nested.on.is_empty() {
            let condition = Ok(Condition::Group(nested.on));
            wheres::push(&mut self.on, &mut self.error, conjunction, condition);
        }
        self
    }
}

impl QueryBuilder {
    fn add_join(
        mut self,
        kind: JoinKind,
        table: String,
        sub: Option<Self>,
        build: impl FnOnce(JoinClause) -> JoinClause,
    ) -> Self {
        let clause = build(JoinClause::default());
        if let Some(message) = clause.error {
            self.fail(message);
        }
        self.joins.push(Join {
            kind,
            table,
            sub: sub.map(Box::new),
            on: clause.on,
        });
        self
    }

    /// `INNER JOIN table ON first op second`.
    pub fn join(
        self,
        table: impl Into<String>,
        first: impl Into<String>,
        op: &str,
        second: impl Into<String>,
    ) -> Self {
        self.add_join(JoinKind::Inner, table.into(), None, |j| {
            j.on(first, op, second)
        })
    }

    /// `LEFT JOIN table ON first op second`.
    pub fn left_join(
        self,
        table: impl Into<String>,
        first: impl Into<String>,
        op: &str,
        second: impl Into<String>,
    ) -> Self {
        self.add_join(JoinKind::Left, table.into(), None, |j| {
            j.on(first, op, second)
        })
    }

    /// `RIGHT JOIN table ON first op second`.
    pub fn right_join(
        self,
        table: impl Into<String>,
        first: impl Into<String>,
        op: &str,
        second: impl Into<String>,
    ) -> Self {
        self.add_join(JoinKind::Right, table.into(), None, |j| {
            j.on(first, op, second)
        })
    }

    /// `CROSS JOIN table`.
    pub fn cross_join(self, table: impl Into<String>) -> Self {
        self.add_join(JoinKind::Cross, table.into(), None, |j| j)
    }

    /// `INNER JOIN table ON ...` with several conditions built by `build`.
    pub fn join_with(
        self,
        table: impl Into<String>,
        build: impl FnOnce(JoinClause) -> JoinClause,
    ) -> Self {
        self.add_join(JoinKind::Inner, table.into(), None, build)
    }

    /// `LEFT JOIN table ON ...` with several conditions built by `build`.
    pub fn left_join_with(
        self,
        table: impl Into<String>,
        build: impl FnOnce(JoinClause) -> JoinClause,
    ) -> Self {
        self.add_join(JoinKind::Left, table.into(), None, build)
    }

    /// `RIGHT JOIN table ON ...` with several conditions built by `build`.
    pub fn right_join_with(
        self,
        table: impl Into<String>,
        build: impl FnOnce(JoinClause) -> JoinClause,
    ) -> Self {
        self.add_join(JoinKind::Right, table.into(), None, build)
    }

    /// `INNER JOIN (SELECT ...) alias ON first op second`, like Laravel's `joinSub`.
    pub fn join_sub(
        self,
        query: Self,
        alias: impl Into<String>,
        first: impl Into<String>,
        op: &str,
        second: impl Into<String>,
    ) -> Self {
        self.add_join(JoinKind::Inner, alias.into(), Some(query), |j| {
            j.on(first, op, second)
        })
    }

    /// `LEFT JOIN (SELECT ...) alias ON first op second`, like Laravel's `leftJoinSub`.
    pub fn left_join_sub(
        self,
        query: Self,
        alias: impl Into<String>,
        first: impl Into<String>,
        op: &str,
        second: impl Into<String>,
    ) -> Self {
        self.add_join(JoinKind::Left, alias.into(), Some(query), |j| {
            j.on(first, op, second)
        })
    }
}

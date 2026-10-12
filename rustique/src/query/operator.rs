//! Comparison operators accepted by `where_`, `having` and join `on`.

/// The allow-list of operators. Only these reach the SQL text, so a
/// user-supplied operator string can never inject SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operator {
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    NotLike,
    Is,
    IsNot,
}

impl Operator {
    /// Parses an operator, ignoring case and surrounding whitespace.
    pub fn parse(op: &str) -> Option<Self> {
        let op = op.trim();
        let parsed = match op {
            "=" => Self::Eq,
            "<>" | "!=" => Self::NotEq,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            _ if op.eq_ignore_ascii_case("like") => Self::Like,
            _ if op.eq_ignore_ascii_case("not like") => Self::NotLike,
            _ if op.eq_ignore_ascii_case("is") => Self::Is,
            _ if op.eq_ignore_ascii_case("is not") => Self::IsNot,
            _ => return None,
        };
        Some(parsed)
    }

    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::NotEq => "<>",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Like => "LIKE",
            Self::NotLike => "NOT LIKE",
            Self::Is => "IS",
            Self::IsNot => "IS NOT",
        }
    }
}

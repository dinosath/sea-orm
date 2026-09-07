//! Strongly typed result types for formula expressions.
//!
//! The formula subsystem is *typed*: every node in a formula tree carries a
//! [`FormulaType`] describing the SQL type of the value it produces. Arithmetic
//! and comparison operators are checked against these types so that, for
//! example, multiplying a numeric column by a string column is rejected at
//! build/compile time rather than producing incorrect SQL.

use crate::sea_query::ColumnType;

/// The SQL value type produced by a formula expression.
///
/// This intentionally mirrors the value categories SeaORM understands instead
/// of a specific database type so that a formula can be compiled to any
/// backend. `Decimal` is used for exact monetary arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FormulaType {
    /// SQL `BOOLEAN`.
    Boolean,
    /// Exact integer (`INT`, `BIGINT`, ...).
    Integer,
    /// Exact fixed point decimal (`NUMERIC`, `DECIMAL`, `MONEY`). Preferred for
    /// monetary values.
    Decimal,
    /// Approximate floating point (`FLOAT`, `REAL`, `DOUBLE`).
    Float,
    /// Character data (`VARCHAR`, `TEXT`, `CHAR`).
    String,
    /// SQL `DATE`.
    Date,
    /// SQL timestamp / datetime.
    DateTime,
    /// UUID / GUID.
    Uuid,
    /// JSON value.
    Json,
    /// An unknown or untyped value (e.g. a raw `NULL` literal before coercion).
    Null,
}

impl FormulaType {
    /// Returns true if this is an arithmetic (numeric) type.
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            FormulaType::Integer | FormulaType::Decimal | FormulaType::Float
        )
    }

    /// The common type promoted when two numeric operands are combined.
    /// Follows the rule "if either side is `Decimal`, the result is `Decimal`".
    pub fn promote_numeric(a: FormulaType, b: FormulaType) -> Option<FormulaType> {
        match (a, b) {
            (FormulaType::Integer, FormulaType::Integer) => Some(FormulaType::Integer),
            (FormulaType::Decimal, _) | (_, FormulaType::Decimal) => Some(FormulaType::Decimal),
            (FormulaType::Float, _) | (_, FormulaType::Float) => Some(FormulaType::Float),
            _ => None,
        }
    }

    /// Map a raw SeaQuery column type onto the closest [`FormulaType`].
    pub fn from_column_type(ty: &ColumnType) -> FormulaType {
        use ColumnType::*;
        match ty {
            TinyInteger | SmallInteger | Integer | BigInteger | TinyUnsigned | SmallUnsigned
            | Unsigned | BigUnsigned | Year => FormulaType::Integer,
            Decimal(_) | Money(_) => FormulaType::Decimal,
            Float | Double => FormulaType::Float,
            Boolean => FormulaType::Boolean,
            Char(_) | String(_) | Text | Enum { .. } => FormulaType::String,
            Date => FormulaType::Date,
            DateTime | Timestamp | TimestampWithTimeZone | Time => FormulaType::DateTime,
            Uuid => FormulaType::Uuid,
            Json | JsonBinary | Array(_) | Vector(_) => FormulaType::Json,
            Custom(_) | Bit(_) | VarBit(_) | Binary(_) | VarBinary(_) | Blob | Interval(..)
            | Cidr | Inet | MacAddr | LTree => FormulaType::String,
            _ => FormulaType::String,
        }
    }
}

impl std::fmt::Display for FormulaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            FormulaType::Boolean => "bool",
            FormulaType::Integer => "integer",
            FormulaType::Decimal => "decimal",
            FormulaType::Float => "float",
            FormulaType::String => "string",
            FormulaType::Date => "date",
            FormulaType::DateTime => "datetime",
            FormulaType::Uuid => "uuid",
            FormulaType::Json => "json",
            FormulaType::Null => "null",
        };
        write!(f, "{name}")
    }
}

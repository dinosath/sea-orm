//! The typed, backend-agnostic Formula AST.
//!
//! A formula is a tree of [`Node`]s. The leaves reference real SeaORM columns
//! (through their generated `Column` types) and the internal nodes are
//! operators, function calls, CASE expressions, relationship hops and
//! correlated aggregates. Every node knows the *value type* it produces so the
//! whole tree can be type-checked before it is lowered to SQL.
//!
//! The AST deliberately contains **no raw SQL**. Lowering happens in the
//! [`crate::formula::compile`] module which validates the tree and turns it
//! into a `sea_query` expression that a backend can render.
#![allow(clippy::should_implement_trait)]

use crate::{
    ColumnTrait, RelationDef, RelationTrait,
    sea_query::{DynIden, Value},
};

use super::{FormulaType, Result, error::FormulaError};

/// A correlated relationship hop between two tables.
///
/// `source` is the table of the row we currently have (the outer/current row),
/// `target` is the related table we traverse into. Correlation is expressed as
/// a set of column equalities `target.col == source.col`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hop {
    /// Table of the current/outer row.
    pub source: DynIden,
    /// Join columns on the source (outer) table.
    pub source_cols: Vec<DynIden>,
    /// Related table we traverse into.
    pub target: DynIden,
    /// Join columns on the target table.
    pub target_cols: Vec<DynIden>,
}

impl Hop {
    /// Build a [`Hop`] from a SeaORM [`RelationDef`].
    ///
    /// The relation must be a variant of the **current** entity's `Relation`
    /// enum so that the "from" side of the relation corresponds to the row we
    /// currently hold.
    pub fn from_relation_def(def: &RelationDef) -> Hop {
        let source = table_of(&def.from_tbl);
        let target = table_of(&def.to_tbl);
        Hop {
            source,
            source_cols: def.from_col.clone().into_iter().collect(),
            target,
            target_cols: def.to_col.clone().into_iter().collect(),
        }
    }
}

fn table_of(tbl: &crate::sea_query::TableRef) -> DynIden {
    match tbl.sea_orm_table_alias() {
        Some(alias) => alias.clone(),
        None => tbl.sea_orm_table().clone(),
    }
}

/// Aggregate function applied over a related collection.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AggregateFunction {
    /// `SUM`
    Sum,
    /// `AVG`
    Avg,
    /// `MIN`
    Min,
    /// `MAX`
    Max,
    /// `COUNT`
    Count,
}

impl std::fmt::Display for AggregateFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AggregateFunction::Sum => "SUM",
            AggregateFunction::Avg => "AVG",
            AggregateFunction::Min => "MIN",
            AggregateFunction::Max => "MAX",
            AggregateFunction::Count => "COUNT",
        };
        write!(f, "{s}")
    }
}

/// Binary operators supported by the formula language.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

/// The internal node kind.
#[derive(Debug, Clone)]
pub(crate) enum NodeKind {
    /// Reference to a column of the row in the current scope.
    Column {
        table: DynIden,
        column: DynIden,
        ty: FormulaType,
        nullable: bool,
    },
    /// A literal value.
    Literal { value: Value, ty: FormulaType },
    /// Unary operator.
    Unary { op: UnaryOperator, operand: Node },
    /// Binary operator.
    Binary {
        op: BinaryOperator,
        left: Node,
        right: Node,
    },
    /// `CASE WHEN ... THEN ... ELSE ... END`.
    Case {
        arms: Vec<(Node, Node)>,
        r#else: Box<Node>,
    },
    /// Function call (coalesce / nullif / abs / lower / upper / length ...).
    Function { fun: Function, args: Vec<Node> },
    /// Correlation into a single related row (belongs_to / has_one).
    One { hop: Hop, inner: Box<Node> },
    /// Correlated aggregate over a related collection.
    Aggregate {
        hop: Hop,
        fun: AggregateFunction,
        arg: Box<Node>,
        filter: Option<Box<Node>>,
    },
    /// `IS NULL` / `IS NOT NULL`.
    IsNull { operand: Box<Node>, negate: bool },
    /// Explicit cast to a target [`FormulaType`].
    Cast {
        operand: Box<Node>,
        target: FormulaType,
    },
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum UnaryOperator {
    Neg,
    Not,
}

/// Named scalar functions available in the formula language.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Function {
    Coalesce,
    NullIf,
    Abs,
    Lower,
    Upper,
    Length,
    Round,
    Floor,
    Ceil,
    Concat,
    /// SQL `CURRENT_DATE` / `TODAY()`.
    CurrentDate,
}

/// A type-safe formula expression.
#[derive(Debug, Clone)]
pub struct Node {
    pub(crate) kind: Box<NodeKind>,
}

impl Node {
    pub(crate) fn new(kind: NodeKind) -> Self {
        Self {
            kind: Box::new(kind),
        }
    }

    // ---- Leaf constructors -------------------------------------------------

    /// Reference a column of an entity. The column carries its owning table and
    /// its SQL type, so both correlation and type-checking are inferred.
    pub fn column<C>(col: C) -> Self
    where
        C: ColumnTrait,
    {
        let (table, column) = col.as_column_ref();
        let def = col.def();
        let ty = FormulaType::from_column_type(def.get_column_type());
        let nullable = def.is_null();
        Node::new(NodeKind::Column {
            table,
            column,
            ty,
            nullable,
        })
    }

    /// Integer literal.
    pub fn int(v: i64) -> Self {
        Node::new(NodeKind::Literal {
            value: Value::BigInt(Some(v)),
            ty: FormulaType::Integer,
        })
    }

    /// Integer literal (i32 convenience).
    pub fn i32(v: i32) -> Self {
        Self::int(v as i64)
    }

    /// Boolean literal.
    pub fn boolean(v: bool) -> Self {
        Node::new(NodeKind::Literal {
            value: Value::Bool(Some(v)),
            ty: FormulaType::Boolean,
        })
    }

    /// String literal.
    pub fn str(v: impl Into<String>) -> Self {
        Node::new(NodeKind::Literal {
            value: Value::String(Some(v.into())),
            ty: FormulaType::String,
        })
    }

    /// Exact decimal literal (available when `with-rust_decimal` is enabled,
    /// which it is by default).
    #[cfg(feature = "with-rust_decimal")]
    pub fn decimal(v: rust_decimal::Decimal) -> Self {
        Node::new(NodeKind::Literal {
            value: Value::Decimal(Some(v)),
            ty: FormulaType::Decimal,
        })
    }

    /// Exact decimal literal parsed from its decimal representation.
    #[cfg(feature = "with-rust_decimal")]
    pub fn decimal_str(v: &str) -> Result<Self> {
        v.parse::<rust_decimal::Decimal>()
            .map(Self::decimal)
            .map_err(|_| FormulaError::new(format!("invalid decimal literal: `{v}`")))
    }

    /// A SQL `NULL` literal.
    pub fn null() -> Self {
        Node::new(NodeKind::Literal {
            value: Value::String(None),
            ty: FormulaType::Null,
        })
    }

    // ---- Relationship hops -------------------------------------------------

    /// Correlate into the single related row of `rel` and evaluate `inner`
    /// against that row. Used for `belongs_to` / `has_one` traversal such as
    /// `quantity * product.cost`.
    pub fn related<R>(rel: R, inner: Node) -> Self
    where
        R: RelationTrait,
    {
        let hop = Hop::from_relation_def(&rel.def());
        Node::new(NodeKind::One {
            hop,
            inner: Box::new(inner),
        })
    }

    /// Correlate into the collection of `rel` and aggregate `arg`.
    pub fn aggregate<R>(rel: R, fun: AggregateFunction, arg: Node) -> Self
    where
        R: RelationTrait,
    {
        Self::aggregate_filtered(rel, fun, arg, None)
    }

    /// Correlate into the collection of `rel`, aggregate `arg`, and keep only
    /// the related rows satisfying the boolean `filter`.
    pub fn aggregate_filtered<R>(
        rel: R,
        fun: AggregateFunction,
        arg: Node,
        filter: Option<Node>,
    ) -> Self
    where
        R: RelationTrait,
    {
        let hop = Hop::from_relation_def(&rel.def());
        Node::new(NodeKind::Aggregate {
            hop,
            fun,
            arg: Box::new(arg),
            filter: filter.map(Box::new),
        })
    }

    // ---- Coalesce / NullIf ------------------------------------------------

    /// `COALESCE(...)` returning the first non-NULL argument.
    pub fn coalesce<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = Node>,
    {
        let args: Vec<Node> = args.into_iter().collect();
        if args.len() < 2 {
            return Err(FormulaError::new(
                "COALESCE requires at least two arguments",
            ));
        }
        Ok(Node::new(NodeKind::Function {
            fun: Function::Coalesce,
            args,
        }))
    }

    /// `NULLIF(a, b)` returning `NULL` when `a = b`, otherwise `a`.
    pub fn nullif(a: Node, b: Node) -> Self {
        Node::new(NodeKind::Function {
            fun: Function::NullIf,
            args: vec![a, b],
        })
    }

    /// The current date (`CURRENT_DATE` / `TODAY()`), used e.g. for `overdue`
    /// conditions such as `due_date < today()`.
    pub fn today() -> Self {
        Node::new(NodeKind::Function {
            fun: Function::CurrentDate,
            args: Vec::new(),
        })
    }

    // ---- CASE --------------------------------------------------------------

    /// Build a `CASE WHEN cond THEN value ... ELSE value END` expression.
    ///
    /// `arms` is a list of `(condition, value)`. The final element of `arms`
    /// (or a separate `else_`) provides the `ELSE` branch.
    pub fn case<I>(arms: I, r#else: Node) -> Self
    where
        I: IntoIterator<Item = (Node, Node)>,
    {
        Node::new(NodeKind::Case {
            arms: arms.into_iter().collect(),
            r#else: Box::new(r#else),
        })
    }

    /// A conditional expression `IF cond THEN value ELSE otherwise` expressed
    /// as a `CASE WHEN ... END`.
    pub fn if_then_else(cond: Node, value: Node, otherwise: Node) -> Self {
        Self::case([(cond, value)], otherwise)
    }

    // ---- IS NULL / IS NOT NULL --------------------------------------------

    /// `operand IS NULL` (boolean result).
    pub fn is_null(self) -> Self {
        Node::new(NodeKind::IsNull {
            operand: Box::new(self),
            negate: false,
        })
    }

    /// `operand IS NOT NULL` (boolean result).
    pub fn is_not_null(self) -> Self {
        Node::new(NodeKind::IsNull {
            operand: Box::new(self),
            negate: true,
        })
    }

    /// Cast this expression to an explicit target [`FormulaType`].
    pub fn cast(self, target: FormulaType) -> Self {
        Node::new(NodeKind::Cast {
            operand: Box::new(self),
            target,
        })
    }

    // ---- Unary -------------------------------------------------------------

    /// Arithmetic negation.
    pub fn neg(self) -> Self {
        Node::new(NodeKind::Unary {
            op: UnaryOperator::Neg,
            operand: self,
        })
    }

    /// Logical negation (`NOT`).
    pub fn not(self) -> Self {
        Node::new(NodeKind::Unary {
            op: UnaryOperator::Not,
            operand: self,
        })
    }

    // ---- Binary ------------------------------------------------------------

    fn binary(op: BinaryOperator, left: Node, right: Node) -> Self {
        Node::new(NodeKind::Binary { op, left, right })
    }

    /// `left + right`
    pub fn add(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Add, self, right)
    }

    /// `left - right`
    pub fn sub(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Sub, self, right)
    }

    /// `left * right`
    pub fn mul(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Mul, self, right)
    }

    /// `left / right`
    pub fn div(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Div, self, right)
    }

    /// `left % right`
    pub fn rem(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Rem, self, right)
    }

    /// `left = right` (boolean)
    pub fn eq(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Eq, self, right)
    }

    /// `left <> right` (boolean)
    pub fn neq(self, right: Node) -> Self {
        Node::binary(BinaryOperator::NotEq, self, right)
    }

    /// `left < right` (boolean)
    pub fn lt(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Lt, self, right)
    }

    /// `left <= right` (boolean)
    pub fn lte(self, right: Node) -> Self {
        Node::binary(BinaryOperator::LtEq, self, right)
    }

    /// `left > right` (boolean)
    pub fn gt(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Gt, self, right)
    }

    /// `left >= right` (boolean)
    pub fn gte(self, right: Node) -> Self {
        Node::binary(BinaryOperator::GtEq, self, right)
    }

    /// `left AND right` (boolean)
    pub fn and(self, right: Node) -> Self {
        Node::binary(BinaryOperator::And, self, right)
    }

    /// `left OR right` (boolean)
    pub fn or(self, right: Node) -> Self {
        Node::binary(BinaryOperator::Or, self, right)
    }

    /// `lower(str)`
    pub fn lower(self) -> Self {
        Node::new(NodeKind::Function {
            fun: Function::Lower,
            args: vec![self],
        })
    }

    /// `upper(str)`
    pub fn upper(self) -> Self {
        Node::new(NodeKind::Function {
            fun: Function::Upper,
            args: vec![self],
        })
    }

    /// `abs(n)` — absolute value of a numeric expression.
    pub fn abs(self) -> Self {
        Node::new(NodeKind::Function {
            fun: Function::Abs,
            args: vec![self],
        })
    }

    /// `length(str)` — character length.
    pub fn length(self) -> Self {
        Node::new(NodeKind::Function {
            fun: Function::Length,
            args: vec![self],
        })
    }

    // ---- Introspection -----------------------------------------------------

    /// Lower-level access to the node kind (used by the compiler).
    pub(crate) fn kind(&self) -> &NodeKind {
        &self.kind
    }
}

// Convenient numeric literal constructors used a lot in tests/docs.
impl From<i32> for Node {
    fn from(v: i32) -> Self {
        Node::i32(v)
    }
}

impl From<i64> for Node {
    fn from(v: i64) -> Self {
        Node::int(v)
    }
}

impl From<&str> for Node {
    fn from(v: &str) -> Self {
        Node::str(v)
    }
}

impl From<String> for Node {
    fn from(v: String) -> Self {
        Node::str(v)
    }
}

impl From<bool> for Node {
    fn from(v: bool) -> Self {
        Node::boolean(v)
    }
}

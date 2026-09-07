//! A first-class, typed, computed-field / formula capability for SeaORM.
//!
//! This module lets you express **query-time formulas** as typed, backend
//! agnostic expressions that compile down to SeaQuery SQL — no user supplied
//! SQL strings are ever accepted.
//!
//! It supports:
//!
//! * scalar computed expressions (`quantity * unit_price - discount`)
//! * arithmetic, comparison, boolean and conditional expressions
//! * NULL handling (`COALESCE`, `NULLIF`, `IS NULL`, `IS NOT NULL`)
//! * aggregates (`SUM`, `AVG`, `MIN`, `MAX`, `COUNT`)
//! * relationship traversal (`product.cost`, `customer.sales`)
//! * correlated relationship aggregates (`SUM(sales.total)`) and filtered
//!   aggregates (`SUM(sales.balance WHERE sales.status = 'OPEN')`)
//! * nested / dependent formulas
//! * strongly typed results and a public, dynamic-friendly AST
//!
//! # Example
//!
//! ```rust,ignore
//! use sea_orm::formula::*;
//! use sea_orm::entity::prelude::*;
//!
//! // net_price = quantity * unit_price - COALESCE(discount, 0)
//! let net_price = Node::column(sale_line::Column::Quantity)
//!     .mul(Node::column(sale_line::Column::UnitPrice))
//!     .sub(Node::coalesce([
//!         Node::column(sale_line::Column::Discount),
//!         Node::int(0),
//!     ])?);
//! ```
//!
//! See the integration examples and documentation crate docs for the complete
//! ERP walkthrough.

mod compile;
mod error;
mod node;
mod select;
mod ty;

pub use error::{FormulaError, Result};
pub use node::{AggregateFunction, BinaryOperator, Function, Hop, Node};
pub(crate) use node::{NodeKind, UnaryOperator};
pub use select::FormulaExt;
pub use ty::FormulaType;

#[cfg(test)]
mod tests;

/// Convenient prelude for the formula subsystem.
pub mod prelude {
    pub use super::{
        AggregateFunction, BinaryOperator, FormulaError, FormulaExt, FormulaType, Function, Hop,
        Node,
    };
}

/// Compile a formula [`Node`] into a SeaQuery expression valid in the scope of
/// the table named by `root`.
///
/// This is the entry point used by higher-level systems (e.g. Ferris CMS) that
/// want to translate a formula descriptor into a SeaORM/SeaQuery expression.
pub fn compile_to_expr(
    node: &Node,
    root: crate::sea_query::DynIden,
) -> Result<crate::sea_query::Expr> {
    compile::compile(node, &root)
}

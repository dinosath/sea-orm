//! Integration of formulas into SeaORM's query builders.
//!
//! [`FormulaExt`] adds `formula`, `filter_formula` and `order_by_formula`
//! helpers to `Select<E>`. The root table for correlation is taken from the
//! entity being queried, so no manual correlation is ever required.

use crate::{
    EntityTrait, IntoIdentity, Order, QueryFilter, QueryOrder, QuerySelect, Select,
    sea_query::DynIden,
};

use super::{Node, Result, compile};

/// Extension trait adding formula projection / filter / ordering to a
/// [`Select`] query.
pub trait FormulaExt: Sized {
    /// Project a formula as an additional (aliased) select column.
    ///
    /// ```rust,ignore
    /// Customer::find().formula(&total_revenue, "total_revenue")?
    /// ```
    fn formula<A>(self, node: &Node, alias: A) -> Result<Self>
    where
        A: IntoIdentity;

    /// Add a WHERE predicate built from a boolean formula. This is used to
    /// filter on a computed value, e.g. `total_revenue > 1000`.
    fn filter_formula(self, node: &Node) -> Result<Self>;

    /// Order the query by a formula expression.
    fn order_by_formula(self, node: &Node, ord: Order) -> Result<Self>;
}

impl<E> FormulaExt for Select<E>
where
    E: EntityTrait,
{
    fn formula<A>(self, node: &Node, alias: A) -> Result<Self>
    where
        A: IntoIdentity,
    {
        let expr = compile::compile(node, &root_table::<E>())?;
        Ok(QuerySelect::expr_as(self, expr, alias))
    }

    fn filter_formula(self, node: &Node) -> Result<Self> {
        let expr = compile::compile(node, &root_table::<E>())?;
        Ok(QueryFilter::filter(self, expr))
    }

    fn order_by_formula(self, node: &Node, ord: Order) -> Result<Self> {
        let expr = compile::compile(node, &root_table::<E>())?;
        Ok(QueryOrder::order_by(self, expr, ord))
    }
}

/// The table name of the entity being queried (the root scope of a formula).
fn root_table<E: EntityTrait>() -> DynIden {
    DynIden::from(E::default())
}

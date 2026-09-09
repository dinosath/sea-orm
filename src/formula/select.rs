//! Integration of formulas into SeaORM's query builders.
//!
//! [`FormulaExt`] adds `formula`, `filter_formula` and `order_by_formula`
//! helpers to `Select<E>`. The root table for correlation is taken from the
//! entity being queried, so no manual correlation is ever required.

use crate::{
    EntityTrait, IntoIdentity, Order, QueryFilter, QueryOrder, QuerySelect, Select,
    sea_query::{DynIden, Expr},
};

use super::{Node, Result, compile};

/// A computed field (formula) attached to an entity and auto-included in every
/// `SELECT` of that entity (i.e. `Entity::find()` / `Entity::find_by_id()`),
/// mirroring Hibernate's `@Formula`.
///
/// The [`Expr`] is compiled (and type-checked) eagerly against the entity's own
/// table when the field is declared, so projecting it later is infallible. The
/// alias is the column name the computed value appears under in the result row,
/// which is what partial models / `FromQueryResult` consumers read it back by.
#[derive(Debug)]
pub struct ComputedField {
    /// Column alias under which the computed value is selected.
    pub(crate) alias: String,
    /// Already-validated, already-lowered expression ready to project.
    pub(crate) expr: Expr,
}

impl ComputedField {
    /// Compile `node` as a computed field of entity `E`, returning the value
    /// under the result-column `alias`.
    ///
    /// Validation runs here (reachability of every referenced column from the
    /// root table of `E`, and type-correctness of every operator), so an
    /// invalid declaration fails fast instead of at query time.
    ///
    /// ```rust,ignore
    /// ComputedField::for_entity::<Customer>(&outstanding, "outstanding")?
    /// ```
    pub fn for_entity<E>(alias: impl Into<String>, node: &Node) -> Result<Self>
    where
        E: EntityTrait,
    {
        let root = root_table::<E>();
        // Validate (and type-check) the formula before lowering it.
        compile::type_of(node, &root)?;
        let expr = compile::compile(node, &root)?;
        Ok(Self {
            alias: alias.into(),
            expr,
        })
    }
}

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

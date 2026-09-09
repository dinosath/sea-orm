use crate as sea_orm;
use sea_orm::entity::prelude::*;
use sea_orm::formula::{ComputedField, Node};

/// A tiny self-contained entity used to demonstrate auto-included computed
/// fields (formulas): every `Entity::find()` / `Entity::find_by_id()` projects
/// `value * 2` as the `value_doubled` column automatically.
///
/// This mirrors how a real entity would opt in:
///
/// ```rust,ignore
/// #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
/// #[sea_orm(table_name = "double_row", computed_fields = "computed_fields")]
/// pub struct Model { /* ... */ }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "double_row", computed_fields = "computed_fields")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub value: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// The computed fields auto-included on every `SELECT` of [`Entity`].
fn computed_fields() -> Vec<ComputedField> {
    let doubled = Node::column(Column::Value).mul(Node::int(2));
    vec![ComputedField::for_entity::<Entity>("value_doubled", &doubled).unwrap()]
}

use crate as sea_orm;
use sea_orm::entity::prelude::*;
use sea_orm::formula::{ComputedField, Node};

/// A dense `#[sea_orm::model]` (ModelEx) entity used to verify auto-included
/// computed fields (formulas) on the dense entity path.
///
/// Unlike the classic `#[derive(DeriveEntityModel)]` form, dense entities emit
/// their `Entity`/`EntityTrait` through the `ModelEx` machinery, which builds the
/// `Entity` with multiple `#[sea_orm(...)]` attributes. Auto-included computed
/// fields must therefore be carried in the same single attribute as the ModelEx
/// wiring.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "dense_row", computed_fields = "computed_fields")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub value: i32,
}

impl ActiveModelBehavior for ActiveModel {}

/// The computed field auto-included on every `SELECT` of [`Entity`].
fn computed_fields() -> Vec<ComputedField> {
    let doubled = Node::column(Column::Value).mul(Node::int(3));
    vec![ComputedField::for_entity::<Entity>("value_tripled", &doubled).unwrap()]
}

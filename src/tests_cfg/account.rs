use crate as sea_orm;
use sea_orm::entity::prelude::*;
use sea_orm::formula::{AggregateFunction, ComputedField, Node};

/// A dense `#[sea_orm::model]` (ModelEx) parent whose `has_many` `account_tx`
/// rows feed an auto-included correlated aggregate computed field. This mirrors
/// the ERP `Customer.total_revenue = SUM(sales.total)` shape.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "account", computed_fields = "computed_fields")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    #[sea_orm(has_many)]
    pub txs: HasMany<super::account_tx::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

/// total_amount = SUM(account_tx.amount) correlated per account.
fn computed_fields() -> Vec<ComputedField> {
    let total = Node::aggregate(
        Relation::AccountTx,
        AggregateFunction::Sum,
        Node::column(super::account_tx::Column::Amount),
    );
    vec![ComputedField::for_entity::<Entity>("total_amount", &total).unwrap()]
}

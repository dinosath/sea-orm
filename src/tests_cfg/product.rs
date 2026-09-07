use crate as sea_orm;
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "product")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    /// Unit cost of the product in the base currency (exact decimal).
    pub cost: Decimal,
}

impl ActiveModelBehavior for ActiveModel {}

use crate as sea_orm;
use sea_orm::entity::prelude::*;

/// A dense `#[sea_orm::model]` child of [`super::account::Entity`], holding an
/// amount that the parent aggregates with an auto-included computed field.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "account_tx")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub account_id: i32,
    pub amount: i32,
    #[sea_orm(belongs_to, from = "account_id", to = "id")]
    pub account: BelongsTo<super::account::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

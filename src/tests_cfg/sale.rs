use crate as sea_orm;
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sale")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub customer_id: i32,
    pub order_id: i32,
    pub status: String,
    /// Remaining balance on the sale. Money (exact decimal).
    pub balance: Decimal,
    pub due_date: Date,
    #[sea_orm(belongs_to, from = "customer_id", to = "id")]
    pub customer: BelongsTo<super::customer::Entity>,
    #[sea_orm(belongs_to, from = "order_id", to = "id")]
    pub order: BelongsTo<super::order::Entity>,
    #[sea_orm(has_many)]
    pub lines: HasMany<super::sale_line::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

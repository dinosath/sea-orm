use crate as sea_orm;
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "order")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub customer_id: i32,
    #[sea_orm(belongs_to, from = "customer_id", to = "id")]
    pub customer: BelongsTo<super::customer::Entity>,
    #[sea_orm(has_many)]
    pub sales: HasMany<super::sale::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

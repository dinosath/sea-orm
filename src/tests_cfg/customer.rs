use crate as sea_orm;
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "customer")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    #[sea_orm(has_many)]
    pub sales: HasMany<super::sale::Entity>,
    #[sea_orm(has_many)]
    pub orders: HasMany<super::order::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

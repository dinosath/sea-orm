use sea_orm::entity::prelude::*;
use sea_orm::tenant::{self, TenantContext};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "notes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub title: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

fn main() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let _query = tenant::find::<Entity>(&tenant);
}

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "tenant_post")]
pub struct Model {
    #[sea_orm(primary_key)]
    #[serde(skip_deserializing)]
    pub id: i32,
    #[sea_orm(tenant_key)]
    pub tenant_id: String,
    pub title: String,
    #[sea_orm(column_type = "Text")]
    pub text: String,
}

impl ActiveModelBehavior for ActiveModel {}

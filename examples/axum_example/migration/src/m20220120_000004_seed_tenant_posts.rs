use entity::tenant_post;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        let seed_data = vec![
            ("acme", "Acme Welcome", "Tenant-scoped seed data for Acme."),
            ("globex", "Globex Welcome", "Tenant-scoped seed data for Globex."),
        ];

        for (tenant_id, title, text) in seed_data {
            let model = tenant_post::ActiveModel {
                tenant_id: Set(tenant_id.to_owned()),
                title: Set(title.to_owned()),
                text: Set(text.to_owned()),
                ..Default::default()
            };
            model.insert(db).await?;
        }

        println!("Tenant posts seeded successfully.");
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        tenant_post::Entity::delete_many()
            .filter(tenant_post::Column::TenantId.is_in(["acme", "globex"]))
            .exec(db)
            .await?;

        println!("Tenant post seed data removed.");
        Ok(())
    }
}
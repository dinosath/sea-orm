use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table("tenant_post")
                    .if_not_exists()
                    .col(pk_auto("id"))
                    .col(string("tenant_id"))
                    .col(string("title"))
                    .col(text("text"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table("tenant_post").to_owned())
            .await
    }
}
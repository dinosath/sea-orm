use ::entity::{tenant_post, tenant_post::Entity as TenantPost};
use sea_orm::{
    ActiveValue::Set,
    DbErr,
    tenant::{MultiTenantConfig, TenantContext, TenantScopedExecutor},
};

pub struct TenantPostService;

impl TenantPostService {
    pub async fn list_posts(
        config: &MultiTenantConfig,
        tenant: TenantContext,
    ) -> Result<Vec<tenant_post::Model>, DbErr> {
        config
            .with_row_level(tenant, |_, tenant_db| async move {
                tenant_db.repository::<TenantPost>().all().await
            })
            .await
    }

    pub async fn create_post(
        config: &MultiTenantConfig,
        tenant: TenantContext,
        form_data: tenant_post::Model,
    ) -> Result<tenant_post::Model, DbErr> {
        config
            .with_row_level(tenant, |_, tenant_db| async move {
                tenant_db
                    .repository::<TenantPost>()
                    .insert(tenant_post::ActiveModel {
                        title: Set(form_data.title),
                        text: Set(form_data.text),
                        ..Default::default()
                    })
                    .await
            })
            .await
    }
}

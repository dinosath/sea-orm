use criterion::{Criterion, criterion_group, criterion_main};

#[cfg(feature = "tenant")]
use sea_orm::tenant::{TenantContext, TenantPoolManager, apply_tenant_filter};
#[cfg(feature = "tenant")]
use sea_orm::{DatabaseConnection, DbBackend, QueryTrait, entity::prelude::*};

#[cfg(feature = "tenant")]
mod bench_entity {
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "bench_tenant_item")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        #[sea_orm(tenant_key)]
        pub tenant_id: String,
        pub name: String,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

fn tenant_benches(c: &mut Criterion) {
    #[cfg(feature = "tenant")]
    {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let tenant = TenantContext::new("bench-tenant").expect("tenant context");
        let manager = TenantPoolManager::new(|_| async { Ok(DatabaseConnection::default()) });

        c.bench_function("tenant_pool_lookup", |b| {
            b.iter(|| {
                runtime
                    .block_on(manager.get_pool(&tenant))
                    .expect("pool lookup");
            })
        });

        c.bench_function("tenant_filter_application", |b| {
            b.iter(|| {
                let query = apply_tenant_filter(bench_entity::Entity::find(), &tenant);
                let _statement = query.build(DbBackend::Postgres);
            })
        });
    }

    #[cfg(not(feature = "tenant"))]
    {
        c.bench_function("tenant_feature_disabled", |b| b.iter(|| 0_u8));
    }
}

criterion_group!(benches, tenant_benches);
criterion_main!(benches);

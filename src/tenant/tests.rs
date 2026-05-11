use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::{
    ActiveValue::Set, ConnectionTrait, DbBackend, DbErr, EntityTrait, MockDatabase, QueryFilter,
    QueryTrait, Statement, Transaction,
};

use super::{
    ConfiguredTenantConnection, MultiTenantConfig, RowLevelTenantConnection,
    TenantConnectionProvider, TenantContext, TenantExecutionKind, TenantId, TenantJob,
    TenantMigrationRunner, TenantPoolManager, TenantScoped, TenantScopedExecutor, TenantService,
    apply_tenant_filter,
};

mod test_entity {
    use crate as sea_orm;
    use crate::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "tenant_items")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        #[sea_orm(tenant_key)]
        pub tenant_id: String,
        pub name: String,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

mod typed_tenant_entity {
    use std::{convert::Infallible, str::FromStr};

    use crate as sea_orm;
    use crate::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveValueType)]
    pub struct OrgId(pub String);

    impl FromStr for OrgId {
        type Err = Infallible;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            Ok(Self(s.to_owned()))
        }
    }

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "typed_tenant_items")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        #[sea_orm(tenant_key)]
        pub tenant_id: OrgId,
        pub name: String,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

fn compile_time_tenant_find<E>(tenant: &TenantContext) -> crate::Select<E>
where
    E: TenantScoped,
{
    super::find::<E>(tenant)
}

struct EchoTenantService;

#[async_trait::async_trait]
impl super::TenantService for EchoTenantService {
    type Output = String;
    type Error = DbErr;

    async fn execute(&self, scope: &TenantContext) -> Result<Self::Output, Self::Error> {
        Ok(scope.tenant_id().to_string())
    }
}

#[test]
fn tenant_context_rejects_empty_identifier() {
    let error = TenantContext::new("   ").expect_err("empty tenant id must fail");
    assert_eq!(error.to_string(), "Custom Error: tenant_id cannot be empty");
}

#[test]
fn tenant_id_is_validated_and_displayable() {
    let tenant_id = TenantId::new("acme").expect("tenant id");
    let scope = TenantContext::from_tenant_id(tenant_id.clone());

    assert_eq!(tenant_id, "acme");
    assert_eq!(scope.tenant_id(), &tenant_id);
    assert_eq!(scope.tenant_id_str(), "acme");
    assert_eq!(tenant_id.to_string(), "acme");
}

#[test]
fn multitenant_builder_requires_strategy() {
    let error = MultiTenantConfig::builder()
        .build()
        .expect_err("builder must reject missing strategy");

    assert_eq!(
        error.to_string(),
        "Custom Error: multi-tenancy strategy must be configured before building"
    );
}

#[test]
fn select_filter_adds_tenant_predicate() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let sql = apply_tenant_filter(test_entity::Entity::find(), &tenant)
        .build(DbBackend::Postgres)
        .to_string();

    assert!(sql.contains(r#"WHERE "tenant_items"."tenant_id" = 'acme'"#));
}

#[test]
fn tenant_key_macro_generates_entity_filter_helper() {
    let sql = test_entity::Entity::find()
        .filter(test_entity::Entity::tenant_filter("acme".to_owned()))
        .build(DbBackend::Postgres)
        .to_string();

    assert!(sql.contains(r#"WHERE "tenant_items"."tenant_id" = 'acme'"#));
}

#[test]
fn compile_time_tenant_scoped_find_helper_filters_queries() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let sql = compile_time_tenant_find::<test_entity::Entity>(&tenant)
        .build(DbBackend::Postgres)
        .to_string();

    assert!(sql.contains(r#"WHERE "tenant_items"."tenant_id" = 'acme'"#));
}

#[test]
fn typed_tenant_id_entities_work_with_tenant_scoped_find() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let sql = super::find::<typed_tenant_entity::Entity>(&tenant)
        .build(DbBackend::Postgres)
        .to_string();

    assert!(sql.contains(r#"WHERE "typed_tenant_items"."tenant_id" = 'acme'"#));
}

#[test]
fn row_level_connection_filters_update_and_delete_queries() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let connection =
        RowLevelTenantConnection::row_level(crate::DatabaseConnection::default(), tenant);

    let update_sql = connection
        .filter_update(test_entity::Entity::update_many())
        .build(DbBackend::Postgres)
        .to_string();
    let delete_sql = connection
        .filter_delete(test_entity::Entity::delete_many())
        .build(DbBackend::Postgres)
        .to_string();

    assert!(update_sql.contains(r#"WHERE "tenant_items"."tenant_id" = 'acme'"#));
    assert!(delete_sql.contains(r#"WHERE "tenant_items"."tenant_id" = 'acme'"#));
}

#[test]
fn guarded_row_level_connection_filters_queries() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let guarded =
        RowLevelTenantConnection::row_level(crate::DatabaseConnection::default(), tenant).guarded();

    let sql = guarded
        .filter(test_entity::Entity::find())
        .build(DbBackend::Postgres)
        .to_string();

    assert!(sql.contains(r#"WHERE "tenant_items"."tenant_id" = 'acme'"#));
}

#[test]
fn guarded_row_level_connection_find_by_id_is_tenant_scoped() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let guarded =
        RowLevelTenantConnection::row_level(crate::DatabaseConnection::default(), tenant).guarded();

    let sql = guarded
        .select_by_id::<test_entity::Entity, _>(7)
        .build(DbBackend::Postgres)
        .to_string();

    assert!(
        sql.contains(r#"WHERE "tenant_items"."id" = 7 AND "tenant_items"."tenant_id" = 'acme'"#)
    );
}

#[tokio::test]
async fn tenant_service_receives_explicit_scope() {
    let tenant = TenantContext::new("acme").expect("tenant context");
    let service = EchoTenantService;

    let output = service.execute(&tenant).await.expect("service output");

    assert_eq!(output, "acme");
}

#[tokio::test]
async fn multitenant_builder_resolves_row_level_connection() {
    let config = MultiTenantConfig::builder()
        .row_level(crate::DatabaseConnection::default())
        .build()
        .expect("row-level config");

    let connection = config
        .connection_for(TenantContext::new("acme").expect("tenant context"))
        .await
        .expect("configured connection");

    assert_eq!(connection.tenant().tenant_id(), "acme");
    assert!(matches!(
        connection,
        ConfiguredTenantConnection::RowLevel(_)
    ));
}

#[tokio::test]
async fn multitenant_builder_resolves_database_connection() {
    let calls = Arc::new(AtomicUsize::new(0));
    let config = MultiTenantConfig::builder()
        .database_per_tenant({
            let calls = calls.clone();
            move |_| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(crate::DatabaseConnection::default())
                }
            }
        })
        .build()
        .expect("database config");

    let tenant = TenantContext::new("globex").expect("tenant context");
    let connection = config
        .connection_for(tenant.clone())
        .await
        .expect("configured connection");

    assert_eq!(connection.tenant(), &tenant);
    assert!(matches!(
        connection,
        ConfiguredTenantConnection::Database(_)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn multitenant_builder_resolves_schema_connection() {
    let config = MultiTenantConfig::builder()
        .schema_per_tenant(
            crate::DatabaseConnection::default(),
            |tenant: &TenantContext| Ok(format!("tenant_{}", tenant.tenant_id())),
        )
        .build()
        .expect("schema config");

    let connection = config
        .connection_for(TenantContext::new("initech").expect("tenant context"))
        .await
        .expect("configured connection");

    assert_eq!(connection.tenant().tenant_id(), "initech");
    assert!(matches!(connection, ConfiguredTenantConnection::Schema(_)));
}

#[tokio::test]
async fn tenant_connection_provider_exposes_row_level_executor() {
    let config = MultiTenantConfig::builder()
        .row_level(crate::DatabaseConnection::default())
        .build()
        .expect("row-level config");

    let executor = config
        .executor_for_tenant(TenantContext::new("acme").expect("tenant context"))
        .await
        .expect("tenant executor");

    assert!(executor.is_row_level());
    assert_eq!(executor.kind(), TenantExecutionKind::RowLevel);
    assert_eq!(executor.tenant().tenant_id(), "acme");
}

#[tokio::test]
async fn tenant_scoped_executor_runs_row_level_operations() {
    let config = MultiTenantConfig::builder()
        .row_level(crate::DatabaseConnection::default())
        .build()
        .expect("row-level config");

    let sql = config
        .with_row_level(
            TenantContext::new("acme").expect("tenant context"),
            |scope, guarded| async move {
                assert_eq!(scope.tenant_id(), "acme");
                Ok::<_, DbErr>(
                    guarded
                        .select::<test_entity::Entity>()
                        .build(DbBackend::Postgres)
                        .to_string(),
                )
            },
        )
        .await
        .expect("row-level operation");

    assert!(sql.contains(r#"WHERE "tenant_items"."tenant_id" = 'acme'"#));
}

#[tokio::test]
async fn tenant_scoped_executor_runs_generic_tenant_operations() {
    let config = MultiTenantConfig::builder()
        .row_level(crate::DatabaseConnection::default())
        .build()
        .expect("row-level config");

    let (tenant_id, kind) = config
        .with_tenant(
            TenantContext::new("acme").expect("tenant context"),
            |scope, executor| {
                Box::pin(
                    async move { Ok::<_, DbErr>((scope.tenant_id().to_string(), executor.kind())) },
                )
            },
        )
        .await
        .expect("tenant operation");

    assert_eq!(tenant_id, "acme");
    assert_eq!(kind, TenantExecutionKind::RowLevel);
}

#[tokio::test]
async fn tenant_job_preserves_scope_for_background_work() {
    let job = TenantJob::new(
        TenantContext::new("acme").expect("tenant context"),
        "reindex".to_owned(),
    );

    assert_eq!(job.tenant().tenant_id(), "acme");
    assert_eq!(job.payload(), "reindex");

    let mapped = job.map(|payload| payload.len());
    assert_eq!(mapped.tenant().tenant_id(), "acme");
    assert_eq!(mapped.payload(), &7_usize);
}

#[tokio::test]
async fn schema_connection_sets_search_path_in_transaction() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_exec_results([
            crate::MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            },
            crate::MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            },
        ])
        .into_connection();
    let tenant = TenantContext::new("acme").expect("tenant context");
    let tenant_db =
        super::SchemaTenantConnection::schema(db.clone(), tenant, |tenant: &TenantContext| {
            Ok(format!("tenant_{}", tenant.tenant_id()))
        });

    tenant_db
        .execute_unprepared("SELECT 1")
        .await
        .expect("execute");

    assert_eq!(
        db.into_transaction_log(),
        [Transaction::many([
            Statement::from_string(DbBackend::Postgres, "BEGIN"),
            Statement::from_string(
                DbBackend::Postgres,
                r#"SET LOCAL search_path = "tenant_acme""#
            ),
            Statement::from_string(DbBackend::Postgres, "SELECT 1"),
            Statement::from_string(DbBackend::Postgres, "COMMIT"),
        ])]
    );
}

#[tokio::test]
async fn tenant_migration_runner_uses_stable_schema_session() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_exec_results([
            crate::MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            },
            crate::MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            },
        ])
        .into_connection();
    let config = MultiTenantConfig::builder()
        .schema_per_tenant(db.clone(), |tenant: &TenantContext| {
            Ok(format!("tenant_{}", tenant.tenant_id()))
        })
        .build()
        .expect("schema config");
    let runner = TenantMigrationRunner::new(config);

    runner
        .run(
            TenantContext::new("acme").expect("tenant context"),
            |executor| {
                Box::pin(async move {
                    executor.execute_unprepared("SELECT 1").await?;
                    Ok::<_, DbErr>(())
                })
            },
        )
        .await
        .expect("migration execution");

    assert_eq!(
        db.into_transaction_log(),
        [Transaction::many([
            Statement::from_string(DbBackend::Postgres, "BEGIN"),
            Statement::from_string(
                DbBackend::Postgres,
                r#"SET LOCAL search_path = "tenant_acme""#
            ),
            Statement::from_string(DbBackend::Postgres, "SELECT 1"),
            Statement::from_string(DbBackend::Postgres, "COMMIT"),
        ])]
    );
}

#[tokio::test]
async fn repository_insert_stamps_tenant_id() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([[test_entity::Model {
            id: 7,
            tenant_id: "acme".to_owned(),
            name: "hello".to_owned(),
        }]])
        .into_connection();
    let tenant = TenantContext::new("acme").expect("tenant context");
    let tenant_db = super::RowLevelTenantConnection::row_level(db.clone(), tenant);

    let model = tenant_db
        .repository::<test_entity::Entity>()
        .insert(test_entity::ActiveModel {
            name: Set("hello".to_owned()),
            ..Default::default()
        })
        .await
        .expect("insert result");

    assert_eq!(
        model,
        test_entity::Model {
            id: 7,
            tenant_id: "acme".to_owned(),
            name: "hello".to_owned(),
        }
    );
    assert_eq!(
        db.into_transaction_log(),
        [Transaction::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO "tenant_items" ("tenant_id", "name") VALUES ($1, $2) RETURNING "id", "tenant_id", "name""#,
            ["acme".into(), "hello".into()]
        )]
    );
}

#[tokio::test]
async fn repository_insert_supports_typed_tenant_ids() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([[typed_tenant_entity::Model {
            id: 9,
            tenant_id: typed_tenant_entity::OrgId("acme".to_owned()),
            name: "typed".to_owned(),
        }]])
        .into_connection();
    let tenant = TenantContext::new("acme").expect("tenant context");
    let tenant_db = super::RowLevelTenantConnection::row_level(db.clone(), tenant);

    let model = tenant_db
        .repository::<typed_tenant_entity::Entity>()
        .insert(typed_tenant_entity::ActiveModel {
            name: Set("typed".to_owned()),
            ..Default::default()
        })
        .await
        .expect("typed insert result");

    assert_eq!(
        model,
        typed_tenant_entity::Model {
            id: 9,
            tenant_id: typed_tenant_entity::OrgId("acme".to_owned()),
            name: "typed".to_owned(),
        }
    );
    assert_eq!(
        db.into_transaction_log(),
        [Transaction::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO "typed_tenant_items" ("tenant_id", "name") VALUES ($1, $2) RETURNING "id", "tenant_id", "name""#,
            ["acme".into(), "typed".into()]
        )]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tenant_pool_manager_reuses_pool_across_concurrent_lookups() {
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = Arc::new(TenantPoolManager::new({
        let calls = calls.clone();
        move |_| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(crate::DatabaseConnection::default())
            }
        }
    }));

    let tenant = TenantContext::new("shared").expect("tenant context");
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let manager = manager.clone();
        let tenant = tenant.clone();
        tasks.push(tokio::spawn(async move {
            manager.get_pool(&tenant).await.expect("pool lookup")
        }));
    }

    for task in tasks {
        task.await.expect("join handle");
    }

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(manager.pool_count(), 1);
}

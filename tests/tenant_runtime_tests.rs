#![cfg(all(feature = "tenant", feature = "mock"))]

use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, MockDatabase, QueryTrait,
    Statement, Transaction,
    tenant::{
        MultiTenantConfig, TenantContext, TenantExecutionKind, TenantId, TenantJob,
        TenantMigrationRunner, TenantScopedExecutor,
    },
};

mod tenant_note {
    use sea_orm;
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "tenant_note")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        #[sea_orm(tenant_key)]
        pub tenant_id: String,
        pub title: String,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

#[tokio::test]
async fn row_level_queries_are_isolated_per_tenant() {
    let config = MultiTenantConfig::builder()
        .row_level(DatabaseConnection::default())
        .build()
        .expect("row-level config");

    let acme_sql = config
        .with_row_level(
            TenantContext::new("acme").expect("tenant context"),
            |_, guarded| async move {
                Ok::<_, DbErr>(
                    guarded
                        .select::<tenant_note::Entity>()
                        .build(DbBackend::Postgres)
                        .to_string(),
                )
            },
        )
        .await
        .expect("acme query");

    let globex_sql = config
        .with_row_level(
            TenantContext::new("globex").expect("tenant context"),
            |_, guarded| async move {
                Ok::<_, DbErr>(
                    guarded
                        .select::<tenant_note::Entity>()
                        .build(DbBackend::Postgres)
                        .to_string(),
                )
            },
        )
        .await
        .expect("globex query");

    assert!(acme_sql.contains(r#"WHERE "tenant_note"."tenant_id" = 'acme'"#));
    assert!(globex_sql.contains(r#"WHERE "tenant_note"."tenant_id" = 'globex'"#));
    assert_ne!(acme_sql, globex_sql);
}

#[tokio::test]
async fn guarded_row_level_find_executes_with_automatic_tenant_filter() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_query_results([[tenant_note::Model {
            id: 1,
            tenant_id: "acme".to_owned(),
            title: "hello".to_owned(),
        }]])
        .into_connection();
    let tenant_db = sea_orm::tenant::RowLevelTenantConnection::row_level(
        db.clone(),
        TenantContext::new("acme").expect("tenant context"),
    )
    .guarded();

    let notes = tenant_db
        .find(tenant_note::Entity::find())
        .await
        .expect("tenant-scoped query");

    assert_eq!(
        notes,
        vec![tenant_note::Model {
            id: 1,
            tenant_id: "acme".to_owned(),
            title: "hello".to_owned(),
        }]
    );
    assert_eq!(
        db.into_transaction_log(),
        [Transaction::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT "tenant_note"."id", "tenant_note"."tenant_id", "tenant_note"."title" FROM "tenant_note" WHERE "tenant_note"."tenant_id" = $1"#,
            ["acme".into()]
        )],
    );
}

#[tokio::test]
async fn tenant_jobs_flow_through_executor_helpers() {
    let config = MultiTenantConfig::builder()
        .row_level(DatabaseConnection::default())
        .build()
        .expect("row-level config");
    let job = TenantJob::new(
        TenantContext::new("acme").expect("tenant context"),
        "refresh-cache".to_owned(),
    );

    let (tenant_id, kind, extracted_id) = config
        .with_tenant(job.tenant().clone(), |scope, executor| {
            Box::pin(async move {
                Ok::<_, DbErr>((
                    scope.tenant_id().to_string(),
                    executor.kind(),
                    TenantId::new("acme").expect("tenant id"),
                ))
            })
        })
        .await
        .expect("tenant execution");

    assert_eq!(tenant_id, "acme");
    assert_eq!(kind, TenantExecutionKind::RowLevel);
    assert_eq!(extracted_id, "acme");
}

#[tokio::test]
async fn per_tenant_migration_runner_keeps_schema_session_stable() {
    let db = MockDatabase::new(DbBackend::Postgres)
        .append_exec_results([
            sea_orm::MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            },
            sea_orm::MockExecResult {
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
        .expect("tenant migration");

    assert_eq!(
        db.into_transaction_log(),
        [Transaction::many([
            Statement::from_string(DbBackend::Postgres, "BEGIN"),
            Statement::from_string(
                DbBackend::Postgres,
                r#"SET LOCAL search_path = "tenant_acme""#,
            ),
            Statement::from_string(DbBackend::Postgres, "SELECT 1"),
            Statement::from_string(DbBackend::Postgres, "COMMIT"),
        ])],
    );
}

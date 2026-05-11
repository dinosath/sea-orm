use std::{fmt, future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;

use crate::{
    ConnectOptions, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, ExecResult, QueryResult,
    Statement,
};

use super::{
    DatabaseTenantConnection, GuardedRowLevelTenantConnection, RowLevelTenantConnection,
    SchemaPerTenant, SchemaTenantConnection, TenantContext, TenantPoolManager, TenantSchemaMapper,
    TenantSession,
};

type TenantConnectionFactory = dyn Fn(TenantContext) -> Pin<Box<dyn Future<Output = Result<DatabaseConnection, DbErr>> + Send>>
    + Send
    + Sync;

/// Runtime multitenancy configuration with strategy-aware connection resolution.
pub struct MultiTenantConfig {
    strategy: MultiTenantStrategy,
}

impl fmt::Debug for MultiTenantConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let strategy = match &self.strategy {
            MultiTenantStrategy::Database { .. } => "database-per-tenant",
            MultiTenantStrategy::Schema { .. } => "schema-per-tenant",
            MultiTenantStrategy::RowLevel { .. } => "row-level",
        };

        let mut debug = f.debug_struct("MultiTenantConfig");
        debug.field("strategy", &strategy);
        debug.finish()
    }
}

enum MultiTenantStrategy {
    Database {
        pool_manager: TenantPoolManager,
    },
    Schema {
        db: DatabaseConnection,
        strategy: SchemaPerTenant,
    },
    RowLevel {
        db: DatabaseConnection,
    },
}

enum BuilderStrategy {
    Database {
        factory: Arc<TenantConnectionFactory>,
    },
    DatabaseConnectOptions {
        resolver: Arc<dyn Fn(&TenantContext) -> ConnectOptions + Send + Sync>,
    },
    Schema {
        db: DatabaseConnection,
        strategy: SchemaPerTenant,
    },
    RowLevel {
        db: DatabaseConnection,
    },
}

/// Builder for configuring a multitenant SeaORM application from one explicit strategy.
#[derive(Default)]
pub struct MultiTenantConfigBuilder {
    strategy: Option<BuilderStrategy>,
}

impl fmt::Debug for MultiTenantConfigBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let strategy = match &self.strategy {
            Some(BuilderStrategy::Database { .. }) => Some("database-per-tenant"),
            Some(BuilderStrategy::DatabaseConnectOptions { .. }) => {
                Some("database-per-tenant-options")
            }
            Some(BuilderStrategy::Schema { .. }) => Some("schema-per-tenant"),
            Some(BuilderStrategy::RowLevel { .. }) => Some("row-level"),
            None => None,
        };

        let mut debug = f.debug_struct("MultiTenantConfigBuilder");
        debug.field("strategy", &strategy);
        debug.finish()
    }
}

/// Strategy-erased tenant connection resolved from [`MultiTenantConfig`].
#[derive(Clone, Debug)]
pub enum ConfiguredTenantConnection {
    /// Database-per-tenant connection resolved from a tenant-specific pool.
    Database(DatabaseTenantConnection),
    /// Schema-per-tenant connection resolved from the shared database and schema mapper.
    Schema(SchemaTenantConnection),
    /// Row-level connection resolved from the shared database and explicit tenant context.
    RowLevel(RowLevelTenantConnection),
}

impl MultiTenantConfig {
    /// Start a builder for configuration-driven multitenancy.
    pub fn builder() -> MultiTenantConfigBuilder {
        MultiTenantConfigBuilder::default()
    }

    /// Resolve the tenant-aware connection for the configured strategy.
    pub async fn connection_for(
        &self,
        tenant: TenantContext,
    ) -> Result<ConfiguredTenantConnection, DbErr> {
        match &self.strategy {
            MultiTenantStrategy::Database { pool_manager } => {
                let connection = pool_manager.connection_for(tenant).await?;
                Ok(ConfiguredTenantConnection::Database(connection))
            }
            MultiTenantStrategy::Schema { db, strategy } => Ok(ConfiguredTenantConnection::Schema(
                SchemaTenantConnection::schema(db.clone(), tenant, strategy.clone()),
            )),
            MultiTenantStrategy::RowLevel { db } => Ok(ConfiguredTenantConnection::RowLevel(
                RowLevelTenantConnection::row_level(db.clone(), tenant),
            )),
        }
    }
}

impl MultiTenantConfigBuilder {
    /// Configure database-per-tenant mode from an async connection factory.
    pub fn database_per_tenant<F, Fut>(mut self, factory: F) -> Self
    where
        F: Fn(TenantContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<DatabaseConnection, DbErr>> + Send + 'static,
    {
        self.strategy = Some(BuilderStrategy::Database {
            factory: Arc::new(move |tenant| Box::pin(factory(tenant))),
        });
        self
    }

    /// Configure database-per-tenant mode from a tenant-specific `ConnectOptions` resolver.
    pub fn database_per_tenant_options<F>(mut self, resolver: F) -> Self
    where
        F: Fn(&TenantContext) -> ConnectOptions + Send + Sync + 'static,
    {
        self.strategy = Some(BuilderStrategy::DatabaseConnectOptions {
            resolver: Arc::new(resolver),
        });
        self
    }

    /// Configure schema-per-tenant mode with a shared database and tenant-to-schema mapper.
    pub fn schema_per_tenant<M>(mut self, db: DatabaseConnection, mapper: M) -> Self
    where
        M: TenantSchemaMapper,
    {
        self.strategy = Some(BuilderStrategy::Schema {
            db,
            strategy: SchemaPerTenant::new(mapper),
        });
        self
    }

    /// Configure row-level multitenancy with a shared database connection pool.
    pub fn row_level(mut self, db: DatabaseConnection) -> Self {
        self.strategy = Some(BuilderStrategy::RowLevel { db });
        self
    }

    /// Finalize the builder into a runtime multitenancy configuration.
    pub fn build(self) -> Result<MultiTenantConfig, DbErr> {
        let strategy = self.strategy.ok_or_else(|| {
            DbErr::Custom("multi-tenancy strategy must be configured before building".to_owned())
        })?;

        let strategy = match strategy {
            BuilderStrategy::Database { factory } => MultiTenantStrategy::Database {
                pool_manager: TenantPoolManager::new(move |tenant| {
                    let factory = factory.clone();
                    async move { (factory)(tenant).await }
                }),
            },
            BuilderStrategy::DatabaseConnectOptions { resolver } => MultiTenantStrategy::Database {
                pool_manager: TenantPoolManager::from_connect_options(move |tenant| {
                    resolver(tenant)
                }),
            },
            BuilderStrategy::Schema { db, strategy } => {
                MultiTenantStrategy::Schema { db, strategy }
            }
            BuilderStrategy::RowLevel { db } => MultiTenantStrategy::RowLevel { db },
        };

        Ok(MultiTenantConfig { strategy })
    }
}

impl ConfiguredTenantConnection {
    /// Borrow the resolved tenant context.
    pub fn tenant(&self) -> &TenantContext {
        match self {
            Self::Database(connection) => connection.tenant(),
            Self::Schema(connection) => connection.tenant(),
            Self::RowLevel(connection) => connection.tenant(),
        }
    }

    /// Borrow the underlying database connection.
    pub fn database_connection(&self) -> &DatabaseConnection {
        match self {
            Self::Database(connection) => connection.database_connection(),
            Self::Schema(connection) => connection.database_connection(),
            Self::RowLevel(connection) => connection.database_connection(),
        }
    }

    /// Borrow the database-per-tenant connection when configured.
    pub fn as_database(&self) -> Option<&DatabaseTenantConnection> {
        match self {
            Self::Database(connection) => Some(connection),
            _ => None,
        }
    }

    /// Borrow the schema-per-tenant connection when configured.
    pub fn as_schema(&self) -> Option<&SchemaTenantConnection> {
        match self {
            Self::Schema(connection) => Some(connection),
            _ => None,
        }
    }

    /// Borrow the row-level connection when configured.
    pub fn as_row_level(&self) -> Option<&RowLevelTenantConnection> {
        match self {
            Self::RowLevel(connection) => Some(connection),
            _ => None,
        }
    }

    /// Borrow a stricter row-level wrapper that avoids raw connection access.
    pub fn guarded_row_level(&self) -> Option<GuardedRowLevelTenantConnection> {
        self.as_row_level().map(RowLevelTenantConnection::guarded)
    }

    /// Open a strategy-aware tenant session.
    pub async fn open_session(&self) -> Result<TenantSession, DbErr> {
        match self {
            Self::Database(connection) => connection.open_session().await,
            Self::Schema(connection) => connection.open_session().await,
            Self::RowLevel(connection) => Ok(TenantSession::Connection(
                connection.database_connection().clone(),
            )),
        }
    }
}

#[async_trait]
impl ConnectionTrait for ConfiguredTenantConnection {
    fn get_database_backend(&self) -> DbBackend {
        match self {
            Self::Database(connection) => connection.get_database_backend(),
            Self::Schema(connection) => connection.get_database_backend(),
            Self::RowLevel(connection) => connection.get_database_backend(),
        }
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            Self::Database(connection) => connection.execute_raw(stmt).await,
            Self::Schema(connection) => connection.execute_raw(stmt).await,
            Self::RowLevel(connection) => connection.execute_raw(stmt).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            Self::Database(connection) => connection.execute_unprepared(sql).await,
            Self::Schema(connection) => connection.execute_unprepared(sql).await,
            Self::RowLevel(connection) => connection.execute_unprepared(sql).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            Self::Database(connection) => connection.query_one_raw(stmt).await,
            Self::Schema(connection) => connection.query_one_raw(stmt).await,
            Self::RowLevel(connection) => connection.query_one_raw(stmt).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            Self::Database(connection) => connection.query_all_raw(stmt).await,
            Self::Schema(connection) => connection.query_all_raw(stmt).await,
            Self::RowLevel(connection) => connection.query_all_raw(stmt).await,
        }
    }
}

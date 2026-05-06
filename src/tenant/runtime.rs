use std::{future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;

use crate::{
    AccessMode, ConnectionTrait, DatabaseExecutor, DatabaseTransaction, DbBackend, DbErr,
    ExecResult, IntoDatabaseExecutor, IsolationLevel, QueryResult, Statement, TransactionError,
    TransactionOptions, TransactionTrait,
};

use super::{
    ConfiguredTenantConnection, DatabaseTenantConnection, GuardedRowLevelTenantConnection,
    MultiTenantConfig, RowLevelTenantConnection, SchemaTenantConnection, TenantAware,
    TenantContext, TenantId, TenantPoolManager, TenantSession,
};

/// Execution strategy used by a resolved tenant executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TenantExecutionKind {
    /// Database-per-tenant execution.
    Database,
    /// Schema-per-tenant execution using a stable SQL session.
    Schema,
    /// Row-level execution in a shared schema.
    RowLevel,
}

/// Stable tenant-bound executor for services, jobs, and migrations.
#[derive(Debug)]
pub struct TenantExecutor {
    tenant: TenantContext,
    session: TenantSession,
    kind: TenantExecutionKind,
    row_level: Option<RowLevelTenantConnection>,
}

impl TenantExecutor {
    async fn from_database(connection: &DatabaseTenantConnection) -> Result<Self, DbErr> {
        Ok(Self {
            tenant: connection.tenant().clone(),
            session: connection.open_session().await?,
            kind: TenantExecutionKind::Database,
            row_level: None,
        })
    }

    async fn from_schema(connection: &SchemaTenantConnection) -> Result<Self, DbErr> {
        Ok(Self {
            tenant: connection.tenant().clone(),
            session: connection.open_session().await?,
            kind: TenantExecutionKind::Schema,
            row_level: None,
        })
    }

    fn from_row_level(connection: &RowLevelTenantConnection) -> Self {
        Self {
            tenant: connection.tenant().clone(),
            session: TenantSession::Connection(connection.database_connection().clone()),
            kind: TenantExecutionKind::RowLevel,
            row_level: Some(connection.clone()),
        }
    }

    /// Borrow the tenant scope for this executor.
    pub fn tenant(&self) -> &TenantContext {
        &self.tenant
    }

    /// Borrow the stable underlying session.
    pub fn session(&self) -> &TenantSession {
        &self.session
    }

    /// Consume the executor and return the underlying session.
    pub fn into_session(self) -> TenantSession {
        self.session
    }

    /// Return the strategy used to resolve this executor.
    pub fn kind(&self) -> TenantExecutionKind {
        self.kind
    }

    /// Finalize the executor, committing any transactional schema session.
    pub async fn finish(self) -> Result<(), DbErr> {
        match self.session {
            TenantSession::Connection(_) => Ok(()),
            TenantSession::Transaction(transaction) => transaction.commit().await,
        }
    }

    /// Abort the executor, rolling back any transactional schema session.
    pub async fn rollback(self) -> Result<(), DbErr> {
        match self.session {
            TenantSession::Connection(_) => Ok(()),
            TenantSession::Transaction(transaction) => transaction.rollback().await,
        }
    }

    /// Returns `true` if this executor uses database-per-tenant routing.
    pub fn is_database_per_tenant(&self) -> bool {
        self.kind == TenantExecutionKind::Database
    }

    /// Returns `true` if this executor uses schema-per-tenant routing.
    pub fn is_schema_per_tenant(&self) -> bool {
        self.kind == TenantExecutionKind::Schema
    }

    /// Returns `true` if this executor uses row-level routing.
    pub fn is_row_level(&self) -> bool {
        self.kind == TenantExecutionKind::RowLevel
    }

    /// Borrow a guarded row-level connection when available.
    pub fn guarded_row_level(&self) -> Option<GuardedRowLevelTenantConnection> {
        self.row_level
            .as_ref()
            .map(RowLevelTenantConnection::guarded)
    }

    /// Require row-level routing and return the guarded connection.
    pub fn require_row_level(&self) -> Result<GuardedRowLevelTenantConnection, DbErr> {
        self.guarded_row_level().ok_or_else(|| {
            DbErr::Custom(
                "tenant executor requires row-level strategy for guarded repository access"
                    .to_owned(),
            )
        })
    }
}

impl TenantAware for TenantExecutor {
    fn tenant_id(&self) -> &TenantId {
        self.tenant.tenant_id()
    }
}

impl<'c> IntoDatabaseExecutor<'c> for &'c TenantSession {
    fn into_database_executor(self) -> DatabaseExecutor<'c> {
        match self {
            TenantSession::Connection(connection) => DatabaseExecutor::Connection(connection),
            TenantSession::Transaction(transaction) => DatabaseExecutor::Transaction(transaction),
        }
    }
}

impl<'c> IntoDatabaseExecutor<'c> for &'c TenantExecutor {
    fn into_database_executor(self) -> DatabaseExecutor<'c> {
        self.session().into_database_executor()
    }
}

#[async_trait]
impl ConnectionTrait for TenantExecutor {
    fn get_database_backend(&self) -> DbBackend {
        self.session.get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.session.execute_raw(stmt).await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.session.execute_unprepared(sql).await
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.session.query_one_raw(stmt).await
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.session.query_all_raw(stmt).await
    }
}

#[async_trait]
impl TransactionTrait for TenantExecutor {
    type Transaction = DatabaseTransaction;

    async fn begin(&self) -> Result<Self::Transaction, DbErr> {
        self.into_database_executor().begin().await
    }

    async fn begin_with_config(
        &self,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<Self::Transaction, DbErr> {
        self.into_database_executor()
            .begin_with_config(isolation_level, access_mode)
            .await
    }

    async fn begin_with_options(
        &self,
        options: TransactionOptions,
    ) -> Result<Self::Transaction, DbErr> {
        self.into_database_executor()
            .begin_with_options(options)
            .await
    }

    async fn transaction<F, T, E>(&self, callback: F) -> Result<T, TransactionError<E>>
    where
        F: for<'c> FnOnce(
                &'c Self::Transaction,
            ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        self.into_database_executor().transaction(callback).await
    }

    async fn transaction_with_config<F, T, E>(
        &self,
        callback: F,
        isolation_level: Option<IsolationLevel>,
        access_mode: Option<AccessMode>,
    ) -> Result<T, TransactionError<E>>
    where
        F: for<'c> FnOnce(
                &'c Self::Transaction,
            ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'c>>
            + Send,
        T: Send,
        E: std::fmt::Display + std::fmt::Debug + Send,
    {
        self.into_database_executor()
            .transaction_with_config(callback, isolation_level, access_mode)
            .await
    }
}

/// Trait for strategy-agnostic tenant-aware connection resolution.
#[async_trait]
pub trait TenantConnectionProvider: Send + Sync {
    /// Resolve a configured connection for a tenant scope.
    async fn connection_for_tenant(
        &self,
        tenant: TenantContext,
    ) -> Result<ConfiguredTenantConnection, DbErr>;

    /// Resolve a stable tenant-bound executor for a tenant scope.
    async fn executor_for_tenant(&self, tenant: TenantContext) -> Result<TenantExecutor, DbErr> {
        self.connection_for_tenant(tenant)
            .await?
            .scoped_executor()
            .await
    }
}

#[async_trait]
impl TenantConnectionProvider for MultiTenantConfig {
    async fn connection_for_tenant(
        &self,
        tenant: TenantContext,
    ) -> Result<ConfiguredTenantConnection, DbErr> {
        MultiTenantConfig::connection_for(self, tenant).await
    }
}

#[async_trait]
impl TenantConnectionProvider for TenantPoolManager {
    async fn connection_for_tenant(
        &self,
        tenant: TenantContext,
    ) -> Result<ConfiguredTenantConnection, DbErr> {
        Ok(ConfiguredTenantConnection::Database(
            TenantPoolManager::connection_for(self, tenant).await?,
        ))
    }
}

#[async_trait]
impl<P> TenantConnectionProvider for Arc<P>
where
    P: TenantConnectionProvider + ?Sized,
{
    async fn connection_for_tenant(
        &self,
        tenant: TenantContext,
    ) -> Result<ConfiguredTenantConnection, DbErr> {
        self.as_ref().connection_for_tenant(tenant).await
    }

    async fn executor_for_tenant(&self, tenant: TenantContext) -> Result<TenantExecutor, DbErr> {
        self.as_ref().executor_for_tenant(tenant).await
    }
}

#[async_trait]
impl<P> TenantConnectionProvider for &P
where
    P: TenantConnectionProvider + ?Sized,
{
    async fn connection_for_tenant(
        &self,
        tenant: TenantContext,
    ) -> Result<ConfiguredTenantConnection, DbErr> {
        (*self).connection_for_tenant(tenant).await
    }

    async fn executor_for_tenant(&self, tenant: TenantContext) -> Result<TenantExecutor, DbErr> {
        (*self).executor_for_tenant(tenant).await
    }
}

/// Higher-level execution helper for running tenant-bound operations.
pub trait TenantScopedExecutor: TenantConnectionProvider {
    /// Resolve a configured connection and execute an async operation within the tenant boundary.
    fn with_connection<'a, F, Fut, R>(
        &'a self,
        tenant: TenantContext,
        operation: F,
    ) -> Pin<Box<dyn Future<Output = Result<R, DbErr>> + Send + 'a>>
    where
        F: FnOnce(TenantContext, ConfiguredTenantConnection) -> Fut + Send + 'a,
        Fut: Future<Output = Result<R, DbErr>> + Send + 'a,
        R: Send + 'a,
    {
        Box::pin(async move {
            let connection = self.connection_for_tenant(tenant.clone()).await?;
            operation(tenant, connection).await
        })
    }

    /// Resolve a stable executor and execute an async operation within the tenant boundary.
    fn with_tenant<'a, F, R>(
        &'a self,
        tenant: TenantContext,
        operation: F,
    ) -> Pin<Box<dyn Future<Output = Result<R, DbErr>> + Send + 'a>>
    where
        F: for<'b> FnOnce(
                TenantContext,
                &'b TenantExecutor,
            )
                -> Pin<Box<dyn Future<Output = Result<R, DbErr>> + Send + 'b>>
            + Send
            + 'a,
        R: Send + 'a,
    {
        Box::pin(async move {
            let executor = self.executor_for_tenant(tenant.clone()).await?;
            let result = operation(tenant, &executor).await;
            match result {
                Ok(value) => {
                    executor.finish().await?;
                    Ok(value)
                }
                Err(err) => {
                    let _ = executor.rollback().await;
                    Err(err)
                }
            }
        })
    }

    /// Resolve row-level routing and execute an async operation using guarded APIs.
    fn with_row_level<'a, F, Fut, R>(
        &'a self,
        tenant: TenantContext,
        operation: F,
    ) -> Pin<Box<dyn Future<Output = Result<R, DbErr>> + Send + 'a>>
    where
        F: FnOnce(TenantContext, GuardedRowLevelTenantConnection) -> Fut + Send + 'a,
        Fut: Future<Output = Result<R, DbErr>> + Send + 'a,
        R: Send + 'a,
    {
        Box::pin(async move {
            let executor = self.executor_for_tenant(tenant.clone()).await?;
            let row_level = executor.require_row_level()?;
            operation(tenant, row_level).await
        })
    }
}

impl<T> TenantScopedExecutor for T where T: TenantConnectionProvider + ?Sized {}

/// Trait for tenant-aware services.
#[async_trait]
pub trait TenantService: Send + Sync {
    /// Successful return value.
    type Output;
    /// Service error type.
    type Error;

    /// Execute the service logic within the provided tenant scope.
    async fn execute(&self, scope: &TenantContext) -> Result<Self::Output, Self::Error>;
}

/// Explicitly tenant-bound background job payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantJob<T> {
    tenant: TenantContext,
    payload: T,
}

impl<T> TenantJob<T> {
    /// Create a tenant-bound job payload.
    pub fn new(tenant: TenantContext, payload: T) -> Self {
        Self { tenant, payload }
    }

    /// Borrow the tenant scope.
    pub fn tenant(&self) -> &TenantContext {
        &self.tenant
    }

    /// Borrow the payload.
    pub fn payload(&self) -> &T {
        &self.payload
    }

    /// Consume the job and return both tenant scope and payload.
    pub fn into_parts(self) -> (TenantContext, T) {
        (self.tenant, self.payload)
    }

    /// Transform the payload while preserving the tenant scope.
    pub fn map<U, F>(self, map: F) -> TenantJob<U>
    where
        F: FnOnce(T) -> U,
    {
        let (tenant, payload) = self.into_parts();
        TenantJob::new(tenant, map(payload))
    }
}

impl<T> TenantAware for TenantJob<T> {
    fn tenant_id(&self) -> &TenantId {
        self.tenant.tenant_id()
    }
}

/// Helper for running per-tenant migrations or bootstrap tasks.
#[derive(Debug)]
pub struct TenantMigrationRunner<P> {
    provider: P,
}

impl<P> TenantMigrationRunner<P> {
    /// Create a new tenant migration runner.
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    /// Borrow the underlying tenant connection provider.
    pub fn provider(&self) -> &P {
        &self.provider
    }
}

impl<P> TenantMigrationRunner<P>
where
    P: TenantConnectionProvider,
{
    /// Run a migration callback within a stable tenant executor.
    pub async fn run<R, F>(&self, tenant: TenantContext, migration: F) -> Result<R, DbErr>
    where
        F: for<'a> FnOnce(
                &'a TenantExecutor,
            )
                -> Pin<Box<dyn Future<Output = Result<R, DbErr>> + Send + 'a>>
            + Send,
        R: Send,
    {
        let executor = self.provider.executor_for_tenant(tenant).await?;
        let result = migration(&executor).await;
        match result {
            Ok(value) => {
                executor.finish().await?;
                Ok(value)
            }
            Err(err) => {
                let _ = executor.rollback().await;
                Err(err)
            }
        }
    }
}

/// Run a migration callback within a stable tenant executor.
pub async fn run_tenant_migration<P, R, F>(
    provider: &P,
    tenant: TenantContext,
    migration: F,
) -> Result<R, DbErr>
where
    P: TenantConnectionProvider + ?Sized,
    F: for<'a> FnOnce(
            &'a TenantExecutor,
        ) -> Pin<Box<dyn Future<Output = Result<R, DbErr>> + Send + 'a>>
        + Send,
    R: Send,
{
    let executor = provider.executor_for_tenant(tenant).await?;
    let result = migration(&executor).await;
    match result {
        Ok(value) => {
            executor.finish().await?;
            Ok(value)
        }
        Err(err) => {
            let _ = executor.rollback().await;
            Err(err)
        }
    }
}

impl TenantAware for ConfiguredTenantConnection {
    fn tenant_id(&self) -> &TenantId {
        self.tenant().tenant_id()
    }
}

impl ConfiguredTenantConnection {
    /// Resolve a stable executor for services, jobs, and per-tenant migrations.
    pub async fn scoped_executor(&self) -> Result<TenantExecutor, DbErr> {
        match self {
            Self::Database(connection) => connection.scoped_executor().await,
            Self::Schema(connection) => connection.scoped_executor().await,
            Self::RowLevel(connection) => Ok(connection.scoped_executor()),
        }
    }

    /// Require row-level routing and return the guarded connection.
    pub fn require_row_level(&self) -> Result<GuardedRowLevelTenantConnection, DbErr> {
        self.guarded_row_level().ok_or_else(|| {
            DbErr::Custom(
                "configured tenant connection requires row-level strategy for guarded repository access"
                    .to_owned(),
            )
        })
    }
}

impl DatabaseTenantConnection {
    /// Resolve a stable executor for database-per-tenant routing.
    pub async fn scoped_executor(&self) -> Result<TenantExecutor, DbErr> {
        TenantExecutor::from_database(self).await
    }
}

impl SchemaTenantConnection {
    /// Resolve a stable executor for schema-per-tenant routing.
    pub async fn scoped_executor(&self) -> Result<TenantExecutor, DbErr> {
        TenantExecutor::from_schema(self).await
    }
}

impl RowLevelTenantConnection {
    /// Resolve a stable executor for row-level routing.
    pub fn scoped_executor(&self) -> TenantExecutor {
        TenantExecutor::from_row_level(self)
    }
}

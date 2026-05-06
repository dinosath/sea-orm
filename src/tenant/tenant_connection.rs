use std::{fmt, future::Future, marker::PhantomData, pin::Pin, sync::Arc};

use async_trait::async_trait;

use crate::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, DbBackend, DbErr, DeleteMany, EntityTrait, ExecResult, IntoActiveModel,
    PrimaryKeyTrait, QueryFilter, QueryResult, Select, Statement, TransactionTrait, UpdateMany,
    Value,
};

use super::{TenantAware, TenantContext, TenantId, TenantIdFromScope};

/// Marker for database-per-tenant routing.
#[derive(Clone, Copy, Debug, Default)]
pub struct DatabasePerTenant;

/// Marker for row-level tenancy in a shared schema.
#[derive(Clone, Copy, Debug, Default)]
pub struct RowLevelTenant;

/// Postgres schema resolver for schema-per-tenant mode.
pub trait TenantSchemaMapper: Send + Sync + 'static {
    /// Resolve the schema name for a tenant.
    fn schema_for(&self, tenant: &TenantContext) -> Result<String, DbErr>;
}

impl<F> TenantSchemaMapper for F
where
    F: Fn(&TenantContext) -> Result<String, DbErr> + Send + Sync + 'static,
{
    fn schema_for(&self, tenant: &TenantContext) -> Result<String, DbErr> {
        self(tenant)
    }
}

/// Strategy data for schema-per-tenant mode.
#[derive(Clone)]
pub struct SchemaPerTenant {
    mapper: Arc<dyn TenantSchemaMapper>,
}

impl fmt::Debug for SchemaPerTenant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchemaPerTenant").finish_non_exhaustive()
    }
}

impl SchemaPerTenant {
    /// Create a schema strategy from a tenant-to-schema mapper.
    pub fn new<M>(mapper: M) -> Self
    where
        M: TenantSchemaMapper,
    {
        Self {
            mapper: Arc::new(mapper),
        }
    }

    fn schema_for(&self, tenant: &TenantContext) -> Result<String, DbErr> {
        self.mapper.schema_for(tenant)
    }
}

impl TenantSchemaMapper for SchemaPerTenant {
    fn schema_for(&self, tenant: &TenantContext) -> Result<String, DbErr> {
        self.mapper.schema_for(tenant)
    }
}

/// Strategy-aware tenant connection wrapper.
#[derive(Clone, Debug)]
pub struct TenantConnection<S> {
    db: DatabaseConnection,
    tenant: TenantContext,
    strategy: S,
}

/// Database-per-tenant connection alias.
pub type DatabaseTenantConnection = TenantConnection<DatabasePerTenant>;
/// Schema-per-tenant connection alias.
pub type SchemaTenantConnection = TenantConnection<SchemaPerTenant>;
/// Row-level connection alias.
pub type RowLevelTenantConnection = TenantConnection<RowLevelTenant>;

/// Stricter row-level wrapper that only exposes tenant-scoped operations.
#[derive(Clone, Debug)]
pub struct GuardedRowLevelTenantConnection {
    inner: RowLevelTenantConnection,
}

/// Scoped tenant session used for schema-per-tenant operations that require a stable SQL session.
#[derive(Debug)]
pub enum TenantSession {
    /// A plain pooled database connection.
    Connection(DatabaseConnection),
    /// A transaction-bound session.
    Transaction(DatabaseTransaction),
}

impl TenantSession {
    /// Borrow the underlying database connection when this session is not transactional.
    pub fn as_connection(&self) -> Option<&DatabaseConnection> {
        match self {
            Self::Connection(connection) => Some(connection),
            Self::Transaction(_) => None,
        }
    }

    /// Borrow the underlying transaction when this session is transactional.
    pub fn as_transaction(&self) -> Option<&DatabaseTransaction> {
        match self {
            Self::Connection(_) => None,
            Self::Transaction(transaction) => Some(transaction),
        }
    }
}

/// Generic entity contract for compile-time tenant-scoped APIs.
pub trait TenantScoped: EntityTrait {
    /// Strongly typed tenant identifier stored by the entity.
    type TenantId: TenantIdFromScope + Into<Value> + Clone + Send + Sync + 'static;

    /// Column storing the tenant identifier.
    fn tenant_column() -> Self::Column;

    /// Backward-compatible alias for the tenant column.
    fn tenant_id_column() -> Self::Column {
        Self::tenant_column()
    }

    /// Convert the request-scoped tenant context into the entity tenant id type.
    fn tenant_id_for(tenant: &TenantContext) -> Self::TenantId
    {
        <Self::TenantId as TenantIdFromScope>::from_tenant_scope(tenant)
            .unwrap_or_else(|err| panic!("failed to convert tenant scope: {err}"))
    }

    /// Build a tenant filter condition for the provided tenant identifier.
    fn tenant_filter(tenant_id: Self::TenantId) -> Condition {
        Condition::all().add(Self::tenant_column().eq(tenant_id))
    }
}

/// String-based convenience contract for row-level tenancy.
pub trait TenantEntity: EntityTrait {
    /// Column storing the tenant identifier.
    fn tenant_id_column() -> Self::Column;
}

impl<E> TenantScoped for E
where
    E: TenantEntity,
{
    type TenantId = String;

    fn tenant_column() -> Self::Column {
        <Self as TenantEntity>::tenant_id_column()
    }
}

/// ActiveModel contract for stamping tenant identifiers during inserts.
pub trait TenantScopedActiveModel<TenantId = String>: ActiveModelTrait {
    /// Set the tenant identifier on the active model.
    fn set_tenant_id(&mut self, tenant_id: TenantId);
}

/// Query helper for typed tenant identifiers.
pub trait TenantQueryExt<E>
where
    E: TenantScoped,
{
    /// Inject a tenant predicate into the query.
    fn with_tenant(self, tenant_id: E::TenantId) -> Self;
}

impl<E> TenantQueryExt<E> for Select<E>
where
    E: TenantScoped,
{
    fn with_tenant(self, tenant_id: E::TenantId) -> Self {
        self.filter(E::tenant_column().eq(tenant_id))
    }
}

/// Select helper for request-scoped row-level tenancy.
pub trait TenantSelectExt<E>
where
    E: TenantScoped,
{
    /// Inject a tenant predicate from a tenant scope into the query.
    fn for_tenant(self, tenant: &TenantContext) -> Self;
}

impl<E> TenantSelectExt<E> for Select<E>
where
    E: TenantScoped,
{
    fn for_tenant(self, tenant: &TenantContext) -> Self {
        self.with_tenant(E::tenant_id_for(tenant))
    }
}

/// Tenant-aware repository for row-level tenancy.
#[derive(Clone, Debug)]
pub struct TenantRepository<E>
where
    E: TenantScoped,
{
    db: DatabaseConnection,
    tenant: TenantContext,
    marker: PhantomData<E>,
}

impl<S> TenantConnection<S> {
    /// Borrow the tenant context.
    pub fn tenant(&self) -> &TenantContext {
        &self.tenant
    }

    /// Borrow the underlying database connection.
    pub fn database_connection(&self) -> &DatabaseConnection {
        &self.db
    }
}

impl<S> TenantAware for TenantConnection<S> {
    fn tenant_id(&self) -> &TenantId {
        self.tenant.tenant_id()
    }
}

impl TenantConnection<DatabasePerTenant> {
    /// Create a database-per-tenant connection wrapper.
    pub fn database(db: DatabaseConnection, tenant: TenantContext) -> Self {
        Self {
            db,
            tenant,
            strategy: DatabasePerTenant,
        }
    }

    /// Open a tenant session backed by the cached pool.
    pub async fn open_session(&self) -> Result<TenantSession, DbErr> {
        Ok(TenantSession::Connection(self.db.clone()))
    }
}

impl TenantConnection<SchemaPerTenant> {
    /// Create a schema-per-tenant connection wrapper.
    pub fn schema<M>(db: DatabaseConnection, tenant: TenantContext, mapper: M) -> Self
    where
        M: TenantSchemaMapper,
    {
        Self {
            db,
            tenant,
            strategy: SchemaPerTenant::new(mapper),
        }
    }

    /// Open a tenant session with `SET LOCAL search_path` inside a transaction.
    pub async fn open_session(&self) -> Result<TenantSession, DbErr> {
        let tx = self.db.begin().await?;
        let schema = self.strategy.schema_for(&self.tenant)?;
        let sql = format!(
            "SET LOCAL search_path = {}",
            quote_postgres_identifier(&schema)
        );
        tx.execute_unprepared(&sql).await?;
        Ok(TenantSession::Transaction(tx))
    }

    async fn with_session<T, F>(&self, callback: F) -> Result<T, DbErr>
    where
        F: for<'a> FnOnce(
                &'a DatabaseTransaction,
            )
                -> Pin<Box<dyn Future<Output = Result<T, DbErr>> + Send + 'a>>
            + Send,
        T: Send,
    {
        let tx = self.db.begin().await?;
        let schema = self.strategy.schema_for(&self.tenant)?;
        let sql = format!(
            "SET LOCAL search_path = {}",
            quote_postgres_identifier(&schema)
        );
        tx.execute_unprepared(&sql).await?;
        let result = callback(&tx).await;
        if result.is_ok() {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        result
    }
}

impl TenantConnection<RowLevelTenant> {
    /// Create a row-level tenant wrapper.
    pub fn row_level(db: DatabaseConnection, tenant: TenantContext) -> Self {
        Self {
            db,
            tenant,
            strategy: RowLevelTenant,
        }
    }

    /// Wrap this row-level connection in a guarded API that avoids raw connection access.
    pub fn guarded(&self) -> GuardedRowLevelTenantConnection {
        GuardedRowLevelTenantConnection {
            inner: self.clone(),
        }
    }

    /// Construct the default tenant-filtered select query for an entity.
    pub fn select<E>(&self) -> Select<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        find::<E>(&self.tenant)
    }

    /// Find a tenant-visible model by primary key.
    pub fn select_by_id<E, T>(&self, values: T) -> Select<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
        T: Into<<E::PrimaryKey as PrimaryKeyTrait>::ValueType>,
    {
        find_by_id::<E, T>(&self.tenant, values)
    }

    /// Execute a tenant-scoped query and fetch all visible rows.
    pub async fn find<E>(&self, query: Select<E>) -> Result<Vec<E::Model>, DbErr>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.filter(query).all(&self.db).await
    }

    /// Execute a tenant-scoped query and fetch one visible row.
    pub async fn one<E>(&self, query: Select<E>) -> Result<Option<E::Model>, DbErr>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.filter(query).one(&self.db).await
    }

    /// Insert a tenant-scoped active model, automatically stamping its tenant id.
    pub async fn insert<E>(&self, model: E::ActiveModel) -> Result<E::Model, DbErr>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
        E::ActiveModel: TenantScopedActiveModel<E::TenantId> + Send,
        E::Model: IntoActiveModel<E::ActiveModel>,
    {
        self.repository::<E>().insert(model).await
    }

    /// Build a typed repository that stamps and filters by tenant id.
    pub fn repository<E>(&self) -> TenantRepository<E>
    where
        E: TenantScoped,
    {
        TenantRepository {
            db: self.db.clone(),
            tenant: self.tenant.clone(),
            marker: PhantomData,
        }
    }

    /// Apply the tenant predicate to a select query.
    pub fn filter<E>(&self, query: Select<E>) -> Select<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        apply_tenant_filter(query, &self.tenant)
    }

    /// Apply the tenant predicate to an update query.
    pub fn filter_update<E>(&self, query: UpdateMany<E>) -> UpdateMany<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        query.filter(<E as TenantScoped>::tenant_column().eq(E::tenant_id_for(&self.tenant)))
    }

    /// Apply the tenant predicate to a delete query.
    pub fn filter_delete<E>(&self, query: DeleteMany<E>) -> DeleteMany<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        query.filter(<E as TenantScoped>::tenant_column().eq(E::tenant_id_for(&self.tenant)))
    }
}

impl GuardedRowLevelTenantConnection {
    /// Borrow the tenant context.
    pub fn tenant(&self) -> &TenantContext {
        self.inner.tenant()
    }

    /// Construct the default tenant-filtered select query for an entity.
    pub fn select<E>(&self) -> Select<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.inner.select::<E>()
    }

    /// Find a tenant-visible model by primary key.
    pub fn select_by_id<E, T>(&self, values: T) -> Select<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
        T: Into<<E::PrimaryKey as PrimaryKeyTrait>::ValueType>,
    {
        self.inner.select_by_id::<E, T>(values)
    }

    /// Execute a tenant-scoped query and fetch all visible rows.
    pub async fn find<E>(&self, query: Select<E>) -> Result<Vec<E::Model>, DbErr>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.inner.find(query).await
    }

    /// Execute a tenant-scoped query and fetch one visible row.
    pub async fn one<E>(&self, query: Select<E>) -> Result<Option<E::Model>, DbErr>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.inner.one(query).await
    }

    /// Insert a tenant-scoped active model, automatically stamping its tenant id.
    pub async fn insert<E>(&self, model: E::ActiveModel) -> Result<E::Model, DbErr>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
        E::ActiveModel: TenantScopedActiveModel<E::TenantId> + Send,
        E::Model: IntoActiveModel<E::ActiveModel>,
    {
        self.inner.insert::<E>(model).await
    }

    /// Build a typed repository that stamps and filters by tenant id.
    pub fn repository<E>(&self) -> TenantRepository<E>
    where
        E: TenantScoped,
    {
        self.inner.repository()
    }

    /// Apply the tenant predicate to a select query.
    pub fn filter<E>(&self, query: Select<E>) -> Select<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.inner.filter(query)
    }

    /// Apply the tenant predicate to an update query.
    pub fn filter_update<E>(&self, query: UpdateMany<E>) -> UpdateMany<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.inner.filter_update(query)
    }

    /// Apply the tenant predicate to a delete query.
    pub fn filter_delete<E>(&self, query: DeleteMany<E>) -> DeleteMany<E>
    where
        E: TenantScoped,
        E::TenantId: TenantIdFromScope,
    {
        self.inner.filter_delete(query)
    }
}

impl TenantAware for GuardedRowLevelTenantConnection {
    fn tenant_id(&self) -> &TenantId {
        self.inner.tenant_id()
    }
}

impl From<RowLevelTenantConnection> for GuardedRowLevelTenantConnection {
    fn from(inner: RowLevelTenantConnection) -> Self {
        Self { inner }
    }
}

/// Construct the default tenant-filtered select query for an entity.
pub fn find<E>(tenant: &TenantContext) -> Select<E>
where
    E: TenantScoped,
{
    apply_tenant_filter(E::find(), tenant)
}

/// Find a tenant-visible model by primary key.
pub fn find_by_id<E, T>(tenant: &TenantContext, values: T) -> Select<E>
where
    E: TenantScoped,
    T: Into<<E::PrimaryKey as PrimaryKeyTrait>::ValueType>,
{
    apply_tenant_filter(E::find_by_id(values), tenant)
}

/// Inject a tenant predicate into a select query.
pub fn apply_tenant_filter<E>(query: Select<E>, tenant: &TenantContext) -> Select<E>
where
    E: TenantScoped,
{
    query.filter(<E as TenantScoped>::tenant_column().eq(E::tenant_id_for(tenant)))
}

impl<E> TenantRepository<E>
where
    E: TenantScoped,
{
    /// Create a tenant-scoped repository from an explicit connection and scope.
    pub fn new(db: DatabaseConnection, tenant: TenantContext) -> Self {
        Self {
            db,
            tenant,
            marker: PhantomData,
        }
    }

    /// Borrow the tenant context.
    pub fn tenant(&self) -> &TenantContext {
        &self.tenant
    }

    /// Construct the default tenant-filtered select query.
    pub fn select(&self) -> Select<E> {
        find::<E>(&self.tenant)
    }

    /// Find a tenant-visible model by primary key.
    pub fn find_by_id<T>(&self, values: T) -> Select<E>
    where
        T: Into<<E::PrimaryKey as PrimaryKeyTrait>::ValueType>,
    {
        find_by_id::<E, T>(&self.tenant, values)
    }

    /// Apply the tenant filter to an existing query.
    pub fn filter(&self, query: Select<E>) -> Select<E> {
        apply_tenant_filter(query, &self.tenant)
    }

    /// Fetch all rows visible to the tenant.
    pub async fn all(&self) -> Result<Vec<E::Model>, DbErr> {
        self.select().all(&self.db).await
    }

    /// Fetch a single row visible to the tenant.
    pub async fn one(&self) -> Result<Option<E::Model>, DbErr> {
        self.select().one(&self.db).await
    }

    /// Insert a model after stamping the tenant id.
    pub async fn insert(&self, mut model: E::ActiveModel) -> Result<E::Model, DbErr>
    where
        E::ActiveModel: TenantScopedActiveModel<E::TenantId> + Send,
        E::Model: IntoActiveModel<E::ActiveModel>,
    {
        model.set_tenant_id(E::tenant_id_for(&self.tenant));
        model.insert(&self.db).await
    }
}

impl<E> TenantAware for TenantRepository<E>
where
    E: TenantScoped,
{
    fn tenant_id(&self) -> &TenantId {
        self.tenant.tenant_id()
    }
}

#[async_trait]
impl ConnectionTrait for TenantConnection<DatabasePerTenant> {
    fn get_database_backend(&self) -> DbBackend {
        self.db.get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.db.execute_raw(stmt).await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.db.execute_unprepared(sql).await
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.db.query_one_raw(stmt).await
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.db.query_all_raw(stmt).await
    }
}

#[async_trait]
impl ConnectionTrait for TenantConnection<SchemaPerTenant> {
    fn get_database_backend(&self) -> DbBackend {
        self.db.get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.with_session(move |tx| Box::pin(async move { tx.execute_raw(stmt).await }))
            .await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        let sql = sql.to_owned();
        self.with_session(move |tx| Box::pin(async move { tx.execute_unprepared(&sql).await }))
            .await
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.with_session(move |tx| Box::pin(async move { tx.query_one_raw(stmt).await }))
            .await
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.with_session(move |tx| Box::pin(async move { tx.query_all_raw(stmt).await }))
            .await
    }
}

#[async_trait]
impl ConnectionTrait for TenantConnection<RowLevelTenant> {
    fn get_database_backend(&self) -> DbBackend {
        self.db.get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.db.execute_raw(stmt).await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.db.execute_unprepared(sql).await
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.db.query_one_raw(stmt).await
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.db.query_all_raw(stmt).await
    }
}

#[async_trait]
impl ConnectionTrait for TenantSession {
    fn get_database_backend(&self) -> DbBackend {
        match self {
            Self::Connection(connection) => connection.get_database_backend(),
            Self::Transaction(transaction) => ConnectionTrait::get_database_backend(transaction),
        }
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            Self::Connection(connection) => connection.execute_raw(stmt).await,
            Self::Transaction(transaction) => transaction.execute_raw(stmt).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            Self::Connection(connection) => connection.execute_unprepared(sql).await,
            Self::Transaction(transaction) => transaction.execute_unprepared(sql).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            Self::Connection(connection) => connection.query_one_raw(stmt).await,
            Self::Transaction(transaction) => transaction.query_one_raw(stmt).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            Self::Connection(connection) => connection.query_all_raw(stmt).await,
            Self::Transaction(transaction) => transaction.query_all_raw(stmt).await,
        }
    }
}

fn quote_postgres_identifier(schema: &str) -> String {
    let escaped = schema.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

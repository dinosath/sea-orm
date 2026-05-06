use std::{fmt, future::Future, pin::Pin, sync::Arc};

use dashmap::DashMap;
use futures_util::lock::Mutex;

use crate::{ConnectOptions, Database, DatabaseConnection, DbErr};

use super::{DatabasePerTenant, TenantConnection, TenantContext};

type TenantPoolFactory = dyn Fn(TenantContext) -> Pin<Box<dyn Future<Output = Result<DatabaseConnection, DbErr>> + Send>>
    + Send
    + Sync;

/// Registry of lazily initialized connection pools for database-per-tenant deployments.
pub struct TenantPoolManager {
    pools: DashMap<String, Arc<Mutex<Option<DatabaseConnection>>>>,
    factory: Arc<TenantPoolFactory>,
}

impl fmt::Debug for TenantPoolManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TenantPoolManager")
            .field("pool_count", &self.pools.len())
            .finish()
    }
}

impl TenantPoolManager {
    /// Construct a pool manager from an async connection factory.
    pub fn new<F, Fut>(factory: F) -> Self
    where
        F: Fn(TenantContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<DatabaseConnection, DbErr>> + Send + 'static,
    {
        Self {
            pools: DashMap::new(),
            factory: Arc::new(move |tenant| Box::pin(factory(tenant))),
        }
    }

    /// Construct a pool manager from a function that maps a tenant to `ConnectOptions`.
    pub fn from_connect_options<F>(resolver: F) -> Self
    where
        F: Fn(&TenantContext) -> ConnectOptions + Send + Sync + 'static,
    {
        Self::new(move |tenant| {
            let options = resolver(&tenant);
            async move { Database::connect(options).await }
        })
    }

    /// Acquire or lazily initialize the shared pool for a tenant.
    pub async fn get_pool(&self, tenant: &TenantContext) -> Result<DatabaseConnection, DbErr> {
        let entry = self
            .pools
            .entry(tenant.tenant_id().to_string())
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone();

        let mut guard = entry.lock().await;
        if let Some(connection) = guard.as_ref() {
            return Ok(connection.clone());
        }

        let connection = (self.factory)(tenant.clone()).await?;
        *guard = Some(connection.clone());
        Ok(connection)
    }

    /// Resolve a tenant-scoped connection wrapper for database-per-tenant mode.
    pub async fn connection_for(
        &self,
        tenant: TenantContext,
    ) -> Result<TenantConnection<DatabasePerTenant>, DbErr> {
        let connection = self.get_pool(&tenant).await?;
        Ok(TenantConnection::database(connection, tenant))
    }

    /// Return the number of tenant pools currently cached.
    pub fn pool_count(&self) -> usize {
        self.pools.len()
    }
}

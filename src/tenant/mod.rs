//! Explicit multi-tenancy primitives for SeaORM.
//!
//! The API mirrors the useful concepts from Hibernate ORM 8 without copying its implicit model:
//! a tenant identifier, a tenant-aware connection provider, and an operation-scoped tenant session.
//! Unlike Hibernate, SeaORM keeps the tenant context explicit and never relies on thread-locals.

mod builder;
mod middleware;
mod runtime;
mod tenant_connection;
mod tenant_context;
mod tenant_pool;
#[cfg(test)]
mod tests;

pub use builder::*;
#[cfg(feature = "with-axum")]
#[cfg_attr(docsrs, doc(cfg(feature = "with-axum")))]
pub use middleware::*;
pub use runtime::*;
pub use tenant_connection::*;
pub use tenant_context::*;
pub use tenant_pool::*;

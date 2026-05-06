use std::{fmt, str::FromStr};

use crate::DbErr;

/// Strongly typed tenant identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TenantId(String);

impl TenantId {
    /// Create a validated tenant identifier.
    pub fn new<T>(tenant_id: T) -> Result<Self, DbErr>
    where
        T: Into<String>,
    {
        let tenant_id = tenant_id.into();
        if tenant_id.trim().is_empty() {
            return Err(DbErr::Custom("tenant_id cannot be empty".to_owned()));
        }

        Ok(Self(tenant_id))
    }

    /// Borrow the tenant identifier as a string slice.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Consume the tenant identifier and return the inner string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for TenantId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<str> for TenantId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for TenantId {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl FromStr for TenantId {
    type Err = DbErr;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl From<TenantId> for String {
    fn from(value: TenantId) -> Self {
        value.into_string()
    }
}

impl TryFrom<String> for TenantId {
    type Error = DbErr;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for TenantId {
    type Error = DbErr;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Request- or operation-scoped tenant context.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TenantScope {
    tenant_id: TenantId,
}

/// Backward-compatible alias for request-scoped tenant context.
pub type TenantContext = TenantScope;

/// Convert a request or job tenant scope into an entity-specific tenant id type.
pub trait TenantIdFromScope: Sized {
    /// Convert a validated tenant scope into this tenant id type.
    fn from_tenant_scope(scope: &TenantScope) -> Result<Self, DbErr>;
}

impl<T> TenantIdFromScope for T
where
    T: FromStr,
    T::Err: fmt::Display,
{
    fn from_tenant_scope(scope: &TenantScope) -> Result<Self, DbErr> {
        scope
            .tenant_id()
            .as_str()
            .parse()
            .map_err(|err| DbErr::Type(format!("failed to parse tenant id: {err}")))
    }
}

/// Trait for values that always carry tenant identity.
pub trait TenantAware {
    /// Borrow the current tenant identifier.
    fn tenant_id(&self) -> &TenantId;

    /// Materialize a tenant scope from the current tenant identifier.
    fn tenant_scope(&self) -> TenantScope {
        TenantScope::from_tenant_id(self.tenant_id().clone())
    }
}

impl TenantScope {
    /// Create a new tenant scope.
    pub fn new<T>(tenant_id: T) -> Result<Self, DbErr>
    where
        T: Into<String>,
    {
        Ok(Self {
            tenant_id: TenantId::new(tenant_id)?,
        })
    }

    /// Create a tenant scope from a validated tenant identifier.
    pub fn from_tenant_id(tenant_id: TenantId) -> Self {
        Self { tenant_id }
    }

    /// Borrow the strongly typed tenant identifier.
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Borrow the tenant identifier as a string slice.
    pub fn tenant_id_str(&self) -> &str {
        self.tenant_id.as_str()
    }

    /// Consume the tenant scope and return the tenant identifier.
    pub fn into_tenant_id(self) -> TenantId {
        self.tenant_id
    }
}

impl TenantAware for TenantScope {
    fn tenant_id(&self) -> &TenantId {
        self.tenant_id()
    }
}

impl TryFrom<String> for TenantScope {
    type Error = DbErr;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Self::from_tenant_id(TenantId::try_from(value)?))
    }
}

impl TryFrom<&str> for TenantScope {
    type Error = DbErr;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(Self::from_tenant_id(TenantId::try_from(value)?))
    }
}

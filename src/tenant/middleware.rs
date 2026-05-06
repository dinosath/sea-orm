#[cfg(feature = "with-axum")]
use std::{fmt, sync::Arc};

#[cfg(feature = "with-axum")]
use axum::{
    extract::FromRequestParts,
    http::{HeaderName, StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

#[cfg(feature = "with-axum")]
use super::{MultiTenantConfig, TenantContext, TenantId};

/// Request-scoped tenant resolver for Axum integration.
#[cfg(feature = "with-axum")]
pub trait TenantRequestResolver: Send + Sync + 'static {
    /// Resolve a tenant context from the incoming request.
    fn resolve(&self, parts: &Parts) -> Result<TenantContext, TenantResolutionError>;
}

#[cfg(feature = "with-axum")]
impl<F> TenantRequestResolver for F
where
    F: Fn(&Parts) -> Result<TenantContext, TenantResolutionError> + Send + Sync + 'static,
{
    fn resolve(&self, parts: &Parts) -> Result<TenantContext, TenantResolutionError> {
        self(parts)
    }
}

/// Failure while resolving a tenant from an Axum request.
#[cfg(feature = "with-axum")]
#[derive(Debug, Clone)]
pub enum TenantResolutionError {
    /// The application did not configure a tenant resolver.
    MissingResolver,
    /// The request did not carry tenant information.
    MissingTenant,
    /// The request carried malformed tenant information.
    InvalidTenant(String),
}

#[cfg(feature = "with-axum")]
impl fmt::Display for TenantResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingResolver => write!(f, "missing tenant resolver"),
            Self::MissingTenant => write!(f, "missing tenant context"),
            Self::InvalidTenant(message) => write!(f, "invalid tenant context: {message}"),
        }
    }
}

#[cfg(feature = "with-axum")]
impl std::error::Error for TenantResolutionError {}

#[cfg(feature = "with-axum")]
impl IntoResponse for TenantResolutionError {
    fn into_response(self) -> Response {
        let body = self.to_string();
        let status = match self {
            Self::MissingResolver => StatusCode::INTERNAL_SERVER_ERROR,
            Self::MissingTenant | Self::InvalidTenant(_) => StatusCode::BAD_REQUEST,
        };
        (status, body).into_response()
    }
}

/// Resolver that extracts the tenant id from a request header.
#[cfg(feature = "with-axum")]
#[derive(Clone, Debug)]
pub struct HeaderTenantResolver {
    header_name: HeaderName,
}

#[cfg(feature = "with-axum")]
impl Default for HeaderTenantResolver {
    fn default() -> Self {
        Self::new(HeaderName::from_static("x-tenant-id"))
    }
}

#[cfg(feature = "with-axum")]
impl HeaderTenantResolver {
    /// Resolve the tenant from the given header.
    pub fn new(header_name: HeaderName) -> Self {
        Self { header_name }
    }
}

#[cfg(feature = "with-axum")]
impl TenantRequestResolver for HeaderTenantResolver {
    fn resolve(&self, parts: &Parts) -> Result<TenantContext, TenantResolutionError> {
        let value = parts
            .headers
            .get(&self.header_name)
            .ok_or(TenantResolutionError::MissingTenant)?;
        let tenant_id = value
            .to_str()
            .map_err(|err| TenantResolutionError::InvalidTenant(err.to_string()))?;

        TenantContext::new(tenant_id)
            .map_err(|err| TenantResolutionError::InvalidTenant(err.to_string()))
    }
}

/// Resolver that derives a tenant id from the request host.
#[cfg(feature = "with-axum")]
#[derive(Clone, Debug)]
pub struct SubdomainTenantResolver {
    base_domain: String,
}

#[cfg(feature = "with-axum")]
impl SubdomainTenantResolver {
    /// Create a subdomain resolver for a known base domain.
    pub fn new<T>(base_domain: T) -> Self
    where
        T: Into<String>,
    {
        Self {
            base_domain: base_domain.into(),
        }
    }
}

#[cfg(feature = "with-axum")]
impl TenantRequestResolver for SubdomainTenantResolver {
    fn resolve(&self, parts: &Parts) -> Result<TenantContext, TenantResolutionError> {
        let host = parts
            .headers
            .get("host")
            .ok_or(TenantResolutionError::MissingTenant)?
            .to_str()
            .map_err(|err| TenantResolutionError::InvalidTenant(err.to_string()))?;

        let host = host.split(':').next().unwrap_or(host);
        let suffix = format!(".{}", self.base_domain);
        let tenant = host.strip_suffix(&suffix).ok_or_else(|| {
            TenantResolutionError::InvalidTenant(format!(
                "host `{host}` does not match `{}`",
                self.base_domain
            ))
        })?;

        TenantContext::new(tenant)
            .map_err(|err| TenantResolutionError::InvalidTenant(err.to_string()))
    }
}

/// Resolver that delegates JWT decoding to user code.
#[cfg(feature = "with-axum")]
#[derive(Clone)]
pub struct JwtTenantResolver {
    decoder: Arc<dyn Fn(&str) -> Result<TenantContext, TenantResolutionError> + Send + Sync>,
}

#[cfg(feature = "with-axum")]
impl fmt::Debug for JwtTenantResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtTenantResolver").finish_non_exhaustive()
    }
}

#[cfg(feature = "with-axum")]
impl JwtTenantResolver {
    /// Create a resolver from an application-provided JWT decoder.
    pub fn new<F>(decoder: F) -> Self
    where
        F: Fn(&str) -> Result<TenantContext, TenantResolutionError> + Send + Sync + 'static,
    {
        Self {
            decoder: Arc::new(decoder),
        }
    }
}

#[cfg(feature = "with-axum")]
impl TenantRequestResolver for JwtTenantResolver {
    fn resolve(&self, parts: &Parts) -> Result<TenantContext, TenantResolutionError> {
        let header = parts
            .headers
            .get("authorization")
            .ok_or(TenantResolutionError::MissingTenant)?
            .to_str()
            .map_err(|err| TenantResolutionError::InvalidTenant(err.to_string()))?;
        let token = header.strip_prefix("Bearer ").ok_or_else(|| {
            TenantResolutionError::InvalidTenant("expected bearer token".to_owned())
        })?;
        (self.decoder)(token)
    }
}

/// Axum middleware that resolves and injects `TenantContext` into request extensions.
#[cfg(feature = "with-axum")]
pub async fn tenant_middleware<R>(
    request: axum::extract::Request,
    next: Next,
    resolver: Arc<R>,
) -> Response
where
    R: TenantRequestResolver + ?Sized,
{
    let (mut parts, body) = request.into_parts();

    match resolver.resolve(&parts) {
        Ok(tenant) => {
            parts.extensions.insert(tenant.tenant_id().clone());
            parts.extensions.insert(tenant);
            let request = axum::http::Request::from_parts(parts, body);
            next.run(request).await
        }
        Err(err) => err.into_response(),
    }
}

/// Axum middleware that resolves the tenant from a shared [`MultiTenantConfig`].
#[cfg(feature = "with-axum")]
pub async fn tenant_middleware_from_config(
    request: axum::extract::Request,
    next: Next,
    config: Arc<MultiTenantConfig>,
) -> Response {
    let (mut parts, body) = request.into_parts();

    match config.resolve_request_tenant(&parts) {
        Ok(tenant) => {
            parts.extensions.insert(tenant.tenant_id().clone());
            parts.extensions.insert(tenant);
            let request = axum::http::Request::from_parts(parts, body);
            next.run(request).await
        }
        Err(err) => err.into_response(),
    }
}

/// Extract a previously injected `TenantContext` from Axum request extensions.
#[cfg(feature = "with-axum")]
impl<S> FromRequestParts<S> for TenantContext
where
    S: Send + Sync,
{
    type Rejection = TenantResolutionError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantContext>()
            .cloned()
            .ok_or(TenantResolutionError::MissingTenant)
    }
}

/// Extract a previously injected `TenantId` from Axum request extensions.
#[cfg(feature = "with-axum")]
impl<S> FromRequestParts<S> for TenantId
where
    S: Send + Sync,
{
    type Rejection = TenantResolutionError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantId>()
            .cloned()
            .ok_or(TenantResolutionError::MissingTenant)
    }
}

use std::{error::Error, net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json, Router,
    extract::State,
    http::{Request, StatusCode, header::HeaderName},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use sea_orm::{
    DatabaseConnection,
    entity::prelude::*,
    tenant::{RowLevelTenantConnection, TenantContext, TenantPoolManager},
};
use serde::Serialize;

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

#[derive(Clone)]
struct AppState {
    pools: Arc<TenantPoolManager>,
}

#[derive(Serialize)]
struct HealthResponse {
    tenant_id: String,
    visible_sql_backend: &'static str,
}

async fn health(
    State(state): State<AppState>,
    Extension(tenant): Extension<TenantContext>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let db = state
        .pools
        .get_pool(&tenant)
        .await
        .map_err(internal_error)?;
    let tenant_db = RowLevelTenantConnection::row_level(db, tenant.clone());
    let _repository = tenant_db.repository::<Entity>();

    Ok(Json(HealthResponse {
        tenant_id: tenant.tenant_id().to_string(),
        visible_sql_backend: "explicit-row-level",
    }))
}

fn internal_error(err: sea_orm::DbErr) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn resolve_tenant<B>(request: &Request<B>) -> Result<TenantContext, (StatusCode, String)> {
    let header_name = HeaderName::from_static("x-tenant-id");
    let value = request
        .headers()
        .get(header_name)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing tenant context".to_owned()))?;
    let tenant_id = value
        .to_str()
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;

    TenantContext::new(tenant_id).map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}

async fn tenant_middleware(request: Request<axum::body::Body>, next: Next) -> Response {
    match resolve_tenant(&request) {
        Ok(tenant) => {
            let mut request = request;
            request.extensions_mut().insert(tenant);
            next.run(request).await
        }
        Err(err) => err.into_response(),
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let pools = Arc::new(TenantPoolManager::new(|_tenant| async {
        Ok(DatabaseConnection::default())
    }));

    let app_state = AppState { pools };
    let app = Router::new()
        .route("/health", get(health))
        .route_layer(middleware::from_fn(tenant_middleware))
        .with_state(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 4001));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run().await
}

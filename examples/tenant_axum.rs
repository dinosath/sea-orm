#![cfg_attr(not(feature = "with-axum"), allow(dead_code, unused_imports))]

#[cfg(feature = "with-axum")]
mod app {
    use std::{error::Error, net::SocketAddr, sync::Arc};

    use axum::{
        Json, Router, extract::State, http::StatusCode, middleware, response::IntoResponse,
        routing::get,
    };
    use sea_orm::{
        DatabaseConnection,
        entity::prelude::*,
        tenant::{
            HeaderTenantResolver, RowLevelTenantConnection, TenantContext, TenantPoolManager,
            tenant_middleware,
        },
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
        tenant: TenantContext,
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

    pub async fn run() -> Result<(), Box<dyn Error>> {
        let pools = Arc::new(TenantPoolManager::new(|_tenant| async {
            Ok(DatabaseConnection::default())
        }));
        let resolver = Arc::new(HeaderTenantResolver::default());

        let app_state = AppState { pools };
        let app = Router::new()
            .route("/health", get(health))
            .route_layer(middleware::from_fn({
                let resolver = resolver.clone();
                move |request, next| tenant_middleware(request, next, resolver.clone())
            }))
            .with_state(app_state);

        let addr = SocketAddr::from(([127, 0, 0, 1], 4001));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

#[cfg(feature = "with-axum")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run().await
}

#[cfg(not(feature = "with-axum"))]
fn main() {
    eprintln!("Run this example with `--features with-axum`");
}

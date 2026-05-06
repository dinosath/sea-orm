mod flash;
pub mod service;

use axum::{
    Router,
    Json,
    extract::{Form, Path, Query, State},
    http::StatusCode,
    middleware,
    response::Html,
    routing::{get, get_service, post},
};
use entity::{post, tenant_post};
use flash::{PostResponse, get_flash_cookie, post_response};
use migration::{Migrator, MigratorTrait};
use sea_orm::{
    Database, DatabaseConnection,
    tenant::{MultiTenantConfig, TenantContext, tenant_middleware_from_config},
};
use serde::{Deserialize, Serialize};
use service::{Mutation, Query as QueryService, TenantPostService};
use std::{env, sync::Arc};
use tera::Tera;
use tower_cookies::{CookieManagerLayer, Cookies};
use tower_http::services::ServeDir;

#[tokio::main]
async fn start() -> anyhow::Result<()> {
    unsafe {
        env::set_var("RUST_LOG", "debug");
    }
    tracing_subscriber::fmt::init();

    dotenvy::dotenv().ok();
    let db_url = env::var("DATABASE_URL").expect("DATABASE_URL is not set in .env file");
    let host = env::var("HOST").expect("HOST is not set in .env file");
    let port = env::var("PORT").expect("PORT is not set in .env file");
    let server_url = format!("{host}:{port}");

    let conn = Database::connect(db_url)
        .await
        .expect("Database connection failed");
    Migrator::up(&conn, None).await.unwrap();

    let templates = Tera::new(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/**/*"))
        .expect("Tera initialization failed");

    let tenant_config = Arc::new(
        MultiTenantConfig::builder()
            .row_level(conn.clone())
            .default_header_resolver()
            .build()
            .expect("tenant config"),
    );
    let state = AppState {
        templates,
        conn,
        tenant_config: tenant_config.clone(),
    };

    let tenant_routes = Router::new()
        .route("/posts", get(list_tenant_posts).post(create_tenant_post))
        .route_layer(middleware::from_fn({
            let tenant_config = tenant_config.clone();
            move |request, next| tenant_middleware_from_config(request, next, tenant_config.clone())
        }));

    let app = Router::new()
        .route("/", get(list_posts).post(create_post))
        .route("/{id}", get(edit_post).post(update_post))
        .route("/new", get(new_post))
        .route("/delete/{id}", post(delete_post))
        .nest("/tenant", tenant_routes)
        .nest_service(
            "/static",
            get_service(ServeDir::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/static"
            )))
            .handle_error(|error| async move {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Unhandled internal error: {error}"),
                )
            }),
        )
        .layer(CookieManagerLayer::new())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&server_url).await.unwrap();
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Clone)]
struct AppState {
    templates: Tera,
    conn: DatabaseConnection,
    tenant_config: Arc<MultiTenantConfig>,
}

#[derive(Deserialize)]
struct Params {
    page: Option<u64>,
    posts_per_page: Option<u64>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct FlashData {
    kind: String,
    message: String,
}

#[derive(Deserialize)]
struct TenantPostInput {
    title: String,
    text: String,
}

async fn list_posts(
    state: State<AppState>,
    Query(params): Query<Params>,
    cookies: Cookies,
) -> Result<Html<String>, (StatusCode, &'static str)> {
    let page = params.page.unwrap_or(1);
    let posts_per_page = params.posts_per_page.unwrap_or(5);

    let (posts, num_pages) = QueryService::find_posts_in_page(&state.conn, page, posts_per_page)
        .await
        .expect("Cannot find posts in page");

    let mut ctx = tera::Context::new();
    ctx.insert("posts", &posts);
    ctx.insert("page", &page);
    ctx.insert("posts_per_page", &posts_per_page);
    ctx.insert("num_pages", &num_pages);

    if let Some(value) = get_flash_cookie::<FlashData>(&cookies) {
        ctx.insert("flash", &value);
    }

    let body = state
        .templates
        .render("index.html.tera", &ctx)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Template error"))?;

    Ok(Html(body))
}

async fn new_post(state: State<AppState>) -> Result<Html<String>, (StatusCode, &'static str)> {
    let ctx = tera::Context::new();
    let body = state
        .templates
        .render("new.html.tera", &ctx)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Template error"))?;

    Ok(Html(body))
}

async fn create_post(
    state: State<AppState>,
    mut cookies: Cookies,
    form: Form<post::Model>,
) -> Result<PostResponse, (StatusCode, &'static str)> {
    let form = form.0;

    Mutation::create_post(&state.conn, form)
        .await
        .expect("could not insert post");

    let data = FlashData {
        kind: "success".to_owned(),
        message: "Post successfully added".to_owned(),
    };

    Ok(post_response(&mut cookies, data))
}

async fn edit_post(
    state: State<AppState>,
    Path(id): Path<i32>,
) -> Result<Html<String>, (StatusCode, &'static str)> {
    let post: post::Model = QueryService::find_post_by_id(&state.conn, id)
        .await
        .expect("could not find post")
        .unwrap_or_else(|| panic!("could not find post with id {id}"));

    let mut ctx = tera::Context::new();
    ctx.insert("post", &post);

    let body = state
        .templates
        .render("edit.html.tera", &ctx)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Template error"))?;

    Ok(Html(body))
}

async fn update_post(
    state: State<AppState>,
    Path(id): Path<i32>,
    mut cookies: Cookies,
    form: Form<post::Model>,
) -> Result<PostResponse, (StatusCode, String)> {
    let form = form.0;

    Mutation::update_post_by_id(&state.conn, id, form)
        .await
        .expect("could not edit post");

    let data = FlashData {
        kind: "success".to_owned(),
        message: "Post successfully updated".to_owned(),
    };

    Ok(post_response(&mut cookies, data))
}

async fn delete_post(
    state: State<AppState>,
    Path(id): Path<i32>,
    mut cookies: Cookies,
) -> Result<PostResponse, (StatusCode, &'static str)> {
    Mutation::delete_post(&state.conn, id)
        .await
        .expect("could not delete post");

    let data = FlashData {
        kind: "success".to_owned(),
        message: "Post successfully deleted".to_owned(),
    };

    Ok(post_response(&mut cookies, data))
}

async fn list_tenant_posts(
    state: State<AppState>,
    tenant: TenantContext,
) -> Result<Json<Vec<tenant_post::Model>>, (StatusCode, String)> {
    TenantPostService::list_posts(&state.tenant_config, tenant)
        .await
        .map(Json)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

async fn create_tenant_post(
    state: State<AppState>,
    tenant: TenantContext,
    Json(input): Json<TenantPostInput>,
) -> Result<Json<tenant_post::Model>, (StatusCode, String)> {
    TenantPostService::create_post(
        &state.tenant_config,
        tenant,
        tenant_post::Model {
            id: 0,
            tenant_id: String::new(),
            title: input.title,
            text: input.text,
        },
    )
    .await
    .map(Json)
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

pub fn main() {
    let result = start();

    if let Some(err) = result.err() {
        println!("Error: {err}");
    }
}

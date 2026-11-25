use axum::{routing::get, Json, Router};
use serde_json::json;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "online",
        "service": "SeaFriend API",
        "version": "0.1.0"
    }))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .init();

    let app = Router::new()
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("Não foi possível abrir a porta 3000");

    tracing::info!("SeaFriend API iniciada na porta 3000");

    axum::serve(listener, app)
        .await
        .expect("Erro ao executar a API");
}

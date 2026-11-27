use seafriend_api::{AppState, build_router};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("info"))
        .init();

    let state = AppState::from_env();
    let cleanup_state = state.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(cleanup_state.cleanup_interval());
        loop {
            interval.tick().await;
            cleanup_state.cleanup_expired().await;
        }
    });

    let app = build_router(state);

    let listener = TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("Não foi possível abrir a porta 3000");

    tracing::info!("SeaFriend API iniciada na porta 3000");

    axum::serve(listener, app)
        .await
        .expect("Erro ao executar a API");
}

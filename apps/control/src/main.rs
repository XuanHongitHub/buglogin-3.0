use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let port = std::env::var("BUGLOGIN_CONTROL_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4310);
    let data_dir = buglogin_control_app::default_data_dir();
    let environment = std::env::var("BUGLOGIN_ENV").ok();
    let postgres_url = std::env::var("BUGLOGIN_POSTGRES_URL").ok();
    let app = buglogin_control_app::router_with_startup_config(
        data_dir,
        environment.as_deref(),
        postgres_url,
    )
    .map_err(std::io::Error::other)?;
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

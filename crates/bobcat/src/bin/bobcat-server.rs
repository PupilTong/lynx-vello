fn main() -> Result<(), bobcat::server::ServerError> {
    let port = std::env::var("LYNX_USE_PORT").unwrap_or_else(|_| "8080".to_owned());
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(bobcat::server::serve(&port))
}

//! `novod`: o daemon do Anthive Novo. Único escritor do cofre; serve HTTP local, socket Unix e roda os agentes.

use anyhow::Result;
use novo_core::Vault;
use novo_service::Service;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let vault = Vault::from_env()?;
    // RNF-20: logs estruturados com rotação diária, 7 arquivos
    let logs = vault.root.join("logs");
    let appender = tracing_appender::rolling::Builder::new().rotation(tracing_appender::rolling::Rotation::DAILY).filename_prefix("novod").filename_suffix("log").max_log_files(7).build(&logs)?;
    let (writer, _guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt().with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))).with_writer(writer).with_ansi(false).init();
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("novod {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let svc = Arc::new(Service::open(vault)?);
    eprintln!("novod {} — cofre {}", env!("CARGO_PKG_VERSION"), svc.vault.root.display());
    novo_service::server::serve(svc).await
}

use fast_ledger_api::FastLedgerService;
use fast_ledger_db::LedgerDb;
use std::env;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = env::var("FAST_LEDGER_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let wal_path = env::var("FAST_LEDGER_WAL").ok();

    let db = match wal_path {
        Some(path) => {
            println!("Opening Fast Ledger WAL at {path}");
            LedgerDb::open_with_wal(path, false)?
        }
        None => {
            println!("Starting Fast Ledger in pure in-memory mode");
            LedgerDb::new()
        }
    };

    let service = FastLedgerService::new(db);
    let listener = TcpListener::bind(&bind_addr).await?;
    println!("Fast Ledger HTTP API listening on http://{bind_addr}");
    axum::serve(listener, service.router()).await?;
    Ok(())
}

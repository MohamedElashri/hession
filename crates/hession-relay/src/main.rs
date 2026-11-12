use hession_relay::{backup, database_path, router, Relay, DEFAULT_QUOTA};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Hession relay: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!("Hession private relay\n\nUsage: hession-relay [serve | backup DESTINATION]\n\nEnvironment:\n  HESSION_RELAY_ADMIN_TOKEN       Required secret of at least 32 characters\n  HESSION_RELAY_ADMIN_TOKEN_FILE  Read the secret from this file instead\n  HESSION_RELAY_DATA_DIR          Storage directory (default ./hession-relay-data)\n  HESSION_RELAY_BIND              Listen address (default 127.0.0.1:8744)\n  HESSION_RELAY_QUOTA_BYTES       Ciphertext quota (default 1073741824)\n\nPlace the relay behind HTTPS before connecting devices.\nThe relay never receives your library decryption key.");
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("hession-relay {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("backup") if arguments.len() == 2 => {
            backup(&database_path(), std::path::Path::new(&arguments[1]))?;
            println!("Consistent relay backup created.");
            return Ok(());
        }
        None | Some("serve") if arguments.len() <= 1 => {}
        _ => return Err("Unknown arguments. Run hession-relay --help.".into()),
    }
    let admin = match std::env::var_os("HESSION_RELAY_ADMIN_TOKEN_FILE") {
        Some(path) => {
            if std::env::var_os("HESSION_RELAY_ADMIN_TOKEN").is_some() {
                return Err("Set either HESSION_RELAY_ADMIN_TOKEN or HESSION_RELAY_ADMIN_TOKEN_FILE, not both.".into());
            }
            std::fs::read_to_string(path)
                .map_err(|_| "Cannot read administrator token file.")?
                .trim_end_matches(['\n', '\r'])
                .to_owned()
        }
        None => std::env::var("HESSION_RELAY_ADMIN_TOKEN")
            .or_else(|_| std::env::var("HESSION_RELAY_BOOTSTRAP_TOKEN"))
            .map_err(|_| "Set HESSION_RELAY_ADMIN_TOKEN or HESSION_RELAY_ADMIN_TOKEN_FILE.")?,
    };
    let quota = std::env::var("HESSION_RELAY_QUOTA_BYTES")
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| "Invalid storage quota.".to_string())
        })
        .unwrap_or(Ok(DEFAULT_QUOTA))?;
    let relay = Relay::open(&database_path(), &admin, quota)?;
    let address = std::env::var("HESSION_RELAY_BIND").unwrap_or_else(|_| "127.0.0.1:8744".into());
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|_| "Cannot bind listen address.")?;
    let bound_address = listener
        .local_addr()
        .map_err(|_| "Cannot inspect listen address.")?;
    println!("Hession relay listening on {bound_address}; encrypted storage only.");
    axum::serve(listener, router(relay))
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|_| "HTTP server failed.".to_owned())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

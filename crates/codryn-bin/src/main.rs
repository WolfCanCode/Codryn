use anyhow::Result;
use codryn_mcp::CodrynServer;
use rmcp::ServiceExt;
use std::path::PathBuf;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    // Init logging to stderr (stdout is for MCP JSON-RPC)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("codryn=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();

    // --version
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("codryn {}", VERSION);
        return Ok(());
    }

    // --help
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }

    // status subcommand
    if args.get(1).map(|s| s.as_str()) == Some("status") {
        let report = codryn_cli::doctor::run_doctor();
        println!("codryn v{}", report.codryn_version);
        println!("Binary: {}", report.codryn_binary);
        println!(
            "Store:  {} {}",
            report.store_path,
            if report.store_exists {
                "✓"
            } else {
                "✗ (not created yet)"
            }
        );
        println!();
        println!(
            "{:<14} {:<12} {:<12} Instructions",
            "Agent", "Installed", "Configured"
        );
        println!("{}", "─".repeat(56));
        for a in &report.agents {
            println!(
                "{:<14} {:<12} {:<12} {}",
                a.name,
                if a.installed { "✓" } else { "–" },
                if a.configured { "✓" } else { "–" },
                if a.has_instructions { "✓" } else { "–" },
            );
        }
        return Ok(());
    }

    // install subcommand
    if args.get(1).map(|s| s.as_str()) == Some("install") {
        let dry_run = args.iter().any(|a| a == "--dry-run");
        let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codryn"));
        let configured = codryn_cli::install::install(&binary, dry_run)?;
        if configured.is_empty() {
            println!("No agents detected.");
        } else {
            println!("Configured MCP for: {}", configured.join(", "));
        }
        if !dry_run {
            reindex_all_projects();
        }
        return Ok(());
    }

    // update subcommand
    if args.get(1).map(|s| s.as_str()) == Some("update") {
        codryn_cli::update::update()?;
        // After update, re-install configs and reindex
        let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codryn"));
        let configured = codryn_cli::install::install(&binary, false)?;
        if !configured.is_empty() {
            println!("Reconfigured MCP for: {}", configured.join(", "));
        }
        reindex_all_projects();
        return Ok(());
    }

    // uninstall subcommand
    if args.get(1).map(|s| s.as_str()) == Some("uninstall") {
        let dry_run = args.iter().any(|a| a == "--dry-run");
        let removed = codryn_cli::install::uninstall(dry_run)?;
        if removed.is_empty() {
            println!("No MCP entries found.");
        } else {
            println!("Removed MCP from: {}", removed.join(", "));
        }

        // Remove the binary itself
        let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codryn"));
        if !dry_run {
            if std::fs::remove_file(&binary).is_err() {
                // Try with sudo for system-wide installs (e.g. /usr/local/bin)
                let _ = std::process::Command::new("sudo")
                    .args(["rm", "-f", &binary.to_string_lossy()])
                    .status();
                println!("Removed binary: {}", binary.display());
            } else {
                println!("Removed binary: {}", binary.display());
            }

            // Remove graph database and all codryn data
            let home = codryn_foundation::platform::home_dir().unwrap_or_else(|| "/tmp".into());
            let codryn_dir = std::path::PathBuf::from(&home).join(".codryn");
            if codryn_dir.exists() {
                let _ = std::fs::remove_dir_all(&codryn_dir).or_else(|_| {
                    std::process::Command::new("sudo")
                        .args(["rm", "-rf", &codryn_dir.to_string_lossy()])
                        .status()
                        .map(|_| ())
                });
                println!("Removed data: {}", codryn_dir.display());
            }
        } else {
            println!("[dry-run] Would remove binary: {}", binary.display());
            let home = codryn_foundation::platform::home_dir().unwrap_or_else(|| "/tmp".into());
            println!("[dry-run] Would remove data: {}/.codryn", home);
        }

        return Ok(());
    }

    // Parse --ui and --port flags
    let ui_enabled = args
        .iter()
        .any(|a| a == "--ui" || a.starts_with("--ui=true"));
    let port: u16 = args
        .iter()
        .find(|a| a.starts_with("--port="))
        .and_then(|a| a.strip_prefix("--port="))
        .and_then(|p| p.parse().ok())
        .unwrap_or(9749);

    // Determine store path
    let store_path = default_store_path();
    std::fs::create_dir_all(&store_path)?;

    // Start UI server in background if enabled
    if ui_enabled {
        let sp = store_path.clone();
        tokio::spawn(async move {
            if let Err(e) = codryn_ui::start_server(&sp, port).await {
                tracing::error!(error = %e, "UI server failed");
            }
        });
        tracing::info!(port, "UI server enabled at http://127.0.0.1:{}", port);
    }

    // Start file watcher in background — watches all indexed project roots from the store
    let watcher_store = store_path.clone();
    let _watcher_handle = {
        let watcher = codryn_watcher::Watcher::new(&watcher_store);
        let stop = watcher.stop_handle();
        let handle = std::thread::spawn(move || {
            if let Err(e) = watcher.run() {
                tracing::warn!(error = %e, "watcher stopped");
            }
        });

        // Register signal handler to stop watcher
        let stop_clone = stop.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            stop_clone.store(false, std::sync::atomic::Ordering::SeqCst);
        });

        Some(handle)
    };

    // Default mode: run MCP server on stdio
    tracing::info!(version = VERSION, "starting MCP server");
    let server = CodrynServer::new(&store_path);
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;

    Ok(())
}

fn default_store_path() -> PathBuf {
    let home = codryn_foundation::platform::home_dir().unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(".codryn").join("store")
}

fn reindex_all_projects() {
    let store_path = default_store_path();
    let db_path = store_path.join("graph.db");
    if !db_path.exists() {
        return;
    }
    let projects = match codryn_store::Store::open(&db_path).and_then(|s| s.list_projects()) {
        Ok(p) => p,
        Err(_) => return,
    };
    if projects.is_empty() {
        return;
    }
    println!("Reindexing {} project(s)…", projects.len());
    for p in &projects {
        let root = PathBuf::from(&p.root_path);
        if !root.exists() {
            println!("  ⚠ {} — path missing, skipped", p.name);
            continue;
        }
        print!("  {} …", p.name);
        match codryn_pipeline::Pipeline::new(&root, &store_path, codryn_pipeline::IndexMode::Full)
            .run()
        {
            Ok(()) => println!(" ✓"),
            Err(e) => println!(" ✗ {}", e),
        }
    }
}

fn print_help() {
    println!(
        "codryn {VERSION}\n\
         \n\
         USAGE:\n\
         \x20 codryn                    Run as MCP server on stdin/stdout\n\
         \x20 codryn status              Show agent installation status\n\
         \x20 codryn install [--dry-run] Auto-configure coding agents\n\
         \x20 codryn uninstall          Remove MCP configuration and binary\n\
         \x20 codryn update             Check for updates and self-update\n\
         \x20 codryn --version          Print version\n\
         \x20 codryn --ui [--port=N]    Enable web UI (default port 9749)\n"
    );
}

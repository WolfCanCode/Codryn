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
        let non_interactive = args.iter().any(|a| a == "--non-interactive");
        let mode_cli = args.windows(2).any(|w| w[0] == "--mode" && w[1] == "cli");

        if mode_cli {
            // CLI-first mode: skip MCP config, install only CLI steering
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let steering_path = workspace
                .join(".kiro")
                .join("steering")
                .join("codebase-memory.md");
            codryn_cli::steering::write_steering(
                &steering_path,
                &codryn_cli::preferences::SteeringIntensity::Lite,
            )?;
            println!("Installed CLI steering at: {}", steering_path.display());
            println!("Use `codryn query <tool-name> --<key> <value>` to run tools directly.");
            return Ok(());
        }

        if non_interactive || dry_run {
            // Use the new interactive install flow
            let prompter = codryn_cli::prompter::StdinPrompter;
            let binary = if dry_run {
                std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codryn"))
            } else {
                match codryn_cli::update::ensure_binary_installed() {
                    Ok(p) => {
                        println!("Binary: {}", p.display());
                        p
                    }
                    Err(e) => {
                        eprintln!("Warning: could not install binary to ~/.local/bin: {e}");
                        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codryn"))
                    }
                }
            };
            match codryn_cli::install::install_interactive(
                &prompter,
                non_interactive,
                dry_run,
                Some(&binary),
            ) {
                Ok(config) => {
                    if !dry_run {
                        println!("Install complete (scope: {:?}).", config.scope);
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        // Default: use new interactive flow with StdinPrompter
        let prompter = codryn_cli::prompter::StdinPrompter;
        let binary = match codryn_cli::update::ensure_binary_installed() {
            Ok(p) => {
                println!("Binary: {}", p.display());
                p
            }
            Err(e) => {
                eprintln!("Warning: could not install binary to ~/.local/bin: {e}");
                std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codryn"))
            }
        };
        match codryn_cli::install::install_interactive(&prompter, false, false, Some(&binary)) {
            Ok(config) => {
                println!("Install complete (scope: {:?}).", config.scope);
            }
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // update subcommand
    if args.get(1).map(|s| s.as_str()) == Some("update") {
        codryn_cli::update::update()?;
        // After update, the new binary is in ~/.local/bin — use that path for agent configs
        let binary = codryn_cli::update::install_dir().join(if cfg!(target_os = "windows") {
            "codryn.exe"
        } else {
            "codryn"
        });
        let configured = codryn_cli::install::install(&binary, false)?;
        if !configured.is_empty() {
            println!("Reconfigured MCP for: {}", configured.join(", "));
        }
        return Ok(());
    }

    // activate subcommand
    if args.get(1).map(|s| s.as_str()) == Some("activate") {
        let global = args.iter().any(|a| a == "--global");
        let intensity = if args
            .windows(2)
            .any(|w| w[0] == "--intensity" && w[1] == "lite")
        {
            codryn_cli::preferences::SteeringIntensity::Lite
        } else {
            // Default: full for workspace, lite for global
            if global {
                codryn_cli::preferences::SteeringIntensity::Lite
            } else {
                codryn_cli::preferences::SteeringIntensity::Full
            }
        };
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match codryn_cli::activate::activate(&workspace, global, &intensity) {
            Ok(()) => {
                let scope = if global { "global" } else { "workspace" };
                println!("Activated codryn ({scope}).");
            }
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // deactivate subcommand
    if args.get(1).map(|s| s.as_str()) == Some("deactivate") {
        let global = args.iter().any(|a| a == "--global");
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match codryn_cli::activate::deactivate(&workspace, global) {
            Ok(()) => {
                let scope = if global { "global" } else { "workspace" };
                println!("Deactivated codryn ({scope}).");
            }
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // steering subcommand
    if args.get(1).map(|s| s.as_str()) == Some("steering") {
        let mode = args
            .windows(2)
            .find(|w| w[0] == "--mode")
            .map(|w| w[1].as_str());
        match mode {
            Some("lite") => {
                let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let path = workspace
                    .join(".kiro")
                    .join("steering")
                    .join("codebase-memory.md");
                codryn_cli::steering::switch_mode(
                    &path,
                    &codryn_cli::preferences::SteeringIntensity::Lite,
                )?;
                println!("Steering mode switched to: lite");
            }
            Some("full") => {
                let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let path = workspace
                    .join(".kiro")
                    .join("steering")
                    .join("codebase-memory.md");
                codryn_cli::steering::switch_mode(
                    &path,
                    &codryn_cli::preferences::SteeringIntensity::Full,
                )?;
                println!("Steering mode switched to: full");
            }
            Some(invalid) => {
                eprintln!(
                    "Error: invalid mode '{}'. Valid options: lite, full",
                    invalid
                );
                std::process::exit(1);
            }
            None => {
                eprintln!("Error: --mode <lite|full> is required");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // mcp-config subcommand
    if args.get(1).map(|s| s.as_str()) == Some("mcp-config") {
        let subcommand = args.get(2).map(|s| s.as_str());
        let skip_mcp_config = args.iter().any(|a| a == "--skip-mcp-config");

        if skip_mcp_config {
            println!("Skipped MCP config operations (--skip-mcp-config).");
            return Ok(());
        }

        let prompter = codryn_cli::prompter::StdinPrompter;
        let manager = codryn_cli::mcp_config::McpConfigManager::new(&prompter);

        match subcommand {
            Some("show") => match manager.show_all() {
                Ok(entries) => {
                    if entries.is_empty() {
                        println!("No codryn entries found in any MCP config files.");
                    } else {
                        for entry in &entries {
                            println!("{}: {}", entry.ide_name, entry.config_path.display());
                            println!(
                                "  {}",
                                serde_json::to_string_pretty(&entry.entry).unwrap_or_default()
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(1);
                }
            },
            Some("add") => {
                let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("codryn"));
                let targets: Vec<PathBuf> = args[3..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(PathBuf::from)
                    .collect();
                if targets.is_empty() {
                    eprintln!("Error: specify one or more MCP config file paths to add to");
                    std::process::exit(1);
                }
                match manager.add(&binary, &targets) {
                    Ok(()) => println!("Done."),
                    Err(e) => {
                        eprintln!("Error: {e:#}");
                        std::process::exit(1);
                    }
                }
            }
            Some("remove") => {
                let targets: Vec<PathBuf> = args[3..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(PathBuf::from)
                    .collect();
                if targets.is_empty() {
                    eprintln!("Error: specify one or more MCP config file paths to remove from");
                    std::process::exit(1);
                }
                match manager.remove(&targets) {
                    Ok(()) => println!("Done."),
                    Err(e) => {
                        eprintln!("Error: {e:#}");
                        std::process::exit(1);
                    }
                }
            }
            Some(other) => {
                eprintln!(
                    "Error: unknown mcp-config subcommand '{}'. Use: show, add, remove",
                    other
                );
                std::process::exit(1);
            }
            None => {
                eprintln!("Error: mcp-config requires a subcommand: show, add, remove");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // uninstall subcommand
    if args.get(1).map(|s| s.as_str()) == Some("uninstall") {
        let keep_data = args.iter().any(|a| a == "--keep-data");
        let workspace_only = args.iter().any(|a| a == "--workspace-only");

        // Discover artifacts
        let prefs = codryn_cli::preferences::InstallPreferences::load().ok();
        let artifacts = codryn_cli::uninstall::discover_artifacts(&prefs);

        if artifacts.is_empty() {
            println!("No installed artifacts found.");
            return Ok(());
        }

        // Display the list
        println!("The following artifacts will be removed:\n");
        println!(
            "{}",
            codryn_cli::uninstall::format_artifact_list(&artifacts)
        );

        // Prompt for confirmation
        print!("\nProceed with uninstall? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            println!("Aborted. No changes were made.");
            return Ok(());
        }

        // Execute
        let workspace_path = if workspace_only {
            Some(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        } else {
            None
        };
        let results = codryn_cli::uninstall::execute_uninstall(
            &artifacts,
            keep_data,
            workspace_only,
            workspace_path.as_deref(),
        );

        println!(
            "\n{}",
            codryn_cli::uninstall::format_results_summary(&results)
        );
        return Ok(());
    }

    // backup subcommand
    if args.get(1).map(|s| s.as_str()) == Some("backup") {
        let store_path = default_store_path();
        let output = args.get(2).map(PathBuf::from);
        match codryn_cli::backup::run_backup(&store_path, output.as_deref()) {
            Ok(dest) => {
                println!("Backup created: {}", dest.display());
            }
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // restore subcommand
    if args.get(1).map(|s| s.as_str()) == Some("restore") {
        let store_path = default_store_path();
        let source = args.get(2).map(PathBuf::from);
        match codryn_cli::backup::run_restore(&store_path, source.as_deref()) {
            Ok(()) => {
                println!("Database restored successfully.");
            }
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // validate subcommand
    if args.get(1).map(|s| s.as_str()) == Some("validate") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let fix_safe = args.iter().any(|a| a == "--fix-safe");
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::validate::run_validate(&store_path, project, fix_safe, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // index-runs subcommand
    if args.get(1).map(|s| s.as_str()) == Some("index-runs") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let limit: usize = args
            .windows(2)
            .find(|w| w[0] == "--limit")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(10);
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::index_runs::run_index_runs(&store_path, project, limit, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // dedupe subcommand
    if args.get(1).map(|s| s.as_str()) == Some("dedupe") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let apply = args.iter().any(|a| a == "--apply");
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::dedupe::run_dedupe(&store_path, project, apply, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // snapshots subcommand
    if args.get(1).map(|s| s.as_str()) == Some("snapshots") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let limit: usize = args
            .windows(2)
            .find(|w| w[0] == "--limit")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(10);
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::snapshots::run_snapshots(&store_path, project, limit, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // complexity subcommand
    if args.get(1).map(|s| s.as_str()) == Some("complexity") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let min_cyclomatic: Option<u32> = args
            .windows(2)
            .find(|w| w[0] == "--min-cyclomatic")
            .and_then(|w| w[1].parse().ok());
        let min_cognitive: Option<u32> = args
            .windows(2)
            .find(|w| w[0] == "--min-cognitive")
            .and_then(|w| w[1].parse().ok());
        let top: Option<usize> = args
            .windows(2)
            .find(|w| w[0] == "--top")
            .and_then(|w| w[1].parse().ok());
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::complexity::run_complexity(
            &store_path,
            project,
            min_cyclomatic,
            min_cognitive,
            top,
            json,
        ) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // doc-coverage subcommand
    if args.get(1).map(|s| s.as_str()) == Some("doc-coverage") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let module_filter: Option<&str> = args
            .windows(2)
            .find(|w| w[0] == "--module")
            .map(|w| w[1].as_str());
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::doc_coverage::run_doc_coverage(&store_path, project, module_filter, json)
        {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // deps subcommand
    if args.get(1).map(|s| s.as_str()) == Some("deps") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let check_freshness = args.iter().any(|a| a == "--check-freshness");
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::deps::run_deps(&store_path, project, check_freshness, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // query subcommand — dispatches to either:
    //   1. `cbm query <tool-name> --<key> <value> [--json]` — one-shot MCP tool execution
    //   2. `cbm query --project <project> "<cypher>" [--json]` — Cypher query (legacy)
    if args.get(1).map(|s| s.as_str()) == Some("query") {
        let has_project_flag = args.windows(2).any(|w| w[0] == "--project");
        let json = args.iter().any(|a| a == "--json");

        if !has_project_flag {
            // New: one-shot MCP tool execution via query_tool
            let tool_name = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                // No tool name — list available tools
                let tools = codryn_cli::query_tool::list_tools();
                println!("Available tools:");
                for tool in &tools {
                    println!("  {}", tool);
                }
                std::process::exit(0);
            });

            // Parse --key value pairs from remaining args
            let mut tool_args: Vec<(String, String)> = Vec::new();
            let remaining = &args[3..];
            let mut i = 0;
            while i < remaining.len() {
                let arg = &remaining[i];
                if arg == "--json" {
                    i += 1;
                    continue;
                }
                if arg.starts_with("--") {
                    let key = arg.trim_start_matches('-').to_string();
                    let value = if i + 1 < remaining.len() && !remaining[i + 1].starts_with("--") {
                        i += 1;
                        remaining[i].clone()
                    } else {
                        "true".to_string()
                    };
                    tool_args.push((key, value));
                }
                i += 1;
            }

            let store_path = default_store_path();
            match codryn_cli::query_tool::run_tool(tool_name, &tool_args, json, &store_path) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        // Legacy: Cypher query with --project
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let cypher = args
            .iter()
            .find(|a| !a.starts_with('-') && *a != "query" && *a != &args[0] && *a != project)
            .map(|s| s.as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: cypher query required");
                std::process::exit(1);
            });
        let store_path = default_store_path();
        match codryn_cli::query::run_query(&store_path, project, cypher, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // symbol subcommand
    if args.get(1).map(|s| s.as_str()) == Some("symbol") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let name = args
            .iter()
            .find(|a| !a.starts_with('-') && *a != "symbol" && *a != &args[0])
            .map(|s| s.as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: symbol name required");
                std::process::exit(1);
            });
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::query::run_symbol(&store_path, project, name, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // refs subcommand
    if args.get(1).map(|s| s.as_str()) == Some("refs") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let qn = args
            .iter()
            .find(|a| !a.starts_with('-') && *a != "refs" && *a != &args[0])
            .map(|s| s.as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: qualified name required");
                std::process::exit(1);
            });
        let min_confidence: Option<f64> = args
            .windows(2)
            .find(|w| w[0] == "--min-confidence")
            .and_then(|w| w[1].parse().ok());
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::query::run_refs(&store_path, project, qn, min_confidence, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // impact subcommand
    if args.get(1).map(|s| s.as_str()) == Some("impact") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let qn = args
            .iter()
            .find(|a| !a.starts_with('-') && *a != "impact" && *a != &args[0])
            .map(|s| s.as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: qualified name required");
                std::process::exit(1);
            });
        let depth: Option<i32> = args
            .windows(2)
            .find(|w| w[0] == "--depth")
            .and_then(|w| w[1].parse().ok());
        let min_confidence: Option<f64> = args
            .windows(2)
            .find(|w| w[0] == "--min-confidence")
            .and_then(|w| w[1].parse().ok());
        let json = args.iter().any(|a| a == "--json");
        let store_path = default_store_path();
        match codryn_cli::query::run_impact(&store_path, project, qn, depth, min_confidence, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // diff subcommand
    if args.get(1).map(|s| s.as_str()) == Some("diff") {
        let project = args
            .windows(2)
            .find(|w| w[0] == "--project")
            .map(|w| w[1].as_str())
            .unwrap_or_else(|| {
                eprintln!("Error: --project <project> is required");
                std::process::exit(1);
            });
        let latest = args.iter().any(|a| a == "--latest");
        let json = args.iter().any(|a| a == "--json");
        let from_id: Option<i64> = args
            .windows(2)
            .find(|w| w[0] == "--from")
            .and_then(|w| w[1].parse().ok());
        let to_id: Option<i64> = args
            .windows(2)
            .find(|w| w[0] == "--to")
            .and_then(|w| w[1].parse().ok());
        let store_path = default_store_path();
        match codryn_cli::snapshots::run_diff(&store_path, project, from_id, to_id, latest, json) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
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

#[allow(dead_code)]
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
         \x20 codryn install [--dry-run] [--non-interactive] [--mode cli]\n\
         \x20                        Auto-configure coding agents (interactive by default)\n\
         \x20 codryn uninstall [--keep-data] [--workspace-only]\n\
         \x20                        Remove installed artifacts with confirmation\n\
         \x20 codryn activate [--global] Activate steering for the current workspace\n\
         \x20 codryn deactivate [--global] Deactivate steering for the current workspace\n\
         \x20 codryn steering --mode <lite|full>\n\
         \x20                        Switch steering intensity mode\n\
         \x20 codryn query <tool-name> [--<key> <value>...] [--json]\n\
         \x20                        Run an MCP tool as a one-shot CLI command\n\
         \x20 codryn query --project <project> \"<cypher>\" [--json]\n\
         \x20                        Execute a Cypher query against the graph\n\
         \x20 codryn mcp-config show    Show codryn entries in MCP configs\n\
         \x20 codryn mcp-config add <paths...> [--skip-mcp-config]\n\
         \x20                        Add MCP entry to config files with confirmation\n\
         \x20 codryn mcp-config remove <paths...> [--skip-mcp-config]\n\
         \x20                        Remove MCP entry from config files with confirmation\n\
         \x20 codryn update             Check for updates and self-update\n\
         \x20 codryn backup [path]      Back up the graph database\n\
         \x20 codryn restore [path]     Restore the graph database from backup\n\
         \x20 codryn validate --project <project> [--fix-safe] [--json]\n\
         \x20                        Validate graph consistency\n\
         \x20 codryn dedupe --project <project> [--apply] [--json]\n\
         \x20                        Deduplicate graph nodes (dry-run by default)\n\
         \x20 codryn index-runs --project <project> [--limit <n>] [--json]\n\
         \x20                        List recent index runs for a project\n\
         \x20 codryn snapshots --project <project> [--limit <n>] [--json]\n\
         \x20                        List recent graph summary snapshots for a project\n\
         \x20 codryn diff --project <project> (--latest | --from <id> --to <id>) [--json]\n\
         \x20                        Compare two graph snapshots (count-based diff)\n\
         \x20 codryn complexity --project <project> [--min-cyclomatic <n>] [--min-cognitive <n>]\n\
         \x20                        [--top <n>] [--json]\n\
         \x20                        Report most complex symbols in the graph\n\
         \x20 codryn doc-coverage --project <project> [--module <filter>] [--json]\n\
         \x20                        Report documentation coverage grouped by module\n\
         \x20 codryn deps --project <project> [--check-freshness] [--json]\n\
         \x20                        List dependencies declared in manifest files\n\
         \x20 codryn symbol --project <project> \"<name>\" [--json]\n\
         \x20                        Look up a symbol by name or qualified name\n\
         \x20 codryn refs --project <project> \"<qn>\" [--min-confidence <f>] [--json]\n\
         \x20                        Find incoming references to a symbol\n\
         \x20 codryn impact --project <project> \"<qn>\" [--depth <n>] [--min-confidence <f>] [--json]\n\
         \x20                        Run impact analysis (BFS) for a symbol\n\
         \x20 codryn --version          Print version\n\
         \x20 codryn --ui [--port=N]    Enable web UI (default port 9749)\n"
    );
}

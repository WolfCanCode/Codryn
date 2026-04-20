use codryn_foundation::platform;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct AgentStatus {
    pub name: String,
    pub installed: bool,
    pub configured: bool,
    pub config_path: String,
    pub has_instructions: bool,
    pub instructions_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub codryn_version: String,
    pub codryn_binary: String,
    pub store_path: String,
    pub store_exists: bool,
    pub agents: Vec<AgentStatus>,
}

pub fn run_doctor() -> DoctorReport {
    let home = platform::home_dir().unwrap_or_default();
    let binary = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let store = PathBuf::from(&home)
        .join(".codryn")
        .join("store")
        .join("graph.db");

    let agents = vec![
        check_agent(
            &home,
            "Claude Code",
            ".claude/mcp_servers.json",
            Some(".claude/CLAUDE.md"),
            &["Claude"],
            &[],
        ),
        check_agent(
            &home,
            "VS Code",
            ".vscode/mcp.json",
            Some(".vscode/AGENTS.md"),
            &["Visual Studio Code", "VSCodium"],
            &["code"],
        ),
        check_agent(
            &home,
            "GitHub Copilot",
            ".vscode/mcp.json",
            Some(".github/copilot-instructions.md"),
            &[],
            &[],
        ),
        check_agent(
            &home,
            "Cursor",
            ".cursor/mcp.json",
            Some(".cursor/AGENTS.md"),
            &["Cursor"],
            &["cursor"],
        ),
        check_agent(
            &home,
            "Codex CLI",
            ".codex/config.toml",
            Some(".codex/AGENTS.md"),
            &[],
            &["codex"],
        ),
        check_agent(
            &home,
            "Gemini CLI",
            ".gemini/mcp.json",
            Some(".gemini/GEMINI.md"),
            &[],
            &["gemini"],
        ),
        check_agent(
            &home,
            "Kiro",
            ".kiro/settings/mcp.json",
            Some(".kiro/steering/codryn.md"),
            &["Kiro"],
            &["kiro-cli", "kiro"],
        ),
        check_agent(&home, "Zed", &zed_config_rel(), None, &["Zed"], &["zed"]),
    ];

    DoctorReport {
        codryn_version: env!("CARGO_PKG_VERSION").to_string(),
        codryn_binary: binary,
        store_path: store.to_string_lossy().to_string(),
        store_exists: store.exists(),
        agents,
    }
}

fn zed_config_rel() -> String {
    if platform::is_macos() {
        "Library/Application Support/Zed/settings.json".to_string()
    } else {
        ".config/zed/settings.json".to_string()
    }
}

fn check_agent(
    home: &str,
    name: &str,
    config_rel: &str,
    instr_rel: Option<&str>,
    apps: &[&str],
    bins: &[&str],
) -> AgentStatus {
    let config_path = PathBuf::from(home).join(config_rel);
    let instr_path = instr_rel.map(|r| PathBuf::from(home).join(r));

    let installed = apps.iter().any(|a| app_exists(a))
        || bins.iter().any(|b| which(b))
        || config_path.parent().is_some_and(|p| p.exists());

    let configured = config_path.exists() && has_codryn_entry(&config_path);

    let has_instructions = instr_path.as_ref().is_some_and(|p| {
        p.exists()
            && std::fs::read_to_string(p)
                .is_ok_and(|c| c.contains("codryn") || c.contains("codebase-memory-mcp"))
    });

    AgentStatus {
        name: name.to_string(),
        installed,
        configured,
        config_path: config_path.to_string_lossy().to_string(),
        has_instructions,
        instructions_path: instr_path
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

fn has_codryn_entry(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|c| c.contains("codryn") || c.contains("codebase-memory-mcp"))
}

fn app_exists(name: &str) -> bool {
    let app = format!("{}.app", name);
    Path::new("/Applications").join(&app).exists()
        || platform::home_dir()
            .map(|h| PathBuf::from(h).join("Applications").join(&app).exists())
            .unwrap_or(false)
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).exists()))
        .unwrap_or(false)
}

use anyhow::{bail, Result};
use std::path::PathBuf;
use std::process::Command;

const REPO_SSH: &str = "ssh://git@code.swisscom.com:2222/tommy.le/codryn.git";
const REPO_HTTPS: &str = "https://code.swisscom.com/tommy.le/codryn.git";

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[0;31m";
const GREEN: &str = "\x1b[0;32m";
const CYAN: &str = "\x1b[0;36m";
const BLUE: &str = "\x1b[0;34m";
const WHITE: &str = "\x1b[1;37m";

/// Returns the install directory for the cbm binary (~/.local/bin).
pub fn install_dir() -> PathBuf {
    let home = codryn_foundation::platform::home_dir().unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(".local").join("bin")
}

/// Ensure the running binary is installed to ~/.local/bin/codryn.
/// Copies the current executable there if it's not already in that location.
/// Returns the path to the installed binary.
pub fn ensure_binary_installed() -> Result<PathBuf> {
    let current_bin = std::env::current_exe()?;
    let dir = install_dir();
    let ext = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    let target_bin = dir.join(format!("codryn{ext}"));

    // If already running from the install dir, nothing to do
    if current_bin == target_bin {
        return Ok(target_bin);
    }

    // If target already exists and is the same size, skip
    if target_bin.exists() {
        let src_meta = std::fs::metadata(&current_bin)?;
        if let Ok(dst_meta) = std::fs::metadata(&target_bin) {
            if src_meta.len() == dst_meta.len() {
                return Ok(target_bin);
            }
        }
    }

    std::fs::create_dir_all(&dir)?;
    std::fs::copy(&current_bin, &target_bin)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target_bin, std::fs::Permissions::from_mode(0o755));
    }

    // Code-sign on macOS
    if cfg!(target_os = "macos") {
        let _ = Command::new("codesign")
            .args(["--sign", "-"])
            .arg(&target_bin)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    Ok(target_bin)
}

fn banner() {
    eprintln!();
    eprintln!("  {BOLD}{BLUE}╔═══════════════════════════════════════════════════╗{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                                                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}   {CYAN}┌──┬──┐{RESET}  {WHITE}codryn{RESET}                    {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}   {CYAN}├──┼──┤{RESET}                                         {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}  {CYAN}=┤  │  ├={RESET}  {DIM}Persistent knowledge graph{RESET}            {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}   {CYAN}├──┼──┤{RESET}  {DIM}for AI coding agents{RESET}                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}   {CYAN}└──┴──┘{RESET}  {DIM}Author: Tommy Le{RESET}                       {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                                                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}╚═══════════════════════════════════════════════════╝{RESET}");
    eprintln!();
}

fn step(msg: &str) {
    eprintln!("\n  {CYAN}▶{RESET} {BOLD}{msg}{RESET}");
}

fn ok(msg: &str) {
    eprintln!("    {GREEN}✓{RESET} {msg}");
}

fn fail(msg: &str) {
    eprintln!("    {RED}✗{RESET} {msg}");
}

fn info(msg: &str) {
    eprintln!("    {DIM}{msg}{RESET}");
}

fn spinner_line(msg: &str) {
    eprint!("    {DIM}{msg}…{RESET}");
}

fn done() {
    eprintln!(" {GREEN}done{RESET}");
}

pub fn update() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    banner();

    step("Checking for updates");
    info(&format!("Current version: v{current}"));

    // Fetch latest tag via git ls-remote
    spinner_line("Fetching latest version");
    let output = Command::new("git")
        .args(["ls-remote", "--tags", REPO_SSH])
        .output()
        .or_else(|_| {
            Command::new("git")
                .args(["ls-remote", "--tags", REPO_HTTPS])
                .output()
        })?;

    if !output.status.success() {
        done();
        fail("Failed to check for updates (network error)");
        bail!("Failed to fetch tags");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let latest = stdout
        .lines()
        .filter_map(|line| line.split("refs/tags/").nth(1))
        .filter(|tag| tag.starts_with('v') && !tag.ends_with("^{}"))
        .max_by(|a, b| crate::version::compare_versions(a, b).cmp(&0))
        .ok_or_else(|| anyhow::anyhow!("No version tags found"))?
        .to_owned();
    done();

    if crate::version::compare_versions(&latest, current) <= 0 {
        ok(&format!("Already up to date (v{current})"));
        eprintln!();
        return Ok(());
    }

    ok(&format!("New version available: {BOLD}{latest}{RESET}"));
    info(&format!("v{current} → {latest}"));

    // Clone
    step("Downloading source");
    let tmp = std::env::temp_dir().join("codryn-update");
    let _ = std::fs::remove_dir_all(&tmp);

    spinner_line(&format!("Cloning {latest}"));
    let cloned = Command::new("git")
        .args(["clone", "--depth=1", "--branch", &latest, REPO_SSH])
        .arg(&tmp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || Command::new("git")
            .args(["clone", "--depth=1", "--branch", &latest, REPO_HTTPS])
            .arg(&tmp)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    if !cloned {
        done();
        fail("Failed to clone repository");
        bail!("Clone failed");
    }
    done();

    // Build
    step("Compiling");
    info("This may take 1–3 minutes…");
    let build = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&tmp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !build.success() {
        fail("Build failed");
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("cargo build --release failed");
    }
    ok("Compilation complete");

    // Install binary to ~/.local/bin (no sudo needed)
    step("Installing");
    let ext = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    let new_bin = tmp.join(format!("target/release/codryn{ext}"));
    let install_dir = install_dir();
    let target_bin = install_dir.join(format!("codryn{ext}"));

    spinner_line(&format!("Installing to {}", target_bin.display()));
    std::fs::create_dir_all(&install_dir)?;
    if std::fs::copy(&new_bin, &target_bin).is_err() {
        done();
        fail(&format!(
            "Failed to copy binary to {}",
            target_bin.display()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("Failed to install binary");
    }

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target_bin, std::fs::Permissions::from_mode(0o755));
    }
    done();

    if cfg!(target_os = "macos") {
        spinner_line("Code-signing (macOS)");
        let _ = Command::new("codesign")
            .args(["--sign", "-"])
            .arg(&target_bin)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        done();
    }

    let _ = std::fs::remove_dir_all(&tmp);

    eprintln!();
    eprintln!("  {GREEN}{BOLD}✓ Updated codryn: v{current} → {latest}{RESET}");
    eprintln!();
    Ok(())
}

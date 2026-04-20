use anyhow::{bail, Result};
use std::process::Command;

const DEFAULT_GITHUB_REPO: &str = "wolfcancode/codryn";

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[0;31m";
const GREEN: &str = "\x1b[0;32m";
const CYAN: &str = "\x1b[0;36m";
const BLUE: &str = "\x1b[0;34m";
const WHITE: &str = "\x1b[1;37m";

fn banner() {
    eprintln!();
    eprintln!("  {BOLD}{BLUE}╔═══════════════════════════════════════════════════╗{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                                                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}         {CYAN}.-========================-.{RESET}          {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}       {CYAN}.-'    o----.  .----o     '-.{RESET}        {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}      {CYAN}/    .---.  \\/  .---.       \\\\{RESET}       {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}     {CYAN}|    | o | {WHITE}c o d r y n{CYAN} | o |      |{RESET}      {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}      {CYAN}\\\\    '---'  /\\\\  '---'     /{RESET}       {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}       {CYAN}'-.      o-'  '-o      .-'{RESET}        {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}              {DIM}agent warehouse{RESET}                    {BOLD}{BLUE}║{RESET}");
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
    let github_repo = std::env::var("CODRYN_GITHUB_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GITHUB_REPO.to_string());
    let repo_ssh = format!("git@github.com:{github_repo}.git");
    let repo_https = format!("https://github.com/{github_repo}.git");
    banner();

    step("Checking for updates");
    info(&format!("Current version: v{current}"));

    // Fetch latest tag via git ls-remote
    spinner_line("Fetching latest version");
    let output = Command::new("git")
        .args(["ls-remote", "--tags", &repo_ssh])
        .output()
        .or_else(|_| {
            Command::new("git")
                .args(["ls-remote", "--tags", &repo_https])
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
        .args(["clone", "--depth=1", "--branch", &latest, &repo_ssh])
        .arg(&tmp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || Command::new("git")
            .args(["clone", "--depth=1", "--branch", &latest, &repo_https])
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

    // Replace binary
    step("Installing");
    let current_bin = std::env::current_exe()?;
    let ext = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    let new_bin = tmp.join(format!("target/release/codryn{ext}"));

    spinner_line(&format!("Replacing {}", current_bin.display()));
    let replaced = std::fs::copy(&new_bin, &current_bin).is_ok()
        || Command::new("sudo")
            .args(["cp", "-f"])
            .arg(&new_bin)
            .arg(&current_bin)
            .status()?
            .success();
    if !replaced {
        done();
        fail(&format!(
            "Failed to replace binary at {}",
            current_bin.display()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("Failed to replace binary");
    }
    done();

    if cfg!(target_os = "macos") {
        spinner_line("Code-signing (macOS)");
        let _ = Command::new("sudo")
            .args(["codesign", "--sign", "-"])
            .arg(&current_bin)
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

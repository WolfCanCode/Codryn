use anyhow::{bail, Context, Result};
use std::io::Read;
use std::process::Command;

const DEFAULT_GITHUB_REPO: &str = "wolfcancode/codryn";
const API_BASE: &str = "https://api.github.com/repos";

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
    eprintln!("  {BOLD}{BLUE}║{RESET}                   {CYAN}╔═══════════╗{RESET}                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                   {CYAN}║{RESET}  {WHITE}▪{RESET}     {WHITE}▪{RESET}  {CYAN}║{RESET}                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                   {CYAN}║           ║{RESET}                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}      {CYAN}─────────────╢           ╟─────────────{RESET}      {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                   {CYAN}║           ║{RESET}                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                   {CYAN}╚═══╦═══╦═══╝{RESET}                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                       {CYAN}║   ║{RESET}                       {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                       {CYAN}╨   ╨{RESET}                       {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                                                   {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                 {WHITE}C  O  D  R  Y  N{RESET}                  {BOLD}{BLUE}║{RESET}");
    eprintln!("  {BOLD}{BLUE}║{RESET}                  {DIM}agent warehouse{RESET}                  {BOLD}{BLUE}║{RESET}");
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

fn done_line() {
    eprintln!(" {GREEN}done{RESET}");
}

/// Map `std::env::consts` to the asset name used in GitHub releases.
fn asset_name() -> Option<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some(format!("codryn-{os}-{arch}.tar.gz"))
}

pub fn update() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let github_repo = std::env::var("CODRYN_GITHUB_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GITHUB_REPO.to_string());

    banner();

    step("Checking for updates");
    info(&format!("Current version: v{current}"));

    // ── Fetch latest release from GitHub API ──────────────────────────────
    spinner_line("Fetching latest release");
    let url = format!("{API_BASE}/{github_repo}/releases/latest");
    let response = ureq::get(&url)
        .set("User-Agent", &format!("codryn/{current}"))
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("Failed to reach GitHub API")?;

    let release: serde_json::Value = response
        .into_json()
        .context("Failed to parse GitHub release JSON")?;
    done_line();

    let latest_tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No tag_name in release"))?;
    let latest = latest_tag.trim_start_matches('v');

    if crate::version::compare_versions(latest_tag, current) <= 0 {
        ok(&format!("Already up to date (v{current})"));
        eprintln!();
        return Ok(());
    }

    ok(&format!("New version available: {BOLD}{latest_tag}{RESET}"));
    info(&format!("v{current} → v{latest}"));

    // ── Find the right asset for this platform ────────────────────────────
    let asset_file = asset_name().ok_or_else(|| {
        anyhow::anyhow!(
            "No pre-built binary for {}/{}. Build from source with: cargo install codryn",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;

    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("No assets in release"))?;

    let asset_url = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(&asset_file))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| anyhow::anyhow!("Asset {asset_file} not found in release {latest_tag}"))?
        .to_owned();

    // ── Download ──────────────────────────────────────────────────────────
    step(&format!("Downloading {latest_tag}"));
    info(&format!("Asset: {asset_file}"));

    spinner_line("Downloading binary");
    let resp = ureq::get(&asset_url)
        .set("User-Agent", &format!("codryn/{current}"))
        .call()
        .context("Failed to download release asset")?;

    let mut compressed = Vec::new();
    resp.into_reader()
        .read_to_end(&mut compressed)
        .context("Failed to read download stream")?;
    done_line();

    // ── Extract codryn binary from tar.gz ─────────────────────────────────
    spinner_line("Extracting binary");
    let decoder = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut archive = tar::Archive::new(decoder);

    let tmp_bin = std::env::temp_dir().join("codryn-update-bin");
    let mut extracted = false;

    for entry in archive.entries().context("Failed to read tar archive")? {
        let mut entry = entry.context("Failed to read tar entry")?;
        let path = entry.path().context("Invalid path in archive")?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name == "codryn" || name == "codryn.exe" {
            entry.unpack(&tmp_bin).context("Failed to extract binary")?;
            extracted = true;
            break;
        }
    }
    done_line();

    if !extracted {
        bail!("codryn binary not found inside {asset_file}");
    }

    // ── Replace running binary ─────────────────────────────────────────────
    step("Installing");
    let current_bin = std::env::current_exe()?;

    // Set executable bit on the extracted binary
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp_bin)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp_bin, perms)?;
    }

    spinner_line(&format!("Replacing {}", current_bin.display()));
    let replaced = std::fs::copy(&tmp_bin, &current_bin).is_ok()
        || Command::new("sudo")
            .args(["cp", "-f"])
            .arg(&tmp_bin)
            .arg(&current_bin)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    let _ = std::fs::remove_file(&tmp_bin);

    if !replaced {
        fail(&format!(
            "Failed to replace binary at {}",
            current_bin.display()
        ));
        bail!("Failed to replace binary");
    }
    done_line();

    if cfg!(target_os = "macos") {
        spinner_line("Code-signing (macOS)");
        let _ = Command::new("codesign")
            .args(["--sign", "-"])
            .arg(&current_bin)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .or_else(|_| {
                Command::new("sudo")
                    .args(["codesign", "--sign", "-"])
                    .arg(&current_bin)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
            });
        done_line();
    }

    eprintln!();
    eprintln!("  {GREEN}{BOLD}✓ Updated codryn: v{current} → v{latest}{RESET}");
    eprintln!();
    Ok(())
}

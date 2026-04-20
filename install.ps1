# codryn installer for Windows
# Run: powershell -ExecutionPolicy Bypass -File install.ps1

$ErrorActionPreference = "Stop"

$GITHUB_REPO = if ($env:CODRYN_GITHUB_REPO) { $env:CODRYN_GITHUB_REPO } else { "wolfcancode/codryn" }
$REPO_SSH = "git@github.com:$GITHUB_REPO.git"
$REPO_HTTPS = "https://github.com/$GITHUB_REPO.git"
$INSTALL_DIR = "$env:USERPROFILE\.local\bin"
$BINARY = "codryn.exe"

# ── Helpers ───────────────────────────────────────────────
function Write-Step($msg)    { Write-Host "`n  ▶ $msg" -ForegroundColor Cyan }
function Write-Ok($msg)      { Write-Host "    ✓ $msg" -ForegroundColor Green }
function Write-Warn($msg)    { Write-Host "    ⚠ $msg" -ForegroundColor Yellow }
function Write-Err($msg)     { Write-Host "`n  ✗ Error: $msg`n" -ForegroundColor Red; exit 1 }
function Write-Info($msg)    { Write-Host "    $msg" -ForegroundColor DarkGray }

# ── Banner ────────────────────────────────────────────────
function Show-Banner {
    Write-Host ""
    Write-Host "  ╔═══════════════════════════════════════════════════╗" -ForegroundColor Blue
    Write-Host "  ║                                                   ║" -ForegroundColor Blue
    Write-Host "  ║                   " -ForegroundColor Blue -NoNewline
    Write-Host "╔═══════════╗" -ForegroundColor Cyan -NoNewline
    Write-Host "                   ║" -ForegroundColor Blue
    Write-Host "  ║                   " -ForegroundColor Blue -NoNewline
    Write-Host "║  " -ForegroundColor Cyan -NoNewline
    Write-Host "▪" -ForegroundColor White -NoNewline
    Write-Host "     " -ForegroundColor Cyan -NoNewline
    Write-Host "▪" -ForegroundColor White -NoNewline
    Write-Host "  ║" -ForegroundColor Cyan -NoNewline
    Write-Host "                   ║" -ForegroundColor Blue
    Write-Host "  ║                   " -ForegroundColor Blue -NoNewline
    Write-Host "║           ║" -ForegroundColor Cyan -NoNewline
    Write-Host "                   ║" -ForegroundColor Blue
    Write-Host "  ║      " -ForegroundColor Blue -NoNewline
    Write-Host "─────────────╢           ╟─────────────" -ForegroundColor Cyan -NoNewline
    Write-Host "      ║" -ForegroundColor Blue
    Write-Host "  ║                   " -ForegroundColor Blue -NoNewline
    Write-Host "║           ║" -ForegroundColor Cyan -NoNewline
    Write-Host "                   ║" -ForegroundColor Blue
    Write-Host "  ║                   " -ForegroundColor Blue -NoNewline
    Write-Host "╚═══╦═══╦═══╝" -ForegroundColor Cyan -NoNewline
    Write-Host "                   ║" -ForegroundColor Blue
    Write-Host "  ║                       " -ForegroundColor Blue -NoNewline
    Write-Host "║   ║" -ForegroundColor Cyan -NoNewline
    Write-Host "                       ║" -ForegroundColor Blue
    Write-Host "  ║                       " -ForegroundColor Blue -NoNewline
    Write-Host "╨   ╨" -ForegroundColor Cyan -NoNewline
    Write-Host "                       ║" -ForegroundColor Blue
    Write-Host "  ║                                                   ║" -ForegroundColor Blue
    Write-Host "  ║                 " -ForegroundColor Blue -NoNewline
    Write-Host "C  O  D  R  Y  N" -ForegroundColor White -NoNewline
    Write-Host "                  ║" -ForegroundColor Blue
    Write-Host "  ║                  " -ForegroundColor Blue -NoNewline
    Write-Host "agent warehouse" -ForegroundColor DarkGray -NoNewline
    Write-Host "                  ║" -ForegroundColor Blue
    Write-Host "  ║                                                   ║" -ForegroundColor Blue
    Write-Host "  ╚═══════════════════════════════════════════════════╝" -ForegroundColor Blue
    Write-Host ""
}

# ── Uninstall ─────────────────────────────────────────────
if ($args -contains "uninstall") {
    Show-Banner
    Write-Step "Uninstalling codryn"

    if (Get-Command codryn -ErrorAction SilentlyContinue) {
        Write-Info "Removing MCP configuration from agents..."
        & codryn uninstall 2>$null
        Write-Ok "MCP configuration removed"
    }

    $locations = @(
        "$INSTALL_DIR\$BINARY",
        "$env:USERPROFILE\.cargo\bin\$BINARY"
    )
    foreach ($loc in $locations) {
        if (Test-Path $loc) {
            Remove-Item $loc -Force
            Write-Ok "Removed $loc"
        }
    }

    $codrynData = "$env:USERPROFILE\.codryn"
    if (Test-Path $codrynData) {
        Remove-Item $codrynData -Recurse -Force
        Write-Ok "Removed $codrynData"
    }

    Write-Host "`n  ✓ Fully uninstalled.`n" -ForegroundColor Green
    exit 0
}

# ── Ensure Rust ───────────────────────────────────────────
function Ensure-Rust {
    # Check cargo env
    $cargoEnv = "$env:USERPROFILE\.cargo\env.ps1"
    if (Test-Path $cargoEnv) { . $cargoEnv }

    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        Write-Info "Installing Rust via rustup..."
        $rustupInit = "$env:TEMP\rustup-init.exe"
        Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit -UseBasicParsing
        & $rustupInit -y --no-modify-path 2>$null
        Remove-Item $rustupInit -Force -ErrorAction SilentlyContinue

        # Add to current session PATH
        $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

        # Add to user PATH permanently
        $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
        if ($userPath -notlike "*\.cargo\bin*") {
            [Environment]::SetEnvironmentVariable("PATH", "$env:USERPROFILE\.cargo\bin;$userPath", "User")
            Write-Ok "Added Rust to user PATH"
        }
    }

    # Ensure default toolchain
    $default = & rustup default 2>$null
    if ($default -notmatch "stable|nightly|beta") {
        Write-Info "Setting default Rust toolchain to stable..."
        & rustup default stable 2>$null
    }

    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Err "Rust installed but cargo not in PATH. Restart your terminal and re-run."
    }

    # Check for MSVC linker (required for Rust on Windows)
    $hasLinker = (Get-Command link.exe -ErrorAction SilentlyContinue) -or
        (Test-Path "C:\Program Files\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\link.exe") -or
        (Test-Path "C:\Program Files (x86)\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\link.exe")
    if (-not $hasLinker) {
        Write-Step "Installing Visual Studio Build Tools"
        if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
            Write-Info "Installing winget..."
            $wingetUrl = "https://github.com/microsoft/winget-cli/releases/latest/download/Microsoft.DesktopAppInstaller_8wekyb3d8bbwe.msixbundle"
            $wingetPath = "$env:TEMP\winget.msixbundle"
            Invoke-WebRequest -Uri $wingetUrl -OutFile $wingetPath -UseBasicParsing
            Add-AppxPackage -Path $wingetPath -ErrorAction SilentlyContinue
            Remove-Item $wingetPath -Force -ErrorAction SilentlyContinue
            # Refresh PATH
            $env:PATH = [System.Environment]::GetEnvironmentVariable("PATH", "Machine") + ";" + [System.Environment]::GetEnvironmentVariable("PATH", "User")
        }
        if (Get-Command winget -ErrorAction SilentlyContinue) {
            Write-Info "Installing via winget (this may take a few minutes)..."
            winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" --accept-source-agreements --accept-package-agreements
            if ($LASTEXITCODE -ne 0) {
                Write-Err "Failed to install Build Tools.`n  Install manually: https://visualstudio.microsoft.com/visual-cpp-build-tools/`n  Select 'Desktop development with C++' workload."
            }
            Write-Ok "Build Tools installed — restart your terminal and re-run."
            exit 0
        } else {
            Write-Err "Visual Studio Build Tools required but not found.`n  Install from: https://visualstudio.microsoft.com/visual-cpp-build-tools/`n  Select 'Desktop development with C++' workload.`n  Then re-run this script."
        }
    }
    $rustVer = (& rustc --version 2>$null) -replace "rustc ",""
    Write-Ok "Rust $rustVer"
}

# ── Ensure Node.js ────────────────────────────────────────
function Ensure-Node {
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        Write-Step "Installing Node.js"
        if (Get-Command winget -ErrorAction SilentlyContinue) {
            Write-Info "Installing via winget..."
            & winget install OpenJS.NodeJS.LTS --accept-source-agreements --accept-package-agreements 2>$null
            $env:PATH = "$env:ProgramFiles\nodejs;$env:PATH"
        } elseif (Get-Command choco -ErrorAction SilentlyContinue) {
            Write-Info "Installing via Chocolatey..."
            & choco install nodejs-lts -y 2>$null
            $env:PATH = "$env:ProgramFiles\nodejs;$env:PATH"
        } else {
            Write-Info "Downloading Node.js installer..."
            $nodeInstaller = "$env:TEMP\node-setup.msi"
            Invoke-WebRequest -Uri "https://nodejs.org/dist/v22.15.0/node-v22.15.0-x64.msi" -OutFile $nodeInstaller -UseBasicParsing
            Start-Process msiexec.exe -ArgumentList "/i `"$nodeInstaller`" /qn" -Wait
            Remove-Item $nodeInstaller -Force -ErrorAction SilentlyContinue
            $env:PATH = "$env:ProgramFiles\nodejs;$env:PATH"
        }
        if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
            Write-Err "Failed to install Node.js.`n  Install from https://nodejs.org and re-run."
        }
        Write-Ok "Node.js installed"
    }
    $nodeVer = (& node -e "process.stdout.write(process.versions.node)") -split "\." | Select-Object -First 1
    if ([int]$nodeVer -lt 20) {
        Write-Err "Node.js 20+ required (found v$nodeVer). Upgrade from https://nodejs.org"
    }
    Write-Ok "Node.js v$(& node --version)"
}

# ── Install binary ────────────────────────────────────────
function Install-Binary($src) {
    if (-not (Test-Path $INSTALL_DIR)) {
        New-Item -ItemType Directory -Path $INSTALL_DIR -Force | Out-Null
    }
    Copy-Item $src "$INSTALL_DIR\$BINARY" -Force
    Write-Ok "Installed to $INSTALL_DIR\$BINARY"

    # Add to user PATH if not already there
    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if ($userPath -notlike "*$INSTALL_DIR*") {
        [Environment]::SetEnvironmentVariable("PATH", "$INSTALL_DIR;$userPath", "User")
        $env:PATH = "$INSTALL_DIR;$env:PATH"
        Write-Ok "Added $INSTALL_DIR to user PATH"
    }
}

# ── Build from source ─────────────────────────────────────
function Build-FromSource {
    Write-Step "Checking prerequisites"
    Ensure-Rust
    Ensure-Node

    # Detect local repo or clone
    $scriptDir = Split-Path -Parent $MyInvocation.ScriptName
    $cargoToml = Join-Path $scriptDir "Cargo.toml"

    if ((Test-Path $cargoToml) -and (Select-String -Path $cargoToml -Pattern "codryn-bin" -Quiet)) {
        Write-Step "Using local repository"
        $buildDir = $scriptDir
        Write-Ok "Found at $buildDir"
    } else {
        Write-Step "Cloning repository"
        $tmp = Join-Path $env:TEMP "codryn-build-$(Get-Random)"
        $cloned = $false
        try {
            & git clone --depth=1 $REPO_SSH "$tmp\codryn" 2>$null
            if ($LASTEXITCODE -eq 0) { $cloned = $true }
        } catch {}
        if (-not $cloned) {
            try {
                & git clone --depth=1 $REPO_HTTPS "$tmp\codryn" 2>$null
                if ($LASTEXITCODE -eq 0) { $cloned = $true }
            } catch {}
        }
        if (-not $cloned) {
            Write-Err "Failed to clone repository.`n  Check the GitHub repository path or set CODRYN_GITHUB_REPO=owner/repo."
        }
        $buildDir = "$tmp\codryn"
        Write-Ok "Cloned successfully"
    }

    Write-Step "Compiling (this takes 1-3 minutes)..."
    Push-Location $buildDir
    $logFile = [System.IO.Path]::GetTempFileName()
    & cargo build --release *> $logFile
    if ($LASTEXITCODE -ne 0) {
        Write-Host ""
        Write-Host "  Build failed. Last 20 lines:" -ForegroundColor Red
        Get-Content $logFile -Tail 20 | ForEach-Object { Write-Host "    $_" }
        Remove-Item $logFile -Force -ErrorAction SilentlyContinue
        Pop-Location
        Write-Err "cargo build --release failed.`n  • Check Node.js 20+ and Rust are installed`n  • Check npm dependencies can be installed from the public registry"
    }
    Remove-Item $logFile -Force -ErrorAction SilentlyContinue
    Write-Ok "Compilation complete"

    Install-Binary (Join-Path $buildDir "target\release\codryn.exe")
    Pop-Location

    # Cleanup temp clone
    if ($buildDir -like "*$env:TEMP*") {
        Remove-Item (Split-Path $buildDir -Parent) -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# ── Main ──────────────────────────────────────────────────
if ($args -contains "update") {
    Show-Banner
    Write-Host "  Platform: Windows / $env:PROCESSOR_ARCHITECTURE" -ForegroundColor DarkGray
    Write-Host ""

    Write-Step "Updating codryn"
    if (Get-Command codryn -ErrorAction SilentlyContinue) {
        Write-Info "Current: $(& codryn --version 2>$null)"
    }

    # Get latest tag
    Write-Info "Fetching latest version..."
    $tags = & git ls-remote --tags $REPO_SSH 2>$null
    if ($LASTEXITCODE -ne 0) { $tags = & git ls-remote --tags $REPO_HTTPS 2>$null }
    $latestTag = ($tags | ForEach-Object { if ($_ -match 'refs/tags/(v[\d.]+)$') { $matches[1] } } | Sort-Object { [version]($_ -replace '^v','') } | Select-Object -Last 1)
    if (-not $latestTag) { Write-Err "No version tags found" }
    Write-Ok "Latest version: $latestTag"

    Write-Step "Cloning $latestTag"
    $tmp = Join-Path $env:TEMP "codryn-update-$(Get-Random)"
    $cloned = $false
    try { & git clone --depth=1 --branch $latestTag $REPO_SSH "$tmp\codryn" 2>$null; if ($LASTEXITCODE -eq 0) { $cloned = $true } } catch {}
    if (-not $cloned) { try { & git clone --depth=1 --branch $latestTag $REPO_HTTPS "$tmp\codryn" 2>$null; if ($LASTEXITCODE -eq 0) { $cloned = $true } } catch {} }
    if (-not $cloned) { Write-Err "Failed to clone" }
    Write-Ok "Cloned $latestTag"

    Ensure-Rust
    Ensure-Node
    Write-Step "Compiling (this takes 1-3 minutes)..."
    Push-Location "$tmp\codryn"
    $logFile = [System.IO.Path]::GetTempFileName()
    & cargo build --release *> $logFile
    if ($LASTEXITCODE -ne 0) {
        Get-Content $logFile -Tail 20 | ForEach-Object { Write-Host "    $_" }
        Remove-Item $logFile -Force -ErrorAction SilentlyContinue
        Pop-Location
        Write-Err "Build failed"
    }
    Remove-Item $logFile -Force -ErrorAction SilentlyContinue
    Write-Ok "Compilation complete"

    Install-Binary (Join-Path "$tmp\codryn" "target\release\codryn.exe")
    Pop-Location
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue

    Write-Host ""
    $version = (& codryn --version 2>$null) -replace "codryn ",""
    Write-Host "  ✓ codryn updated to $version" -ForegroundColor Green
    Write-Host ""
    exit 0
}

Show-Banner
Write-Host "  Platform: Windows / $env:PROCESSOR_ARCHITECTURE" -ForegroundColor DarkGray
Write-Host ""

Write-Step "Installing codryn"
Write-Warn "No pre-built binary, building from source"
Build-FromSource

Write-Step "Finalizing"
Start-Sleep -Seconds 1

Write-Host ""
if (Get-Command codryn -ErrorAction SilentlyContinue) {
    $version = (& codryn --version 2>$null) -replace "codryn ",""
    Write-Host "  ✓ codryn $version installed successfully!" -ForegroundColor Green
} else {
    # Try with updated PATH
    $env:PATH = "$INSTALL_DIR;$env:PATH"
    $version = (& "$INSTALL_DIR\$BINARY" --version 2>$null) -replace "codryn ",""
    Write-Host "  ✓ codryn $version installed successfully!" -ForegroundColor Green
    Write-Host "    Restart your terminal for PATH changes to take effect." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "  Configuring coding agents..." -ForegroundColor Cyan
try { & codryn install 2>$null; Write-Host "  ✓ Agent configs updated" -ForegroundColor Green } catch { Write-Host "  ⚠ codryn install failed — run manually: codryn install" -ForegroundColor Yellow }
Write-Host ""
Write-Host "  Next steps:" -ForegroundColor White
Write-Host "  1. Index your project:            " -NoNewline; Write-Host "codryn" -ForegroundColor DarkGray -NoNewline; Write-Host "  → tell agent: " -NoNewline; Write-Host '"Index this project"' -ForegroundColor DarkGray
Write-Host "  2. Open the dashboard:            " -NoNewline; Write-Host "codryn --ui" -ForegroundColor DarkGray -NoNewline; Write-Host "  → http://localhost:9749"
Write-Host ""
Write-Host "  Uninstall:  " -NoNewline; Write-Host "codryn uninstall" -ForegroundColor DarkGray
Write-Host ""

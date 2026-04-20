#!/usr/bin/env bash
# codryn installer
set -e

GITHUB_REPO="${CODRYN_GITHUB_REPO:-wolfcancode/codryn}"
REPO_SSH="git@github.com:${GITHUB_REPO}.git"
REPO_HTTPS="https://github.com/${GITHUB_REPO}.git"
INSTALL_DIR="/usr/local/bin"
BINARY="codryn"

# ── Colors & styles ───────────────────────────────────────
RESET='\033[0m'; BOLD='\033[1m'; DIM='\033[2m'
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BLUE='\033[0;34m'; WHITE='\033[1;37m'

step()    { echo -e "\n${CYAN}  ▶${RESET} ${BOLD}$*${RESET}"; }
info()    { echo -e "    ${DIM}$*${RESET}"; }
ok()      { echo -e "    ${GREEN}✓${RESET} $*"; }
warn()    { echo -e "    ${YELLOW}⚠${RESET}  $*"; }
die()     { echo -e "\n  ${RED}✗ Error:${RESET} $*\n" >&2; exit 1; }
progress(){ echo -ne "    ${DIM}$*…${RESET}"; }
done_()   { echo -e " ${GREEN}done${RESET}"; }

# ── Banner ────────────────────────────────────────────────
print_banner() {
  local B="${BOLD}${BLUE}" R="${RESET}" W="${WHITE}${BOLD}" D="${DIM}" C="${CYAN}"
  echo ""
  echo -e "  ${B}╔═══════════════════════════════════════════════════╗${R}"
  echo -e "  ${B}║${R}                                                   ${B}║${R}"
  echo -e "  ${B}║${R}         ${C}.-========================-.${R}          ${B}║${R}"
  echo -e "  ${B}║${R}       ${C}.-'    o----.  .----o     '-.${R}        ${B}║${R}"
  echo -e "  ${B}║${R}      ${C}/    .---.  \\/  .---.       \\\\${R}       ${B}║${R}"
  echo -e "  ${B}║${R}     ${C}|    | o | ${W}c o d r y n${C} | o |      |${R}      ${B}║${R}"
  echo -e "  ${B}║${R}      ${C}\\\\    '---'  /\\\\  '---'     /${R}       ${B}║${R}"
  echo -e "  ${B}║${R}       ${C}'-.      o-'  '-o      .-'${R}        ${B}║${R}"
  echo -e "  ${B}║${R}              ${iD}agent warehouse${R}                    ${B}║${R}"
  echo -e "  ${B}║${R}                                                   ${B}║${R}"
  echo -e "  ${B}╚═══════════════════════════════════════════════════╝${R}"
  echo ""
}

# ── Spinner ───────────────────────────────────────────────
SPINNER_PID=""
start_spinner() {
  local msg="$1"
  local frames=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
  (
    local i=0
    while true; do
      echo -ne "\r    ${CYAN}${frames[$i]}${RESET}  ${DIM}${msg}${RESET}   "
      i=$(( (i+1) % ${#frames[@]} ))
      sleep 0.1
    done
  ) &
  SPINNER_PID=$!
}
stop_spinner() {
  if [ -n "$SPINNER_PID" ]; then
    kill "$SPINNER_PID" 2>/dev/null; wait "$SPINNER_PID" 2>/dev/null || true
    SPINNER_PID=""; echo -ne "\r\033[2K"
  fi
}
trap 'stop_spinner' EXIT

OS="$(uname -s)"; ARCH="$(uname -m)"

# ── Uninstall ─────────────────────────────────────────────
do_uninstall() {
  print_banner
  step "Uninstalling codryn"

  if command -v codryn &>/dev/null; then
    progress "Removing MCP configuration from agents"
    codryn uninstall 2>/dev/null || true
    done_
  fi

  for loc in "${INSTALL_DIR}/${BINARY}" "${HOME}/.local/bin/${BINARY}" "${HOME}/.cargo/bin/${BINARY}"; do
    if [ -f "$loc" ]; then
      progress "Removing $loc"
      rm -f "$loc" 2>/dev/null || sudo rm -f "$loc"
      done_
    fi
  done

  if [ -d "${HOME}/.codryn" ]; then
    progress "Removing ~/.codryn"
    rm -rf "${HOME}/.codryn" 2>/dev/null || sudo rm -rf "${HOME}/.codryn"
    done_
  fi

  echo -e "\n  ${GREEN}${BOLD}✓ Fully uninstalled.${RESET}\n"
  exit 0
}

# ── Pre-built binary ──────────────────────────────────────
try_prebuilt() {
  local tag
  tag=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" \
    2>/dev/null | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
  [ -z "$tag" ] && return 1

  local asset=""
  case "${OS}-${ARCH}" in
    Linux-x86_64)   asset="codryn-linux-x86_64.tar.gz" ;;
    Linux-aarch64)  asset="codryn-linux-aarch64.tar.gz" ;;
    Darwin-x86_64)  asset="codryn-macos-x86_64.tar.gz" ;;
    Darwin-arm64)   asset="codryn-macos-aarch64.tar.gz" ;;
    *) return 1 ;;
  esac

  local url="https://github.com/${GITHUB_REPO}/releases/download/${tag}/${asset}"
  local tmp; tmp=$(mktemp -d)

  start_spinner "Downloading ${tag} for ${OS}/${ARCH}"
  if curl -fsSL "$url" -o "${tmp}/${asset}" 2>/dev/null; then
    tar -xzf "${tmp}/${asset}" -C "$tmp" 2>/dev/null
    stop_spinner; ok "Downloaded ${tag}"
    install_binary "${tmp}/codryn"
    rm -rf "$tmp"
    return 0
  fi
  stop_spinner; rm -rf "$tmp"
  return 1
}

install_binary() {
  local src="$1"; chmod +x "$src"
  if [ -w "$INSTALL_DIR" ]; then
    progress "Installing to ${INSTALL_DIR}/${BINARY}"
    cp "$src" "${INSTALL_DIR}/${BINARY}"
    done_
  else
    echo ""
    info "sudo is required to copy codryn to ${INSTALL_DIR}"
    sudo cp "$src" "${INSTALL_DIR}/${BINARY}"
    ok "Installed to ${INSTALL_DIR}/${BINARY}"
  fi
  if [ "$OS" = "Darwin" ]; then
    progress "Code-signing binary (macOS Gatekeeper)"
    sudo codesign --sign - "${INSTALL_DIR}/${BINARY}" 2>/dev/null || true
    done_
  fi
}

# ── Prerequisites ─────────────────────────────────────────
ensure_rust() {
  # Source cargo env if it exists (might be installed but not in current PATH)
  [ -f "${HOME}/.cargo/env" ] && source "${HOME}/.cargo/env"

  if ! command -v rustup &>/dev/null; then
    progress "Installing Rust via rustup"
    if ! curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path >/dev/null 2>&1; then
      die "Failed to install Rust. Visit https://rustup.rs"
    fi
    done_
    [ -f "${HOME}/.cargo/env" ] && source "${HOME}/.cargo/env"
  fi

  # Ensure a default toolchain is set
  if ! rustup default 2>/dev/null | grep -q "stable\|nightly\|beta"; then
    progress "Setting default Rust toolchain to stable"
    rustup default stable >/dev/null 2>&1 || die "Failed to set default Rust toolchain.\n  Run: rustup default stable"
    done_
  fi

  # Verify cargo works
  if ! command -v cargo &>/dev/null; then
    die "Rust installed but cargo not in PATH.\n  Run: source ~/.cargo/env\n  Then re-run."
  fi

  # Persist to shell RC
  local shell_rc=""
  case "${SHELL}" in
    */zsh)  shell_rc="${HOME}/.zshrc" ;;
    */bash) shell_rc="${HOME}/.bashrc" ;;
    */fish) shell_rc="${HOME}/.config/fish/config.fish" ;;
  esac
  if [ -n "$shell_rc" ] && [ -f "$shell_rc" ]; then
    if ! grep -q '.cargo/env' "$shell_rc" 2>/dev/null; then
      printf '\n# Rust (added by codryn installer)\n. "${HOME}/.cargo/env"\n' >> "$shell_rc"
      ok "Added Rust to ${shell_rc}"
    fi
  fi

  ok "Rust $(rustc --version 2>/dev/null | awk '{print $2}')"
}

ensure_node() {
  if ! command -v node &>/dev/null; then
    step "Installing Node.js"
    if [ "$OS" = "Darwin" ]; then
      if command -v brew &>/dev/null; then
        progress "Installing via Homebrew"
        brew install node >/dev/null 2>&1 || die "Failed to install Node.js via brew"
        done_
      else
        progress "Installing Homebrew first"
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)" >/dev/null 2>&1
        eval "$(/opt/homebrew/bin/brew shellenv 2>/dev/null || /usr/local/bin/brew shellenv 2>/dev/null)"
        brew install node >/dev/null 2>&1 || die "Failed to install Node.js via brew"
        done_
      fi
    else
      progress "Installing via nodesource"
      curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null 2>&1
      apt-get install -y nodejs >/dev/null 2>&1 || \
        (yum install -y nodejs >/dev/null 2>&1) || \
        die "Failed to install Node.js.\n  Install from https://nodejs.org and re-run."
      done_
    fi
  fi
  local v; v=$(node -e "process.stdout.write(process.versions.node.split('.')[0])")
  [ "$v" -lt 20 ] && die "Node.js 20+ required (found v${v}). Upgrade from https://nodejs.org"
  ok "Node.js v$(node --version | tr -d v)"
}

# ── Build ─────────────────────────────────────────────────
build_and_install() {
  step "Checking prerequisites"
  ensure_rust
  ensure_node

  # Detect source: local repo or clone
  local build_dir="" cleanup=""
  local script_dir; script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd || echo "")"

  if [ -n "$script_dir" ] && [ -f "${script_dir}/Cargo.toml" ] && grep -q "codryn-bin" "${script_dir}/Cargo.toml" 2>/dev/null; then
    step "Using local repository"
    build_dir="$script_dir"
    ok "Found at ${build_dir}"
  else
    step "Cloning repository"
    local tmp; tmp=$(mktemp -d); cleanup="$tmp"
    git clone --depth=1 "$REPO_SSH" "$tmp/codryn" 2>/dev/null || \
    git clone --depth=1 "$REPO_HTTPS" "$tmp/codryn" 2>/dev/null || \
      { rm -rf "$tmp"; die "Failed to clone.\n  Check the GitHub repository path or set CODRYN_GITHUB_REPO=owner/repo."; }
    build_dir="$tmp/codryn"
    ok "Cloned successfully"
  fi

  step "Compiling  ${DIM}(this takes 1–3 minutes)${RESET}"
  cd "$build_dir"
  start_spinner "cargo build --release"
  local log; log=$(mktemp)
  if ! cargo build --release >"$log" 2>&1; then
    stop_spinner
    echo -e "\n  ${RED}${BOLD}Build failed.${RESET} Last 20 lines:"
    echo -e "  ${DIM}─────────────────────────────────────────${RESET}"
    tail -20 "$log" | sed 's/^/    /'
    echo -e "  ${DIM}─────────────────────────────────────────${RESET}"
    rm -f "$log"
    die "cargo build --release failed.\n  • Check Node.js 20+ and Rust are installed\n  • Check npm dependencies can be installed from the public registry"
  fi
  stop_spinner; rm -f "$log"
  ok "Compilation complete"

  install_binary "${build_dir}/target/release/codryn"
  cd /
  [ -n "$cleanup" ] && rm -rf "$cleanup"
}

# ── Main ──────────────────────────────────────────────────
case "${1:-}" in
  uninstall) do_uninstall ;;
  update)
    print_banner
    echo -e "  ${DIM}Platform: ${OS} / ${ARCH}${RESET}"
    echo ""
    step "Updating codryn"
    if command -v codryn &>/dev/null; then
      info "Current: $(codryn --version 2>/dev/null)"
    fi
    # Get latest tag
    LATEST_TAG=$(git ls-remote --tags "$REPO_SSH" 2>/dev/null | grep 'refs/tags/v' | grep -v '\^{}' | sed 's|.*refs/tags/||' | sort -V | tail -1)
    [ -z "$LATEST_TAG" ] && LATEST_TAG=$(git ls-remote --tags "$REPO_HTTPS" 2>/dev/null | grep 'refs/tags/v' | grep -v '\^{}' | sed 's|.*refs/tags/||' | sort -V | tail -1)
    [ -z "$LATEST_TAG" ] && die "No version tags found"
    ok "Latest version: ${LATEST_TAG}"
    step "Cloning ${LATEST_TAG}"
    tmp=$(mktemp -d)
    git clone --depth=1 --branch "$LATEST_TAG" "$REPO_SSH" "$tmp/codryn" 2>/dev/null || \
    git clone --depth=1 --branch "$LATEST_TAG" "$REPO_HTTPS" "$tmp/codryn" 2>/dev/null || \
      { rm -rf "$tmp"; die "Failed to clone"; }
    ok "Cloned ${LATEST_TAG}"
    ensure_rust
    ensure_node
    step "Compiling  ${DIM}(this takes 1–3 minutes)${RESET}"
    cd "$tmp/codryn"
    start_spinner "cargo build --release"
    log=$(mktemp)
    if ! cargo build --release >"$log" 2>&1; then
      stop_spinner; tail -20 "$log" | sed 's/^/    /'; rm -f "$log"
      die "Build failed"
    fi
    stop_spinner; rm -f "$log"
    ok "Compilation complete"
    install_binary "$tmp/codryn/target/release/codryn"
    rm -rf "$tmp"
    echo ""
    VERSION=$(codryn --version 2>/dev/null | awk '{print $NF}')
    echo -e "  ${GREEN}${BOLD}✓ codryn updated to ${VERSION}${RESET}"
    echo ""
    exit 0
    ;;
  *)         ;; # install (default)
esac

print_banner
echo -e "  ${DIM}Platform: ${OS} / ${ARCH}${RESET}"
echo ""

step "Installing codryn"

if try_prebuilt; then
  ok "Installed from pre-built binary"
else
  warn "No pre-built binary for ${OS}/${ARCH}"
  build_and_install
fi

# Ensure PATH
command -v codryn &>/dev/null || export PATH="${INSTALL_DIR}:${PATH}"

step "Finalizing"
start_spinner "Verifying installation"
sleep 1
stop_spinner

echo ""
if command -v codryn &>/dev/null; then
  VERSION=$(codryn --version 2>/dev/null | awk '{print $NF}')
  ok "codryn is ready"
  echo ""
  echo -e "  ${GREEN}${BOLD}  ✓ codryn ${VERSION} installed successfully!${RESET}"
  echo ""
  step "Configuring coding agents"
  codryn install 2>/dev/null && ok "Agent configs updated" || warn "codryn install failed — run manually: codryn install"
  echo ""
  echo -e "  ${BOLD}  Next steps:${RESET}"
  echo -e "  ${CYAN}  1.${RESET} Index your project:            ${DIM}codryn${RESET}  → tell agent: ${DIM}\"Index this project\"${RESET}"
  echo -e "  ${CYAN}  2.${RESET} Open the dashboard:            ${DIM}codryn --ui${RESET}  → http://localhost:9749"
  echo ""
  echo -e "  ${DIM}  Uninstall:  codryn uninstall${RESET}"
  echo ""
else
  warn "codryn installed to ${INSTALL_DIR} but not in PATH."
  echo -e "  Add: ${DIM}export PATH=\"${INSTALL_DIR}:\$PATH\"${RESET}"
  echo ""
fi

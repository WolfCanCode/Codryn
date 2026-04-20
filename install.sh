#!/usr/bin/env sh
# codryn installer — POSIX sh compatible
set -e

GITHUB_REPO="${CODRYN_GITHUB_REPO:-WolfCanCode/Codryn}"
REPO_SSH="git@github.com:${GITHUB_REPO}.git"
REPO_HTTPS="https://github.com/${GITHUB_REPO}.git"
INSTALL_DIR="/usr/local/bin"
BINARY="codryn"

# ── Colors & styles ───────────────────────────────────────
RESET='\033[0m'; BOLD='\033[1m'; DIM='\033[2m'
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BLUE='\033[0;34m'; WHITE='\033[1;37m'

step()    { printf "\n${CYAN}  ▶${RESET} ${BOLD}%s${RESET}\n" "$*"; }
info()    { printf "    ${DIM}%s${RESET}\n" "$*"; }
ok()      { printf "    ${GREEN}✓${RESET} %s\n" "$*"; }
warn()    { printf "    ${YELLOW}⚠${RESET}  %s\n" "$*"; }
die()     { printf "\n  ${RED}✗ Error:${RESET} %s\n\n" "$*" >&2; exit 1; }
progress(){ printf "    ${DIM}%s…${RESET}" "$*"; }
done_()   { printf " ${GREEN}done${RESET}\n"; }

# ── Banner ────────────────────────────────────────────────
print_banner() {
  B="${BOLD}${BLUE}"; R="${RESET}"; W="${WHITE}${BOLD}"; D="${DIM}"; C="${CYAN}"
  printf "\n"
  printf "  ${B}╔═══════════════════════════════════════════════════╗${R}\n"
  printf "  ${B}║${R}                                                   ${B}║${R}\n"
  printf "  ${B}║${R}                   ${C}╔═══════════╗${R}                   ${B}║${R}\n"
  printf "  ${B}║${R}                   ${C}║${R}  ${W}▪${R}     ${W}▪${R}  ${C}║${R}                   ${B}║${R}\n"
  printf "  ${B}║${R}                   ${C}║           ║${R}                   ${B}║${R}\n"
  printf "  ${B}║${R}      ${C}─────────────╢           ╟─────────────${R}      ${B}║${R}\n"
  printf "  ${B}║${R}                   ${C}║           ║${R}                   ${B}║${R}\n"
  printf "  ${B}║${R}                   ${C}╚═══╦═══╦═══╝${R}                   ${B}║${R}\n"
  printf "  ${B}║${R}                       ${C}║   ║${R}                       ${B}║${R}\n"
  printf "  ${B}║${R}                       ${C}╨   ╨${R}                       ${B}║${R}\n"
  printf "  ${B}║${R}                                                   ${B}║${R}\n"
  printf "  ${B}║${R}                 ${W}C  O  D  R  Y  N${R}                  ${B}║${R}\n"
  printf "  ${B}║${R}                  ${D}agent warehouse${R}                  ${B}║${R}\n"
  printf "  ${B}║${R}                                                   ${B}║${R}\n"
  printf "  ${B}╚═══════════════════════════════════════════════════╝${R}\n"
  printf "\n"
}

# ── Spinner ───────────────────────────────────────────────
SPINNER_PID=""
start_spinner() {
  msg="$1"
  [ -t 1 ] || { printf "    ${DIM}%s…${RESET}\n" "$msg"; return; }
  (
    i=0
    while true; do
      case $((i % 10)) in
        0) f='⠋' ;; 1) f='⠙' ;; 2) f='⠹' ;; 3) f='⠸' ;;
        4) f='⠼' ;; 5) f='⠴' ;; 6) f='⠦' ;; 7) f='⠧' ;;
        8) f='⠇' ;; *) f='⠏' ;;
      esac
      printf "\r\033[2K    ${CYAN}%s${RESET}  ${DIM}%s${RESET}" "$f" "$msg"
      i=$((i + 1))
      sleep 0.1
    done
  ) &
  SPINNER_PID=$!
}
stop_spinner() {
  if [ -n "$SPINNER_PID" ]; then
    kill "$SPINNER_PID" 2>/dev/null; wait "$SPINNER_PID" 2>/dev/null || true
    SPINNER_PID=""
    printf "\r\033[2K"
  fi
}
trap 'stop_spinner' EXIT

OS="$(uname -s)"; ARCH="$(uname -m)"

# ── Uninstall ─────────────────────────────────────────────
do_uninstall() {
  print_banner
  step "Uninstalling codryn"

  if command -v codryn >/dev/null 2>&1; then
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

  printf "\n  ${GREEN}${BOLD}✓ Fully uninstalled.${RESET}\n\n"
  exit 0
}

# ── Pre-built binary ──────────────────────────────────────
try_prebuilt() {
  tag=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" \
    2>/dev/null | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
  [ -z "$tag" ] && return 1

  asset=""
  case "${OS}-${ARCH}" in
    Linux-x86_64)   asset="codryn-linux-x86_64.tar.gz" ;;
    Linux-aarch64)  asset="codryn-linux-aarch64.tar.gz" ;;
    Darwin-x86_64)  asset="codryn-macos-x86_64.tar.gz" ;;
    Darwin-arm64)   asset="codryn-macos-aarch64.tar.gz" ;;
    *) return 1 ;;
  esac

  url="https://github.com/${GITHUB_REPO}/releases/download/${tag}/${asset}"
  tmp=$(mktemp -d)

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
  src="$1"; chmod +x "$src"

  if [ -w "$INSTALL_DIR" ]; then
    # Directory is writable — no sudo needed
    progress "Installing to ${INSTALL_DIR}/${BINARY}"
    cp "$src" "${INSTALL_DIR}/${BINARY}"
    done_
  elif [ -t 0 ] && sudo -v 2>/dev/null; then
    # Interactive terminal — can prompt for sudo
    printf "\n"
    info "sudo is required to copy codryn to ${INSTALL_DIR}"
    sudo cp "$src" "${INSTALL_DIR}/${BINARY}"
    ok "Installed to ${INSTALL_DIR}/${BINARY}"
  else
    # Non-interactive (e.g. curl | sh) — fall back to ~/.local/bin
    INSTALL_DIR="${HOME}/.local/bin"
    mkdir -p "$INSTALL_DIR"
    cp "$src" "${INSTALL_DIR}/${BINARY}"
    ok "Installed to ${INSTALL_DIR}/${BINARY}"
    # Auto-add to shell rc if not already there
    shell_rc=""
    rc_updated="false"
    case "${SHELL:-}" in
      */zsh)  shell_rc="${HOME}/.zshrc" ;;
      */bash) shell_rc="${HOME}/.bashrc" ;;
    esac
    if [ -n "$shell_rc" ] && [ -f "$shell_rc" ]; then
      if ! grep -q '\.local/bin' "$shell_rc" 2>/dev/null; then
        printf '\n# codryn\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$shell_rc"
        rc_updated="true"
        ok "Added ${INSTALL_DIR} to ${shell_rc}"
      fi
    fi
    if [ "$rc_updated" = "true" ]; then
      warn "Run: export PATH=\"${INSTALL_DIR}:\$PATH\"  (also added to your shell rc)"
    else
      warn "Run: export PATH=\"${INSTALL_DIR}:\$PATH\""
    fi
  fi

  if [ "$OS" = "Darwin" ]; then
    progress "Code-signing binary (macOS Gatekeeper)"
    codesign --sign - "${INSTALL_DIR}/${BINARY}" 2>/dev/null || \
      sudo codesign --sign - "${INSTALL_DIR}/${BINARY}" 2>/dev/null || true
    done_
  fi
}

# ── Prerequisites ─────────────────────────────────────────
ensure_rust() {
  [ -f "${HOME}/.cargo/env" ] && . "${HOME}/.cargo/env"

  if ! command -v rustup >/dev/null 2>&1; then
    progress "Installing Rust via rustup"
    if ! curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path >/dev/null 2>&1; then
      die "Failed to install Rust. Visit https://rustup.rs"
    fi
    done_
    [ -f "${HOME}/.cargo/env" ] && . "${HOME}/.cargo/env"
  fi

  if ! rustup default 2>/dev/null | grep -q "stable\|nightly\|beta"; then
    progress "Setting default Rust toolchain to stable"
    rustup default stable >/dev/null 2>&1 || die "Failed to set default Rust toolchain. Run: rustup default stable"
    done_
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    die "Rust installed but cargo not in PATH. Run: source ~/.cargo/env  Then re-run."
  fi

  shell_rc=""
  case "${SHELL:-}" in
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
  if ! command -v node >/dev/null 2>&1; then
    step "Installing Node.js"
    if [ "$OS" = "Darwin" ]; then
      if command -v brew >/dev/null 2>&1; then
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
      curl -fsSL https://deb.nodesource.com/setup_22.x | sh - >/dev/null 2>&1
      apt-get install -y nodejs >/dev/null 2>&1 || \
        yum install -y nodejs >/dev/null 2>&1 || \
        die "Failed to install Node.js. Install from https://nodejs.org and re-run."
      done_
    fi
  fi
  v=$(node -e "process.stdout.write(process.versions.node.split('.')[0])")
  [ "$v" -lt 20 ] && die "Node.js 20+ required (found v${v}). Upgrade from https://nodejs.org"
  ok "Node.js v$(node --version | tr -d v)"
}

# ── Build ─────────────────────────────────────────────────
build_and_install() {
  step "Checking prerequisites"
  ensure_rust
  ensure_node

  build_dir=""; cleanup=""
  script_dir="$(cd "$(dirname "${0}")" 2>/dev/null && pwd || printf "")"

  if [ -n "$script_dir" ] && [ -f "${script_dir}/Cargo.toml" ] && grep -q "codryn-bin" "${script_dir}/Cargo.toml" 2>/dev/null; then
    step "Using local repository"
    build_dir="$script_dir"
    ok "Found at ${build_dir}"
  else
    step "Cloning repository"
    tmp=$(mktemp -d); cleanup="$tmp"
    git clone --depth=1 "$REPO_SSH" "$tmp/codryn" 2>/dev/null || \
    git clone --depth=1 "$REPO_HTTPS" "$tmp/codryn" 2>/dev/null || \
      { rm -rf "$tmp"; die "Failed to clone. Check the GitHub repository path or set CODRYN_GITHUB_REPO=owner/repo."; }
    build_dir="$tmp/codryn"
    ok "Cloned successfully"
  fi

  step "Compiling (this takes 1-3 minutes)"
  cd "$build_dir"
  start_spinner "cargo build --release"
  log=$(mktemp)
  if ! cargo build --release >"$log" 2>&1; then
    stop_spinner
    printf "\n  ${RED}${BOLD}Build failed.${RESET} Last 20 lines:\n"
    printf "  ${DIM}─────────────────────────────────────────${RESET}\n"
    tail -20 "$log" | sed 's/^/    /'
    printf "  ${DIM}─────────────────────────────────────────${RESET}\n"
    rm -f "$log"
    die "cargo build --release failed.\n  Check Node.js 20+ and Rust are installed."
  fi
  stop_spinner; rm -f "$log"
  ok "Compilation complete"

  install_binary "${build_dir}/target/release/codryn"
  cd /
  if [ -n "$cleanup" ]; then
    rm -rf "$cleanup"
  fi
}

# ── Main ──────────────────────────────────────────────────
case "${1:-}" in
  uninstall) do_uninstall ;;
  update)
    print_banner
    printf "  ${DIM}Platform: ${OS} / ${ARCH}${RESET}\n\n"
    step "Updating codryn"
    if command -v codryn >/dev/null 2>&1; then
      info "Current: $(codryn --version 2>/dev/null)"
    fi
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
    step "Compiling (this takes 1-3 minutes)"
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
    printf "\n"
    VERSION=$(codryn --version 2>/dev/null | awk '{print $NF}')
    printf "  ${GREEN}${BOLD}✓ codryn updated to %s${RESET}\n\n" "$VERSION"
    exit 0
    ;;
  *)  ;; # install (default)
esac

print_banner
printf "  ${DIM}Platform: ${OS} / ${ARCH}${RESET}\n\n"

step "Installing codryn"

if try_prebuilt; then
  ok "Installed from pre-built binary"
else
  warn "No pre-built binary for ${OS}/${ARCH} — building from source"
  build_and_install
fi

# Ensure PATH includes install dir for remainder of script
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *) PATH="${INSTALL_DIR}:${PATH}"; export PATH ;;
esac

step "Finalizing"
start_spinner "Verifying installation"
sleep 1
stop_spinner

printf "\n"
if command -v codryn >/dev/null 2>&1; then
  VERSION=$(codryn --version 2>/dev/null | awk '{print $NF}')
  printf "\n  ${GREEN}${BOLD}✓ codryn %s installed successfully!${RESET}\n\n" "$VERSION"

  step "Configuring coding agents"
  if codryn install 2>/dev/null; then
    ok "Agent configs updated"
  else
    warn "codryn install failed — run manually: codryn install"
  fi

  printf "\n"
  printf "  ${BOLD}Available commands:${RESET}\n"
  printf "  ${CYAN}  codryn install${RESET}     Auto-configure MCP for all detected coding agents\n"
  printf "  ${CYAN}  codryn status${RESET}      Show which agents are configured\n"
  printf "  ${CYAN}  codryn update${RESET}      Update codryn to the latest version\n"
  printf "  ${CYAN}  codryn uninstall${RESET}   Remove MCP config and binary\n"
  printf "  ${CYAN}  codryn --ui${RESET}        Open web dashboard at http://localhost:9749\n"
  printf "\n"
  printf "  ${BOLD}Next steps:${RESET}\n"
  printf "  ${CYAN}  1.${RESET} Open your project in your coding agent\n"
  printf "  ${CYAN}  2.${RESET} Tell the agent: ${DIM}\"Index this project\"${RESET}\n"
  printf "  ${CYAN}  3.${RESET} Ask anything about your codebase!\n"
  printf "\n"
  printf "  ${DIM}Uninstall: codryn uninstall${RESET}\n"
  printf "\n"
else
  warn "codryn installed to ${INSTALL_DIR} but not in PATH."
  printf "  Add to PATH: ${DIM}export PATH=\"${INSTALL_DIR}:\$PATH\"${RESET}\n"
  printf "  Then run:    ${DIM}codryn install${RESET}\n"
  printf "\n"
fi

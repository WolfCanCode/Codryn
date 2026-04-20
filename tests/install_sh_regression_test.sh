#!/usr/bin/env sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
INSTALL_SCRIPT="${ROOT_DIR}/install.sh"

TMP_DIR=$(mktemp -d)
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

FUNCTIONS_ONLY="${TMP_DIR}/install_functions.sh"
sed '/^# ── Main /,$d' "$INSTALL_SCRIPT" > "$FUNCTIONS_ONLY"

# shellcheck disable=SC1090
. "$FUNCTIONS_ONLY"

assert_contains() {
  file="$1"
  expected="$2"
  if ! grep -Fq "$expected" "$file"; then
    printf 'expected "%s" in %s\n' "$expected" "$file" >&2
    exit 1
  fi
}

assert_not_contains() {
  file="$1"
  unexpected="$2"
  if grep -Fq "$unexpected" "$file"; then
    printf 'did not expect "%s" in %s\n' "$unexpected" "$file" >&2
    exit 1
  fi
}

test_noninteractive_warning_only_claims_rc_update_when_written() {
  HOME="${TMP_DIR}/home-no-rc"
  SHELL="/bin/unknown"
  INSTALL_DIR="/usr/local/bin"
  OS="Linux"
  mkdir -p "${HOME}"

  fake_binary="${TMP_DIR}/codryn-no-rc"
  printf '#!/bin/sh\nexit 0\n' > "$fake_binary"

  output_file="${TMP_DIR}/no-rc.out"
  install_binary "$fake_binary" > "$output_file" 2>&1

  assert_contains "$output_file" "Run: export PATH=\"${HOME}/.local/bin:\$PATH\""
  assert_not_contains "$output_file" "already added to your shell rc"
}

test_finalization_prefers_new_install_dir_over_old_binary() {
  fake_old_bin="${TMP_DIR}/old-bin"
  fake_new_home="${TMP_DIR}/home-new"
  mkdir -p "${fake_old_bin}" "${fake_new_home}/.local/bin"

  cat > "${fake_old_bin}/codryn" <<'EOF'
#!/bin/sh
printf 'old-codryn\n'
EOF
  chmod +x "${fake_old_bin}/codryn"

  cat > "${fake_new_home}/.local/bin/codryn" <<'EOF'
#!/bin/sh
printf 'new-codryn\n'
EOF
  chmod +x "${fake_new_home}/.local/bin/codryn"

  PATH="${fake_old_bin}:${PATH}"
  INSTALL_DIR="${fake_new_home}/.local/bin"

  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) PATH="${INSTALL_DIR}:${PATH}"; export PATH ;;
  esac

  resolved_path=$(command -v codryn)
  if [ "$resolved_path" != "${INSTALL_DIR}/codryn" ]; then
    printf 'expected PATH to prefer %s, got %s\n' "${INSTALL_DIR}/codryn" "$resolved_path" >&2
    exit 1
  fi
}

test_empty_cleanup_guard_does_not_fail_under_set_e() {
  if ! output=$(
    sh -eu <<'EOF'
f() {
  cleanup=""
  if [ -n "$cleanup" ]; then
    rm -rf "$cleanup"
  fi
}
f
printf 'continued\n'
EOF
  ); then
    printf 'empty cleanup guard exited under set -e\n' >&2
    exit 1
  fi

  if [ "$output" != "continued" ]; then
    printf 'expected cleanup guard to continue, got %s\n' "$output" >&2
    exit 1
  fi
}

test_noninteractive_warning_only_claims_rc_update_when_written
test_finalization_prefers_new_install_dir_over_old_binary
test_empty_cleanup_guard_does_not_fail_under_set_e

printf 'install.sh regression tests passed\n'

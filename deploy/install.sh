#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${TASKD_REPO_URL:-https://github.com/eric9n/taskd.git}"
REPO_REF="${TASKD_REPO_REF:-main}"
INSTALL_DIR="${TASKD_INSTALL_DIR:-/opt/taskd}"
CONFIG_DIR="${TASKD_CONFIG_DIR:-/etc/taskd}"
SYSTEMD_UNIT_PATH="${TASKD_SYSTEMD_UNIT_PATH:-/etc/systemd/system/taskd.service}"
BUILD_ROOT="${TASKD_BUILD_ROOT:-/tmp/taskd-build}"
RUST_TOOLCHAIN="${TASKD_RUST_TOOLCHAIN:-stable}"
RUST_LOG_VALUE="${TASKD_RUST_LOG:-info}"

log() {
  printf '[taskd-deploy] %s\n' "$*"
}

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    echo "please run as root" >&2
    exit 1
  fi
}

ensure_apt_packages() {
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    curl \
    git \
    pkg-config
}

ensure_rust() {
  if command -v cargo >/dev/null 2>&1; then
    return
  fi

  log "installing rust toolchain"
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "${RUST_TOOLCHAIN}"
}

load_rust_env() {
  if [[ -f "${HOME}/.cargo/env" ]]; then
    # shellcheck disable=SC1090
    source "${HOME}/.cargo/env"
  fi
}

checkout_source() {
  rm -rf "${BUILD_ROOT}"
  git clone --depth 1 --branch "${REPO_REF}" "${REPO_URL}" "${BUILD_ROOT}"
}

build_binary() {
  load_rust_env
  cargo build --release --manifest-path "${BUILD_ROOT}/Cargo.toml"
}

install_files() {
  install -d "${INSTALL_DIR}" "${CONFIG_DIR}"
  install -m 0755 "${BUILD_ROOT}/target/release/taskd" "${INSTALL_DIR}/taskd"
  install -m 0755 "${BUILD_ROOT}/target/release/taskctl" "${INSTALL_DIR}/taskctl"

  if [[ ! -f "${CONFIG_DIR}/tasks.yaml" ]]; then
    install -m 0644 "${BUILD_ROOT}/config/tasks.yaml" "${CONFIG_DIR}/tasks.yaml"
  fi

  sed \
    -e "s|__TASKD_INSTALL_DIR__|${INSTALL_DIR}|g" \
    -e "s|__TASKD_CONFIG_DIR__|${CONFIG_DIR}|g" \
    -e "s|__TASKD_RUST_LOG__|${RUST_LOG_VALUE}|g" \
    "${BUILD_ROOT}/deploy/taskd.service" > "${SYSTEMD_UNIT_PATH}"
}

start_service() {
  systemctl daemon-reload
  systemctl enable --now taskd
  systemctl restart taskd
  systemctl --no-pager --full status taskd
}

main() {
  require_root
  log "installing system packages"
  ensure_apt_packages
  ensure_rust
  load_rust_env
  log "cloning ${REPO_URL}#${REPO_REF}"
  checkout_source
  log "building release binary"
  build_binary
  log "installing taskd binaries and config"
  install_files
  log "starting systemd service"
  start_service
  log "deployment complete"
}

main "$@"

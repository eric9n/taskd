#!/usr/bin/env bash
set -euo pipefail

GITHUB_REPOSITORY="${TASKD_GITHUB_REPOSITORY:-eric9n/taskd}"
TASKD_RELEASE="${TASKD_RELEASE:-latest}"
TASKD_ASSET_NAME="${TASKD_ASSET_NAME:-taskd-x86_64-unknown-linux-gnu.tar.gz}"
INSTALL_DIR="${TASKD_INSTALL_DIR:-/opt/taskd}"
CONFIG_DIR="${TASKD_CONFIG_DIR:-/etc/taskd}"
DATA_DIR="${TASKD_DATA_DIR:-/var/lib/taskd}"
SYSTEMD_UNIT_PATH="${TASKD_SYSTEMD_UNIT_PATH:-/etc/systemd/system/taskd.service}"
DOWNLOAD_ROOT="${TASKD_DOWNLOAD_ROOT:-/tmp/taskd-release}"
RUST_LOG_VALUE="${TASKD_RUST_LOG:-info}"

ARCHIVE_PATH="${DOWNLOAD_ROOT}/${TASKD_ASSET_NAME}"
ARCHIVE_ROOT="${TASKD_ASSET_NAME%.tar.gz}"
EXTRACT_ROOT="${DOWNLOAD_ROOT}/extract"

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
    ca-certificates \
    curl \
    tar
}

release_base_url() {
  if [[ "${TASKD_RELEASE}" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download' "${GITHUB_REPOSITORY}"
  else
    printf 'https://github.com/%s/releases/download/%s' "${GITHUB_REPOSITORY}" "${TASKD_RELEASE}"
  fi
}

download_release() {
  local base_url checksum_path checksum_url

  rm -rf "${DOWNLOAD_ROOT}"
  install -d "${EXTRACT_ROOT}"

  base_url="$(release_base_url)"
  log "downloading ${TASKD_ASSET_NAME} from ${base_url}"
  curl -fL "${base_url}/${TASKD_ASSET_NAME}" -o "${ARCHIVE_PATH}"

  checksum_url="${base_url}/${TASKD_ASSET_NAME}.sha256"
  checksum_path="${ARCHIVE_PATH}.sha256"
  if curl -fsSL "${checksum_url}" -o "${checksum_path}"; then
    (
      cd "${DOWNLOAD_ROOT}"
      sha256sum -c "$(basename "${checksum_path}")"
    )
  else
    log "checksum file not found, skipping verification"
  fi

  tar -xzf "${ARCHIVE_PATH}" -C "${EXTRACT_ROOT}"
}

release_root() {
  printf '%s/%s' "${EXTRACT_ROOT}" "${ARCHIVE_ROOT}"
}

install_files() {
  local root
  root="$(release_root)"

  if [[ ! -x "${root}/bin/taskd" || ! -x "${root}/bin/taskctl" ]]; then
    echo "release archive is missing taskd binaries" >&2
    exit 1
  fi

  install -d "${INSTALL_DIR}" "${CONFIG_DIR}" "${DATA_DIR}"
  install -m 0755 "${root}/bin/taskd" "${INSTALL_DIR}/taskd"
  install -m 0755 "${root}/bin/taskctl" "${INSTALL_DIR}/taskctl"

  if [[ ! -f "${CONFIG_DIR}/tasks.yaml" ]]; then
    install -m 0644 "${root}/config/tasks.yaml" "${CONFIG_DIR}/tasks.yaml"
  fi
  if [[ ! -f "${CONFIG_DIR}/taskd.env.example" ]]; then
    install -m 0644 "${root}/config/taskd.env.example" "${CONFIG_DIR}/taskd.env.example"
  fi

  if [[ "${CONFIG_DIR}" == "/etc/taskd" ]]; then
    migrate_runtime_data
  fi

  sed \
    -e "s|__TASKD_INSTALL_DIR__|${INSTALL_DIR}|g" \
    -e "s|__TASKD_CONFIG_DIR__|${CONFIG_DIR}|g" \
    -e "s|__TASKD_RUST_LOG__|${RUST_LOG_VALUE}|g" \
    "${root}/deploy/taskd.service" > "${SYSTEMD_UNIT_PATH}"
}

migrate_runtime_data() {
  if [[ -f "${CONFIG_DIR}/tasks.state.yaml" && ! -f "${DATA_DIR}/tasks.state.yaml" ]]; then
    mv "${CONFIG_DIR}/tasks.state.yaml" "${DATA_DIR}/tasks.state.yaml"
  fi

  if [[ -f "${CONFIG_DIR}/tasks.history.db" && ! -f "${DATA_DIR}/tasks.history.db" ]]; then
    mv "${CONFIG_DIR}/tasks.history.db" "${DATA_DIR}/tasks.history.db"
  fi
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
  download_release
  log "installing taskd binaries and config"
  install_files
  log "starting systemd service"
  start_service
  log "deployment complete"
}

main "$@"

//! Default config path resolution for daemon and control-plane binaries.

use std::path::{Path, PathBuf};

const SYSTEM_CONFIG_PATH: &str = "/etc/taskd/tasks.yaml";
const LOCAL_CONFIG_PATH: &str = "config/tasks.yaml";
const SYSTEM_ARTIFACTS_CONFIG_PATH: &str = "/etc/taskd/artifacts.yaml";
const LOCAL_ARTIFACTS_CONFIG_PATH: &str = "config/artifacts.yaml";

pub fn default_config_path() -> PathBuf {
    resolve_default_config_path(Path::new(SYSTEM_CONFIG_PATH), Path::new(LOCAL_CONFIG_PATH))
}

pub fn default_artifacts_config_path() -> PathBuf {
    resolve_default_config_path(
        Path::new(SYSTEM_ARTIFACTS_CONFIG_PATH),
        Path::new(LOCAL_ARTIFACTS_CONFIG_PATH),
    )
}

pub(crate) fn resolve_default_config_path(system_path: &Path, local_path: &Path) -> PathBuf {
    if system_path.exists() {
        system_path.to_path_buf()
    } else {
        local_path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::resolve_default_config_path;

    #[test]
    fn prefers_system_config_when_present() {
        let dir = tempdir().expect("tempdir");
        let system_path = dir.path().join("etc/taskd/tasks.yaml");
        let local_path = dir.path().join("config/tasks.yaml");
        fs::create_dir_all(system_path.parent().expect("system parent")).expect("mkdir system");
        fs::write(&system_path, "version: 1\ntasks: []\n").expect("write system config");

        let resolved = resolve_default_config_path(&system_path, &local_path);

        assert_eq!(resolved, system_path);
    }

    #[test]
    fn falls_back_to_local_config_when_system_config_is_missing() {
        let dir = tempdir().expect("tempdir");
        let system_path = dir.path().join("etc/taskd/tasks.yaml");
        let local_path = dir.path().join("config/tasks.yaml");

        let resolved = resolve_default_config_path(&system_path, &local_path);

        assert_eq!(resolved, local_path);
    }
}

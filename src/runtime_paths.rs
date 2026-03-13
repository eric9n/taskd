//! Runtime data path helpers for persisted task state and history.

use std::path::{Path, PathBuf};

const SYSTEM_CONFIG_DIR: &str = "/etc/taskd";
const SYSTEM_DATA_DIR: &str = "/var/lib/taskd";

pub fn runtime_data_dir_for_config(config_path: &Path) -> PathBuf {
    if config_path.parent() == Some(Path::new(SYSTEM_CONFIG_DIR)) {
        PathBuf::from(SYSTEM_DATA_DIR)
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }
}

pub fn runtime_data_path_for_config(config_path: &Path, extension: &str) -> PathBuf {
    let stem = config_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("tasks");
    runtime_data_dir_for_config(config_path).join(format!("{stem}.{extension}"))
}

pub fn last_good_config_path_for_config(config_path: &Path) -> PathBuf {
    runtime_data_path_for_config(config_path, "last-good.yaml")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        last_good_config_path_for_config, runtime_data_dir_for_config, runtime_data_path_for_config,
    };

    #[test]
    fn uses_var_lib_for_default_system_config_dir() {
        let config_path = Path::new("/etc/taskd/tasks.yaml");

        assert_eq!(
            runtime_data_dir_for_config(config_path),
            PathBuf::from("/var/lib/taskd")
        );
        assert_eq!(
            runtime_data_path_for_config(config_path, "state.yaml"),
            PathBuf::from("/var/lib/taskd/tasks.state.yaml")
        );
    }

    #[test]
    fn uses_config_sibling_for_non_system_config_dir() {
        let config_path = Path::new("/tmp/taskd/tasks.yaml");

        assert_eq!(
            runtime_data_dir_for_config(config_path),
            PathBuf::from("/tmp/taskd")
        );
        assert_eq!(
            runtime_data_path_for_config(config_path, "history.db"),
            PathBuf::from("/tmp/taskd/tasks.history.db")
        );
    }

    #[test]
    fn derives_last_good_config_path() {
        assert_eq!(
            last_good_config_path_for_config(Path::new("/etc/taskd/tasks.yaml")),
            PathBuf::from("/var/lib/taskd/tasks.last-good.yaml")
        );
        assert_eq!(
            last_good_config_path_for_config(Path::new("/tmp/taskd/tasks.yaml")),
            PathBuf::from("/tmp/taskd/tasks.last-good.yaml")
        );
    }
}

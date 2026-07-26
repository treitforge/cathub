//! Runtime endpoint publication for launcher-managed CatHub processes.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::CatHubError;

const SCHEMA_VERSION: u32 = 1;

/// One effective Hamlib network endpoint.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HamlibEndpoint {
    pub(crate) name: String,
    pub(crate) endpoint: String,
}

/// Effective endpoints for one ready CatHub process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RuntimeInfo {
    pub(crate) schema_version: u32,
    pub(crate) pid: u32,
    pub(crate) winkeyer_endpoint: Option<String>,
    #[serde(default, skip_deserializing)]
    pub(crate) hamlib_endpoints: Vec<HamlibEndpoint>,
}

impl RuntimeInfo {
    pub(crate) fn new(
        winkeyer_endpoint: Option<String>,
        hamlib_endpoints: Vec<HamlibEndpoint>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            pid: std::process::id(),
            winkeyer_endpoint,
            hamlib_endpoints,
        }
    }
}

/// Removes a runtime file when the publishing process exits normally.
pub(crate) struct RuntimeInfoLease {
    path: PathBuf,
    pid: u32,
}

impl Drop for RuntimeInfoLease {
    fn drop(&mut self) {
        let belongs_to_process = fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RuntimeInfo>(&bytes).ok())
            .is_some_and(|info| info.pid == self.pid);
        if belongs_to_process {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Publish effective endpoints after all configured listeners bind.
pub(crate) fn publish(path: &Path, info: &RuntimeInfo) -> Result<RuntimeInfoLease, CatHubError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(info)
        .map_err(|error| CatHubError::Backend(format!("cannot serialize runtime info: {error}")))?;
    let temporary = path.with_extension(format!("tmp-{}", info.pid));
    fs::write(&temporary, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temporary, path)?;
    tracing::info!(path = %path.display(), "published runtime endpoint information");
    Ok(RuntimeInfoLease {
        path: path.to_path_buf(),
        pid: info.pid,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn publishes_and_removes_runtime_info() {
        let directory =
            std::env::temp_dir().join(format!("cathub-runtime-info-test-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("runtime.json");
        let info = RuntimeInfo::new(
            Some("http://127.0.0.1:54321".to_string()),
            vec![HamlibEndpoint {
                name: "engine".to_string(),
                endpoint: "127.0.0.1:4532".to_string(),
            }],
        );

        let lease = publish(&path, &info).expect("publish runtime info");
        let published: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read runtime info"))
                .expect("parse runtime info");
        assert_eq!(published["schema_version"], 1);
        assert_eq!(published["pid"], std::process::id());
        assert_eq!(published["winkeyer_endpoint"], "http://127.0.0.1:54321");
        assert_eq!(
            published["hamlib_endpoints"][0]["endpoint"],
            "127.0.0.1:4532"
        );

        drop(lease);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(directory);
    }
}

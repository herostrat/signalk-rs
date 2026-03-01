/// File-based resource provider — stores resources as JSON files on disk.
///
/// Directory layout:
/// ```text
/// {base_dir}/
///   waypoints/
///     {uuid}.json
///   routes/
///     {uuid}.json
///   ...
/// ```
use async_trait::async_trait;
use serde_json::Value;
use signalk_plugin_api::{PluginError, ResourceProvider};
use signalk_types::v2::ResourceQueryParams;
use std::path::PathBuf;
use tracing::debug;

pub struct FileResourceProvider {
    base_dir: PathBuf,
}

impl FileResourceProvider {
    pub fn new(base_dir: PathBuf) -> Self {
        FileResourceProvider { base_dir }
    }

    /// Resolve the directory for a resource type, rejecting path traversal.
    fn type_dir(&self, resource_type: &str) -> Result<PathBuf, PluginError> {
        reject_traversal(resource_type)?;
        Ok(self.base_dir.join(resource_type))
    }

    /// Resolve the file path for a specific resource, rejecting path traversal.
    fn resource_path(&self, resource_type: &str, id: &str) -> Result<PathBuf, PluginError> {
        reject_traversal(resource_type)?;
        reject_traversal(id)?;
        Ok(self.base_dir.join(resource_type).join(format!("{id}.json")))
    }
}

/// Reject path components containing `..` or `/` to prevent traversal attacks.
fn reject_traversal(component: &str) -> Result<(), PluginError> {
    if component.contains("..") || component.contains('/') || component.contains('\\') {
        return Err(PluginError::config(format!(
            "invalid path component: {component}"
        )));
    }
    Ok(())
}

#[async_trait]
impl ResourceProvider for FileResourceProvider {
    async fn list(
        &self,
        resource_type: &str,
        query: &ResourceQueryParams,
    ) -> Result<Value, PluginError> {
        let dir = self.type_dir(resource_type)?;

        if !dir.exists() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| PluginError::runtime(format!("failed to read {dir:?}: {e}")))?;

        let mut result = serde_json::Map::new();
        let limit = query.limit.unwrap_or(usize::MAX);

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| PluginError::runtime(format!("read dir entry: {e}")))?
        {
            if result.len() >= limit {
                break;
            }

            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();

            match tokio::fs::read_to_string(&path).await {
                Ok(contents) => match serde_json::from_str::<Value>(&contents) {
                    Ok(value) => {
                        result.insert(id, value);
                    }
                    Err(e) => {
                        debug!("skipping malformed JSON {path:?}: {e}");
                    }
                },
                Err(e) => {
                    debug!("skipping unreadable file {path:?}: {e}");
                }
            }
        }

        Ok(Value::Object(result))
    }

    async fn get(&self, resource_type: &str, id: &str) -> Result<Option<Value>, PluginError> {
        let path = self.resource_path(resource_type, id)?;

        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let value: Value = serde_json::from_str(&contents).map_err(|e| {
                    PluginError::runtime(format!("malformed JSON in {path:?}: {e}"))
                })?;
                Ok(Some(value))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(PluginError::runtime(format!(
                "failed to read {path:?}: {e}"
            ))),
        }
    }

    async fn create(&self, resource_type: &str, value: Value) -> Result<String, PluginError> {
        let id = uuid::Uuid::new_v4().to_string();
        let path = self.resource_path(resource_type, &id)?;

        // Ensure the type directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| PluginError::runtime(format!("mkdir {parent:?}: {e}")))?;
        }

        // Atomic write: write to temp file, then rename
        let tmp_path = path.with_extension("json.tmp");
        let contents = serde_json::to_string_pretty(&value)
            .map_err(|e| PluginError::runtime(format!("serialize: {e}")))?;

        tokio::fs::write(&tmp_path, &contents)
            .await
            .map_err(|e| PluginError::runtime(format!("write {tmp_path:?}: {e}")))?;

        tokio::fs::rename(&tmp_path, &path)
            .await
            .map_err(|e| PluginError::runtime(format!("rename {tmp_path:?} → {path:?}: {e}")))?;

        Ok(id)
    }

    async fn update(&self, resource_type: &str, id: &str, value: Value) -> Result<(), PluginError> {
        let path = self.resource_path(resource_type, id)?;

        if !path.exists() {
            return Err(PluginError::not_found(format!("{resource_type}/{id}")));
        }

        let tmp_path = path.with_extension("json.tmp");
        let contents = serde_json::to_string_pretty(&value)
            .map_err(|e| PluginError::runtime(format!("serialize: {e}")))?;

        tokio::fs::write(&tmp_path, &contents)
            .await
            .map_err(|e| PluginError::runtime(format!("write {tmp_path:?}: {e}")))?;

        tokio::fs::rename(&tmp_path, &path)
            .await
            .map_err(|e| PluginError::runtime(format!("rename {tmp_path:?} → {path:?}: {e}")))?;

        Ok(())
    }

    async fn delete(&self, resource_type: &str, id: &str) -> Result<(), PluginError> {
        let path = self.resource_path(resource_type, id)?;

        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(PluginError::not_found(format!("{resource_type}/{id}")))
            }
            Err(e) => Err(PluginError::runtime(format!("delete {path:?}: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn crud_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileResourceProvider::new(tmp.path().to_path_buf());

        // Create
        let id = provider
            .create("waypoints", serde_json::json!({"name": "Test WP", "position": {"latitude": 49.0, "longitude": -123.0}}))
            .await
            .unwrap();
        assert!(!id.is_empty());

        // Get
        let value = provider.get("waypoints", &id).await.unwrap().unwrap();
        assert_eq!(value["name"], "Test WP");

        // List
        let list = provider
            .list("waypoints", &ResourceQueryParams::default())
            .await
            .unwrap();
        assert_eq!(list.as_object().unwrap().len(), 1);
        assert!(list.get(&id).is_some());

        // Update
        provider
            .update("waypoints", &id, serde_json::json!({"name": "Updated WP"}))
            .await
            .unwrap();
        let updated = provider.get("waypoints", &id).await.unwrap().unwrap();
        assert_eq!(updated["name"], "Updated WP");

        // Delete
        provider.delete("waypoints", &id).await.unwrap();
        assert!(provider.get("waypoints", &id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileResourceProvider::new(tmp.path().to_path_buf());

        let result = provider.get("waypoints", "no-such-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileResourceProvider::new(tmp.path().to_path_buf());

        let result = provider.delete("waypoints", "no-such-id").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_empty_returns_empty_object() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileResourceProvider::new(tmp.path().to_path_buf());

        let list = provider
            .list("waypoints", &ResourceQueryParams::default())
            .await
            .unwrap();
        assert_eq!(list, serde_json::json!({}));
    }

    #[tokio::test]
    async fn list_with_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileResourceProvider::new(tmp.path().to_path_buf());

        // Create 3 resources
        for i in 0..3 {
            provider
                .create("routes", serde_json::json!({"name": format!("Route {i}")}))
                .await
                .unwrap();
        }

        let query = ResourceQueryParams {
            limit: Some(2),
            ..Default::default()
        };
        let list = provider.list("routes", &query).await.unwrap();
        assert_eq!(list.as_object().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn path_traversal_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileResourceProvider::new(tmp.path().to_path_buf());

        assert!(provider.get("../etc", "passwd").await.is_err());
        assert!(provider.get("waypoints", "../../etc/passwd").await.is_err());
        assert!(provider.get("way/points", "id").await.is_err());
    }

    #[tokio::test]
    async fn update_nonexistent_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileResourceProvider::new(tmp.path().to_path_buf());

        let result = provider
            .update(
                "waypoints",
                "no-such-id",
                serde_json::json!({"name": "test"}),
            )
            .await;
        assert!(result.is_err());
    }
}

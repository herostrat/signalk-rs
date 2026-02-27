/// Webapp discovery and registry — serves static files from SignalK webapp packages.
///
/// SignalK webapps are npm packages with `"signalk-webapp"` in their `keywords`.
/// They contain a `public/` directory with HTML/JS/CSS that gets served by the
/// Rust server via `tower-http::ServeDir`.
///
/// The `WebappRegistry` supports multiple sources:
/// - **npm Discovery** (startup): Scans `node_modules` for `signalk-webapp` keyword
/// - **Tier 1 Rust plugins** (runtime): Plugins call `ctx.register_webapp()`
/// - **Tier 2 Bridge plugins** (runtime): Bridge reports plugin webapps
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// How this webapp was discovered / registered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum WebappSource {
    /// Discovered from node_modules at startup
    NpmPackage,
    /// Registered by a Tier 1 Rust plugin at runtime
    RustPlugin { plugin_id: String },
    /// Registered by a Tier 2 Bridge plugin at runtime
    BridgePlugin { plugin_id: String },
}

/// Metadata for a discovered/registered webapp.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAppInfo {
    /// npm package name or plugin-scoped name, e.g. "@signalk/instrumentpanel"
    pub name: String,
    /// SemVer version
    pub version: String,
    /// Human-readable display name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// URL path this webapp is served at, e.g. "/@signalk/instrumentpanel"
    pub url: String,
    /// Absolute path to the static files directory
    #[serde(skip)]
    pub public_dir: PathBuf,
    /// How this webapp was discovered
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<WebappSource>,
}

/// Central registry for all webapps, regardless of origin.
#[derive(Debug, Default)]
pub struct WebappRegistry {
    entries: Vec<WebAppInfo>,
}

impl WebappRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a webapp entry.
    pub fn register(&mut self, info: WebAppInfo) {
        // Avoid duplicates by name
        self.entries.retain(|e| e.name != info.name);
        self.entries.push(info);
    }

    /// Get all registered webapps.
    pub fn all(&self) -> &[WebAppInfo] {
        &self.entries
    }

    /// Find a webapp by name.
    pub fn find(&self, name: &str) -> Option<&WebAppInfo> {
        self.entries.iter().find(|e| e.name == name)
    }
}

/// Scan a `node_modules` directory for packages with the `signalk-webapp` keyword.
///
/// Returns a list of `WebAppInfo` for each package that has:
/// 1. `"signalk-webapp"` in `package.json` `keywords`
/// 2. A `public/` directory with static files
pub fn discover_webapps(modules_dir: &Path) -> Vec<WebAppInfo> {
    let mut webapps = Vec::new();

    if !modules_dir.exists() {
        debug!(dir = %modules_dir.display(), "node_modules directory not found, skipping webapp discovery");
        return webapps;
    }

    let entries = match std::fs::read_dir(modules_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(dir = %modules_dir.display(), error = %e, "Failed to read node_modules");
            return webapps;
        }
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Handle scoped packages: @scope/package-name
        if name.starts_with('@') && entry_path.is_dir() {
            if let Ok(scoped) = std::fs::read_dir(&entry_path) {
                for scoped_entry in scoped.flatten() {
                    let scoped_path = scoped_entry.path();
                    let scoped_name =
                        format!("{}/{}", name, scoped_entry.file_name().to_string_lossy());
                    if let Some(info) = try_read_webapp(&scoped_path, &scoped_name) {
                        webapps.push(info);
                    }
                }
            }
            continue;
        }

        if let Some(info) = try_read_webapp(&entry_path, &name) {
            webapps.push(info);
        }
    }

    webapps
}

/// Try to read a webapp from a package directory.
fn try_read_webapp(package_path: &Path, package_name: &str) -> Option<WebAppInfo> {
    let pkg_json_path = package_path.join("package.json");
    let content = std::fs::read_to_string(&pkg_json_path).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&content).ok()?;

    // Check for signalk-webapp keyword
    let keywords = pkg.get("keywords")?.as_array()?;
    let is_webapp = keywords
        .iter()
        .any(|k| k.as_str() == Some("signalk-webapp"));

    if !is_webapp {
        return None;
    }

    // Must have a public/ directory
    let public_dir = package_path.join("public");
    if !public_dir.is_dir() {
        debug!(package = %package_name, "signalk-webapp without public/ directory, skipping");
        return None;
    }

    let version = pkg
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();

    let description = pkg
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Try signalk.displayName first, then fallback to package name
    let display_name = pkg
        .get("signalk")
        .and_then(|sk| sk.get("displayName"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // URL: /{package_name} (preserving scoped names)
    let url = format!("/{package_name}");

    Some(WebAppInfo {
        name: package_name.to_string(),
        version,
        display_name,
        description,
        url,
        public_dir,
        source: Some(WebappSource::NpmPackage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_fake_webapp(modules_dir: &Path, name: &str, has_public: bool) {
        let pkg_dir = modules_dir.join(name);
        fs::create_dir_all(&pkg_dir).unwrap();

        let pkg_json = serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "description": format!("Fake webapp {name}"),
            "keywords": ["signalk-webapp"],
            "signalk": { "displayName": format!("Fake {name}") }
        });
        fs::write(
            pkg_dir.join("package.json"),
            serde_json::to_string_pretty(&pkg_json).unwrap(),
        )
        .unwrap();

        if has_public {
            let public_dir = pkg_dir.join("public");
            fs::create_dir_all(&public_dir).unwrap();
            fs::write(public_dir.join("index.html"), "<html>Hello</html>").unwrap();
        }
    }

    fn create_fake_plugin(modules_dir: &Path, name: &str) {
        let pkg_dir = modules_dir.join(name);
        fs::create_dir_all(&pkg_dir).unwrap();

        let pkg_json = serde_json::json!({
            "name": name,
            "version": "2.0.0",
            "keywords": ["signalk-node-server-plugin"]
        });
        fs::write(
            pkg_dir.join("package.json"),
            serde_json::to_string_pretty(&pkg_json).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn discover_webapps_finds_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = tmp.path().join("node_modules");
        fs::create_dir_all(&modules).unwrap();

        create_fake_webapp(&modules, "instrumentpanel", true);
        create_fake_plugin(&modules, "some-plugin"); // not a webapp

        let webapps = discover_webapps(&modules);
        assert_eq!(webapps.len(), 1);
        assert_eq!(webapps[0].name, "instrumentpanel");
        assert_eq!(webapps[0].url, "/instrumentpanel");
        assert!(webapps[0].public_dir.is_dir());
    }

    #[test]
    fn discover_webapps_handles_scoped_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = tmp.path().join("node_modules");
        let scoped = modules.join("@signalk");
        fs::create_dir_all(&scoped).unwrap();

        create_fake_webapp(&scoped, "instrumentpanel", true);

        let webapps = discover_webapps(&modules);
        assert_eq!(webapps.len(), 1);
        assert_eq!(webapps[0].name, "@signalk/instrumentpanel");
        assert_eq!(webapps[0].url, "/@signalk/instrumentpanel");
    }

    #[test]
    fn discover_webapps_skips_without_public_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let modules = tmp.path().join("node_modules");
        fs::create_dir_all(&modules).unwrap();

        create_fake_webapp(&modules, "broken-webapp", false);

        let webapps = discover_webapps(&modules);
        assert!(webapps.is_empty());
    }

    #[test]
    fn discover_webapps_empty_on_missing_dir() {
        let webapps = discover_webapps(Path::new("/nonexistent/path"));
        assert!(webapps.is_empty());
    }

    #[test]
    fn registry_deduplicates_by_name() {
        let mut reg = WebappRegistry::new();
        reg.register(WebAppInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            display_name: None,
            description: None,
            url: "/test".to_string(),
            public_dir: PathBuf::from("/tmp/test"),
            source: Some(WebappSource::NpmPackage),
        });
        reg.register(WebAppInfo {
            name: "test".to_string(),
            version: "2.0.0".to_string(),
            display_name: None,
            description: None,
            url: "/test".to_string(),
            public_dir: PathBuf::from("/tmp/test2"),
            source: Some(WebappSource::NpmPackage),
        });

        assert_eq!(reg.all().len(), 1);
        assert_eq!(reg.all()[0].version, "2.0.0");
    }
}

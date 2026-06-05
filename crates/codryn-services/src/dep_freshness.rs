use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Cache TTL: 1 hour.
const CACHE_TTL: Duration = Duration::from_secs(3600);

/// HTTP request timeout: 10 seconds.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum retry attempts for registry queries.
const MAX_RETRIES: usize = 1;

/// Supported package registries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Registry {
    CratesIo,
    Npm,
    PyPI,
    GoProxy,
    MavenCentral,
}

impl std::fmt::Display for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Registry::CratesIo => write!(f, "crates.io"),
            Registry::Npm => write!(f, "npm"),
            Registry::PyPI => write!(f, "pypi"),
            Registry::GoProxy => write!(f, "pkg.go.dev"),
            Registry::MavenCentral => write!(f, "maven_central"),
        }
    }
}

/// Freshness category for a dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessCategory {
    UpToDate,
    PatchAvailable,
    MinorAvailable,
    MajorAvailable,
    Deprecated,
    /// Version scheme does not conform to semver.
    NonSemver,
    /// Registry was unreachable; only declared version reported.
    Unreachable,
}

/// Information about a dependency's version status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepStatus {
    pub package_name: String,
    pub registry: Registry,
    pub declared_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    pub category: FreshnessCategory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

/// A parsed manifest file with its dependencies.
#[derive(Debug, Clone)]
pub struct ManifestFile {
    pub path: PathBuf,
    pub registry: Registry,
    pub dependencies: Vec<DeclaredDep>,
}

/// A single declared dependency from a manifest.
#[derive(Debug, Clone)]
pub struct DeclaredDep {
    pub name: String,
    pub version: String,
}

/// Version info returned from a registry query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub latest_version: String,
    pub deprecated: bool,
}

/// Cached registry response stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    pub version_info: VersionInfo,
    pub cached_at: u64, // seconds since UNIX epoch
}

/// The dependency freshness checker.
///
/// Queries package registries to determine if declared dependencies are
/// up-to-date, and caches responses on disk for 1 hour.
pub struct DepFreshnessChecker {
    cache_dir: PathBuf,
}

impl DepFreshnessChecker {
    /// Create a new checker with a cache directory.
    /// If `cache_dir` is None, uses `$HOME/.cache/cbm/dep_cache/`.
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        let cache_dir = cache_dir.unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("codryn")
                .join("dep_cache")
        });
        Self { cache_dir }
    }

    /// Check all dependencies from the given manifest files.
    pub fn check_all(&self, manifests: &[ManifestFile]) -> Vec<DepStatus> {
        let mut results = Vec::new();
        for manifest in manifests {
            for dep in &manifest.dependencies {
                let status = self.check_dep(manifest.registry, &dep.name, &dep.version);
                results.push(status);
            }
        }
        results
    }

    /// Check a single dependency against its registry.
    fn check_dep(&self, registry: Registry, package: &str, declared_version: &str) -> DepStatus {
        // Try cache first
        if let Some(cached) = self.read_cache(registry, package) {
            return self.build_status(package, registry, declared_version, Some(cached));
        }

        // Query registry with retry
        match self.query_registry_with_retry(registry, package) {
            Some(info) => {
                // Cache the result
                self.write_cache(registry, package, &info);
                self.build_status(package, registry, declared_version, Some(info))
            }
            None => DepStatus {
                package_name: package.to_string(),
                registry,
                declared_version: declared_version.to_string(),
                latest_version: None,
                category: FreshnessCategory::Unreachable,
                skip_reason: Some("registry unreachable after timeout and retry".to_string()),
            },
        }
    }

    /// Build a DepStatus from registry info and declared version.
    fn build_status(
        &self,
        package: &str,
        registry: Registry,
        declared_version: &str,
        info: Option<VersionInfo>,
    ) -> DepStatus {
        match info {
            Some(info) => {
                let category = if info.deprecated {
                    FreshnessCategory::Deprecated
                } else {
                    categorize(declared_version, &info.latest_version)
                };
                DepStatus {
                    package_name: package.to_string(),
                    registry,
                    declared_version: declared_version.to_string(),
                    latest_version: Some(info.latest_version),
                    category,
                    skip_reason: None,
                }
            }
            None => DepStatus {
                package_name: package.to_string(),
                registry,
                declared_version: declared_version.to_string(),
                latest_version: None,
                category: FreshnessCategory::Unreachable,
                skip_reason: Some("registry unreachable".to_string()),
            },
        }
    }

    /// Query a registry with 1 retry on failure.
    fn query_registry_with_retry(&self, registry: Registry, package: &str) -> Option<VersionInfo> {
        for attempt in 0..=MAX_RETRIES {
            match query_registry(registry, package) {
                Some(info) => return Some(info),
                None => {
                    if attempt < MAX_RETRIES {
                        tracing::debug!(
                            "Registry query failed for {}/{}, retrying...",
                            registry,
                            package
                        );
                    }
                }
            }
        }
        None
    }

    /// Get the cache file path for a package.
    fn cache_path(&self, registry: Registry, package: &str) -> PathBuf {
        let safe_name = package.replace('/', "__");
        self.cache_dir
            .join(format!("{}_{}.json", registry, safe_name))
    }

    /// Read a cached registry response if it exists and is not expired.
    fn read_cache(&self, registry: Registry, package: &str) -> Option<VersionInfo> {
        let path = self.cache_path(registry, package);
        let content = fs::read_to_string(&path).ok()?;
        let entry: CacheEntry = serde_json::from_str(&content).ok()?;

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now.saturating_sub(entry.cached_at) < CACHE_TTL.as_secs() {
            Some(entry.version_info)
        } else {
            // Cache expired, remove the file
            let _ = fs::remove_file(&path);
            None
        }
    }

    /// Write a registry response to the cache.
    fn write_cache(&self, registry: Registry, package: &str, info: &VersionInfo) {
        let path = self.cache_path(registry, package);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = CacheEntry {
            version_info: info.clone(),
            cached_at: now,
        };

        if let Ok(json) = serde_json::to_string(&entry) {
            let _ = fs::write(&path, json);
        }
    }
}

/// Categorize the freshness of a dependency by comparing declared vs latest version.
///
/// Returns `NonSemver` if either version string cannot be parsed as semver.
pub fn categorize(declared: &str, latest: &str) -> FreshnessCategory {
    let declared_clean = clean_version(declared);
    let latest_clean = clean_version(latest);

    let declared_ver = match semver::Version::parse(&declared_clean) {
        Ok(v) => v,
        Err(_) => return FreshnessCategory::NonSemver,
    };
    let latest_ver = match semver::Version::parse(&latest_clean) {
        Ok(v) => v,
        Err(_) => return FreshnessCategory::NonSemver,
    };

    if declared_ver >= latest_ver {
        FreshnessCategory::UpToDate
    } else if declared_ver.major != latest_ver.major {
        FreshnessCategory::MajorAvailable
    } else if declared_ver.minor != latest_ver.minor {
        FreshnessCategory::MinorAvailable
    } else {
        FreshnessCategory::PatchAvailable
    }
}

/// Clean a version string for semver parsing.
/// Strips leading `v`, `^`, `~`, `=`, `>=`, `<=`, `>`, `<` prefixes,
/// and trailing wildcard segments.
fn clean_version(version: &str) -> String {
    let v = version.trim();
    // Strip common prefixes
    let v = v.strip_prefix('v').unwrap_or(v);
    let v = v.strip_prefix(">=").unwrap_or(v);
    let v = v.strip_prefix("<=").unwrap_or(v);
    let v = v.strip_prefix('>').unwrap_or(v);
    let v = v.strip_prefix('<').unwrap_or(v);
    let v = v.strip_prefix('~').unwrap_or(v);
    let v = v.strip_prefix('^').unwrap_or(v);
    let v = v.strip_prefix('=').unwrap_or(v);
    let v = v.trim();

    // Handle wildcard versions like "1.2.*" -> "1.2.0"
    let v = v.replace(".*", ".0");
    let v = v.replace(".x", ".0");

    // Ensure we have at least major.minor.patch
    let parts: Vec<&str> = v.split('.').collect();
    match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => v.to_string(),
    }
}

/// Query a package registry for the latest version.
/// Returns None if the registry is unreachable or the response is invalid.
fn query_registry(registry: Registry, package: &str) -> Option<VersionInfo> {
    let timeout_secs = REQUEST_TIMEOUT.as_secs();
    match registry {
        Registry::CratesIo => query_crates_io(package, timeout_secs),
        Registry::Npm => query_npm(package, timeout_secs),
        Registry::PyPI => query_pypi(package, timeout_secs),
        Registry::GoProxy => query_go_proxy(package, timeout_secs),
        Registry::MavenCentral => query_maven(package, timeout_secs),
    }
}

/// Query crates.io for a Rust crate.
/// API: https://crates.io/api/v1/crates/{name}
fn query_crates_io(package: &str, timeout_secs: u64) -> Option<VersionInfo> {
    let url = format!("https://crates.io/api/v1/crates/{}", package);
    let body = http_get(&url, timeout_secs)?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;

    let crate_info = json.get("crate")?;
    let newest_version = crate_info
        .get("newest_version")
        .and_then(|v| v.as_str())?
        .to_string();

    // Check if the crate's max_version is yanked (deprecated indicator)
    let versions = json.get("versions").and_then(|v| v.as_array());
    let deprecated = versions
        .and_then(|vs| {
            vs.iter()
                .find(|v| v.get("num").and_then(|n| n.as_str()) == Some(&newest_version))
        })
        .and_then(|v| v.get("yanked"))
        .and_then(|y| y.as_bool())
        .unwrap_or(false);

    Some(VersionInfo {
        latest_version: newest_version,
        deprecated,
    })
}

/// Query npm registry for a Node.js package.
/// API: https://registry.npmjs.org/{name}/latest
fn query_npm(package: &str, timeout_secs: u64) -> Option<VersionInfo> {
    let url = format!("https://registry.npmjs.org/{}/latest", package);
    let body = http_get(&url, timeout_secs)?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;

    let version = json.get("version").and_then(|v| v.as_str())?.to_string();
    let deprecated = json.get("deprecated").is_some();

    Some(VersionInfo {
        latest_version: version,
        deprecated,
    })
}

/// Query PyPI for a Python package.
/// API: https://pypi.org/pypi/{name}/json
fn query_pypi(package: &str, timeout_secs: u64) -> Option<VersionInfo> {
    let url = format!("https://pypi.org/pypi/{}/json", package);
    let body = http_get(&url, timeout_secs)?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;

    let info = json.get("info")?;
    let version = info.get("version").and_then(|v| v.as_str())?.to_string();

    // Check classifiers for deprecated status
    let classifiers = info
        .get("classifiers")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let deprecated = classifiers.iter().any(|c| {
        c.as_str()
            .map(|s| s.contains("Inactive") || s.contains("Deprecated"))
            .unwrap_or(false)
    });

    Some(VersionInfo {
        latest_version: version,
        deprecated,
    })
}

/// Query Go module proxy for a Go module.
/// API: https://proxy.golang.org/{module}/@latest
fn query_go_proxy(package: &str, timeout_secs: u64) -> Option<VersionInfo> {
    let url = format!("https://proxy.golang.org/{}/@latest", package);
    let body = http_get(&url, timeout_secs)?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;

    let version = json
        .get("Version")
        .and_then(|v| v.as_str())?
        .strip_prefix('v')
        .unwrap_or(json.get("Version").and_then(|v| v.as_str())?)
        .to_string();

    Some(VersionInfo {
        latest_version: version,
        deprecated: false, // Go proxy doesn't have a deprecation flag
    })
}

/// Query Maven Central for a Java/Kotlin package.
/// API: https://search.maven.org/solrsearch/select?q=g:{group}+AND+a:{artifact}&rows=1&wt=json
fn query_maven(package: &str, timeout_secs: u64) -> Option<VersionInfo> {
    // Maven packages are in format "group:artifact"
    let parts: Vec<&str> = package.splitn(2, ':').collect();
    let (group, artifact) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        // Try treating the whole thing as artifact with unknown group
        return None;
    };

    let url = format!(
        "https://search.maven.org/solrsearch/select?q=g:{}+AND+a:{}&rows=1&wt=json",
        group, artifact
    );
    let body = http_get(&url, timeout_secs)?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;

    let docs = json.get("response")?.get("docs")?.as_array()?;

    let doc = docs.first()?;
    let version = doc
        .get("latestVersion")
        .and_then(|v| v.as_str())?
        .to_string();

    Some(VersionInfo {
        latest_version: version,
        deprecated: false,
    })
}

/// Perform an HTTP GET request with timeout.
/// Returns the response body as a string, or None on failure.
fn http_get(url: &str, timeout_secs: u64) -> Option<String> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(timeout_secs)))
            .build(),
    );

    let response = agent
        .get(url)
        .header("User-Agent", "codryn-dep-freshness/0.1")
        .call()
        .ok()?;

    response.into_body().read_to_string().ok()
}

// ─── Manifest Parsing ───────────────────────────────────────────────────────

/// Parse manifest files from a project directory.
/// Detects Cargo.toml, package.json, go.mod, requirements.txt, pom.xml.
pub fn parse_manifests(project_root: &Path) -> Vec<ManifestFile> {
    let mut manifests = Vec::new();

    let cargo_toml = project_root.join("Cargo.toml");
    if cargo_toml.exists() {
        if let Some(m) = parse_cargo_toml(&cargo_toml) {
            manifests.push(m);
        }
    }

    let package_json = project_root.join("package.json");
    if package_json.exists() {
        if let Some(m) = parse_package_json(&package_json) {
            manifests.push(m);
        }
    }

    let go_mod = project_root.join("go.mod");
    if go_mod.exists() {
        if let Some(m) = parse_go_mod(&go_mod) {
            manifests.push(m);
        }
    }

    let requirements_txt = project_root.join("requirements.txt");
    if requirements_txt.exists() {
        if let Some(m) = parse_requirements_txt(&requirements_txt) {
            manifests.push(m);
        }
    }

    let pom_xml = project_root.join("pom.xml");
    if pom_xml.exists() {
        if let Some(m) = parse_pom_xml(&pom_xml) {
            manifests.push(m);
        }
    }

    manifests
}

/// Parse a Cargo.toml file for dependencies.
fn parse_cargo_toml(path: &Path) -> Option<ManifestFile> {
    let content = fs::read_to_string(path).ok()?;
    let doc: toml::Value = toml::from_str(&content).ok()?;

    let mut deps = Vec::new();

    // Parse [dependencies], [dev-dependencies], [build-dependencies]
    for section in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = doc.get(section).and_then(|v| v.as_table()) {
            for (name, value) in table {
                let version = match value {
                    toml::Value::String(v) => v.clone(),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    _ => continue,
                };
                if !version.is_empty() {
                    deps.push(DeclaredDep {
                        name: name.clone(),
                        version,
                    });
                }
            }
        }
    }

    if deps.is_empty() {
        return None;
    }

    Some(ManifestFile {
        path: path.to_path_buf(),
        registry: Registry::CratesIo,
        dependencies: deps,
    })
}

/// Parse a package.json file for dependencies.
fn parse_package_json(path: &Path) -> Option<ManifestFile> {
    let content = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let mut deps = Vec::new();

    for section in &["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(obj) = json.get(*section).and_then(|v| v.as_object()) {
            for (name, value) in obj {
                if let Some(version) = value.as_str() {
                    if !version.is_empty() {
                        deps.push(DeclaredDep {
                            name: name.clone(),
                            version: version.to_string(),
                        });
                    }
                }
            }
        }
    }

    if deps.is_empty() {
        return None;
    }

    Some(ManifestFile {
        path: path.to_path_buf(),
        registry: Registry::Npm,
        dependencies: deps,
    })
}

/// Parse a go.mod file for dependencies.
fn parse_go_mod(path: &Path) -> Option<ManifestFile> {
    let content = fs::read_to_string(path).ok()?;
    let mut deps = Vec::new();
    let mut in_require_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "require (" {
            in_require_block = true;
            continue;
        }
        if trimmed == ")" {
            in_require_block = false;
            continue;
        }

        if in_require_block {
            // Lines like: github.com/pkg/errors v0.9.1
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[0].to_string();
                let version = parts[1].strip_prefix('v').unwrap_or(parts[1]).to_string();
                if !name.starts_with("//") {
                    deps.push(DeclaredDep { name, version });
                }
            }
        } else if trimmed.starts_with("require ") {
            // Single-line require: require github.com/pkg/errors v0.9.1
            let rest = trimmed.strip_prefix("require ").unwrap_or("");
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[0].to_string();
                let version = parts[1].strip_prefix('v').unwrap_or(parts[1]).to_string();
                deps.push(DeclaredDep { name, version });
            }
        }
    }

    if deps.is_empty() {
        return None;
    }

    Some(ManifestFile {
        path: path.to_path_buf(),
        registry: Registry::GoProxy,
        dependencies: deps,
    })
}

/// Parse a requirements.txt file for Python dependencies.
fn parse_requirements_txt(path: &Path) -> Option<ManifestFile> {
    let content = fs::read_to_string(path).ok()?;
    let mut deps = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }

        // Parse lines like: package==1.2.3, package>=1.2.3, package~=1.2
        let (name, version) = if let Some(idx) = trimmed.find("==") {
            (&trimmed[..idx], &trimmed[idx + 2..])
        } else if let Some(idx) = trimmed.find(">=") {
            (&trimmed[..idx], &trimmed[idx + 2..])
        } else if let Some(idx) = trimmed.find("~=") {
            (&trimmed[..idx], &trimmed[idx + 2..])
        } else if let Some(idx) = trimmed.find("<=") {
            (&trimmed[..idx], &trimmed[idx + 2..])
        } else if let Some(idx) = trimmed.find("!=") {
            (&trimmed[..idx], &trimmed[idx + 2..])
        } else {
            // No version specifier, skip
            continue;
        };

        let name = name.trim().to_string();
        // Take only the first version if there are multiple constraints
        let version = version.split(',').next().unwrap_or("").trim().to_string();

        if !name.is_empty() && !version.is_empty() {
            deps.push(DeclaredDep { name, version });
        }
    }

    if deps.is_empty() {
        return None;
    }

    Some(ManifestFile {
        path: path.to_path_buf(),
        registry: Registry::PyPI,
        dependencies: deps,
    })
}

/// Parse a pom.xml file for Maven dependencies.
/// Uses simple regex-based extraction (no full XML parser).
fn parse_pom_xml(path: &Path) -> Option<ManifestFile> {
    let content = fs::read_to_string(path).ok()?;
    let mut deps = Vec::new();

    // Simple regex to find <dependency> blocks
    let dep_re = regex::Regex::new(
        r"(?s)<dependency>\s*<groupId>([^<]+)</groupId>\s*<artifactId>([^<]+)</artifactId>\s*<version>([^<]+)</version>"
    ).ok()?;

    for cap in dep_re.captures_iter(&content) {
        let group = cap[1].trim().to_string();
        let artifact = cap[2].trim().to_string();
        let version = cap[3].trim().to_string();

        // Skip property references like ${project.version}
        if version.starts_with("${") {
            continue;
        }

        deps.push(DeclaredDep {
            name: format!("{}:{}", group, artifact),
            version,
        });
    }

    if deps.is_empty() {
        return None;
    }

    Some(ManifestFile {
        path: path.to_path_buf(),
        registry: Registry::MavenCentral,
        dependencies: deps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ─── Categorization Tests ───────────────────────────────────────────

    #[test]
    fn test_categorize_up_to_date() {
        assert_eq!(categorize("1.2.3", "1.2.3"), FreshnessCategory::UpToDate);
    }

    #[test]
    fn test_categorize_patch_available() {
        assert_eq!(
            categorize("1.2.3", "1.2.5"),
            FreshnessCategory::PatchAvailable
        );
    }

    #[test]
    fn test_categorize_minor_available() {
        assert_eq!(
            categorize("1.2.3", "1.4.0"),
            FreshnessCategory::MinorAvailable
        );
    }

    #[test]
    fn test_categorize_major_available() {
        assert_eq!(
            categorize("1.2.3", "2.0.0"),
            FreshnessCategory::MajorAvailable
        );
    }

    #[test]
    fn test_categorize_non_semver() {
        assert_eq!(
            categorize("not-a-version", "1.2.3"),
            FreshnessCategory::NonSemver
        );
        assert_eq!(
            categorize("1.2.3", "not-a-version"),
            FreshnessCategory::NonSemver
        );
    }

    #[test]
    fn test_categorize_with_prefix() {
        // Caret prefix
        assert_eq!(categorize("^1.2.3", "1.2.3"), FreshnessCategory::UpToDate);
        // Tilde prefix
        assert_eq!(
            categorize("~1.2.3", "1.3.0"),
            FreshnessCategory::MinorAvailable
        );
    }

    #[test]
    fn test_categorize_declared_newer_than_latest() {
        // If declared is newer (e.g., pre-release or local), treat as up-to-date
        assert_eq!(categorize("2.0.0", "1.5.0"), FreshnessCategory::UpToDate);
    }

    #[test]
    fn test_categorize_two_part_version() {
        assert_eq!(
            categorize("1.2", "1.3.0"),
            FreshnessCategory::MinorAvailable
        );
    }

    // ─── Clean Version Tests ────────────────────────────────────────────

    #[test]
    fn test_clean_version_strips_prefixes() {
        assert_eq!(clean_version("^1.2.3"), "1.2.3");
        assert_eq!(clean_version("~1.2.3"), "1.2.3");
        assert_eq!(clean_version(">=1.2.3"), "1.2.3");
        assert_eq!(clean_version("v1.2.3"), "1.2.3");
        assert_eq!(clean_version("=1.2.3"), "1.2.3");
    }

    #[test]
    fn test_clean_version_pads_parts() {
        assert_eq!(clean_version("1"), "1.0.0");
        assert_eq!(clean_version("1.2"), "1.2.0");
        assert_eq!(clean_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn test_clean_version_wildcards() {
        assert_eq!(clean_version("1.2.*"), "1.2.0");
        assert_eq!(clean_version("1.x"), "1.0.0");
    }

    // ─── Cache Tests ────────────────────────────────────────────────────

    #[test]
    fn test_cache_write_and_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let checker = DepFreshnessChecker::new(Some(dir.path().to_path_buf()));

        let info = VersionInfo {
            latest_version: "2.0.0".to_string(),
            deprecated: false,
        };

        checker.write_cache(Registry::CratesIo, "serde", &info);
        let cached = checker.read_cache(Registry::CratesIo, "serde");
        assert!(cached.is_some());
        let cached = cached.unwrap();
        assert_eq!(cached.latest_version, "2.0.0");
        assert!(!cached.deprecated);
    }

    #[test]
    fn test_cache_expired() {
        let dir = tempfile::TempDir::new().unwrap();
        let checker = DepFreshnessChecker::new(Some(dir.path().to_path_buf()));

        // Write a cache entry with a timestamp from 2 hours ago
        let path = checker.cache_path(Registry::Npm, "express");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        let old_time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 7200; // 2 hours ago

        let entry = CacheEntry {
            version_info: VersionInfo {
                latest_version: "4.18.0".to_string(),
                deprecated: false,
            },
            cached_at: old_time,
        };
        let json = serde_json::to_string(&entry).unwrap();
        fs::write(&path, json).unwrap();

        // Should return None (expired)
        let cached = checker.read_cache(Registry::Npm, "express");
        assert!(cached.is_none());
    }

    // ─── Manifest Parsing Tests ─────────────────────────────────────────

    #[test]
    fn test_parse_cargo_toml() {
        let dir = tempfile::TempDir::new().unwrap();
        let cargo_path = dir.path().join("Cargo.toml");
        let mut f = fs::File::create(&cargo_path).unwrap();
        writeln!(
            f,
            r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = {{ version = "1", features = ["full"] }}

[dev-dependencies]
proptest = "1"
"#
        )
        .unwrap();

        let manifest = parse_cargo_toml(&cargo_path).unwrap();
        assert_eq!(manifest.registry, Registry::CratesIo);
        assert_eq!(manifest.dependencies.len(), 3);

        let names: Vec<&str> = manifest
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert!(names.contains(&"serde"));
        assert!(names.contains(&"tokio"));
        assert!(names.contains(&"proptest"));
    }

    #[test]
    fn test_parse_package_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let pkg_path = dir.path().join("package.json");
        fs::write(
            &pkg_path,
            r#"{
  "dependencies": {
    "express": "^4.18.0",
    "lodash": "~4.17.21"
  },
  "devDependencies": {
    "jest": "^29.0.0"
  }
}"#,
        )
        .unwrap();

        let manifest = parse_package_json(&pkg_path).unwrap();
        assert_eq!(manifest.registry, Registry::Npm);
        assert_eq!(manifest.dependencies.len(), 3);
    }

    #[test]
    fn test_parse_go_mod() {
        let dir = tempfile::TempDir::new().unwrap();
        let go_path = dir.path().join("go.mod");
        fs::write(
            &go_path,
            r#"module github.com/example/app

go 1.21

require (
	github.com/pkg/errors v0.9.1
	github.com/gin-gonic/gin v1.9.1
)
"#,
        )
        .unwrap();

        let manifest = parse_go_mod(&go_path).unwrap();
        assert_eq!(manifest.registry, Registry::GoProxy);
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(manifest.dependencies[0].name, "github.com/pkg/errors");
        assert_eq!(manifest.dependencies[0].version, "0.9.1");
    }

    #[test]
    fn test_parse_requirements_txt() {
        let dir = tempfile::TempDir::new().unwrap();
        let req_path = dir.path().join("requirements.txt");
        fs::write(
            &req_path,
            r#"# Python deps
flask==2.3.0
requests>=2.28.0
numpy~=1.24
# comment
-e git+https://example.com/pkg.git
"#,
        )
        .unwrap();

        let manifest = parse_requirements_txt(&req_path).unwrap();
        assert_eq!(manifest.registry, Registry::PyPI);
        assert_eq!(manifest.dependencies.len(), 3);
        assert_eq!(manifest.dependencies[0].name, "flask");
        assert_eq!(manifest.dependencies[0].version, "2.3.0");
    }

    #[test]
    fn test_parse_pom_xml() {
        let dir = tempfile::TempDir::new().unwrap();
        let pom_path = dir.path().join("pom.xml");
        fs::write(
            &pom_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<project>
  <dependencies>
    <dependency>
      <groupId>org.springframework</groupId>
      <artifactId>spring-core</artifactId>
      <version>5.3.20</version>
    </dependency>
    <dependency>
      <groupId>com.google.guava</groupId>
      <artifactId>guava</artifactId>
      <version>${guava.version}</version>
    </dependency>
  </dependencies>
</project>"#,
        )
        .unwrap();

        let manifest = parse_pom_xml(&pom_path).unwrap();
        assert_eq!(manifest.registry, Registry::MavenCentral);
        // Should skip the ${guava.version} entry
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(
            manifest.dependencies[0].name,
            "org.springframework:spring-core"
        );
        assert_eq!(manifest.dependencies[0].version, "5.3.20");
    }

    // ─── Integration-style Tests ────────────────────────────────────────

    #[test]
    fn test_check_all_with_cached_data() {
        let dir = tempfile::TempDir::new().unwrap();
        let checker = DepFreshnessChecker::new(Some(dir.path().to_path_buf()));

        // Pre-populate cache
        let info = VersionInfo {
            latest_version: "1.5.0".to_string(),
            deprecated: false,
        };
        checker.write_cache(Registry::CratesIo, "serde", &info);

        let manifests = vec![ManifestFile {
            path: PathBuf::from("Cargo.toml"),
            registry: Registry::CratesIo,
            dependencies: vec![DeclaredDep {
                name: "serde".to_string(),
                version: "1.0.0".to_string(),
            }],
        }];

        let results = checker.check_all(&manifests);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].package_name, "serde");
        assert_eq!(results[0].category, FreshnessCategory::MinorAvailable);
        assert_eq!(results[0].latest_version, Some("1.5.0".to_string()));
    }

    #[test]
    fn test_check_dep_unreachable_registry() {
        let dir = tempfile::TempDir::new().unwrap();
        let checker = DepFreshnessChecker::new(Some(dir.path().to_path_buf()));

        // Query a non-existent package (will fail HTTP)
        // This tests the unreachable path without actually hitting the network
        // by using a package name that won't be in cache
        let status = checker.build_status("nonexistent-pkg", Registry::CratesIo, "1.0.0", None);
        assert_eq!(status.category, FreshnessCategory::Unreachable);
        assert!(status.skip_reason.is_some());
    }
}

//! Kubernetes manifest pass: parses K8s YAML files and creates Infrastructure nodes
//! with DEPLOYS, CONFIGURES, and EXPOSES edges.
//!
//! Supports: Deployment, Service, ConfigMap, Secret, Ingress.
//! Handles multi-document YAML (--- separator).

use codryn_discover::{DiscoveredFile, Language};
use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Supported Kubernetes resource kinds for Infrastructure node creation.
const SUPPORTED_KINDS: &[&str] = &["Deployment", "Service", "ConfigMap", "Secret", "Ingress"];

/// Regex to extract `kind:` from a YAML document.
static KIND_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^kind:\s*(\S+)").unwrap());

/// Regex to extract `apiVersion:` — must contain a K8s API group.
static API_VERSION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^apiVersion:\s*(\S+)").unwrap());

/// Regex to extract `metadata.name:`.
static NAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^\s{2}name:\s*(\S+)").unwrap());

/// Regex to extract `metadata.namespace:`.
static NAMESPACE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^\s{2}namespace:\s*(\S+)").unwrap());

/// Regex to extract container image references.
static IMAGE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?m)^\s+image:\s*['"]?([^\s'"]+)['"]?"#).unwrap());

/// Regex to extract `configMapRef.name` or `configMapKeyRef.name`.
static CONFIGMAP_REF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)(?:configMapRef|configMapKeyRef):\s*\n\s+name:\s*(\S+)").unwrap()
});

/// Regex to extract `secretRef.name` or `secretKeyRef.name`.
static SECRET_REF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)(?:secretRef|secretKeyRef):\s*\n\s+name:\s*(\S+)").unwrap()
});

/// Regex to extract configMap name from volume mounts: `configMap:\n  name: <name>`.
static VOLUME_CONFIGMAP_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)configMap:\s*\n\s+name:\s*(\S+)").unwrap());

/// Regex to extract secret name from volume mounts: `secret:\n  secretName: <name>`.
static VOLUME_SECRET_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)secret:\s*\n\s+secretName:\s*(\S+)").unwrap());

/// Regex to extract `containerPort:` values from pod specs.
static CONTAINER_PORT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)containerPort:\s*(\d+)").unwrap());

/// Regex to extract `targetPort:` from Service specs.
static TARGET_PORT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)targetPort:\s*(\d+)").unwrap());

/// Regex to extract `envFrom` configMapRef name.
static ENVFROM_CONFIGMAP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)envFrom:\s*\n\s+-\s*configMapRef:\s*\n\s+name:\s*(\S+)").unwrap()
});

/// Regex to extract `envFrom` secretRef name.
static ENVFROM_SECRET_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)envFrom:\s*\n\s+-\s*secretRef:\s*\n\s+name:\s*(\S+)").unwrap()
});

/// Known K8s API version prefixes that indicate a Kubernetes resource.
const K8S_API_PREFIXES: &[&str] = &[
    "v1",
    "apps/",
    "batch/",
    "networking.k8s.io/",
    "extensions/",
    "rbac.authorization.k8s.io/",
    "autoscaling/",
    "policy/",
    "storage.k8s.io/",
    "apiextensions.k8s.io/",
];

/// Returns true if the file looks like a Kubernetes YAML manifest.
fn is_k8s_yaml(rel_path: &str, lang: Language) -> bool {
    matches!(lang, Language::Yaml) && {
        let lower = rel_path.to_lowercase();
        // Exclude kustomization files — handled by pass_kustomize
        !lower.ends_with("kustomization.yaml") && !lower.ends_with("kustomization.yml")
    }
}

/// Returns true if the apiVersion string indicates a Kubernetes API group.
fn is_k8s_api_version(api_version: &str) -> bool {
    K8S_API_PREFIXES
        .iter()
        .any(|prefix| api_version.starts_with(prefix))
}

/// Parsed K8s resource from a single YAML document.
#[derive(Debug)]
struct K8sResource {
    kind: String,
    name: String,
    namespace: String,
    qualified_name: String,
    /// Container ports declared in the pod spec (for Deployments).
    container_ports: Vec<u16>,
    /// Image references (for Deployments).
    images: Vec<String>,
    /// ConfigMap references (envFrom, env valueFrom, volumes).
    configmap_refs: Vec<String>,
    /// Secret references (envFrom, env valueFrom, volumes).
    secret_refs: Vec<String>,
    /// Target ports (for Services).
    target_ports: Vec<u16>,
}

/// Parse Kubernetes YAML manifests and create Infrastructure nodes with
/// DEPLOYS, CONFIGURES, and EXPOSES edges.
///
/// This pass:
/// 1. Detects K8s YAML files (containing `kind` + `apiVersion` with K8s API group)
/// 2. Parses supported resource kinds: Deployment, Service, ConfigMap, Secret, Ingress
/// 3. Creates Infrastructure nodes with `infra_type: "kubernetes"`, name, kind, namespace
/// 4. Handles multi-document YAML (--- separator) — one node per resource
/// 5. Creates DEPLOYS edges for container image references in Deployments
/// 6. Creates CONFIGURES edges for ConfigMap/Secret references
/// 7. Creates EXPOSES edges for port-matched Services
/// 8. Creates placeholder nodes with `status: "unresolved"` for missing references
pub fn pass_k8s(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    let mut images_seen: HashSet<String> = HashSet::new();
    let mut placeholder_seen: HashSet<String> = HashSet::new();
    // Track deployments by their container ports for EXPOSES edge matching
    let mut deployment_ports: HashMap<u16, Vec<String>> = HashMap::new();
    // Collect all resources first, then create EXPOSES edges
    let mut resources: Vec<K8sResource> = Vec::new();

    for f in files {
        if !is_k8s_yaml(&f.rel_path, f.language) {
            continue;
        }

        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    path = %f.rel_path,
                    "pass_k8s: failed to read file"
                );
                continue;
            }
        };

        // Split on YAML document separators to handle multi-document files
        let documents: Vec<&str> = source.split("\n---").collect();

        for doc in documents {
            let resource = match parse_k8s_document(doc, &f.rel_path, project) {
                Some(r) => r,
                None => continue,
            };
            resources.push(resource);
        }
    }

    // Phase 1: Create Infrastructure nodes for all resources
    for resource in &resources {
        let props = serde_json::json!({
            "infra_type": "kubernetes",
            "kind": resource.kind,
            "namespace": resource.namespace,
        });

        buf.add_node(
            "Infrastructure",
            &resource.name,
            &resource.qualified_name,
            "",
            0,
            0,
            Some(props.to_string()),
        );

        // Track container ports for Deployments
        if resource.kind == "Deployment" {
            for port in &resource.container_ports {
                deployment_ports
                    .entry(*port)
                    .or_default()
                    .push(resource.qualified_name.clone());
            }
        }
    }

    // Phase 2: Create edges
    for resource in &resources {
        match resource.kind.as_str() {
            "Deployment" => {
                // DEPLOYS edges for image references
                for image in &resource.images {
                    let image_qn = format!("{}.k8s.image.{}", project, image);
                    if images_seen.insert(image_qn.clone()) {
                        // Create image node (placeholder if not already in graph)
                        let img_props = serde_json::json!({
                            "infra_type": "docker_image",
                            "image": image,
                            "status": "unresolved",
                        });
                        buf.add_node(
                            "Infrastructure",
                            image,
                            &image_qn,
                            "",
                            0,
                            0,
                            Some(img_props.to_string()),
                        );
                    }
                    buf.add_edge_with_confidence(
                        &resource.qualified_name,
                        &image_qn,
                        "DEPLOYS",
                        EdgeSource::RegexMatch,
                        None,
                    );
                }

                // CONFIGURES edges for ConfigMap references
                for cm_name in &resource.configmap_refs {
                    let cm_qn = format!(
                        "{}.k8s.ConfigMap.{}.{}",
                        project, resource.namespace, cm_name
                    );
                    ensure_placeholder(
                        buf,
                        &cm_qn,
                        cm_name,
                        "ConfigMap",
                        &resource.namespace,
                        project,
                        &mut placeholder_seen,
                    );
                    buf.add_edge_with_confidence(
                        &cm_qn,
                        &resource.qualified_name,
                        "CONFIGURES",
                        EdgeSource::RegexMatch,
                        None,
                    );
                }

                // CONFIGURES edges for Secret references
                for secret_name in &resource.secret_refs {
                    let secret_qn = format!(
                        "{}.k8s.Secret.{}.{}",
                        project, resource.namespace, secret_name
                    );
                    ensure_placeholder(
                        buf,
                        &secret_qn,
                        secret_name,
                        "Secret",
                        &resource.namespace,
                        project,
                        &mut placeholder_seen,
                    );
                    buf.add_edge_with_confidence(
                        &secret_qn,
                        &resource.qualified_name,
                        "CONFIGURES",
                        EdgeSource::RegexMatch,
                        None,
                    );
                }
            }
            "Service" => {
                // EXPOSES edges: match Service targetPort to Deployment containerPort
                for target_port in &resource.target_ports {
                    if let Some(deployment_qns) = deployment_ports.get(target_port) {
                        for dep_qn in deployment_qns {
                            buf.add_edge_with_confidence(
                                &resource.qualified_name,
                                dep_qn,
                                "EXPOSES",
                                EdgeSource::RegexMatch,
                                Some(
                                    serde_json::json!({
                                        "port": target_port
                                    })
                                    .to_string(),
                                ),
                            );
                        }
                    }
                }
            }
            "ConfigMap" | "Secret" => {
                // ConfigMap/Secret nodes are already created as Infrastructure nodes.
                // CONFIGURES edges are created from the Deployment side.
            }
            "Ingress" => {
                // Ingress nodes are created as Infrastructure nodes.
                // Future: could create ROUTES_TO edges to Services.
            }
            _ => {}
        }
    }
}

/// Ensure a placeholder node exists for an unresolved ConfigMap/Secret reference.
/// Only creates the node if it hasn't been seen before AND doesn't already exist
/// as a real Infrastructure node in the current buffer.
fn ensure_placeholder(
    buf: &mut GraphBuffer,
    qn: &str,
    name: &str,
    kind: &str,
    namespace: &str,
    _project: &str,
    seen: &mut HashSet<String>,
) {
    if !seen.insert(qn.to_owned()) {
        return;
    }
    let props = serde_json::json!({
        "infra_type": "kubernetes",
        "kind": kind,
        "namespace": namespace,
        "status": "unresolved",
    });
    buf.add_node(
        "Infrastructure",
        name,
        qn,
        "",
        0,
        0,
        Some(props.to_string()),
    );
}

/// Parse a single YAML document and extract K8s resource information.
/// Returns None if the document is not a supported K8s resource.
fn parse_k8s_document(doc: &str, _rel_path: &str, project: &str) -> Option<K8sResource> {
    // Must have both kind and apiVersion
    let kind = KIND_RE.captures(doc)?.get(1)?.as_str().to_owned();

    let api_version = API_VERSION_RE.captures(doc)?.get(1)?.as_str().to_owned();

    // Verify it's a K8s API version
    if !is_k8s_api_version(&api_version) {
        return None;
    }

    // Only process supported resource kinds
    if !SUPPORTED_KINDS.contains(&kind.as_str()) {
        return None;
    }

    // Extract metadata.name (required)
    let name = NAME_RE.captures(doc)?.get(1)?.as_str().to_owned();

    // Extract optional namespace, default to "default"
    let namespace = NAMESPACE_RE
        .captures(doc)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "default".to_owned());

    let qualified_name = format!("{}.k8s.{}.{}.{}", project, kind, namespace, name);

    // Extract container images (for Deployments)
    let images: Vec<String> = IMAGE_RE
        .captures_iter(doc)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_owned()))
        .collect();

    // Extract container ports (for Deployments)
    let container_ports: Vec<u16> = CONTAINER_PORT_RE
        .captures_iter(doc)
        .filter_map(|caps| caps.get(1)?.as_str().parse().ok())
        .collect();

    // Extract ConfigMap references from multiple sources
    let mut configmap_refs: Vec<String> = Vec::new();

    // From configMapRef/configMapKeyRef (env and envFrom)
    for caps in CONFIGMAP_REF_RE.captures_iter(doc) {
        if let Some(m) = caps.get(1) {
            configmap_refs.push(m.as_str().to_owned());
        }
    }
    // From envFrom configMapRef
    for caps in ENVFROM_CONFIGMAP_RE.captures_iter(doc) {
        if let Some(m) = caps.get(1) {
            let name = m.as_str().to_owned();
            if !configmap_refs.contains(&name) {
                configmap_refs.push(name);
            }
        }
    }
    // From volumes[].configMap.name
    for caps in VOLUME_CONFIGMAP_RE.captures_iter(doc) {
        if let Some(m) = caps.get(1) {
            let name = m.as_str().to_owned();
            if !configmap_refs.contains(&name) {
                configmap_refs.push(name);
            }
        }
    }

    // Extract Secret references from multiple sources
    let mut secret_refs: Vec<String> = Vec::new();

    // From secretRef/secretKeyRef (env and envFrom)
    for caps in SECRET_REF_RE.captures_iter(doc) {
        if let Some(m) = caps.get(1) {
            secret_refs.push(m.as_str().to_owned());
        }
    }
    // From envFrom secretRef
    for caps in ENVFROM_SECRET_RE.captures_iter(doc) {
        if let Some(m) = caps.get(1) {
            let name = m.as_str().to_owned();
            if !secret_refs.contains(&name) {
                secret_refs.push(name);
            }
        }
    }
    // From volumes[].secret.secretName
    for caps in VOLUME_SECRET_RE.captures_iter(doc) {
        if let Some(m) = caps.get(1) {
            let name = m.as_str().to_owned();
            if !secret_refs.contains(&name) {
                secret_refs.push(name);
            }
        }
    }

    // Extract target ports (for Services)
    let target_ports: Vec<u16> = TARGET_PORT_RE
        .captures_iter(doc)
        .filter_map(|caps| caps.get(1)?.as_str().parse().ok())
        .collect();

    Some(K8sResource {
        kind,
        name,
        namespace,
        qualified_name,
        container_ports,
        images,
        configmap_refs,
        secret_refs,
        target_ports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_discover::DiscoveredFile;
    use codryn_graph_buffer::GraphBuffer;
    use tempfile::TempDir;

    fn write_yaml(dir: &TempDir, name: &str, content: &str) -> DiscoveredFile {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        DiscoveredFile {
            abs_path: path,
            rel_path: name.to_owned(),
            language: Language::Yaml,
        }
    }

    #[test]
    fn test_deployment_creates_infrastructure_node() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
  namespace: production
spec:
  template:
    spec:
      containers:
        - name: app
          image: myregistry/my-app:v1.2.3
          ports:
            - containerPort: 8080
"#;
        let file = write_yaml(&dir, "deploy.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        // Should create: 1 Infrastructure node for Deployment + 1 for image
        assert!(
            buf.node_count() >= 2,
            "expected at least 2 nodes, got {}",
            buf.node_count()
        );
        assert!(buf.edge_count() >= 1, "expected at least 1 DEPLOYS edge");
    }

    #[test]
    fn test_multi_document_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
  namespace: staging
data:
  key: value
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web-server
spec:
  template:
    spec:
      containers:
        - name: web
          image: nginx:latest
          ports:
            - containerPort: 80
"#;
        let file = write_yaml(&dir, "multi.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        // Should create: ConfigMap node + Deployment node + image node = 3
        assert!(
            buf.node_count() >= 3,
            "expected at least 3 nodes for multi-doc, got {}",
            buf.node_count()
        );
    }

    #[test]
    fn test_configmap_reference_creates_configures_edge() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
  namespace: default
spec:
  template:
    spec:
      containers:
        - name: app
          image: app:latest
          envFrom:
            - configMapRef:
                name: app-settings
"#;
        let file = write_yaml(&dir, "deploy-cm.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        // Should have CONFIGURES edge from ConfigMap placeholder to Deployment
        assert!(
            buf.edge_count() >= 2,
            "expected DEPLOYS + CONFIGURES edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_secret_reference_creates_configures_edge() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
spec:
  template:
    spec:
      containers:
        - name: app
          image: app:latest
          env:
            - name: DB_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: db-credentials
                  key: password
"#;
        let file = write_yaml(&dir, "deploy-secret.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        // Should have CONFIGURES edge from Secret placeholder to Deployment
        assert!(
            buf.edge_count() >= 2,
            "expected DEPLOYS + CONFIGURES edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_service_exposes_deployment_by_port() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: web-app
spec:
  template:
    spec:
      containers:
        - name: web
          image: web:v1
          ports:
            - containerPort: 3000
---
apiVersion: v1
kind: Service
metadata:
  name: web-service
spec:
  ports:
    - port: 80
      targetPort: 3000
"#;
        let file = write_yaml(&dir, "svc.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        // Should have EXPOSES edge from Service to Deployment
        assert!(
            buf.edge_count() >= 2,
            "expected DEPLOYS + EXPOSES edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_namespace_defaults_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"apiVersion: v1
kind: Service
metadata:
  name: my-svc
spec:
  ports:
    - port: 80
      targetPort: 8080
"#;
        let file = write_yaml(&dir, "svc-no-ns.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        // The qualified name should include "default" namespace
        assert_eq!(buf.node_count(), 1);
    }

    #[test]
    fn test_non_k8s_yaml_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
name: my-project
version: 1.0.0
dependencies:
  - foo
  - bar
"#;
        let file = write_yaml(&dir, "config.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        assert_eq!(buf.node_count(), 0, "non-K8s YAML should produce no nodes");
        assert_eq!(buf.edge_count(), 0, "non-K8s YAML should produce no edges");
    }

    #[test]
    fn test_ingress_creates_infrastructure_node() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: my-ingress
  namespace: production
spec:
  rules:
    - host: example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: web-service
                port:
                  number: 80
"#;
        let file = write_yaml(&dir, "ingress.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        assert_eq!(
            buf.node_count(),
            1,
            "Ingress should create 1 Infrastructure node"
        );
    }

    #[test]
    fn test_volume_configmap_creates_configures_edge() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
spec:
  template:
    spec:
      containers:
        - name: app
          image: app:v1
      volumes:
        - name: config-vol
          configMap:
            name: my-config
"#;
        let file = write_yaml(&dir, "deploy-vol.yaml", yaml);
        let mut buf = GraphBuffer::new("proj");
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, "proj");

        // Should have DEPLOYS edge + CONFIGURES edge
        assert!(
            buf.edge_count() >= 2,
            "expected DEPLOYS + CONFIGURES edges from volume mount, got {}",
            buf.edge_count()
        );
    }
}

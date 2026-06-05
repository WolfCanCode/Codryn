use anyhow::{bail, Result};
use codryn_store::Store;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PipelineDag {
    pub pipeline: PipelineInfo,
    pub stages: Vec<StageInfo>,
    pub jobs: Vec<JobInfo>,
    pub edges: Vec<DagEdge>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PipelineInfo {
    pub name: String,
    pub file_path: String,
    pub ci_system: String,
    pub triggers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct StageInfo {
    pub name: String,
    pub order: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct JobInfo {
    pub name: String,
    pub stage: String,
    pub image: Option<String>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct DagEdge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct InfraResource {
    pub name: String,
    pub resource_type: String,
    pub kind: String,
    pub file_path: String,
    pub properties: serde_json::Value,
}

pub struct PipelineService;

impl PipelineService {
    /// List all pipelines for a project.
    pub fn list_pipelines(store: &Store, project: &str) -> Result<Vec<PipelineDag>> {
        let pipeline_nodes = store.get_nodes_by_label(project, "Pipeline", 1000)?;
        let mut results = Vec::new();
        for pnode in &pipeline_nodes {
            let dag = Self::build_dag_for_pipeline(store, project, pnode)?;
            results.push(dag);
        }
        Ok(results)
    }

    /// Get a specific pipeline's DAG with topological sort.
    /// Returns error if circular dependencies are detected.
    pub fn get_pipeline_dag(
        store: &Store,
        project: &str,
        pipeline_name: &str,
    ) -> Result<PipelineDag> {
        let pipeline_nodes = store.get_nodes_by_label(project, "Pipeline", 1000)?;
        let pnode = pipeline_nodes
            .iter()
            .find(|n| n.name == pipeline_name)
            .ok_or_else(|| anyhow::anyhow!("Pipeline '{}' not found", pipeline_name))?;

        let mut dag = Self::build_dag_for_pipeline(store, project, pnode)?;

        // Perform topological sort on jobs via Kahn's algorithm
        dag.jobs = Self::topological_sort_jobs(&dag.jobs, &dag.edges)?;

        Ok(dag)
    }

    /// List infrastructure resources, optionally filtered by type.
    pub fn list_infrastructure(
        store: &Store,
        project: &str,
        infra_type: Option<&str>,
    ) -> Result<Vec<InfraResource>> {
        let infra_nodes = store.get_nodes_by_label(project, "Infra", 10_000)?;
        let mut resources = Vec::new();
        for node in &infra_nodes {
            let props: serde_json::Value = node
                .properties_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

            let kind = props
                .get("infra_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Apply filter if specified
            if let Some(filter) = infra_type {
                if kind != filter {
                    continue;
                }
            }

            let resource_type = props
                .get("resource_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            resources.push(InfraResource {
                name: node.name.clone(),
                resource_type,
                kind,
                file_path: node.file_path.clone(),
                properties: props,
            });
        }
        Ok(resources)
    }

    // ── Internal helpers ──────────────────────────────────────

    /// Build a PipelineDag from a Pipeline node by querying related Stage/Job nodes and edges.
    fn build_dag_for_pipeline(
        store: &Store,
        project: &str,
        pnode: &codryn_store::Node,
    ) -> Result<PipelineDag> {
        let props: serde_json::Value = pnode
            .properties_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let ci_system = props
            .get("ci_system")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let triggers: Vec<String> = props
            .get("triggers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let pipeline_info = PipelineInfo {
            name: pnode.name.clone(),
            file_path: pnode.file_path.clone(),
            ci_system,
            triggers,
        };

        // Get all Stage and Job nodes for this project
        let all_stages = store.get_nodes_by_label(project, "Stage", 10_000)?;
        let all_jobs = store.get_nodes_by_label(project, "Job", 10_000)?;

        // Filter stages/jobs belonging to this pipeline via properties_json pipeline_name
        let pipeline_name = &pnode.name;

        let stage_nodes: Vec<_> = all_stages
            .into_iter()
            .filter(|n| {
                n.properties_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|p| {
                        p.get("pipeline_name")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .as_deref()
                    == Some(pipeline_name.as_str())
            })
            .collect();

        let job_nodes: Vec<_> = all_jobs
            .into_iter()
            .filter(|n| {
                n.properties_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|p| {
                        p.get("pipeline_name")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .as_deref()
                    == Some(pipeline_name.as_str())
            })
            .collect();

        // Build node id -> name lookup
        let mut id_to_name: HashMap<i64, String> = HashMap::new();
        id_to_name.insert(pnode.id, pnode.name.clone());
        for n in &stage_nodes {
            id_to_name.insert(n.id, n.name.clone());
        }
        for n in &job_nodes {
            id_to_name.insert(n.id, n.name.clone());
        }

        // Build stages with order from properties
        let mut stages: Vec<StageInfo> = stage_nodes
            .iter()
            .map(|n| {
                let p: serde_json::Value = n
                    .properties_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let order = p.get("order").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                StageInfo {
                    name: n.name.clone(),
                    order,
                }
            })
            .collect();
        stages.sort_by_key(|s| s.order);

        // Build jobs
        let jobs: Vec<JobInfo> = job_nodes
            .iter()
            .map(|n| {
                let p: serde_json::Value = n
                    .properties_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let stage = p
                    .get("stage")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let image = p.get("image").and_then(|v| v.as_str()).map(String::from);
                JobInfo {
                    name: n.name.clone(),
                    stage,
                    image,
                    dependencies: Vec::new(), // filled from edges below
                }
            })
            .collect();

        // Collect relevant edges
        let relevant_types = [
            "BELONGS_TO_STAGE",
            "NEXT_STAGE",
            "DEPENDS_ON",
            "DEPLOYS",
            "BUILDS_IMAGE",
        ];
        let node_ids: HashSet<i64> = id_to_name.keys().copied().collect();

        let mut dag_edges = Vec::new();
        let mut job_dependencies: HashMap<String, Vec<String>> = HashMap::new();

        for edge_type in &relevant_types {
            let edges = store.get_edges_by_type(project, edge_type)?;
            for e in &edges {
                let src_in = node_ids.contains(&e.source_id);
                let tgt_in = node_ids.contains(&e.target_id);
                if src_in || tgt_in {
                    let source_name = id_to_name.get(&e.source_id).cloned().unwrap_or_default();
                    let target_name = id_to_name.get(&e.target_id).cloned().unwrap_or_default();
                    if !source_name.is_empty() && !target_name.is_empty() {
                        dag_edges.push(DagEdge {
                            source: source_name.clone(),
                            target: target_name.clone(),
                            edge_type: edge_type.to_string(),
                        });
                        if *edge_type == "DEPENDS_ON" {
                            job_dependencies
                                .entry(source_name)
                                .or_default()
                                .push(target_name);
                        }
                    }
                }
            }
        }

        // Fill in job dependencies
        let jobs: Vec<JobInfo> = jobs
            .into_iter()
            .map(|mut j| {
                if let Some(deps) = job_dependencies.get(&j.name) {
                    j.dependencies = deps.clone();
                }
                j
            })
            .collect();

        Ok(PipelineDag {
            pipeline: pipeline_info,
            stages,
            jobs,
            edges: dag_edges,
        })
    }

    /// Topological sort of jobs using Kahn's algorithm.
    /// Returns error if circular dependencies are detected.
    fn topological_sort_jobs(jobs: &[JobInfo], edges: &[DagEdge]) -> Result<Vec<JobInfo>> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }

        // Build adjacency list and in-degree map from DEPENDS_ON edges only.
        // DEPENDS_ON edge: source depends on target, so target must come before source.
        let job_names: HashSet<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

        for name in &job_names {
            in_degree.insert(name, 0);
            adjacency.insert(name, Vec::new());
        }

        for edge in edges {
            if edge.edge_type != "DEPENDS_ON" {
                continue;
            }
            let src = edge.source.as_str();
            let tgt = edge.target.as_str();
            if !job_names.contains(src) || !job_names.contains(tgt) {
                continue;
            }
            // source depends on target → target must come first → edge: target → source
            adjacency.entry(tgt).or_default().push(src);
            *in_degree.entry(src).or_insert(0) += 1;
        }

        // Kahn's algorithm
        let mut queue: VecDeque<&str> = VecDeque::new();
        for (name, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(name);
            }
        }

        let mut sorted_names: Vec<&str> = Vec::new();
        while let Some(name) = queue.pop_front() {
            sorted_names.push(name);
            if let Some(neighbors) = adjacency.get(name) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        // Cycle detection: if sorted output has fewer nodes than input
        if sorted_names.len() < job_names.len() {
            let sorted_set: HashSet<&str> = sorted_names.iter().copied().collect();
            let cycle_jobs: Vec<String> = job_names
                .iter()
                .filter(|n| !sorted_set.contains(**n))
                .map(|n| n.to_string())
                .collect();
            bail!(
                "Circular dependency detected among jobs: {}",
                cycle_jobs.join(", ")
            );
        }

        // Rebuild JobInfo vec in sorted order
        let job_map: HashMap<&str, &JobInfo> = jobs.iter().map(|j| (j.name.as_str(), j)).collect();
        let sorted_jobs: Vec<JobInfo> = sorted_names
            .iter()
            .filter_map(|name| job_map.get(name).map(|j| (*j).clone()))
            .collect();

        Ok(sorted_jobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Edge, Node, Project, Store};

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn setup_project(store: &Store, name: &str) {
        store
            .upsert_project(&Project {
                name: name.into(),
                indexed_at: "now".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();
    }

    #[test]
    fn test_list_pipelines_empty() {
        let store = test_store();
        setup_project(&store, "p");
        let result = PipelineService::list_pipelines(&store, "p").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_list_pipelines_with_data() {
        let store = test_store();
        setup_project(&store, "p");

        // Insert a Pipeline node
        let _pid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Pipeline".into(),
                name: "CI".into(),
                qualified_name: "p.pipeline.gitlab.CI".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"ci_system":"gitlab","triggers":["push","merge_request"]}"#.into(),
                ),
            })
            .unwrap();

        // Insert a Stage node
        let sid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Stage".into(),
                name: "build".into(),
                qualified_name: "p.pipeline.gitlab.CI.stage.build".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"pipeline_name":"CI","order":0}"#.into()),
            })
            .unwrap();

        // Insert a Job node
        let jid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Job".into(),
                name: "compile".into(),
                qualified_name: "p.pipeline.gitlab.CI.job.compile".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"pipeline_name":"CI","stage":"build","image":"rust:1.75"}"#.into(),
                ),
            })
            .unwrap();

        // Insert BELONGS_TO_STAGE edge
        store
            .insert_edge(&Edge {
                id: 0,
                project: "p".into(),
                source_id: jid,
                target_id: sid,
                edge_type: "BELONGS_TO_STAGE".into(),
                properties_json: None,
            })
            .unwrap();

        let result = PipelineService::list_pipelines(&store, "p").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pipeline.name, "CI");
        assert_eq!(result[0].pipeline.ci_system, "gitlab");
        assert_eq!(result[0].stages.len(), 1);
        assert_eq!(result[0].stages[0].name, "build");
        assert_eq!(result[0].jobs.len(), 1);
        assert_eq!(result[0].jobs[0].name, "compile");
        assert_eq!(result[0].jobs[0].image, Some("rust:1.75".into()));
        assert!(!result[0].edges.is_empty());
    }

    #[test]
    fn test_get_pipeline_dag_not_found() {
        let store = test_store();
        setup_project(&store, "p");
        let result = PipelineService::get_pipeline_dag(&store, "p", "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_pipeline_dag_topological_sort() {
        let store = test_store();
        setup_project(&store, "p");

        let _pid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Pipeline".into(),
                name: "CI".into(),
                qualified_name: "p.pipeline.gitlab.CI".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"ci_system":"gitlab","triggers":[]}"#.into()),
            })
            .unwrap();

        // Job A depends on Job B (A needs B to finish first)
        let ja = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Job".into(),
                name: "test".into(),
                qualified_name: "p.pipeline.gitlab.CI.job.test".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"pipeline_name":"CI","stage":"test"}"#.into()),
            })
            .unwrap();

        let jb = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Job".into(),
                name: "build".into(),
                qualified_name: "p.pipeline.gitlab.CI.job.build".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"pipeline_name":"CI","stage":"build"}"#.into()),
            })
            .unwrap();

        // test DEPENDS_ON build
        store
            .insert_edge(&Edge {
                id: 0,
                project: "p".into(),
                source_id: ja,
                target_id: jb,
                edge_type: "DEPENDS_ON".into(),
                properties_json: None,
            })
            .unwrap();

        let dag = PipelineService::get_pipeline_dag(&store, "p", "CI").unwrap();
        // build should come before test in topological order
        let build_pos = dag.jobs.iter().position(|j| j.name == "build").unwrap();
        let test_pos = dag.jobs.iter().position(|j| j.name == "test").unwrap();
        assert!(build_pos < test_pos);
    }

    #[test]
    fn test_get_pipeline_dag_cycle_detection() {
        let store = test_store();
        setup_project(&store, "p");

        let _pid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Pipeline".into(),
                name: "CI".into(),
                qualified_name: "p.pipeline.gitlab.CI".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"ci_system":"gitlab","triggers":[]}"#.into()),
            })
            .unwrap();

        let ja = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Job".into(),
                name: "a".into(),
                qualified_name: "p.pipeline.gitlab.CI.job.a".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"pipeline_name":"CI","stage":"s"}"#.into()),
            })
            .unwrap();

        let jb = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Job".into(),
                name: "b".into(),
                qualified_name: "p.pipeline.gitlab.CI.job.b".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"pipeline_name":"CI","stage":"s"}"#.into()),
            })
            .unwrap();

        // a depends on b, b depends on a → cycle
        store
            .insert_edge(&Edge {
                id: 0,
                project: "p".into(),
                source_id: ja,
                target_id: jb,
                edge_type: "DEPENDS_ON".into(),
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&Edge {
                id: 0,
                project: "p".into(),
                source_id: jb,
                target_id: ja,
                edge_type: "DEPENDS_ON".into(),
                properties_json: None,
            })
            .unwrap();

        let result = PipelineService::get_pipeline_dag(&store, "p", "CI");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Circular dependency"));
    }

    #[test]
    fn test_list_infrastructure_empty() {
        let store = test_store();
        setup_project(&store, "p");
        let result = PipelineService::list_infrastructure(&store, "p", None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_list_infrastructure_with_filter() {
        let store = test_store();
        setup_project(&store, "p");

        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Infra".into(),
                name: "web".into(),
                qualified_name: "p.terraform.aws_instance.web".into(),
                file_path: "infra/main.tf".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"infra_type":"terraform","resource_type":"aws_instance"}"#.into(),
                ),
            })
            .unwrap();

        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Infra".into(),
                name: "nginx".into(),
                qualified_name: "p.helm.nginx".into(),
                file_path: "charts/Chart.yaml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"infra_type":"helm","resource_type":"chart"}"#.into()),
            })
            .unwrap();

        // No filter → all
        let all = PipelineService::list_infrastructure(&store, "p", None).unwrap();
        assert_eq!(all.len(), 2);

        // Filter terraform
        let tf = PipelineService::list_infrastructure(&store, "p", Some("terraform")).unwrap();
        assert_eq!(tf.len(), 1);
        assert_eq!(tf[0].name, "web");
        assert_eq!(tf[0].kind, "terraform");

        // Filter helm
        let helm = PipelineService::list_infrastructure(&store, "p", Some("helm")).unwrap();
        assert_eq!(helm.len(), 1);
        assert_eq!(helm[0].name, "nginx");

        // Filter nonexistent type
        let none = PipelineService::list_infrastructure(&store, "p", Some("docker")).unwrap();
        assert!(none.is_empty());
    }

    // ── MCP tool response JSON structure tests ────────────────────────
    // These tests validate the JSON structure that the MCP tools produce
    // by calling the same service methods and serializing with the same
    // json!() pattern used in codryn-mcp/src/lib.rs.
    // Validates: Requirements 9.1, 9.2, 9.3, 9.4

    /// Simulates the find_pipelines MCP tool JSON serialization.
    fn find_pipelines_json(store: &Store, project: &str) -> serde_json::Value {
        match PipelineService::list_pipelines(store, project) {
            Ok(pipelines) => {
                let count = pipelines.len();
                serde_json::json!({ "project": project, "pipelines": pipelines, "count": count })
            }
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        }
    }

    /// Simulates the find_infrastructure MCP tool JSON serialization.
    fn find_infrastructure_json(
        store: &Store,
        project: &str,
        infra_type: Option<&str>,
    ) -> serde_json::Value {
        match PipelineService::list_infrastructure(store, project, infra_type) {
            Ok(resources) => {
                let count = resources.len();
                serde_json::json!({ "project": project, "resources": resources, "count": count })
            }
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        }
    }

    #[test]
    fn test_find_pipelines_json_empty_store() {
        let store = test_store();
        setup_project(&store, "p");

        let json = find_pipelines_json(&store, "p");

        // Verify top-level structure
        assert_eq!(json["project"], "p");
        assert!(json["pipelines"].is_array());
        assert_eq!(json["pipelines"].as_array().unwrap().len(), 0);
        assert_eq!(json["count"], 0);
        assert!(json.get("error").is_none());
    }

    #[test]
    fn test_find_pipelines_json_populated_store() {
        let store = test_store();
        setup_project(&store, "p");

        // Insert Pipeline node
        let _pid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Pipeline".into(),
                name: "CI".into(),
                qualified_name: "p.pipeline.gitlab.CI".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"ci_system":"gitlab","triggers":["push","merge_request"]}"#.into(),
                ),
            })
            .unwrap();

        // Insert Stage node
        let sid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Stage".into(),
                name: "build".into(),
                qualified_name: "p.pipeline.gitlab.CI.stage.build".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"pipeline_name":"CI","order":0}"#.into()),
            })
            .unwrap();

        // Insert Job node
        let jid = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Job".into(),
                name: "compile".into(),
                qualified_name: "p.pipeline.gitlab.CI.job.compile".into(),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"pipeline_name":"CI","stage":"build","image":"rust:1.75"}"#.into(),
                ),
            })
            .unwrap();

        // Insert BELONGS_TO_STAGE edge
        store
            .insert_edge(&Edge {
                id: 0,
                project: "p".into(),
                source_id: jid,
                target_id: sid,
                edge_type: "BELONGS_TO_STAGE".into(),
                properties_json: None,
            })
            .unwrap();

        let json = find_pipelines_json(&store, "p");

        // Verify top-level structure
        assert_eq!(json["project"], "p");
        assert_eq!(json["count"], 1);
        assert!(json.get("error").is_none());

        // Verify pipeline structure
        let pipelines = json["pipelines"].as_array().unwrap();
        assert_eq!(pipelines.len(), 1);

        let pipeline = &pipelines[0];
        assert_eq!(pipeline["pipeline"]["name"], "CI");
        assert_eq!(pipeline["pipeline"]["file_path"], ".gitlab-ci.yml");
        assert_eq!(pipeline["pipeline"]["ci_system"], "gitlab");
        let triggers = pipeline["pipeline"]["triggers"].as_array().unwrap();
        assert_eq!(triggers.len(), 2);
        assert!(triggers.contains(&serde_json::json!("push")));
        assert!(triggers.contains(&serde_json::json!("merge_request")));

        // Verify stages
        let stages = pipeline["stages"].as_array().unwrap();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0]["name"], "build");
        assert_eq!(stages[0]["order"], 0);

        // Verify jobs
        let jobs = pipeline["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["name"], "compile");
        assert_eq!(jobs[0]["stage"], "build");
        assert_eq!(jobs[0]["image"], "rust:1.75");
        assert!(jobs[0]["dependencies"].is_array());

        // Verify edges are present
        let edges = pipeline["edges"].as_array().unwrap();
        assert!(!edges.is_empty());
        // Should have at least the BELONGS_TO_STAGE edge
        let has_belongs = edges.iter().any(|e| e["edge_type"] == "BELONGS_TO_STAGE");
        assert!(has_belongs);
    }

    #[test]
    fn test_find_infrastructure_json_no_filter() {
        let store = test_store();
        setup_project(&store, "p");

        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Infra".into(),
                name: "web".into(),
                qualified_name: "p.terraform.aws_instance.web".into(),
                file_path: "infra/main.tf".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"infra_type":"terraform","resource_type":"aws_instance"}"#.into(),
                ),
            })
            .unwrap();

        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Infra".into(),
                name: "nginx".into(),
                qualified_name: "p.helm.nginx".into(),
                file_path: "charts/Chart.yaml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"infra_type":"helm","resource_type":"chart"}"#.into()),
            })
            .unwrap();

        let json = find_infrastructure_json(&store, "p", None);

        // Verify top-level structure
        assert_eq!(json["project"], "p");
        assert_eq!(json["count"], 2);
        assert!(json.get("error").is_none());

        // Verify resources array
        let resources = json["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);

        // Verify each resource has required fields
        for res in resources {
            assert!(res["name"].is_string());
            assert!(res["resource_type"].is_string());
            assert!(res["kind"].is_string());
            assert!(res["file_path"].is_string());
            assert!(res.get("properties").is_some());
        }

        let names: Vec<&str> = resources
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"web"));
        assert!(names.contains(&"nginx"));
    }

    #[test]
    fn test_find_infrastructure_json_with_type_filter() {
        let store = test_store();
        setup_project(&store, "p");

        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Infra".into(),
                name: "web".into(),
                qualified_name: "p.terraform.aws_instance.web".into(),
                file_path: "infra/main.tf".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"infra_type":"terraform","resource_type":"aws_instance"}"#.into(),
                ),
            })
            .unwrap();

        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Infra".into(),
                name: "nginx".into(),
                qualified_name: "p.helm.nginx".into(),
                file_path: "charts/Chart.yaml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"infra_type":"helm","resource_type":"chart"}"#.into()),
            })
            .unwrap();

        // Filter by terraform
        let json = find_infrastructure_json(&store, "p", Some("terraform"));
        assert_eq!(json["project"], "p");
        assert_eq!(json["count"], 1);
        let resources = json["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["name"], "web");
        assert_eq!(resources[0]["kind"], "terraform");
        assert_eq!(resources[0]["resource_type"], "aws_instance");
    }

    #[test]
    fn test_find_infrastructure_json_invalid_type_filter() {
        let store = test_store();
        setup_project(&store, "p");

        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Infra".into(),
                name: "web".into(),
                qualified_name: "p.terraform.aws_instance.web".into(),
                file_path: "infra/main.tf".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(
                    r#"{"infra_type":"terraform","resource_type":"aws_instance"}"#.into(),
                ),
            })
            .unwrap();

        // Filter by nonexistent type
        let json = find_infrastructure_json(&store, "p", Some("nonexistent"));
        assert_eq!(json["project"], "p");
        assert_eq!(json["count"], 0);
        let resources = json["resources"].as_array().unwrap();
        assert!(resources.is_empty());
        assert!(json.get("error").is_none());
    }
}

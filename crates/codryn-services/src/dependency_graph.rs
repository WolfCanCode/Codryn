use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy)]
pub enum Granularity {
    File,
    Folder,
    Package,
}

#[derive(Debug, Serialize)]
pub struct DependencyEdge {
    pub from: String,
    pub to: String,
    pub weight: usize,
}

#[derive(Debug, Serialize)]
pub struct CyclePath {
    pub nodes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DependencyGraphResult {
    pub nodes: Vec<String>,
    pub edges: Vec<DependencyEdge>,
    pub cycles: Vec<CyclePath>,
    pub topological_order: Vec<String>,
}

fn aggregate_key(file_path: &str, granularity: Granularity) -> String {
    match granularity {
        Granularity::File => file_path.to_string(),
        Granularity::Folder => file_path
            .rfind('/')
            .map(|i| &file_path[..i])
            .unwrap_or(file_path)
            .to_string(),
        Granularity::Package => file_path
            .find('/')
            .map(|i| &file_path[..i])
            .unwrap_or(file_path)
            .to_string(),
    }
}

pub fn get_dependency_graph(
    store: &Store,
    project: &str,
    granularity: Granularity,
    scope: Option<&str>,
    include_cycles_only: bool,
) -> Result<DependencyGraphResult> {
    let conn = store.conn();
    let mut stmt = conn.prepare(
        "SELECT n_src.file_path, n_tgt.file_path, COUNT(*) \
         FROM edges e \
         JOIN nodes n_src ON n_src.id = e.source_id \
         JOIN nodes n_tgt ON n_tgt.id = e.target_id \
         WHERE e.project = ?1 AND e.type = 'IMPORTS' \
         GROUP BY n_src.file_path, n_tgt.file_path",
    )?;

    let rows = stmt.query_map(rusqlite::params![project], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, usize>(2)?,
        ))
    })?;

    // Aggregate edges by granularity
    let mut edge_map: HashMap<(String, String), usize> = HashMap::new();
    let mut node_set: HashSet<String> = HashSet::new();

    for row in rows.flatten() {
        let from = aggregate_key(&row.0, granularity);
        let to = aggregate_key(&row.1, granularity);
        if from == to {
            continue;
        }
        if let Some(s) = scope {
            if !from.starts_with(s) && !to.starts_with(s) {
                continue;
            }
        }
        node_set.insert(from.clone());
        node_set.insert(to.clone());
        *edge_map.entry((from, to)).or_default() += row.2;
    }

    let nodes: Vec<String> = node_set.iter().cloned().collect();
    let node_idx: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    // Build adjacency list
    let mut adj: Vec<Vec<usize>> = vec![vec![]; nodes.len()];
    for (from, to) in edge_map.keys() {
        if let (Some(&fi), Some(&ti)) = (node_idx.get(from.as_str()), node_idx.get(to.as_str())) {
            adj[fi].push(ti);
        }
    }

    // Detect cycles via DFS with in-stack tracking
    let cycles = find_cycles(&adj, &nodes);

    // Topological sort (Kahn's algorithm)
    let topological_order = topo_sort(&adj, &nodes);

    let edges: Vec<DependencyEdge> = if include_cycles_only {
        let cycle_nodes: HashSet<&str> = cycles
            .iter()
            .flat_map(|c| c.nodes.iter().map(|s| s.as_str()))
            .collect();
        edge_map
            .into_iter()
            .filter(|((f, t), _)| {
                cycle_nodes.contains(f.as_str()) && cycle_nodes.contains(t.as_str())
            })
            .map(|((from, to), weight)| DependencyEdge { from, to, weight })
            .collect()
    } else {
        edge_map
            .into_iter()
            .map(|((from, to), weight)| DependencyEdge { from, to, weight })
            .collect()
    };

    Ok(DependencyGraphResult {
        nodes,
        edges,
        cycles,
        topological_order,
    })
}

fn find_cycles(adj: &[Vec<usize>], nodes: &[String]) -> Vec<CyclePath> {
    let n = adj.len();
    let mut visited = vec![false; n];
    let mut in_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut cycles = Vec::new();

    for start in 0..n {
        if visited[start] {
            continue;
        }
        // Iterative DFS
        let mut dfs_stack: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some((node, idx)) = dfs_stack.last_mut() {
            let node = *node;
            if !visited[node] {
                visited[node] = true;
                in_stack[node] = true;
                stack.push(node);
            }
            if *idx < adj[node].len() {
                let next = adj[node][*idx];
                *idx += 1;
                if !visited[next] {
                    dfs_stack.push((next, 0));
                } else if in_stack[next] {
                    // Found a cycle
                    let pos = stack.iter().position(|&x| x == next).unwrap_or(0);
                    let cycle_nodes: Vec<String> =
                        stack[pos..].iter().map(|&i| nodes[i].clone()).collect();
                    if cycle_nodes.len() > 1 {
                        cycles.push(CyclePath { nodes: cycle_nodes });
                    }
                }
            } else {
                in_stack[node] = false;
                stack.pop();
                dfs_stack.pop();
            }
        }
    }
    cycles
}

fn topo_sort(adj: &[Vec<usize>], nodes: &[String]) -> Vec<String> {
    let n = adj.len();
    let mut in_degree = vec![0usize; n];
    for neighbors in adj {
        for &t in neighbors {
            in_degree[t] += 1;
        }
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut order = Vec::new();
    while let Some(node) = queue.pop() {
        order.push(nodes[node].clone());
        for &next in &adj[node] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.push(next);
            }
        }
    }
    order
}

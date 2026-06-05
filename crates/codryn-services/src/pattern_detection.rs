use anyhow::Result;
use codryn_store::Store;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct PatternInstance {
    pub pattern_name: String,
    pub confidence: f64,
    pub involved_symbols: Vec<String>,
    pub recommendation: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PatternDetectionResult {
    pub patterns: Vec<PatternInstance>,
    pub antipatterns: Vec<PatternInstance>,
}

pub struct PatternDetectionService;

impl PatternDetectionService {
    pub fn detect_patterns(
        store: &Store,
        project: &str,
        patterns_only: bool,
        antipatterns_only: bool,
    ) -> Result<PatternDetectionResult> {
        let mut patterns = Vec::new();
        let mut antipatterns = Vec::new();
        let conn = store.conn();

        if !antipatterns_only {
            // MVC pattern
            let mut stmt = conn.prepare(
                "SELECT name FROM nodes WHERE project = ?1 AND label = 'Class' AND (name LIKE '%Controller%' OR name LIKE '%Service%' OR name LIKE '%Repository%')"
            )?;
            let names: Vec<String> = stmt
                .query_map(params![project], |r| r.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            if names.len() >= 2 {
                patterns.push(PatternInstance {
                    pattern_name: "MVC".into(),
                    confidence: 0.8,
                    involved_symbols: names,
                    recommendation: None,
                });
            }

            // Singleton pattern
            let mut stmt = conn.prepare(
                "SELECT name FROM nodes WHERE project = ?1 AND label = 'Class' AND (name LIKE '%Singleton%' OR name LIKE '%getInstance%')"
            )?;
            let singletons: Vec<String> = stmt
                .query_map(params![project], |r| r.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            if !singletons.is_empty() {
                patterns.push(PatternInstance {
                    pattern_name: "Singleton".into(),
                    confidence: 0.7,
                    involved_symbols: singletons,
                    recommendation: None,
                });
            }
        }

        if !patterns_only {
            // God Class: fan_in + fan_out > 50
            let mut stmt = conn.prepare(
                "SELECT n.name FROM nodes n WHERE n.project = ?1 AND n.label = 'Class' AND \
                 (SELECT COUNT(*) FROM edges e WHERE e.source_id = n.id OR e.target_id = n.id) > 50"
            )?;
            let gods: Vec<String> = stmt
                .query_map(params![project], |r| r.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            for name in gods {
                antipatterns.push(PatternInstance {
                    pattern_name: "God Class".into(),
                    confidence: 0.75,
                    involved_symbols: vec![name],
                    recommendation: Some("Consider splitting into smaller, focused classes".into()),
                });
            }

            // Circular dependency (2-cycle in IMPORTS)
            let mut stmt = conn.prepare(
                "SELECT DISTINCT n1.name, n2.name FROM edges e1 \
                 JOIN edges e2 ON e1.source_id = e2.target_id AND e1.target_id = e2.source_id \
                 JOIN nodes n1 ON n1.id = e1.source_id \
                 JOIN nodes n2 ON n2.id = e1.target_id \
                 WHERE e1.type = 'IMPORTS' AND e2.type = 'IMPORTS' AND n1.project = ?1 AND n1.id < n2.id \
                 LIMIT 20"
            )?;
            let mut cycles: Vec<PatternInstance> = Vec::new();
            stmt.query_map(params![project], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .for_each(|(a, b)| {
                cycles.push(PatternInstance {
                    pattern_name: "Circular Dependency".into(),
                    confidence: 0.9,
                    involved_symbols: vec![a, b],
                    recommendation: Some("Break the cycle by extracting shared types".into()),
                });
            });
            antipatterns.extend(cycles);
        }

        Ok(PatternDetectionResult {
            patterns,
            antipatterns,
        })
    }
}

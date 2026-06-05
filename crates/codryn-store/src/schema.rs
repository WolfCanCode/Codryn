pub const DDL: &str = "\
CREATE TABLE IF NOT EXISTS projects (\
  name TEXT PRIMARY KEY,\
  indexed_at TEXT NOT NULL,\
  root_path TEXT NOT NULL\
);\
CREATE TABLE IF NOT EXISTS file_hashes (\
  project TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,\
  rel_path TEXT NOT NULL,\
  sha256 TEXT NOT NULL,\
  mtime_ns INTEGER NOT NULL DEFAULT 0,\
  size INTEGER NOT NULL DEFAULT 0,\
  is_deleted INTEGER NOT NULL DEFAULT 0,\
  PRIMARY KEY (project, rel_path)\
);\
CREATE TABLE IF NOT EXISTS nodes (\
  id INTEGER PRIMARY KEY AUTOINCREMENT,\
  project TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,\
  label TEXT NOT NULL,\
  name TEXT NOT NULL,\
  qualified_name TEXT NOT NULL,\
  file_path TEXT DEFAULT '',\
  start_line INTEGER DEFAULT 0,\
  end_line INTEGER DEFAULT 0,\
  properties TEXT DEFAULT '{}',\
  UNIQUE(project, qualified_name)\
);\
CREATE TABLE IF NOT EXISTS edges (\
  id INTEGER PRIMARY KEY AUTOINCREMENT,\
  project TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,\
  source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,\
  target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,\
  type TEXT NOT NULL,\
  properties TEXT DEFAULT '{}',\
  UNIQUE(source_id, target_id, type)\
);\
CREATE TABLE IF NOT EXISTS project_summaries (\
  project TEXT PRIMARY KEY,\
  summary TEXT NOT NULL,\
  source_hash TEXT NOT NULL,\
  created_at TEXT NOT NULL,\
  updated_at TEXT NOT NULL\
);\
CREATE TABLE IF NOT EXISTS project_links (\
  source_project TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,\
  target_project TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,\
  created_at TEXT NOT NULL,\
  PRIMARY KEY (source_project, target_project)\
);\
CREATE TABLE IF NOT EXISTS decisions (\
  id TEXT NOT NULL,\
  project TEXT NOT NULL REFERENCES projects(name) ON DELETE CASCADE,\
  title TEXT NOT NULL,\
  content TEXT NOT NULL,\
  created_at TEXT NOT NULL,\
  PRIMARY KEY (project, id)\
);\
CREATE TABLE IF NOT EXISTS tool_calls (\
  id INTEGER PRIMARY KEY AUTOINCREMENT,\
  tool_name TEXT NOT NULL,\
  project TEXT DEFAULT '',\
  source TEXT DEFAULT 'ui',\
  duration_ms INTEGER DEFAULT 0,\
  success INTEGER DEFAULT 1,\
  called_at TEXT NOT NULL\
);\
CREATE VIRTUAL TABLE IF NOT EXISTS code_fts USING fts5(\
  project, qualified_name, content,\
  tokenize='porter unicode61'\
);\
CREATE TABLE IF NOT EXISTS code_blobs (\
  project TEXT NOT NULL,\
  qualified_name TEXT NOT NULL,\
  content BLOB NOT NULL,\
  is_compressed INTEGER NOT NULL DEFAULT 0,\
  PRIMARY KEY (project, qualified_name)\
);\
CREATE TABLE IF NOT EXISTS _index_progress (\
  project TEXT NOT NULL,\
  phase TEXT NOT NULL,\
  phase_index INTEGER NOT NULL,\
  files_processed INTEGER NOT NULL DEFAULT 0,\
  started_at TEXT NOT NULL,\
  completed INTEGER NOT NULL DEFAULT 0,\
  PRIMARY KEY (project, phase)\
);\
CREATE TABLE IF NOT EXISTS _index_runs (\
  id TEXT PRIMARY KEY,\
  project TEXT NOT NULL,\
  mode TEXT NOT NULL,\
  status TEXT NOT NULL,\
  git_commit TEXT,\
  started_at TEXT NOT NULL,\
  completed_at TEXT,\
  node_count INTEGER DEFAULT 0,\
  edge_count INTEGER DEFAULT 0,\
  error TEXT\
);\
CREATE TABLE IF NOT EXISTS _snapshots (\
  id INTEGER PRIMARY KEY AUTOINCREMENT,\
  project TEXT NOT NULL,\
  index_run_id TEXT,\
  timestamp TEXT NOT NULL,\
  total_nodes INTEGER NOT NULL,\
  total_edges INTEGER NOT NULL,\
  label_counts_json TEXT NOT NULL,\
  edge_type_counts_json TEXT NOT NULL,\
  content_hash TEXT NOT NULL\
);";

pub const INDEXES: &str = "\
CREATE INDEX IF NOT EXISTS idx_nodes_label ON nodes(project, label);\
CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(project, name);\
CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(project, file_path);\
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);\
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);\
CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(project, type);\
CREATE INDEX IF NOT EXISTS idx_edges_target_type ON edges(project, target_id, type);\
CREATE INDEX IF NOT EXISTS idx_edges_source_type ON edges(project, source_id, type);\
CREATE INDEX IF NOT EXISTS idx_nodes_properties_test ON nodes(project, json_extract(properties, '$.is_test'));\
CREATE INDEX IF NOT EXISTS idx_nodes_properties_exported ON nodes(project, json_extract(properties, '$.is_exported'));\
CREATE INDEX IF NOT EXISTS idx_nodes_properties_complexity ON nodes(project, json_extract(properties, '$.complexity'));\
CREATE INDEX IF NOT EXISTS idx_index_runs_project_started ON _index_runs(project, started_at DESC);\
CREATE INDEX IF NOT EXISTS idx_snapshots_project ON _snapshots(project, timestamp DESC);";

/// DDL for the embeddings sidecar table.
/// This table stores 384-dimensional float32 embeddings for semantic code search.
/// It is designed to be created in the same database but logically acts as a sidecar
/// for the main graph schema (Requirement 23.6).
pub const EMBEDDINGS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS embeddings (\
  node_id INTEGER PRIMARY KEY,\
  project TEXT NOT NULL,\
  embedding BLOB NOT NULL,\
  text_hash TEXT NOT NULL,\
  created_at TEXT NOT NULL\
);\
CREATE INDEX IF NOT EXISTS idx_embeddings_project ON embeddings(project);";

/// DDL for the dependency freshness cache table.
/// This table caches registry responses (latest version, deprecation status)
/// to avoid repeated HTTP calls. Entries older than 1 hour are considered stale.
/// (Requirement 33.4)
pub const DEP_CACHE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS dep_cache (\
  package_name TEXT NOT NULL,\
  registry TEXT NOT NULL,\
  latest_version TEXT,\
  deprecated INTEGER DEFAULT 0,\
  checked_at TEXT NOT NULL,\
  PRIMARY KEY (package_name, registry)\
);";

/// Migrate the `tool_calls` table to add new columns for agent/model tracking
/// and token spend. Uses try-and-ignore because SQLite doesn't support
/// `ALTER TABLE ADD COLUMN IF NOT EXISTS`.
pub fn migrate_tool_calls(conn: &rusqlite::Connection) {
    let migrations = [
        "ALTER TABLE tool_calls ADD COLUMN agent_name TEXT DEFAULT 'unknown'",
        "ALTER TABLE tool_calls ADD COLUMN model_name TEXT DEFAULT 'unknown'",
        "ALTER TABLE tool_calls ADD COLUMN input_tokens INTEGER DEFAULT 0",
        "ALTER TABLE tool_calls ADD COLUMN output_tokens INTEGER DEFAULT 0",
        "ALTER TABLE tool_calls ADD COLUMN response_bytes INTEGER DEFAULT 0",
        "ALTER TABLE file_hashes ADD COLUMN is_deleted INTEGER DEFAULT 0",
        "ALTER TABLE tool_calls ADD COLUMN request_body TEXT DEFAULT ''",
        "ALTER TABLE tool_calls ADD COLUMN response_body TEXT DEFAULT ''",
        // Confidence scoring columns on edges
        "ALTER TABLE edges ADD COLUMN confidence REAL DEFAULT NULL",
        "ALTER TABLE edges ADD COLUMN edge_source TEXT DEFAULT NULL",
        // Index run tracking: add run_id to _index_progress
        "ALTER TABLE _index_progress ADD COLUMN run_id TEXT",
        // Route snapshot data for API surface diff (Requirement 25.4)
        "ALTER TABLE _snapshots ADD COLUMN routes_json TEXT DEFAULT NULL",
    ];
    for sql in &migrations {
        let _ = conn.execute_batch(sql); // ignore "duplicate column" errors
    }
}

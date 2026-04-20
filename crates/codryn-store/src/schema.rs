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
);";

pub const INDEXES: &str = "\
CREATE INDEX IF NOT EXISTS idx_nodes_label ON nodes(project, label);\
CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(project, name);\
CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(project, file_path);\
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);\
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);\
CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(project, type);\
CREATE INDEX IF NOT EXISTS idx_edges_target_type ON edges(project, target_id, type);\
CREATE INDEX IF NOT EXISTS idx_edges_source_type ON edges(project, source_id, type);";

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
    ];
    for sql in &migrations {
        let _ = conn.execute_batch(sql); // ignore "duplicate column" errors
    }
}

//! Index and search the text chunks.
//!
//! The Indexer actor manages a SQLite database with two main tables: `files` and `chunks`. The
//! `files` table tracks metadata about each file (path, source, size, updated_at). The `chunks`
//! table stores the text chunks with their path, line numbers, and embedding model. A virtual
//! FTS5 table `chunks_fts` enables full-text search on chunk text. A virtual table `chunks_vec`
//! (sqlite-vec) stores the chunk embeddings for efficient vector search.

use std::path::Path;
use std::sync::{Arc, Mutex};

use acktor::{Actor, Context, Handler};
use chrono::Utc;
use rusqlite::{Connection, params};

use super::error::IndexError;
use super::message::{
    EnsureVecReady, IndexDelete, IndexInsert, IndexSearch, SearchResult, SearchScope,
};

/// The Indexer actor. Owns a shared SQLite connection.
pub struct Indexer {
    conn: Arc<Mutex<Connection>>,
}

impl Indexer {
    /// Opens an in-memory index. The vector table is created lazily on first insert via
    /// [`EnsureVecReady`].
    pub fn open_in_memory() -> Result<Self, IndexError> {
        load_sqlite_vec();
        let conn = Connection::open_in_memory()?;
        let index = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        index.init_schema()?;

        Ok(index)
    }

    /// Opens a persistent index at the given path.
    ///
    /// If a previous embedding dimension is recorded in `meta`, the vec table is rebuilt eagerly.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        load_sqlite_vec();
        let conn = Connection::open(path)?;
        let index = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        index.init_schema()?;
        index.restore_vec_table()?;

        Ok(index)
    }

    fn init_schema(&self) -> Result<(), IndexError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                path       TEXT PRIMARY KEY,
                source     TEXT NOT NULL,
                size       INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chunks (
                id         TEXT PRIMARY KEY,
                path       TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line   INTEGER NOT NULL,
                model      TEXT NOT NULL,
                text       TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_chunks_path ON chunks(path);

            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                text,
                id UNINDEXED,
                path UNINDEXED,
                model UNINDEXED,
                start_line UNINDEXED,
                end_line UNINDEXED,
                content=chunks,
                content_rowid=rowid,
                tokenize='unicode61'
            );

            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, text, id, path, model, start_line, end_line)
                VALUES (new.rowid, new.text, new.id, new.path, new.model, new.start_line, new.end_line);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, text, id, path, model, start_line, end_line)
                VALUES ('delete', old.rowid, old.text, old.id, old.path, old.model, old.start_line, old.end_line);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, text, id, path, model, start_line, end_line)
                VALUES ('delete', old.rowid, old.text, old.id, old.path, old.model, old.start_line, old.end_line);
                INSERT INTO chunks_fts(rowid, text, id, path, model, start_line, end_line)
                VALUES (new.rowid, new.text, new.id, new.path, new.model, new.start_line, new.end_line);
            END;

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        Ok(())
    }

    /// On reopen of a persistent index, rebuild `chunks_vec` if a dimension is recorded in `meta`.
    fn restore_vec_table(&self) -> Result<(), IndexError> {
        let conn = self.conn.lock().unwrap();
        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'embedding_dim'",
                [],
                |row| row.get(0),
            )
            .ok();
        if let Some(s) = stored
            && let Ok(dim) = s.parse::<usize>()
        {
            create_vec_table(&conn, dim)?;
        }

        Ok(())
    }
}

fn create_vec_table(conn: &Connection, dim: usize) -> Result<(), IndexError> {
    let ddl = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(
            chunk_id TEXT PRIMARY KEY,
            embedding float[{dim}]
        )"
    );
    conn.execute_batch(&ddl)?;

    Ok(())
}

fn do_ensure_vec_ready(conn: &Connection, dim: usize) -> Result<(), IndexError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'embedding_dim'",
            [],
            |row| row.get(0),
        )
        .ok();
    if let Some(s) = stored {
        let stored_dim: usize = s.parse().map_err(|_| IndexError::DimensionMismatch {
            stored: 0,
            received: dim,
        })?;
        if stored_dim != dim {
            return Err(IndexError::DimensionMismatch {
                stored: stored_dim,
                received: dim,
            });
        }
        create_vec_table(conn, dim)?;
        return Ok(());
    }
    create_vec_table(conn, dim)?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('embedding_dim', ?1)",
        params![dim.to_string()],
    )?;

    Ok(())
}

/// Register the sqlite-vec extension globally via `sqlite3_auto_extension`.
///
/// Safe to call multiple times since SQLite deduplicates auto-extensions.
#[inline]
fn load_sqlite_vec() {
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut i8,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    }
}

/// Convert a float slice to a little-endian byte blob for sqlite-vec.
fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

// ---------------------------------------------------------------------------
// DB operations (run inside spawn_blocking)
// ---------------------------------------------------------------------------

fn do_insert(conn: &Connection, msg: &IndexInsert) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;
    let now = Utc::now().timestamp();

    // Delete old vector entries for this path's chunks.
    let old_chunk_ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM chunks WHERE path = ?1")?;
        stmt.query_map(params![msg.path], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?
    };
    for cid in &old_chunk_ids {
        tx.execute("DELETE FROM chunks_vec WHERE chunk_id = ?1", params![cid])?;
    }

    // Delete old chunks and file entry.
    tx.execute("DELETE FROM chunks WHERE path = ?1", params![msg.path])?;
    tx.execute("DELETE FROM files WHERE path = ?1", params![msg.path])?;

    // Insert new file entry.
    tx.execute(
        "INSERT INTO files (path, source, size, updated_at) VALUES (?1, ?2, ?3, ?4)",
        params![msg.path, msg.source, msg.size as i64, now],
    )?;

    // Insert new chunks and their embeddings.
    for (i, chunk) in msg.chunks.iter().enumerate() {
        let chunk_id = format!("{}#{}", msg.path, i);
        tx.execute(
            "INSERT INTO chunks (id, path, start_line, end_line, model, text, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                chunk_id,
                msg.path,
                chunk.start_line,
                chunk.end_line,
                msg.model,
                chunk.text,
                now,
            ],
        )?;

        let blob = vec_to_blob(&chunk.embedding.0);
        tx.execute(
            "INSERT INTO chunks_vec (chunk_id, embedding) VALUES (?1, ?2)",
            params![chunk_id, blob],
        )?;
    }

    tx.commit()?;
    Ok(())
}

fn do_delete(conn: &Connection, path: &str) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction()?;

    let old_chunk_ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM chunks WHERE path = ?1")?;
        stmt.query_map(params![path], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?
    };
    for cid in &old_chunk_ids {
        tx.execute("DELETE FROM chunks_vec WHERE chunk_id = ?1", params![cid])?;
    }

    tx.execute("DELETE FROM chunks WHERE path = ?1", params![path])?;
    tx.execute("DELETE FROM files WHERE path = ?1", params![path])?;

    tx.commit()?;

    Ok(())
}

fn do_search(conn: &Connection, msg: &IndexSearch) -> Result<Vec<SearchResult>, IndexError> {
    let path_prefix = match msg.scope {
        SearchScope::All => "%".to_string(),
        SearchScope::User => format!("{}/%", msg.username),
        SearchScope::Agent => format!("{}/{}/%", msg.username, msg.agent_id),
    };
    let source_clause = if msg.raw_only {
        " AND f.source = 'raw'"
    } else {
        ""
    };
    let sql = format!(
        "SELECT cv.chunk_id, cv.distance, c.path, c.start_line, c.end_line, c.text
         FROM chunks_vec cv
         JOIN chunks c ON c.id = cv.chunk_id
         JOIN files f ON f.path = c.path
         WHERE cv.embedding MATCH ?1 AND k = ?2 AND c.path LIKE ?3{source_clause}
         ORDER BY cv.distance ASC"
    );

    let mut all_results: Vec<SearchResult> = Vec::new();
    for emb in &msg.embeddings {
        let blob = vec_to_blob(&emb.0);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![blob, msg.limit as i64, path_prefix], |row| {
            Ok(SearchResult {
                path: row.get(2)?,
                start_line: row.get(3)?,
                end_line: row.get(4)?,
                score: {
                    let distance: f32 = row.get(1)?;
                    1.0 - distance
                },
                snippet: row.get(5)?,
            })
        })?;
        for row in rows {
            all_results.push(row?);
        }
    }

    all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    all_results.truncate(msg.limit);
    Ok(all_results)
}

// ---------------------------------------------------------------------------
// Actor + Handler impls
// ---------------------------------------------------------------------------

impl Actor for Indexer {
    type Context = Context<Self>;
    type Error = IndexError;
}

impl Handler<EnsureVecReady> for Indexer {
    type Result = Result<(), IndexError>;

    async fn handle(
        &mut self,
        msg: EnsureVecReady,
        _ctx: &mut Self::Context,
    ) -> Result<(), IndexError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            do_ensure_vec_ready(&conn, msg.dim)
        })
        .await?
    }
}

impl Handler<IndexInsert> for Indexer {
    type Result = Result<(), IndexError>;

    async fn handle(
        &mut self,
        msg: IndexInsert,
        _ctx: &mut Self::Context,
    ) -> Result<(), IndexError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            do_insert(&conn, &msg)
        })
        .await?
    }
}

impl Handler<IndexDelete> for Indexer {
    type Result = Result<(), IndexError>;

    async fn handle(
        &mut self,
        msg: IndexDelete,
        _ctx: &mut Self::Context,
    ) -> Result<(), IndexError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            do_delete(&conn, &msg.path)
        })
        .await?
    }
}

impl Handler<IndexSearch> for Indexer {
    type Result = Result<Vec<SearchResult>, IndexError>;

    async fn handle(
        &mut self,
        msg: IndexSearch,
        _ctx: &mut Self::Context,
    ) -> Result<Vec<SearchResult>, IndexError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            do_search(&conn, &msg)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::super::message::{Chunk, EnsureVecReady, IndexDelete, IndexInsert, IndexSearch};
    use super::*;
    use crate::llm::Embedding;

    fn test_index() -> Indexer {
        Indexer::open_in_memory().unwrap()
    }

    #[tokio::test]
    async fn insert_and_search() {
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        addr.send(EnsureVecReady { dim: 128 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        addr.send(IndexInsert {
            path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            source: "raw".to_string(),
            size: 100,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "Rust programming language".to_string(),
                start_line: 1,
                end_line: 5,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![0.0; 128])],
                username: "alice".to_string(),
                agent_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                scope: super::super::message::SearchScope::User,
                raw_only: false,
                limit: 10,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].path, "alice/agent1/daily_note/2026-03-31.md");
    }

    #[tokio::test]
    async fn delete_removes_chunks() {
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        addr.send(EnsureVecReady { dim: 128 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        addr.send(IndexInsert {
            path: "alice/agent1/daily_note/temp.md".to_string(),
            source: "raw".to_string(),
            size: 50,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "to be deleted".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(IndexDelete {
            path: "alice/agent1/daily_note/temp.md".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![0.0; 128])],
                username: "alice".to_string(),
                agent_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                scope: super::super::message::SearchScope::User,
                raw_only: false,
                limit: 10,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn insert_replaces_existing() {
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        let path = "alice/agent1/daily_note/replace.md".to_string();

        addr.send(EnsureVecReady { dim: 128 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        addr.send(IndexInsert {
            path: path.clone(),
            source: "raw".to_string(),
            size: 10,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "version one".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(IndexInsert {
            path: path.clone(),
            source: "raw".to_string(),
            size: 12,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "version two".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: Embedding(vec![0.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![0.0; 128])],
                username: "alice".to_string(),
                agent_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                scope: super::super::message::SearchScope::User,
                raw_only: false,
                limit: 10,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("version two"));
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        let result = addr
            .send(IndexDelete {
                path: "never/existed.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap();

        assert!(result.is_ok());
    }

    async fn seed_two_users(addr: &acktor::Address<Indexer>) {
        addr.send(EnsureVecReady { dim: 128 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        for (path, source) in [
            ("alice/agent1/proj/a.md", "raw"),
            ("alice/_synthesized/2026-05-20-01.md", "synthesized"),
            ("bob/agent9/proj/b.md", "raw"),
        ] {
            addr.send(IndexInsert {
                path: path.to_string(),
                source: source.to_string(),
                size: 10,
                model: "mock".to_string(),
                chunks: vec![Chunk {
                    text: "shared topic".to_string(),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(vec![0.0; 128]),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        }
    }

    fn alice() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    async fn run_search(
        addr: &acktor::Address<Indexer>,
        scope: super::super::message::SearchScope,
        raw_only: bool,
    ) -> Vec<SearchResult> {
        addr.send(IndexSearch {
            embeddings: vec![Embedding(vec![0.0; 128])],
            username: "alice".to_string(),
            agent_id: alice(),
            scope,
            raw_only,
            limit: 50,
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap()
    }

    #[tokio::test]
    async fn scope_all_spans_every_user() {
        use super::super::message::SearchScope;
        let (addr, _h) = test_index().start("ix").unwrap();
        seed_two_users(&addr).await;
        let paths: Vec<String> = run_search(&addr, SearchScope::All, false)
            .await
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert!(paths.iter().any(|p| p.starts_with("alice/agent1/")));
        assert!(paths.iter().any(|p| p.starts_with("bob/")));
        assert!(
            paths
                .iter()
                .any(|p| p == "alice/_synthesized/2026-05-20-01.md")
        );
    }

    #[tokio::test]
    async fn scope_user_and_raw_only() {
        use super::super::message::SearchScope;
        let (addr, _h) = test_index().start("ix").unwrap();
        seed_two_users(&addr).await;

        let user_paths: Vec<String> = run_search(&addr, SearchScope::User, false)
            .await
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert!(user_paths.iter().all(|p| p.starts_with("alice/")));
        assert!(user_paths.iter().any(|p| p.contains("_synthesized")));

        let raw_paths: Vec<String> = run_search(&addr, SearchScope::User, true)
            .await
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert!(
            raw_paths
                .iter()
                .all(|p| p.starts_with("alice/") && !p.contains("_synthesized"))
        );
    }
}

//! Index and search the text chunks.
//!
//! The Indexer actor manages a SQLite database with two main tables: `files` and `chunks`. The
//! `files` table tracks metadata about each file (path, source, size, updated_at). The `chunks`
//! table stores the text chunks with their path, line numbers, and embedding model. A virtual
//! FTS5 table `chunks_fts` enables full-text search on chunk text. A virtual table `chunks_vec`
//! (sqlite-vec) stores the chunk embeddings for efficient vector search.

use std::path::Path;
use std::sync::{Arc, Mutex};

use acktor::{Actor, Context, Handler, utils::debug_trace};
use ahash::HashMap;
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
            embedding float[{dim}] distance_metric=cosine
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

/// Escapes the SQL `LIKE` wildcards (`\`, `%`, `_`) so a username containing them can't widen
/// the scope prefix and match another user's path. Paired with an `ESCAPE '\'` clause.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// How many extra candidates to pull from the KNN per requested result when a search is scoped.
///
/// `sqlite-vec` applies its `k` limit *before* our scope filter, so without over-fetching a
/// scoped search can return fewer than `limit` results when the globally-nearest chunks all
/// belong to other users.
const SCOPE_OVERFETCH: usize = 10;

fn do_search(conn: &Connection, msg: &IndexSearch) -> Result<Vec<SearchResult>, IndexError> {
    let path_prefix = match msg.scope {
        SearchScope::All => "%".to_string(),
        SearchScope::User => format!("{}/%", like_escape(&msg.username)),
        SearchScope::Agent => format!("{}/{}/%", like_escape(&msg.username), msg.agent_id),
    };
    // `All` needs no filtering, so `limit` candidates suffice; scoped searches over-fetch.
    let fetch_k = match msg.scope {
        SearchScope::All => msg.limit,
        SearchScope::User | SearchScope::Agent => msg.limit.saturating_mul(SCOPE_OVERFETCH),
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
         WHERE cv.embedding MATCH ?1 AND k = ?2 AND c.path LIKE ?3 ESCAPE '\\'{source_clause}
         ORDER BY cv.distance ASC"
    );

    // A multi-embedding query (a long query split into chunks) runs one KNN per query
    // embedding, so the same stored chunk can match several of them. Dedup by chunk_id,
    // keeping the highest score, so one chunk can't occupy several result slots and crowd
    // out distinct matches.
    let mut best: HashMap<String, SearchResult> = HashMap::default();
    for emb in &msg.embeddings {
        let blob = vec_to_blob(&emb.0);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![blob, fetch_k as i64, path_prefix], |row| {
            // A degenerate stored vector (e.g. all-zeros) has an undefined cosine distance,
            // which sqlite-vec returns as NULL. Drop such rows rather than failing the whole
            // search.
            let chunk_id: String = row.get(0)?;
            let distance: Option<f32> = row.get(1)?;
            let path: String = row.get(2)?;
            let start_line = row.get(3)?;
            let end_line = row.get(4)?;
            let snippet: String = row.get(5)?;

            Ok(distance.map(|distance| {
                (
                    chunk_id,
                    SearchResult {
                        path,
                        start_line,
                        end_line,
                        // Cosine distance is in [0, 2] (0 = identical direction); map to a [0, 1]
                        // similarity score where 1.0 is most similar.
                        score: 1.0 - distance / 2.0,
                        snippet,
                    },
                )
            }))
        })?;

        for row in rows {
            if let Some((chunk_id, result)) = row? {
                match best.get(&chunk_id) {
                    Some(existing) if existing.score >= result.score => {}
                    _ => {
                        best.insert(chunk_id, result);
                    }
                }
            }
        }
    }

    let mut all_results: Vec<SearchResult> = best.into_values().collect();
    // `total_cmp` orders deterministically without panicking even on a NaN score.
    all_results.sort_by(|a, b| b.score.total_cmp(&a.score));
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
        debug_trace!("Handle command {:?}", msg);

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
        debug_trace!("Handle command {:?}", msg);

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
        debug_trace!("Handle command {:?}", msg);

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
        debug_trace!("Handle command {:?}", msg);

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
                embedding: Embedding(vec![1.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![1.0; 128])],
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
    async fn user_scope_does_not_leak_across_underscore_usernames() {
        // Usernames `a_b` and `axb` are both valid, but a `_` in a LIKE pattern is a
        // single-char wildcard, so an unescaped `a_b/%` would also match `axb/...`.
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        addr.send(EnsureVecReady { dim: 4 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        for owner in ["a_b", "axb"] {
            addr.send(IndexInsert {
                path: format!("{owner}/agent1/note.md"),
                source: "raw".to_string(),
                size: 10,
                model: "mock".to_string(),
                chunks: vec![Chunk {
                    text: format!("note for {owner}"),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(vec![1.0; 4]),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        }

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![1.0; 4])],
                username: "a_b".to_string(),
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

        assert!(!results.is_empty(), "a_b's own note should be found");
        assert!(
            results.iter().all(|r| r.path.starts_with("a_b/")),
            "search scoped to a_b leaked another user's memory: {:?}",
            results.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn scoped_search_overfetches_past_closer_out_of_scope_chunks() {
        // The k nearest chunks globally can all be out of scope; if the KNN only fetches
        // `limit` rows, the scope filter then leaves nothing even though in-scope matches exist.
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        addr.send(EnsureVecReady { dim: 2 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        // Three "other" chunks sit exactly on the query vector (closest of all).
        for i in 0..3 {
            addr.send(IndexInsert {
                path: format!("other/agent1/n{i}.md"),
                source: "raw".to_string(),
                size: 10,
                model: "mock".to_string(),
                chunks: vec![Chunk {
                    text: format!("other {i}"),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(vec![1.0, 0.0]),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        }
        // Two "alice" chunks point elsewhere, so they rank below every "other" chunk.
        for i in 0..2 {
            addr.send(IndexInsert {
                path: format!("alice/agent1/n{i}.md"),
                source: "raw".to_string(),
                size: 10,
                model: "mock".to_string(),
                chunks: vec![Chunk {
                    text: format!("alice {i}"),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(vec![0.0, 1.0]),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        }

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![1.0, 0.0])],
                username: "alice".to_string(),
                agent_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                scope: super::super::message::SearchScope::User,
                raw_only: false,
                limit: 2,
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            results.len(),
            2,
            "alice's two chunks should be found despite closer out-of-scope chunks, got {:?}",
            results.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
        assert!(results.iter().all(|r| r.path.starts_with("alice/")));
    }

    #[tokio::test]
    async fn search_scores_cosine_similarity() {
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        addr.send(EnsureVecReady { dim: 2 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        // One chunk points the same way as the query; one is orthogonal.
        for (name, emb) in [("same", vec![1.0, 0.0]), ("orth", vec![0.0, 1.0])] {
            addr.send(IndexInsert {
                path: format!("alice/agent1/{name}.md"),
                source: "raw".to_string(),
                size: 10,
                model: "mock".to_string(),
                chunks: vec![Chunk {
                    text: name.to_string(),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(emb),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        }

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![1.0, 0.0])],
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

        // Identical direction -> cosine distance 0 -> score 1.0; orthogonal -> distance 1 -> 0.5.
        assert_eq!(results[0].path, "alice/agent1/same.md");
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "identical-direction score should be ~1.0, got {}",
            results[0].score
        );
        let orth = results
            .iter()
            .find(|r| r.path.ends_with("orth.md"))
            .unwrap();
        assert!(
            (orth.score - 0.5).abs() < 1e-5,
            "orthogonal cosine score should be ~0.5, got {}",
            orth.score
        );
    }

    #[tokio::test]
    async fn multi_embedding_search_deduplicates_chunks() {
        // A query that splits into multiple embeddings runs one KNN each; the same chunk
        // matching several of them must appear only once in the results.
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        addr.send(EnsureVecReady { dim: 2 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        addr.send(IndexInsert {
            path: "alice/agent1/only.md".to_string(),
            source: "raw".to_string(),
            size: 10,
            model: "mock".to_string(),
            chunks: vec![Chunk {
                text: "the one chunk".to_string(),
                start_line: 1,
                end_line: 1,
                embedding: Embedding(vec![1.0, 0.0]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        // Two query embeddings, both matching the single stored chunk.
        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![1.0, 0.0]), Embedding(vec![1.0, 0.0])],
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

        assert_eq!(
            results.len(),
            1,
            "the same chunk matched by two query embeddings must be deduplicated, got {:?}",
            results.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
        assert_eq!(results[0].path, "alice/agent1/only.md");
    }

    #[tokio::test]
    async fn search_tolerates_degenerate_zero_vector() {
        // A stored zero vector has undefined cosine distance (NaN); the score sort must not panic.
        let index = test_index();
        let (addr, _handle) = index.start("index-test").unwrap();

        addr.send(EnsureVecReady { dim: 2 })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        for (name, emb) in [("good", vec![1.0, 0.0]), ("zero", vec![0.0, 0.0])] {
            addr.send(IndexInsert {
                path: format!("alice/agent1/{name}.md"),
                source: "raw".to_string(),
                size: 10,
                model: "mock".to_string(),
                chunks: vec![Chunk {
                    text: name.to_string(),
                    start_line: 1,
                    end_line: 1,
                    embedding: Embedding(emb),
                }],
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        }

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![1.0, 0.0])],
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

        // Must return without panicking; the well-defined match is present.
        assert!(results.iter().any(|r| r.path.ends_with("good.md")));
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
                embedding: Embedding(vec![1.0; 128]),
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
                embeddings: vec![Embedding(vec![1.0; 128])],
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
                embedding: Embedding(vec![1.0; 128]),
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
                embedding: Embedding(vec![1.0; 128]),
            }],
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let results = addr
            .send(IndexSearch {
                embeddings: vec![Embedding(vec![1.0; 128])],
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
                    embedding: Embedding(vec![1.0; 128]),
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
            embeddings: vec![Embedding(vec![1.0; 128])],
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

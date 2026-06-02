//! Storage memory files as markdown files on disk.

use std::path::PathBuf;

use acktor::{Actor, Context, message::Handler, utils::debug_trace};
use tokio::fs;

use super::error::StorageError;
use super::message::{StorageDelete, StorageRead, StorageWrite};

/// The Storage actor. Owns the root directory path.
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    /// Constructs a new `Storage` actor.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn resolve(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

impl Actor for Storage {
    type Context = Context<Self>;
    type Error = StorageError;
}

impl Handler<StorageWrite> for Storage {
    type Result = Result<(), StorageError>;

    async fn handle(
        &mut self,
        msg: StorageWrite,
        _ctx: &mut Self::Context,
    ) -> Result<(), StorageError> {
        debug_trace!("Handle command {:?}", msg);

        let path = self.resolve(&msg.path);
        let tmp_path = path.with_extension("tmp");

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Io {
                    path: path.clone(),
                    source: e,
                })?;
        }

        fs::write(&tmp_path, &msg.content)
            .await
            .map_err(|e| StorageError::Io {
                path: tmp_path.clone(),
                source: e,
            })?;

        fs::rename(&tmp_path, &path)
            .await
            .map_err(|e| StorageError::Io {
                path: path.clone(),
                source: e,
            })?;

        Ok(())
    }
}

impl Handler<StorageRead> for Storage {
    type Result = Result<Option<String>, StorageError>;

    async fn handle(
        &mut self,
        msg: StorageRead,
        _ctx: &mut Self::Context,
    ) -> Result<Option<String>, StorageError> {
        debug_trace!("Handle command {:?}", msg);

        let path = self.resolve(&msg.path);
        match fs::read_to_string(&path).await {
            Ok(content) => Ok(Some(content)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StorageError::Io { path, source: e }),
        }
    }
}

impl Handler<StorageDelete> for Storage {
    type Result = Result<(), StorageError>;

    async fn handle(
        &mut self,
        msg: StorageDelete,
        _ctx: &mut Self::Context,
    ) -> Result<(), StorageError> {
        debug_trace!("Handle command {:?}", msg);

        let path = self.resolve(&msg.path);
        match fs::remove_file(&path).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(StorageError::Io { path, source: e })
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::message::{StorageDelete, StorageRead, StorageWrite};

    #[tokio::test]
    async fn write_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.start("storage-test").unwrap();

        addr.send(StorageWrite {
            path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            content: "Hello world".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let content = addr
            .send(StorageRead {
                path: "alice/agent1/daily_note/2026-03-31.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(content, Some("Hello world".to_string()));
    }

    #[tokio::test]
    async fn read_nonexistent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.start("storage-test").unwrap();

        let result = addr
            .send(StorageRead {
                path: "nonexistent.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn delete_then_read_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.start("storage-test").unwrap();

        addr.send(StorageWrite {
            path: "to_delete.md".to_string(),
            content: "bye".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(StorageDelete {
            path: "to_delete.md".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let result = addr
            .send(StorageRead {
                path: "to_delete.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.start("storage-test").unwrap();

        let result = addr
            .send(StorageDelete {
                path: "never_existed.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn overwrite_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path().to_path_buf());
        let (addr, _handle) = store.start("storage-test").unwrap();

        addr.send(StorageWrite {
            path: "overwrite.md".to_string(),
            content: "version 1".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        addr.send(StorageWrite {
            path: "overwrite.md".to_string(),
            content: "version 2".to_string(),
        })
        .await
        .unwrap()
        .await
        .unwrap()
        .unwrap();

        let content = addr
            .send(StorageRead {
                path: "overwrite.md".to_string(),
            })
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(content, Some("version 2".to_string()));
    }
}

use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use mandatum_core::{PersistenceError, Workspace};

pub(crate) const MAX_WORKSPACE_FILE_BYTES: u64 = 1024 * 1024;
static WORKSPACE_FILE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistenceCoordinator {
    workspace_file: PathBuf,
}

impl PersistenceCoordinator {
    pub(crate) fn new(workspace_file: PathBuf) -> Self {
        Self { workspace_file }
    }

    pub(crate) fn workspace_file(&self) -> &Path {
        &self.workspace_file
    }

    pub(crate) fn save_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceFileError> {
        write_workspace_file(&self.workspace_file, workspace)
    }

    pub(crate) fn read_workspace(&self) -> Result<Workspace, WorkspaceFileError> {
        read_workspace_file(&self.workspace_file)
    }
}

#[derive(Debug)]
pub(crate) enum WorkspaceFileError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    UnsafePath {
        path: PathBuf,
        message: String,
    },
    Persistence {
        path: PathBuf,
        source: PersistenceError,
    },
}

impl fmt::Display for WorkspaceFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::UnsafePath { path, message } => {
                write!(formatter, "{}: {message}", path.display())
            }
            Self::Persistence { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for WorkspaceFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::UnsafePath { .. } => None,
            Self::Persistence { source, .. } => Some(source),
        }
    }
}

pub(crate) fn write_workspace_file(
    path: &Path,
    workspace: &Workspace,
) -> Result<(), WorkspaceFileError> {
    let json = workspace
        .to_json()
        .map_err(|source| WorkspaceFileError::Persistence {
            path: path.to_path_buf(),
            source,
        })?;
    ensure_parent_dir(path)?;
    reject_unsafe_existing_file(path)?;
    let temp_path = workspace_temp_path(path)?;
    let write_result = write_workspace_file_atomically(path, &temp_path, json.as_bytes());
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn write_workspace_file_atomically(
    path: &Path,
    temp_path: &Path,
    contents: &[u8],
) -> Result<(), WorkspaceFileError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|source| WorkspaceFileError::Io {
            path: temp_path.to_path_buf(),
            source,
        })?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|source| WorkspaceFileError::Io {
            path: temp_path.to_path_buf(),
            source,
        })?;
    drop(file);

    fs::rename(temp_path, path).map_err(|source| WorkspaceFileError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn read_workspace_file(path: &Path) -> Result<Workspace, WorkspaceFileError> {
    let metadata = safe_workspace_file_metadata(path)?;
    if metadata.len() > MAX_WORKSPACE_FILE_BYTES {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: format!(
                "workspace file is too large: {} byte(s), max {MAX_WORKSPACE_FILE_BYTES}",
                metadata.len()
            ),
        });
    }

    let mut file = fs::File::open(path).map_err(|source| WorkspaceFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut json = String::new();
    let mut limited = (&mut file).take(MAX_WORKSPACE_FILE_BYTES + 1);
    limited
        .read_to_string(&mut json)
        .map_err(|source| WorkspaceFileError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if json.len() as u64 > MAX_WORKSPACE_FILE_BYTES {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: format!(
                "workspace file is too large: more than {MAX_WORKSPACE_FILE_BYTES} byte(s)"
            ),
        });
    }
    Workspace::from_json(&json).map_err(|source| WorkspaceFileError::Persistence {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<(), WorkspaceFileError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };

    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(WorkspaceFileError::UnsafePath {
            path: parent.to_path_buf(),
            message: "workspace directory must not be a symlink".to_owned(),
        }),
        Ok(metadata) if !metadata.is_dir() => Err(WorkspaceFileError::UnsafePath {
            path: parent.to_path_buf(),
            message: "workspace parent path is not a directory".to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(parent)
            .map_err(|source| WorkspaceFileError::Io {
                path: parent.to_path_buf(),
                source,
            }),
        Err(source) => Err(WorkspaceFileError::Io {
            path: parent.to_path_buf(),
            source,
        }),
    }
}

fn reject_unsafe_existing_file(path: &Path) -> Result<(), WorkspaceFileError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace file must not be a symlink".to_owned(),
        }),
        Ok(metadata) if !metadata.is_file() => Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path is not a regular file".to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(WorkspaceFileError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn safe_workspace_file_metadata(path: &Path) -> Result<fs::Metadata, WorkspaceFileError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| WorkspaceFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace file must not be a symlink".to_owned(),
        });
    }
    if !metadata.is_file() {
        return Err(WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path is not a regular file".to_owned(),
        });
    }
    Ok(metadata)
}

fn workspace_temp_path(path: &Path) -> Result<PathBuf, WorkspaceFileError> {
    let parent = path
        .parent()
        .ok_or_else(|| WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path has no parent directory".to_owned(),
        })?;
    let file_name = path
        .file_name()
        .ok_or_else(|| WorkspaceFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "workspace path has no file name".to_owned(),
        })?;
    let file_name = file_name.to_string_lossy();
    let counter = WORKSPACE_FILE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id())))
}

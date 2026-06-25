use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Workspace, WorkspaceError};

pub const SESSION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedWorkspace {
    pub schema_version: u32,
    pub workspace: Workspace,
}

pub fn serialize_workspace(workspace: &Workspace) -> Result<String, PersistenceError> {
    workspace.validate()?;
    serde_json::to_string_pretty(&PersistedWorkspace {
        schema_version: SESSION_SCHEMA_VERSION,
        workspace: workspace.clone(),
    })
    .map_err(|error| PersistenceError::InvalidSession {
        message: error.to_string(),
    })
}

pub fn deserialize_workspace(input: &str) -> Result<Workspace, PersistenceError> {
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|error| PersistenceError::CorruptJson {
            message: error.to_string(),
        })?;

    let found = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| PersistenceError::InvalidSession {
            message: "missing numeric schema_version".to_owned(),
        })? as u32;

    if found != SESSION_SCHEMA_VERSION {
        return Err(PersistenceError::UnsupportedSchema {
            found,
            supported: SESSION_SCHEMA_VERSION,
        });
    }

    let persisted: PersistedWorkspace =
        serde_json::from_value(value).map_err(|error| PersistenceError::InvalidSession {
            message: error.to_string(),
        })?;
    persisted.workspace.validate()?;
    Ok(persisted.workspace)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistenceError {
    CorruptJson { message: String },
    UnsupportedSchema { found: u32, supported: u32 },
    InvalidSession { message: String },
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CorruptJson { message } => write!(formatter, "corrupt session JSON: {message}"),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "unsupported session schema {found}; supported schema is {supported}"
            ),
            Self::InvalidSession { message } => {
                write!(formatter, "invalid persisted session: {message}")
            }
        }
    }
}

impl std::error::Error for PersistenceError {}

impl From<WorkspaceError> for PersistenceError {
    fn from(error: WorkspaceError) -> Self {
        Self::InvalidSession {
            message: error.to_string(),
        }
    }
}

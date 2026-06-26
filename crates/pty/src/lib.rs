//! PTY boundary types and native process sessions.
//!
//! This crate defines process/session intent, byte-stream backpressure behavior,
//! and a native OS PTY wrapper without depending on parser, renderer, app, or
//! core crates.

use std::{
    collections::VecDeque,
    fmt,
    io::{Read, Write},
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PtySessionId(String);

impl PtySessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PtySessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for PtySessionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for PtySessionId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChildProcessId(u32);

impl ChildProcessId {
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn get(&self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    columns: u16,
    rows: u16,
}

impl PtySize {
    pub fn new(columns: u16, rows: u16) -> Result<Self, PtySizeError> {
        if columns == 0 || rows == 0 {
            return Err(PtySizeError { columns, rows });
        }

        Ok(Self { columns, rows })
    }

    pub fn columns(&self) -> u16 {
        self.columns
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySizeError {
    pub columns: u16,
    pub rows: u16,
}

impl fmt::Display for PtySizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "PTY size must be non-zero, got {}x{}",
            self.columns, self.rows
        )
    }
}

impl std::error::Error for PtySizeError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnIntent {
    session_id: PtySessionId,
    program: String,
    arguments: Vec<String>,
    cwd: Option<PathBuf>,
    environment: Vec<(String, String)>,
    size: PtySize,
}

impl SpawnIntent {
    pub fn new(
        session_id: PtySessionId,
        program: impl Into<String>,
        size: PtySize,
    ) -> Result<Self, SpawnIntentError> {
        let program = program.into();
        if program.trim().is_empty() {
            return Err(SpawnIntentError::EmptyProgram);
        }

        Ok(Self {
            session_id,
            program,
            arguments: Vec::new(),
            cwd: None,
            environment: Vec::new(),
            size,
        })
    }

    pub fn with_arguments(
        mut self,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.arguments = arguments.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_environment(
        mut self,
        environment: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.environment = environment
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    pub fn cwd(&self) -> Option<&PathBuf> {
        self.cwd.as_ref()
    }

    pub fn environment(&self) -> &[(String, String)] {
        &self.environment
    }

    pub fn size(&self) -> PtySize {
        self.size
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnIntentError {
    EmptyProgram,
}

impl fmt::Display for SpawnIntentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyProgram => formatter.write_str("spawn intent requires a program"),
        }
    }
}

impl std::error::Error for SpawnIntentError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResizeIntent {
    session_id: PtySessionId,
    size: PtySize,
}

impl ResizeIntent {
    pub fn new(session_id: PtySessionId, size: PtySize) -> Self {
        Self { session_id, size }
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn size(&self) -> PtySize {
        self.size
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartIntent {
    session_id: PtySessionId,
    reason: RestartReason,
}

impl RestartIntent {
    pub fn new(session_id: PtySessionId, reason: RestartReason) -> Self {
        Self { session_id, reason }
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn reason(&self) -> RestartReason {
        self.reason
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartReason {
    UserRequested,
    ChildExited,
    SpawnFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyEvent {
    Output(ByteStreamEvent),
    ChildExited(ChildExit),
    BackpressureChanged(BackpressureEvent),
}

pub struct NativePtySession {
    session_id: PtySessionId,
    process_id: Option<ChildProcessId>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Option<Box<dyn Write + Send>>,
}

impl NativePtySession {
    pub fn spawn(intent: SpawnIntent) -> Result<Self, NativePtyError> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(to_native_size(intent.size()))
            .map_err(|error| NativePtyError::OpenFailed {
                message: error.to_string(),
            })?;

        let mut command = portable_pty::CommandBuilder::new(intent.program());
        for argument in intent.arguments() {
            command.arg(argument);
        }
        if let Some(cwd) = intent.cwd() {
            command.cwd(cwd.as_os_str());
        }
        for (key, value) in intent.environment() {
            command.env(key, value);
        }

        let mut child =
            pair.slave
                .spawn_command(command)
                .map_err(|error| NativePtyError::SpawnFailed {
                    session_id: intent.session_id().clone(),
                    message: error.to_string(),
                })?;
        let process_id = child.process_id().map(ChildProcessId::new);

        drop(pair.slave);

        let reader =
            pair.master
                .try_clone_reader()
                .map_err(|error| NativePtyError::ReaderCloneFailed {
                    session_id: intent.session_id().clone(),
                    message: error.to_string(),
                });
        let reader = match reader {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };

        let writer = pair
            .master
            .take_writer()
            .map_err(|error| NativePtyError::WriterTakeFailed {
                session_id: intent.session_id().clone(),
                message: error.to_string(),
            });
        let writer = match writer {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };

        Ok(Self {
            session_id: intent.session_id().clone(),
            process_id,
            master: pair.master,
            child,
            reader,
            writer: Some(writer),
        })
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn process_id(&self) -> Option<ChildProcessId> {
        self.process_id
    }

    pub fn read_output(
        &mut self,
        max_bytes: usize,
    ) -> Result<Option<ByteStreamEvent>, NativePtyError> {
        if max_bytes == 0 {
            return Ok(None);
        }

        let mut bytes = vec![0; max_bytes];
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) => return Ok(None),
                Ok(read_bytes) => {
                    bytes.truncate(read_bytes);
                    return Ok(Some(ByteStreamEvent::output(
                        self.session_id.clone(),
                        bytes,
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(NativePtyError::ReadFailed {
                        session_id: self.session_id.clone(),
                        message: error.to_string(),
                    });
                }
            }
        }
    }

    pub fn read_event(&mut self, max_bytes: usize) -> Result<Option<PtyEvent>, NativePtyError> {
        Ok(self.read_output(max_bytes)?.map(PtyEvent::Output))
    }

    pub fn write_input(&mut self, bytes: &[u8]) -> Result<(), NativePtyError> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(NativePtyError::InputClosed {
                session_id: self.session_id.clone(),
            });
        };

        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| NativePtyError::WriteFailed {
                session_id: self.session_id.clone(),
                message: error.to_string(),
            })
    }

    pub fn close_input(&mut self) {
        self.writer.take();
    }

    pub fn resize(&self, intent: ResizeIntent) -> Result<(), NativePtyError> {
        if intent.session_id() != &self.session_id {
            return Err(NativePtyError::SessionMismatch {
                expected: self.session_id.clone(),
                actual: intent.session_id().clone(),
            });
        }

        self.master
            .resize(to_native_size(intent.size()))
            .map_err(|error| NativePtyError::ResizeFailed {
                session_id: self.session_id.clone(),
                message: error.to_string(),
            })
    }

    pub fn current_size(&self) -> Result<PtySize, NativePtyError> {
        let native_size =
            self.master
                .get_size()
                .map_err(|error| NativePtyError::SizeReadFailed {
                    session_id: self.session_id.clone(),
                    message: error.to_string(),
                })?;

        PtySize::new(native_size.cols, native_size.rows).map_err(|error| {
            NativePtyError::SizeReadFailed {
                session_id: self.session_id.clone(),
                message: error.to_string(),
            }
        })
    }

    pub fn try_wait(&mut self) -> Result<Option<ChildExit>, NativePtyError> {
        self.child
            .try_wait()
            .map(|status| {
                status.map(|status| {
                    ChildExit::new(
                        self.session_id.clone(),
                        self.process_id,
                        child_exit_status(status),
                    )
                })
            })
            .map_err(|error| NativePtyError::WaitFailed {
                session_id: self.session_id.clone(),
                message: error.to_string(),
            })
    }

    pub fn try_wait_event(&mut self) -> Result<Option<PtyEvent>, NativePtyError> {
        Ok(self.try_wait()?.map(PtyEvent::ChildExited))
    }

    pub fn wait(&mut self) -> Result<ChildExit, NativePtyError> {
        self.child
            .wait()
            .map(|status| {
                ChildExit::new(
                    self.session_id.clone(),
                    self.process_id,
                    child_exit_status(status),
                )
            })
            .map_err(|error| NativePtyError::WaitFailed {
                session_id: self.session_id.clone(),
                message: error.to_string(),
            })
    }

    pub fn wait_event(&mut self) -> Result<PtyEvent, NativePtyError> {
        Ok(PtyEvent::ChildExited(self.wait()?))
    }

    pub fn kill(&mut self) -> Result<(), NativePtyError> {
        self.child
            .kill()
            .map_err(|error| NativePtyError::KillFailed {
                session_id: self.session_id.clone(),
                message: error.to_string(),
            })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativePtyError {
    OpenFailed {
        message: String,
    },
    SpawnFailed {
        session_id: PtySessionId,
        message: String,
    },
    ReaderCloneFailed {
        session_id: PtySessionId,
        message: String,
    },
    WriterTakeFailed {
        session_id: PtySessionId,
        message: String,
    },
    ReadFailed {
        session_id: PtySessionId,
        message: String,
    },
    WriteFailed {
        session_id: PtySessionId,
        message: String,
    },
    InputClosed {
        session_id: PtySessionId,
    },
    ResizeFailed {
        session_id: PtySessionId,
        message: String,
    },
    SizeReadFailed {
        session_id: PtySessionId,
        message: String,
    },
    WaitFailed {
        session_id: PtySessionId,
        message: String,
    },
    KillFailed {
        session_id: PtySessionId,
        message: String,
    },
    SessionMismatch {
        expected: PtySessionId,
        actual: PtySessionId,
    },
}

impl fmt::Display for NativePtyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenFailed { message } => write!(formatter, "failed to open PTY: {message}"),
            Self::SpawnFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to spawn child for PTY session {session_id}: {message}"
            ),
            Self::ReaderCloneFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to clone reader for PTY session {session_id}: {message}"
            ),
            Self::WriterTakeFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to take writer for PTY session {session_id}: {message}"
            ),
            Self::ReadFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to read output for PTY session {session_id}: {message}"
            ),
            Self::WriteFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to write input for PTY session {session_id}: {message}"
            ),
            Self::InputClosed { session_id } => {
                write!(formatter, "input is closed for PTY session {session_id}")
            }
            Self::ResizeFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to resize PTY session {session_id}: {message}"
            ),
            Self::SizeReadFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to read size for PTY session {session_id}: {message}"
            ),
            Self::WaitFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to wait for PTY session {session_id}: {message}"
            ),
            Self::KillFailed {
                session_id,
                message,
            } => write!(
                formatter,
                "failed to kill child for PTY session {session_id}: {message}"
            ),
            Self::SessionMismatch { expected, actual } => write!(
                formatter,
                "PTY session mismatch: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for NativePtyError {}

fn to_native_size(size: PtySize) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: size.rows(),
        cols: size.columns(),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn child_exit_status(status: portable_pty::ExitStatus) -> ChildExitStatus {
    if status.signal().is_some() {
        ChildExitStatus::Unknown
    } else {
        ChildExitStatus::Exited {
            code: i32::try_from(status.exit_code()).unwrap_or(i32::MAX),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteStreamEvent {
    session_id: PtySessionId,
    bytes: Vec<u8>,
}

impl ByteStreamEvent {
    pub fn output(session_id: PtySessionId, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            session_id,
            bytes: bytes.into(),
        }
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildExit {
    session_id: PtySessionId,
    process_id: Option<ChildProcessId>,
    status: ChildExitStatus,
}

impl ChildExit {
    pub fn new(
        session_id: PtySessionId,
        process_id: Option<ChildProcessId>,
        status: ChildExitStatus,
    ) -> Self {
        Self {
            session_id,
            process_id,
            status,
        }
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn process_id(&self) -> Option<ChildProcessId> {
        self.process_id
    }

    pub fn status(&self) -> ChildExitStatus {
        self.status
    }

    pub fn succeeded(&self) -> bool {
        self.status == ChildExitStatus::Exited { code: 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildExitStatus {
    Exited { code: i32 },
    Signaled { signal: i32 },
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackpressureEvent {
    session_id: PtySessionId,
    state: BackpressureState,
}

impl BackpressureEvent {
    pub fn new(session_id: PtySessionId, state: BackpressureState) -> Self {
        Self { session_id, state }
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn state(&self) -> BackpressureState {
        self.state
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackpressureState {
    queued_bytes: usize,
    capacity_bytes: usize,
}

impl BackpressureState {
    pub fn new(queued_bytes: usize, capacity_bytes: usize) -> Result<Self, BackpressureStateError> {
        if capacity_bytes == 0 {
            return Err(BackpressureStateError::ZeroCapacity);
        }

        if queued_bytes > capacity_bytes {
            return Err(BackpressureStateError::QueuedExceedsCapacity {
                queued_bytes,
                capacity_bytes,
            });
        }

        Ok(Self {
            queued_bytes,
            capacity_bytes,
        })
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    pub fn remaining_bytes(&self) -> usize {
        self.capacity_bytes - self.queued_bytes
    }

    pub fn is_full(&self) -> bool {
        self.queued_bytes == self.capacity_bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackpressureStateError {
    ZeroCapacity,
    QueuedExceedsCapacity {
        queued_bytes: usize,
        capacity_bytes: usize,
    },
}

impl fmt::Display for BackpressureStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("backpressure capacity must be non-zero"),
            Self::QueuedExceedsCapacity {
                queued_bytes,
                capacity_bytes,
            } => write!(
                formatter,
                "queued byte count {queued_bytes} exceeds capacity {capacity_bytes}"
            ),
        }
    }
}

impl std::error::Error for BackpressureStateError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferWrite {
    accepted_bytes: usize,
    rejected_bytes: usize,
    state: BackpressureState,
}

impl BufferWrite {
    fn new(accepted_bytes: usize, rejected_bytes: usize, state: BackpressureState) -> Self {
        Self {
            accepted_bytes,
            rejected_bytes,
            state,
        }
    }

    pub fn accepted_bytes(&self) -> usize {
        self.accepted_bytes
    }

    pub fn rejected_bytes(&self) -> usize {
        self.rejected_bytes
    }

    pub fn state(&self) -> BackpressureState {
        self.state
    }

    pub fn fully_accepted(&self) -> bool {
        self.rejected_bytes == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedByteBuffer {
    bytes: VecDeque<u8>,
    capacity_bytes: usize,
}

impl BoundedByteBuffer {
    pub fn new(capacity_bytes: usize) -> Result<Self, BackpressureStateError> {
        BackpressureState::new(0, capacity_bytes)?;

        Ok(Self {
            bytes: VecDeque::new(),
            capacity_bytes,
        })
    }

    pub fn push(&mut self, bytes: &[u8]) -> BufferWrite {
        let available = self.capacity_bytes - self.bytes.len();
        let accepted_bytes = bytes.len().min(available);
        self.bytes.extend(bytes[..accepted_bytes].iter().copied());

        BufferWrite::new(
            accepted_bytes,
            bytes.len() - accepted_bytes,
            self.backpressure(),
        )
    }

    pub fn drain(&mut self, max_bytes: usize) -> Vec<u8> {
        let drained_bytes = max_bytes.min(self.bytes.len());
        self.bytes.drain(..drained_bytes).collect()
    }

    pub fn backpressure(&self) -> BackpressureState {
        BackpressureState {
            queued_bytes: self.bytes.len(),
            capacity_bytes: self.capacity_bytes,
        }
    }

    pub fn queued_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

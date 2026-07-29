//! PTY boundary types and native process sessions.
//!
//! This crate defines process/session intent and a native OS PTY wrapper
//! without depending on parser, renderer, app, or core crates.

use std::{
    collections::VecDeque,
    fmt,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
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
        // portable-pty silently substitutes `$HOME` for any cwd that is not
        // an existing directory; reject it here so a stale pane cwd (e.g. a
        // renamed project restored from durable state) fails loudly instead
        // of quietly rerooting the child in the wrong directory.
        if let Some(cwd) = intent.cwd()
            && !cwd.is_dir()
        {
            return Err(NativePtyError::CwdNotFound {
                session_id: intent.session_id().clone(),
                cwd: cwd.clone(),
            });
        }

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

    pub fn into_split(mut self) -> Result<NativePtyParts, NativePtyError> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| NativePtyError::InputClosed {
                session_id: self.session_id.clone(),
            })?;

        Ok(NativePtyParts {
            controller: NativePtyController {
                session_id: self.session_id.clone(),
                process_id: self.process_id,
                master: self.master,
                child: self.child,
            },
            reader: NativePtyReader {
                session_id: self.session_id.clone(),
                reader: self.reader,
                scratch: Vec::new(),
            },
            writer: NativePtyWriter::spawn(self.session_id, writer),
        })
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

pub struct NativePtyParts {
    pub controller: NativePtyController,
    pub reader: NativePtyReader,
    pub writer: NativePtyWriter,
}

pub struct NativePtyReader {
    session_id: PtySessionId,
    reader: Box<dyn Read + Send>,
    /// Reused across reads so each chunk costs one exact-sized copy of the
    /// bytes actually read instead of a fresh zeroed allocation per read.
    scratch: Vec<u8>,
}

impl NativePtyReader {
    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn read_output(
        &mut self,
        max_bytes: usize,
    ) -> Result<Option<ByteStreamEvent>, NativePtyError> {
        if max_bytes == 0 {
            return Ok(None);
        }

        if self.scratch.len() < max_bytes {
            self.scratch.resize(max_bytes, 0);
        }
        loop {
            match self.reader.read(&mut self.scratch[..max_bytes]) {
                Ok(0) => return Ok(None),
                Ok(read_bytes) => {
                    return Ok(Some(ByteStreamEvent::output(
                        self.session_id.clone(),
                        self.scratch[..read_bytes].to_vec(),
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
}

/// Most bytes `NativePtyWriter::write_input` may hold queued for its writer
/// thread. Interactive input is tiny; only a huge paste into a child that is
/// not reading can approach this, and such a write fails with
/// [`NativePtyError::WriteQueueFull`] instead of blocking the caller.
pub const WRITE_QUEUE_CAPACITY_BYTES: usize = 4 * 1024 * 1024;

/// Shared handoff between `write_input` and the writer thread. `write_input`
/// only locks, pushes, and notifies, so the caller never blocks on a full
/// kernel tty buffer; the thread alone performs the blocking writes.
struct WriteQueue {
    state: Mutex<WriteQueueState>,
    changed: Condvar,
}

struct WriteQueueState {
    chunks: VecDeque<Vec<u8>>,
    queued_bytes: usize,
    closed: bool,
    /// First write failure observed by the writer thread; surfaced to the
    /// caller on the next `write_input`.
    error: Option<String>,
}

/// The writer thread: drain queued chunks into the PTY, blocking in the OS
/// where the caller no longer can. Exits once the queue is closed and fully
/// drained, or on the first write failure.
fn run_pty_writer(queue: &Arc<WriteQueue>, mut writer: Box<dyn Write + Send>) {
    loop {
        let chunk = {
            let mut state = queue.state.lock().expect("PTY write queue lock");
            loop {
                if let Some(chunk) = state.chunks.pop_front() {
                    state.queued_bytes = state.queued_bytes.saturating_sub(chunk.len());
                    break chunk;
                }
                if state.closed {
                    return;
                }
                state = queue.changed.wait(state).expect("PTY write queue lock");
            }
        };

        if let Err(error) = writer.write_all(&chunk).and_then(|()| writer.flush()) {
            let mut state = queue.state.lock().expect("PTY write queue lock");
            state.error = Some(error.to_string());
            state.chunks.clear();
            state.queued_bytes = 0;
            return;
        }
    }
}

pub struct NativePtyWriter {
    session_id: PtySessionId,
    queue: Arc<WriteQueue>,
}

impl NativePtyWriter {
    /// Wrap the blocking PTY writer in a dedicated writer thread so
    /// `write_input` can enqueue and return immediately. The thread is
    /// deliberately detached: it exits on its own once the queue closes and
    /// drains (or a write fails after the master closes), and joining it here
    /// could park the caller behind a child that never reads.
    fn spawn(session_id: PtySessionId, writer: Box<dyn Write + Send>) -> Self {
        let queue = Arc::new(WriteQueue {
            state: Mutex::new(WriteQueueState {
                chunks: VecDeque::new(),
                queued_bytes: 0,
                closed: false,
                error: None,
            }),
            changed: Condvar::new(),
        });
        let thread_queue = Arc::clone(&queue);
        thread::spawn(move || run_pty_writer(&thread_queue, writer));

        Self { session_id, queue }
    }

    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    /// Queue `bytes` for the writer thread and return immediately. A full
    /// queue rejects the whole write with [`NativePtyError::WriteQueueFull`]
    /// rather than blocking the caller or dropping bytes mid-stream.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<(), NativePtyError> {
        let mut state = self.queue.state.lock().expect("PTY write queue lock");
        if state.closed {
            return Err(NativePtyError::InputClosed {
                session_id: self.session_id.clone(),
            });
        }
        if let Some(message) = &state.error {
            return Err(NativePtyError::WriteFailed {
                session_id: self.session_id.clone(),
                message: message.clone(),
            });
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if state.queued_bytes.saturating_add(bytes.len()) > WRITE_QUEUE_CAPACITY_BYTES {
            return Err(NativePtyError::WriteQueueFull {
                session_id: self.session_id.clone(),
                queued_bytes: state.queued_bytes,
                rejected_bytes: bytes.len(),
            });
        }

        state.queued_bytes += bytes.len();
        state.chunks.push_back(bytes.to_vec());
        self.queue.changed.notify_all();
        Ok(())
    }

    /// Refuse further writes; the writer thread drains what is already queued
    /// and then exits, closing its PTY fd.
    pub fn close_input(&mut self) {
        let mut state = self.queue.state.lock().expect("PTY write queue lock");
        state.closed = true;
        self.queue.changed.notify_all();
    }
}

impl Drop for NativePtyWriter {
    fn drop(&mut self) {
        self.close_input();
    }
}

pub struct NativePtyController {
    session_id: PtySessionId,
    process_id: Option<ChildProcessId>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl NativePtyController {
    pub fn session_id(&self) -> &PtySessionId {
        &self.session_id
    }

    pub fn process_id(&self) -> Option<ChildProcessId> {
        self.process_id
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
    CwdNotFound {
        session_id: PtySessionId,
        cwd: PathBuf,
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
    /// The writer thread's queue is full: the child has stopped reading and
    /// [`WRITE_QUEUE_CAPACITY_BYTES`] of input are already pending. The whole
    /// write is rejected — nothing was partially queued.
    WriteQueueFull {
        session_id: PtySessionId,
        queued_bytes: usize,
        rejected_bytes: usize,
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
            Self::CwdNotFound { session_id, cwd } => write!(
                formatter,
                "cannot spawn PTY session {session_id}: working directory {} does not exist",
                cwd.display()
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
            Self::WriteQueueFull {
                session_id,
                queued_bytes,
                rejected_bytes,
            } => write!(
                formatter,
                "write queue full for PTY session {session_id}: rejected \
                 {rejected_bytes}-byte write with {queued_bytes} bytes queued \
                 (capacity {WRITE_QUEUE_CAPACITY_BYTES})"
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
    // portable-pty reports a signal only as a `strsignal` name, never a
    // number, so a signalled child cannot be mapped onto `Signaled` without
    // reaping the child ourselves.
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

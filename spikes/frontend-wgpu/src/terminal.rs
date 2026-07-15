//! Terminal session: owns the PTY runtime and VT parser from the engine crates
//! and exposes exactly the read-only view + input/resize/scroll operations the
//! GPU frontend needs. This is an adapter over `mandatum-pty` and
//! `mandatum-terminal-vt`; it copies no product behavior.

use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::thread::JoinHandle;

/// Bytes per PTY read. Kept modest so a flood streams as many small chunks
/// rather than one giant burst, giving the render loop intermediate frames.
const READ_CHUNK: usize = 8192;

/// Bounded reader->render queue depth. When full, the reader's send blocks,
/// which stops it reading, which lets the PTY buffer fill, which back-pressures
/// the shell — pacing a flood to the frontend's render rate instead of buffering
/// the whole thing instantly. This is the real terminal backpressure behavior.
const QUEUE_DEPTH: usize = 4;

/// Max PTY bytes fed into the parser per rendered frame. Without this, one
/// `pump` races the reader and consumes an entire flood in a single frame,
/// collapsing a sustained scroll into one repaint. Capping per frame paces the
/// scroll to the display: a flood renders as a stream of intermediate frames,
/// which is both what a user sees and what makes frame-time meaningful.
const MAX_BYTES_PER_FRAME: usize = 16384;

use mandatum_pty::{
    NativePtyController, NativePtySession, NativePtyWriter, PtySize, ResizeIntent, SpawnIntent,
};
use mandatum_terminal_vt::{TerminalGrid, TerminalParser, TerminalSize};

/// Result of draining pending PTY output in one pump.
#[derive(Clone, Copy, Debug)]
pub struct PumpOutcome {
    /// The visible screen changed and should be repainted.
    pub screen_changed: bool,
    /// The per-frame byte cap was hit, so a backlog remains: the caller should
    /// keep the render loop running to drain it at the display rate.
    pub more_pending: bool,
}

/// A rectangular-in-reading-order selection in absolute grid coordinates.
/// Rows are absolute indices into the combined scrollback-plus-screen buffer.
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub start_row: isize,
    pub start_col: u16,
    pub end_row: isize,
    pub end_col: u16,
}

/// A live shell session parsed into a terminal grid.
pub struct TerminalSession {
    parser: TerminalParser,
    writer: NativePtyWriter,
    controller: NativePtyController,
    rx: Receiver<Vec<u8>>,
    reader_thread: Option<JoinHandle<()>>,
    cols: u16,
    rows: u16,
    /// Rows scrolled up from the live bottom. `0` == following the newest output.
    scroll_offset: usize,
    last_total_rows: usize,
    shell_name: String,
}

impl TerminalSession {
    /// Spawn the user's shell on a PTY of `cols`x`rows` and start a background
    /// reader thread that forwards raw output and invokes `wake` after each read
    /// so the event loop can pull and repaint.
    pub fn spawn(cols: u16, rows: u16, wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let shell_name = shell.rsplit('/').next().unwrap_or(&shell).to_string();

        let size = PtySize::new(cols, rows).map_err(|e| e.to_string())?;
        let mut intent = SpawnIntent::new("frontend-wgpu-spike".into(), shell.clone(), size)
            .map_err(|e| e.to_string())?
            .with_environment([
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string()),
            ]);
        if let Some(home) = std::env::var_os("HOME") {
            intent = intent.with_cwd(home);
        }

        let session = NativePtySession::spawn(intent).map_err(|e| e.to_string())?;
        let parts = session.into_split().map_err(|e| e.to_string())?;
        let mut reader = parts.reader;
        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_DEPTH);

        let reader_thread = std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                loop {
                    match reader.read_output(READ_CHUNK) {
                        Ok(Some(event)) => {
                            if tx.send(event.into_bytes()).is_err() {
                                break;
                            }
                            wake();
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        let parser = TerminalParser::new(TerminalSize::new(cols, rows).map_err(|e| e.to_string())?);

        Ok(Self {
            parser,
            writer: parts.writer,
            controller: parts.controller,
            rx,
            reader_thread: Some(reader_thread),
            cols,
            rows,
            scroll_offset: 0,
            last_total_rows: usize::from(rows),
            shell_name,
        })
    }

    /// Drain any pending PTY output into the parser. Reports whether bytes were
    /// consumed (so the caller can keep the vsync render loop alive while output
    /// streams) and whether the visible screen changed (so it can repaint).
    pub fn pump(&mut self) -> PumpOutcome {
        let mut screen_changed = false;
        let mut consumed = 0usize;
        let mut capped = false;
        loop {
            if consumed >= MAX_BYTES_PER_FRAME {
                capped = true;
                break;
            }
            match self.rx.try_recv() {
                Ok(bytes) => {
                    consumed += bytes.len();
                    if let Ok(update) = self.parser.feed_pty_bytes(&bytes) {
                        screen_changed |= update.screen_changed;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        // Keep a scrolled-up viewport anchored to the same history rows as new
        // output pushes lines into scrollback.
        let total = self.parser.grid().total_rows();
        if self.scroll_offset > 0 && total > self.last_total_rows {
            let grew = total - self.last_total_rows;
            let max_offset = self.parser.grid().scrollback_len();
            self.scroll_offset = (self.scroll_offset + grew).min(max_offset);
        }
        self.last_total_rows = total;

        PumpOutcome {
            screen_changed,
            more_pending: capped,
        }
    }

    /// Feed bytes to the shell's stdin. Any input snaps the viewport back to the
    /// live bottom, matching terminal convention.
    pub fn write_input(&mut self, bytes: &[u8]) {
        self.scroll_offset = 0;
        let _ = self.writer.write_input(bytes);
    }

    /// Resize the PTY and the parser grid together.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        if let (Ok(pty_size), Ok(term_size)) =
            (PtySize::new(cols, rows), TerminalSize::new(cols, rows))
        {
            let _ = self
                .controller
                .resize(ResizeIntent::new("frontend-wgpu-spike".into(), pty_size));
            self.parser.resize(term_size);
            self.cols = cols;
            self.rows = rows;
        }
    }

    /// Scroll by `delta` rows: positive scrolls up into history.
    pub fn scroll(&mut self, delta: isize) {
        let max_offset = self.parser.grid().scrollback_len() as isize;
        let next = (self.scroll_offset as isize + delta).clamp(0, max_offset);
        self.scroll_offset = next as usize;
    }

    pub fn grid(&self) -> &TerminalGrid {
        self.parser.grid()
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn shell_name(&self) -> &str {
        &self.shell_name
    }

    /// Absolute row index shown on the top visible line, given the current
    /// scroll offset. May be negative when history is shorter than the screen.
    pub fn top_absolute_row(&self) -> isize {
        let total = self.grid().total_rows() as isize;
        let bottom = total - 1 - self.scroll_offset as isize;
        bottom - (self.rows as isize - 1)
    }

    /// Extract text between two absolute (row, column) points, inclusive of the
    /// start column and exclusive of the end column, trimming trailing blanks
    /// per line. Used for mouse-selection copy.
    pub fn text_in_range(
        &self,
        start_row: isize,
        start_col: u16,
        end_row: isize,
        end_col: u16,
    ) -> String {
        let ((r0, c0), (r1, c1)) = if (start_row, start_col) <= (end_row, end_col) {
            ((start_row, start_col), (end_row, end_col))
        } else {
            ((end_row, end_col), (start_row, start_col))
        };

        let grid = self.grid();
        let mut out = String::new();
        for row in r0..=r1 {
            if row < 0 {
                out.push('\n');
                continue;
            }
            let from = if row == r0 { c0 } else { 0 };
            // Inclusive end column, matching the scene contract's inclusive
            // selection span so copied text agrees with the highlight.
            let to = if row == r1 {
                c1.saturating_add(1)
            } else {
                self.cols
            };
            let mut line = String::new();
            for col in from..to.min(self.cols) {
                let ch = grid
                    .history_cell(row as usize, col)
                    .map(|cell| cell.character())
                    .unwrap_or(' ');
                line.push(ch);
            }
            out.push_str(line.trim_end());
            if row != r1 {
                out.push('\n');
            }
        }
        out
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Killing the child EOFs the PTY master, which unblocks the reader's
        // blocking read. We detach rather than join: the thread is parked in a
        // blocking read and joining could stall process exit if EOF is delayed.
        let _ = self.controller.kill();
        drop(self.reader_thread.take());
    }
}

//! External latency probe for the product's ratatui/crossterm frontend.
//!
//! This does NOT edit or link the product. It spawns the real `mandatum`
//! binary inside a PTY at a fixed size, waits for its first render, then for a
//! run of iterations types one character into the app's PTY and times how long
//! until the echoed character appears in the app's output stream.
//!
//! What this measures (and what it does not) is asymmetric with the GPU spike,
//! by construction:
//!   - TUI number  = key -> bytes emitted by the app (the app's ratatui diff
//!     paints the echoed cell into its output). The HOST TERMINAL'S paint of
//!     those bytes is NOT included. The app's run loop is event-driven (a
//!     dedicated input thread wakes the main loop; ~8 ms redraw cap), so the
//!     number includes the redraw-cap coalescing window but no poll interval.
//!   - GPU number  = key -> GPU present, which DOES include the on-screen paint.
//! So the TUI figure is if anything flattered (it stops at bytes-out, before
//! pixels), yet still carries the poll-loop cost. The comparison table in
//! RESULTS.md states this caveat next to the numbers.

use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use mandatum_pty::{NativePtySession, PtySize, SpawnIntent};

const COLS: u16 = 100;
const ROWS: u16 = 30;
const TARGET_SAMPLES: usize = 100;
const MAX_ATTEMPTS: usize = 200;
const PROBE_CHAR: u8 = b'z'; // never a byte inside the app's ANSI control output
const PER_KEY_TIMEOUT: Duration = Duration::from_millis(400);

fn main() {
    let mut args = std::env::args().skip(1);
    let app_bin = args.next().unwrap_or_else(default_app_bin);

    if !std::path::Path::new(&app_bin).exists() {
        println!(
            "{{\"error\":\"app binary not found\",\"path\":{app_bin:?},\"notes\":\"build it first: cargo build -p mandatum-app --release\"}}"
        );
        std::process::exit(2);
    }

    // Fresh temp cwd so the app builds a default single-terminal workspace with
    // no stale .mandatum/workspace.json to restore.
    let cwd = std::env::temp_dir().join(format!("mandatum-tui-probe-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&cwd);

    let size = PtySize::new(COLS, ROWS).expect("valid pty size");
    let intent = SpawnIntent::new("tui-probe".into(), app_bin.clone(), size)
        .expect("valid spawn intent")
        .with_cwd(&cwd)
        .with_environment([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]);

    let session = match NativePtySession::spawn(intent) {
        Ok(session) => session,
        Err(error) => {
            println!(
                "{{\"error\":\"spawn failed\",\"detail\":{:?}}}",
                error.to_string()
            );
            std::process::exit(2);
        }
    };
    let parts = session.into_split().expect("split pty");
    let mut reader = parts.reader;
    let mut writer = parts.writer;
    let mut controller = parts.controller;

    let (tx, rx) = channel::<Vec<u8>>();
    let reader_thread = std::thread::spawn(move || {
        loop {
            match reader.read_output(65536) {
                Ok(Some(event)) => {
                    if tx.send(event.into_bytes()).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
    });

    // Let the app enter its alternate screen, spawn the shell, and render the
    // first prompt. The app repaints on runtime events and a ~250 ms heartbeat,
    // so there is no true idle to wait for; just give it a fixed initialization
    // window, draining as we go, then clear whatever is buffered.
    let startup_end = Instant::now() + Duration::from_millis(2500);
    while Instant::now() < startup_end {
        let _ = rx.recv_timeout(Duration::from_millis(100));
    }
    drain_now(&rx);

    let mut samples_ms: Vec<f64> = Vec::with_capacity(TARGET_SAMPLES);
    let mut misses = 0usize;
    let mut disconnected = false;

    for _ in 0..MAX_ATTEMPTS {
        if samples_ms.len() >= TARGET_SAMPLES {
            break;
        }

        // Clear the shell input line (Ctrl+U) so the probe char is always a new
        // cell on screen. Give the clear's render a fixed settle, then discard
        // everything buffered so the next output is the probe char's echo.
        if writer.write_input(&[0x15]).is_err() {
            disconnected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(90));
        drain_now(&rx);

        let t0 = Instant::now();
        if writer.write_input(&[PROBE_CHAR]).is_err() {
            disconnected = true;
            break;
        }

        match wait_for_char(&rx, PROBE_CHAR, t0) {
            WaitResult::Seen(delta) => samples_ms.push(delta.as_secs_f64() * 1000.0),
            WaitResult::Timeout => misses += 1,
            WaitResult::Disconnected => {
                disconnected = true;
                break;
            }
        }
        if std::env::var_os("MANDATUM_DEBUG").is_some() && (samples_ms.len() + misses) % 25 == 0 {
            eprintln!("[probe] samples={} misses={}", samples_ms.len(), misses);
        }
        // If the app never echoes, bail rather than burning the full attempt
        // budget at the per-key timeout.
        if samples_ms.is_empty() && misses >= 20 {
            break;
        }

        // Gap between iterations, then discard the probe char's render.
        std::thread::sleep(Duration::from_millis(30));
        drain_now(&rx);
    }

    // Quit the app cleanly (Ctrl+Q), then ensure the child is gone.
    let _ = writer.write_input(&[0x11]);
    std::thread::sleep(Duration::from_millis(150));
    let _ = controller.kill();
    drop(rx);
    let _ = reader_thread.join();
    let _ = std::fs::remove_dir_all(&cwd);

    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let notes = format!(
        "frontend=ratatui/crossterm binary={app_bin} grid={COLS}x{ROWS} loop=event-driven samples={} misses={} disconnected={} measures=key->app-output-bytes (host-terminal paint NOT included)",
        samples_ms.len(),
        misses,
        disconnected,
    );
    println!(
        "{{\"tui_key_to_output_ms\":{{\"p50\":{:.2},\"p95\":{:.2},\"max\":{:.2}}},\"samples\":{},\"notes\":{:?}}}",
        percentile(&samples_ms, 50.0),
        percentile(&samples_ms, 95.0),
        samples_ms.last().copied().unwrap_or(0.0),
        samples_ms.len(),
        notes,
    );
}

enum WaitResult {
    Seen(Duration),
    Timeout,
    Disconnected,
}

/// Wait until an output chunk containing `needle` arrives, or the per-key
/// timeout elapses. The app's ratatui diff only emits changed cells, so the
/// probe char appears in output exactly when the app paints its echo.
fn wait_for_char(rx: &Receiver<Vec<u8>>, needle: u8, t0: Instant) -> WaitResult {
    let deadline = t0 + PER_KEY_TIMEOUT;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return WaitResult::Timeout;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(chunk) => {
                if chunk.contains(&needle) {
                    return WaitResult::Seen(Instant::now() - t0);
                }
            }
            Err(RecvTimeoutError::Timeout) => return WaitResult::Timeout,
            Err(RecvTimeoutError::Disconnected) => return WaitResult::Disconnected,
        }
    }
}

/// Drain all currently-buffered output without blocking. Used instead of an
/// idle wait because the app also repaints on a heartbeat, so there is never
/// a guaranteed silent gap to wait on.
fn drain_now(rx: &Receiver<Vec<u8>>) {
    while rx.try_recv().is_ok() {}
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn default_app_bin() -> String {
    // The workspace target dir (the spike is excluded and builds elsewhere).
    format!(
        "{}/../../target/release/mandatum",
        env!("CARGO_MANIFEST_DIR")
    )
}

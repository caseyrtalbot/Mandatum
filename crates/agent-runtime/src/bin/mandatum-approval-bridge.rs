//! Claude Code PreToolUse hook binary for Mandatum approval gating.
//!
//! Invoked by the Claude CLI with the hook payload on stdin and the approval
//! socket path as argv[1] (or `MANDATUM_APPROVAL_BRIDGE_SOCKET`). It forwards
//! one JSON request line over the socket, waits (bounded) for one verdict
//! line, and prints the `hookSpecificOutput` decision on stdout.
//!
//! **Fail closed**: every failure — bad payload, missing socket, connect or
//! read error, malformed verdict, or a listener that never answers — prints
//! a deny that names Mandatum. The only path to "allow" is an explicit
//! `{"allow": true}` verdict. The verdict wait is bounded on the bridge's
//! own clock (argv[2] seconds, default below Claude's hook timeout) so a
//! stalled listener can never leave the gated tool undecided until Claude's
//! hook-timeout policy — outside Mandatum's control — kicks in.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use mandatum_agent_runtime::bridge_protocol::{
    BridgeVerdict, allow_hook_output, bridge_request_from_hook_json, deny_hook_output,
};

/// Environment variable alternative to the argv socket path.
const SOCKET_ENV_VAR: &str = "MANDATUM_APPROVAL_BRIDGE_SOCKET";

/// Cap on the hook payload we are willing to read.
const MAX_STDIN_BYTES: u64 = 1024 * 1024;

/// Verdict wait bound when argv[2] is absent or unparseable: safely under
/// the default 600s PreToolUse hook timeout so the bridge always denies on
/// its own clock first (checked at compile time below).
const DEFAULT_VERDICT_TIMEOUT_SECS: u64 = 570;
const _: () = assert!(
    DEFAULT_VERDICT_TIMEOUT_SECS < 600,
    "the bridge must give up before Claude's default hook timeout"
);

fn main() {
    let output = match run() {
        Ok(()) => allow_hook_output(),
        Err(reason) => deny_hook_output(&reason),
    };
    println!("{output}");
}

/// `Ok(())` means the listener explicitly allowed the tool call. Any error
/// becomes a deny.
fn run() -> Result<(), String> {
    let socket_path = socket_path()?;

    let mut raw = String::new();
    std::io::stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut raw)
        .map_err(|error| format!("could not read the hook payload: {error}"))?;
    let request = bridge_request_from_hook_json(&raw)?;
    let mut line = serde_json::to_string(&request)
        .map_err(|error| format!("could not encode the approval request: {error}"))?;
    line.push('\n');

    let mut stream = UnixStream::connect(&socket_path).map_err(|error| {
        format!(
            "could not reach the Mandatum approval socket {}: {error}",
            socket_path.display()
        )
    })?;
    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|error| format!("could not send the approval request: {error}"))?;

    let verdict = await_verdict(&stream, verdict_timeout())?;
    if verdict.allow {
        Ok(())
    } else {
        Err(verdict
            .reason
            .unwrap_or_else(|| "Mandatum denied this action".to_owned()))
    }
}

/// Read one verdict line, bounded by `timeout`. A stalled or dead listener
/// times out into an error (→ deny): the bridge fails closed on its own
/// clock and never depends on Claude's hook-timeout behaviour.
fn await_verdict(stream: &UnixStream, timeout: Duration) -> Result<BridgeVerdict, String> {
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("could not bound the Mandatum verdict wait: {error}"))?;
    let mut verdict_line = String::new();
    BufReader::new(stream)
        .read_line(&mut verdict_line)
        .map_err(|error| format!("no Mandatum verdict arrived in time: {error}"))?;
    serde_json::from_str(verdict_line.trim())
        .map_err(|error| format!("unreadable Mandatum verdict: {error}"))
}

fn socket_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::args().nth(1)
        && !path.trim().is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var(SOCKET_ENV_VAR)
        && !path.trim().is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    Err("no Mandatum approval socket was configured".to_owned())
}

/// argv[2] in whole seconds, else the default. Zero or garbage falls back
/// to the default so the wait is always bounded.
fn verdict_timeout() -> Duration {
    let secs = std::env::args()
        .nth(2)
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_VERDICT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::net::UnixListener,
        sync::atomic::{AtomicU64, Ordering},
        time::Instant,
    };

    use super::*;

    static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_socket(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mandatum-bridge-{tag}-{}-{}.sock",
            std::process::id(),
            SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed),
        ))
    }

    /// A listener that accepts the connection, reads the request, and then
    /// never answers: the bridge must deny on its own clock instead of
    /// hanging until Claude's hook timeout.
    #[test]
    fn stalled_listener_times_out_into_a_deny() {
        let socket = temp_socket("stall");
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            let _ = reader.read_line(&mut line);
            // Hold the connection open past the bridge's bound: no reply,
            // no close, until well after the 200ms verdict timeout.
            std::thread::sleep(Duration::from_millis(500));
            drop(stream);
        });

        let mut stream = UnixStream::connect(&socket).unwrap();
        stream.write_all(b"{\"tool_name\":\"Bash\"}\n").unwrap();
        let started = Instant::now();
        let verdict = await_verdict(&stream, Duration::from_millis(200));
        let elapsed = started.elapsed();

        assert!(verdict.is_err(), "a silent listener must fail closed");
        assert!(
            elapsed < Duration::from_secs(2),
            "bridge waited {elapsed:?}; it must time out on its own clock"
        );
        drop(stream);
        server.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    /// An explicit verdict still round-trips within the bound.
    #[test]
    fn explicit_verdict_is_read_before_the_timeout() {
        let socket = temp_socket("verdict");
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            stream.write_all(b"{\"allow\":true}\n").unwrap();
        });

        let mut stream = UnixStream::connect(&socket).unwrap();
        stream.write_all(b"{\"tool_name\":\"Bash\"}\n").unwrap();
        let verdict = await_verdict(&stream, Duration::from_secs(5)).unwrap();
        assert!(verdict.allow);
        server.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }
}

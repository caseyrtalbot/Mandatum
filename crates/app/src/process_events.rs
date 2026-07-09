use std::{
    sync::mpsc::Sender,
    thread::{self, JoinHandle},
};

use mandatum_core::PaneId;
use mandatum_pty::{NativePtyReader, PtyEvent};

const PTY_READ_CHUNK_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PtyRuntimeEvent {
    Output {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
        bytes: Vec<u8>,
    },
    ReaderClosed {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
    },
    Error {
        pane_id: PaneId,
        restart_generation: u64,
        runtime_token: u64,
        message: String,
    },
}

pub(crate) fn spawn_reader_thread(
    pane_id: PaneId,
    restart_generation: u64,
    runtime_token: u64,
    mut reader: NativePtyReader,
    tx: Sender<PtyRuntimeEvent>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match reader.read_event(PTY_READ_CHUNK_BYTES) {
                Ok(Some(PtyEvent::Output(output))) => {
                    let _ = tx.send(PtyRuntimeEvent::Output {
                        pane_id: pane_id.clone(),
                        restart_generation,
                        runtime_token,
                        bytes: output.into_bytes(),
                    });
                }
                Ok(Some(PtyEvent::ChildExited(_))) | Ok(Some(PtyEvent::BackpressureChanged(_))) => {
                }
                Ok(None) => {
                    let _ = tx.send(PtyRuntimeEvent::ReaderClosed {
                        pane_id,
                        restart_generation,
                        runtime_token,
                    });
                    break;
                }
                Err(error) => {
                    let _ = tx.send(PtyRuntimeEvent::Error {
                        pane_id,
                        restart_generation,
                        runtime_token,
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
    })
}

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::thread;

use locusfs_graph::{DynamicGraph, GraphError, Result};
use niri_ipc::socket::Socket;
use niri_ipc::{Output, Request, Response};

use crate::state::NiriState;

pub type SharedNiriState = Arc<RwLock<NiriState>>;

#[derive(Debug, Default)]
pub struct IpcNiriClient;

impl IpcNiriClient {
    pub fn start(graph: DynamicGraph) -> Result<SharedNiriState> {
        let mut socket = Socket::connect()?;
        let outputs = request_outputs(&mut socket)?;
        let state = Arc::new(RwLock::new(NiriState::new(outputs)));
        spawn_event_stream(state.clone(), graph)?;

        Ok(state)
    }
}

fn spawn_event_stream(state: SharedNiriState, graph: DynamicGraph) -> Result<()> {
    let mut socket = Socket::connect()?;
    match send(&mut socket, Request::EventStream)? {
        Response::Handled => {}
        response => return Err(unexpected_response("event stream", response)),
    }

    thread::Builder::new()
        .name("locusfs-niri-event-stream".to_string())
        .spawn(move || {
            let mut read_event = socket.read_events();
            loop {
                match read_event() {
                    Ok(event) => {
                        let Ok(mut state) = state.write() else {
                            eprintln!("locusfs-niri: live state lock poisoned");
                            break;
                        };
                        match state.apply_event(event) {
                            Ok(changes) => {
                                for change in changes {
                                    if let Err(error) = graph.emit_change(change) {
                                        eprintln!(
                                            "locusfs-niri: failed to emit graph change: {error}"
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                eprintln!("locusfs-niri: failed to apply event: {error}");
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("locusfs-niri: failed to read event stream: {error}");
                        break;
                    }
                }
            }
        })
        .map_err(GraphError::from)?;

    Ok(())
}

fn request_outputs(socket: &mut Socket) -> Result<HashMap<String, Output>> {
    match send(socket, Request::Outputs)? {
        Response::Outputs(outputs) => Ok(outputs),
        response => Err(unexpected_response("outputs", response)),
    }
}

fn send(socket: &mut Socket, request: Request) -> Result<Response> {
    socket
        .send(request)
        .map_err(GraphError::from)?
        .map_err(|message| GraphError::Io(format!("niri rejected IPC request: {message}")))
}

fn unexpected_response(request: &'static str, response: Response) -> GraphError {
    GraphError::Io(format!(
        "unexpected niri response for {request} request: {response:?}"
    ))
}

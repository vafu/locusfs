use std::collections::HashMap;
use std::env;
use std::io;
use std::sync::Arc;

use locusfs_graph::{DynamicGraph, GraphError, Result};
use niri_ipc::socket::SOCKET_PATH_ENV;
use niri_ipc::{Event, Output, Reply, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use crate::state::NiriState;

pub type SharedNiriState = Arc<RwLock<NiriState>>;

#[derive(Debug, Default)]
pub struct IpcNiriClient;

impl IpcNiriClient {
    pub async fn start(
        graph: DynamicGraph,
        runtime: Handle,
    ) -> Result<(SharedNiriState, JoinHandle<()>)> {
        let mut socket = AsyncNiriSocket::connect().await?;
        let outputs = request_outputs(&mut socket).await?;
        let state = Arc::new(RwLock::new(NiriState::new(outputs)));
        let event_stream = spawn_event_stream(state.clone(), graph, runtime).await?;

        Ok((state, event_stream))
    }
}

async fn spawn_event_stream(
    state: SharedNiriState,
    graph: DynamicGraph,
    runtime: Handle,
) -> Result<JoinHandle<()>> {
    let socket = connect_event_stream().await?;

    Ok(runtime.spawn(async move {
        let mut socket = socket;
        loop {
            read_event_stream(&mut socket, &state, &graph).await;
            sleep_retry().await;
            loop {
                match connect_event_stream().await {
                    Ok(next_socket) => {
                        socket = next_socket;
                        break;
                    }
                    Err(error) => {
                        eprintln!("locusfs-niri: failed to reconnect event stream: {error}");
                        sleep_retry().await;
                    }
                }
            }
        }
    }))
}

async fn connect_event_stream() -> Result<AsyncNiriSocket> {
    let mut socket = AsyncNiriSocket::connect().await?;
    match socket.send(Request::EventStream).await? {
        Response::Handled => {}
        response => return Err(unexpected_response("event stream", response)),
    }
    socket.shutdown_write().await?;
    Ok(socket)
}

async fn read_event_stream(
    socket: &mut AsyncNiriSocket,
    state: &SharedNiriState,
    graph: &DynamicGraph,
) {
    loop {
        match socket.read_event().await {
            Ok(event) => {
                let mut state = state.write().await;
                match state.apply_event(event) {
                    Ok(changes) => {
                        for change in changes {
                            if let Err(error) = graph.emit_global_change(change) {
                                eprintln!("locusfs-niri: failed to emit graph change: {error}");
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
}

async fn sleep_retry() {
    sleep(Duration::from_secs(1)).await;
}

async fn request_outputs(socket: &mut AsyncNiriSocket) -> Result<HashMap<String, Output>> {
    match socket.send(Request::Outputs).await? {
        Response::Outputs(outputs) => Ok(outputs),
        response => Err(unexpected_response("outputs", response)),
    }
}

fn unexpected_response(request: &'static str, response: Response) -> GraphError {
    GraphError::Io(format!(
        "unexpected niri response for {request} request: {response:?}"
    ))
}

struct AsyncNiriSocket {
    stream: BufReader<UnixStream>,
}

impl AsyncNiriSocket {
    async fn connect() -> io::Result<Self> {
        let socket_path = env::var_os(SOCKET_PATH_ENV).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{SOCKET_PATH_ENV} is not set, are you running this within niri?"),
            )
        })?;
        let stream = UnixStream::connect(socket_path).await?;
        Ok(Self {
            stream: BufReader::new(stream),
        })
    }

    async fn send(&mut self, request: Request) -> Result<Response> {
        let reply = self.send_raw(request).await.map_err(GraphError::from)?;
        reply.map_err(|message| GraphError::Io(format!("niri rejected IPC request: {message}")))
    }

    async fn send_raw(&mut self, request: Request) -> io::Result<Reply> {
        let mut request = serde_json::to_string(&request)?;
        request.push('\n');
        self.stream.get_mut().write_all(request.as_bytes()).await?;
        self.stream.get_mut().flush().await?;

        let mut response = String::new();
        self.stream.read_line(&mut response).await?;
        serde_json::from_str(&response).map_err(Into::into)
    }

    async fn shutdown_write(&mut self) -> io::Result<()> {
        self.stream.get_mut().shutdown().await
    }

    async fn read_event(&mut self) -> io::Result<Event> {
        let mut event = String::new();
        self.stream.read_line(&mut event).await?;
        serde_json::from_str(&event).map_err(Into::into)
    }
}

use locusfs_graph::{DynamicGraph, GraphError, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::config::PipeWireConfig;
use crate::state::{PactlEndpoint, PactlInfo, SharedPipeWireState, snapshot_from_pactl};

#[derive(Debug, Default)]
pub struct PipeWireRuntime;

impl PipeWireRuntime {
    pub fn start(
        graph: DynamicGraph,
        config: PipeWireConfig,
        runtime: Handle,
    ) -> (SharedPipeWireState, JoinHandle<()>) {
        let state = crate::state::PipeWireState::shared();
        let task_state = state.clone();
        let task = runtime.spawn(async move {
            run_pipewire_watcher(config, task_state, graph).await;
        });
        (state, task)
    }
}

async fn run_pipewire_watcher(
    config: PipeWireConfig,
    state: SharedPipeWireState,
    graph: DynamicGraph,
) {
    refresh_and_publish(&config, &state, &graph).await;

    loop {
        let mut child = match Command::new(&config.pactl)
            .arg("subscribe")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                eprintln!("locusfs-pipewire: failed to run pactl subscribe: {error}");
                sleep_retry().await;
                refresh_and_publish(&config, &state, &graph).await;
                continue;
            }
        };

        let Some(stdout) = child.stdout.take() else {
            eprintln!("locusfs-pipewire: pactl subscribe did not expose stdout");
            let _ = child.kill().await;
            sleep_retry().await;
            continue;
        };

        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if subscription_line_affects_snapshot(&line) {
                        refresh_and_publish(&config, &state, &graph).await;
                    }
                }
                Ok(None) => {
                    eprintln!("locusfs-pipewire: pactl subscribe ended");
                    break;
                }
                Err(error) => {
                    eprintln!("locusfs-pipewire: failed to read pactl subscribe: {error}");
                    break;
                }
            }
        }

        let _ = child.kill().await;
        sleep_retry().await;
        refresh_and_publish(&config, &state, &graph).await;
    }
}

async fn refresh_and_publish(
    config: &PipeWireConfig,
    state: &SharedPipeWireState,
    graph: &DynamicGraph,
) {
    let snapshot = match read_snapshot(config).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("locusfs-pipewire: failed to read PipeWire snapshot: {error}");
            return;
        }
    };

    let changes = {
        let mut state = state.write().await;
        match state.apply_snapshot(snapshot) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("locusfs-pipewire: failed to apply PipeWire snapshot: {error}");
                Vec::new()
            }
        }
    };

    for change in changes {
        if let Err(error) = graph.emit_global_change(change) {
            eprintln!("locusfs-pipewire: failed to emit graph change: {error}");
        }
    }
}

async fn read_snapshot(config: &PipeWireConfig) -> Result<crate::state::PipeWireSnapshot> {
    let info = pactl_json::<PactlInfo>(&config.pactl, &["-f", "json", "info"]).await?;
    let sinks =
        pactl_json::<Vec<PactlEndpoint>>(&config.pactl, &["-f", "json", "list", "sinks"]).await?;
    let sources =
        pactl_json::<Vec<PactlEndpoint>>(&config.pactl, &["-f", "json", "list", "sources"]).await?;
    Ok(snapshot_from_pactl(info, sinks, sources))
}

async fn pactl_json<T>(pactl: &str, args: &[&str]) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let output = Command::new(pactl)
        .args(args)
        .output()
        .await
        .map_err(|error| GraphError::Io(format!("run {pactl}: {error}")))?;
    if !output.status.success() {
        return Err(GraphError::Io(format!(
            "{pactl} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| GraphError::Io(format!("parse {pactl} JSON: {error}")))
}

fn subscription_line_affects_snapshot(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("sink") || lower.contains("source") || lower.contains("server")
}

async fn sleep_retry() {
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}

#[cfg(test)]
mod test {
    use super::subscription_line_affects_snapshot;

    #[test]
    fn subscription_filter_tracks_relevant_pactl_events() {
        assert!(subscription_line_affects_snapshot(
            "Event 'change' on sink #149"
        ));
        assert!(subscription_line_affects_snapshot(
            "Event 'change' on server #0"
        ));
        assert!(!subscription_line_affects_snapshot(
            "Event 'new' on client #42"
        ));
    }
}

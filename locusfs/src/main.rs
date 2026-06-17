use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use locusfs::fuse::{FuseMountConfig, mount};
use locusfs::graph::{DynamicGraph, InMemoryProvider, NodeKind, Result};

mod watch;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("locusfs: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let command = parse_command()?;

    if let Command::Watch { path } = command {
        return watch::watch_path(&path).map_err(Into::into);
    }

    let Command::Mount { mountpoint } = command else {
        unreachable!("all commands are handled above");
    };
    fs::create_dir_all(&mountpoint)?;

    let (graph, _plugins) = default_graph().await?;
    let _mount = mount(FuseMountConfig::new(&mountpoint), graph).await?;

    eprintln!("locusfs mounted at {}", mountpoint.display());
    eprintln!("press Ctrl-C to unmount");

    wait_for_shutdown().await?;

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(true)
        .with_thread_names(true)
        .try_init();
}

enum Command {
    Mount { mountpoint: PathBuf },
    Watch { path: PathBuf },
}

fn parse_command() -> std::result::Result<Command, String> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_else(|| "locusfs".into());
    let Some(first) = args.next() else {
        return Err(usage(&program.to_string_lossy()));
    };

    if first == "--watch" {
        let Some(path) = args.next() else {
            return Err(usage(&program.to_string_lossy()));
        };
        if args.next().is_some() {
            return Err(usage(&program.to_string_lossy()));
        }
        return Ok(Command::Watch {
            path: PathBuf::from(path),
        });
    }

    if args.next().is_some() {
        return Err(usage(&program.to_string_lossy()));
    }

    Ok(Command::Mount {
        mountpoint: PathBuf::from(first),
    })
}

fn usage(program: &str) -> String {
    format!("usage: {program} <mountpoint>\n       {program} --watch <path>")
}

#[derive(Debug)]
struct PluginHandles {
    _dbus: locusfs_plugin_dbus::DbusPluginHandle,
    _niri: locusfs_plugin_niri::NiriPluginHandle,
}

async fn default_graph() -> Result<(DynamicGraph, PluginHandles)> {
    let kind = NodeKind::new("node")?;
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph.register_node_provider(provider.clone()).await?;
    graph
        .register_node_mutation_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_property_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_property_mutation_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_relation_provider(kind.clone(), provider.clone())
        .await?;
    graph
        .register_relation_mutation_provider(kind, provider)
        .await?;
    let dbus = locusfs_plugin_dbus::register(&graph).await?;
    let niri = locusfs_plugin_niri::register(&graph).await?;
    Ok((
        graph,
        PluginHandles {
            _dbus: dbus,
            _niri: niri,
        },
    ))
}

async fn wait_for_shutdown() -> std::result::Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        Ok(())
    }
}

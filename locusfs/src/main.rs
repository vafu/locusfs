use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use locusfs::config::Config;
use locusfs::fuse::{FuseMount, FuseMountConfig, mount};
use locusfs::graph::DynamicGraph;
use locusfs::plugin::PluginManager;
use tracing_subscriber::prelude::*;

mod perfetto;
mod watch;

type AppError = Box<dyn std::error::Error + Send + Sync>;

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

async fn run() -> std::result::Result<(), AppError> {
    let _perfetto_trace = init_tracing();
    let command = parse_command()?;

    if let Command::Watch { path } = command {
        return watch::watch_path(&path).await.map_err(Into::into);
    }

    let Command::Mount { mountpoint, config } = command else {
        unreachable!("all commands are handled above");
    };
    let mountpoint_state = prepare_mountpoint(&mountpoint).await?;
    if let Err(error) = tokio::fs::create_dir_all(&mountpoint).await {
        cleanup_mountpoint(&mountpoint, mountpoint_state).await?;
        return Err(error.into());
    }

    let config = match Config::load(config).await {
        Ok(config) => config,
        Err(error) => {
            cleanup_mountpoint(&mountpoint, mountpoint_state).await?;
            return Err(error.into());
        }
    };
    let (graph, mut plugins) = match default_graph(&config).await {
        Ok(result) => result,
        Err(error) => {
            cleanup_mountpoint(&mountpoint, mountpoint_state).await?;
            return Err(error);
        }
    };
    let mount = match mount(FuseMountConfig::new(&mountpoint), graph).await {
        Ok(mount) => mount,
        Err(error) => {
            plugins.shutdown().await;
            cleanup_mountpoint(&mountpoint, mountpoint_state).await?;
            return Err(error.into());
        }
    };

    eprintln!("locusfs mounted at {}", mountpoint.display());
    eprintln!("press Ctrl-C to unmount");

    let shutdown_result = wait_for_shutdown().await;
    plugins.shutdown().await;
    let unmount_result = unmount_with_fallback(mount, &mountpoint).await;
    cleanup_mountpoint(&mountpoint, mountpoint_state).await?;

    shutdown_result?;
    unmount_result?;

    Ok(())
}

async fn unmount_with_fallback(
    mount: FuseMount,
    mountpoint: &PathBuf,
) -> std::result::Result<(), AppError> {
    match mount.unmount().await {
        Ok(()) => Ok(()),
        Err(error) if error.to_string().contains("Device or resource busy") => {
            eprintln!("locusfs: normal unmount failed ({error}); trying lazy unmount");
            unmount_stale_mountpoint(mountpoint).await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MountpointState {
    Created,
    Existing,
}

async fn prepare_mountpoint(mountpoint: &PathBuf) -> io::Result<MountpointState> {
    match tokio::fs::try_exists(mountpoint).await {
        Ok(true) => Ok(MountpointState::Existing),
        Ok(false) => Ok(MountpointState::Created),
        Err(error) if is_disconnected_fuse_mount(&error) => {
            unmount_stale_mountpoint(mountpoint).await?;
            Ok(MountpointState::Existing)
        }
        Err(error) => Err(error),
    }
}

async fn cleanup_mountpoint(mountpoint: &PathBuf, state: MountpointState) -> io::Result<()> {
    if matches!(state, MountpointState::Created) {
        remove_mountpoint_dir(mountpoint).await?;
    }
    Ok(())
}

fn is_disconnected_fuse_mount(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOTCONN)
}

async fn unmount_stale_mountpoint(mountpoint: &PathBuf) -> io::Result<()> {
    let status = tokio::process::Command::new("fusermount3")
        .args(["-u", "-z"])
        .arg(mountpoint)
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to unmount stale FUSE mountpoint {}",
            mountpoint.display()
        )))
    }
}

async fn remove_mountpoint_dir(mountpoint: &PathBuf) -> io::Result<()> {
    match tokio::fs::remove_dir(mountpoint).await {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn init_tracing() -> Option<perfetto::PerfettoTraceSession> {
    let perfetto_config = perfetto::PerfettoTraceConfig::from_env();
    if perfetto_config.is_some() {
        tracing_perfetto_sdk::init_in_process();
    }

    let perfetto_layer = perfetto_config
        .as_ref()
        .map(|_| tracing_perfetto_sdk::PerfettoLayer::new());
    let plugin_track_layer = perfetto_config
        .as_ref()
        .map(|_| perfetto::PluginTrackLayer::new());

    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_names(true),
        )
        .with(perfetto_layer)
        .with(plugin_track_layer);

    if let Err(error) = subscriber.try_init() {
        if perfetto_config.is_some() {
            eprintln!("locusfs: Perfetto tracing disabled because tracing setup failed: {error}");
        }
        return None;
    }

    perfetto_config.and_then(|config| {
        let output = config.output().clone();
        match perfetto::PerfettoTraceSession::start(config) {
            Ok(session) => {
                eprintln!("locusfs: recording Perfetto trace to {}", output.display());
                Some(session)
            }
            Err(error) => {
                eprintln!(
                    "locusfs: failed to start Perfetto trace {}: {error}",
                    output.display()
                );
                None
            }
        }
    })
}

enum Command {
    Mount {
        mountpoint: PathBuf,
        config: Option<PathBuf>,
    },
    Watch {
        path: PathBuf,
    },
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

    let mut config = None;
    let mut mountpoint = None;
    let mut current = Some(first);
    while let Some(arg) = current.take().or_else(|| args.next()) {
        if arg == "--config" {
            let Some(path) = args.next() else {
                return Err(usage(&program.to_string_lossy()));
            };
            if config.replace(PathBuf::from(path)).is_some() {
                return Err(usage(&program.to_string_lossy()));
            }
            continue;
        }
        if mountpoint.replace(PathBuf::from(arg)).is_some() {
            return Err(usage(&program.to_string_lossy()));
        }
    }

    let Some(mountpoint) = mountpoint else {
        return Err(usage(&program.to_string_lossy()));
    };
    Ok(Command::Mount { mountpoint, config })
}

fn usage(program: &str) -> String {
    format!("usage: {program} [--config <path>] <mountpoint>\n       {program} --watch <path>")
}

async fn default_graph(
    config: &Config,
) -> std::result::Result<(DynamicGraph, PluginManager), AppError> {
    let graph = DynamicGraph::new();
    let plugins = PluginManager::load_enabled(config, graph.clone()).await?;
    Ok((graph, plugins))
}

async fn wait_for_shutdown() -> std::result::Result<(), AppError> {
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

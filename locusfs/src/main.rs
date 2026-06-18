use std::env;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use locusfs::fuse::{FuseMountConfig, mount};
use locusfs::graph::{DynamicGraph, Result};

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
        return watch::watch_path(&path).await.map_err(Into::into);
    }

    let Command::Mount { mountpoint } = command else {
        unreachable!("all commands are handled above");
    };
    let created_mountpoint = prepare_mountpoint(&mountpoint).await?;
    tokio::fs::create_dir_all(&mountpoint).await?;

    let (graph, _plugins) = default_graph().await?;
    let mount = mount(FuseMountConfig::new(&mountpoint), graph).await?;

    eprintln!("locusfs mounted at {}", mountpoint.display());
    eprintln!("press Ctrl-C to unmount");

    let shutdown_result = wait_for_shutdown().await;
    let unmount_result = mount.unmount().await;
    if created_mountpoint {
        remove_mountpoint_dir(&mountpoint).await?;
    }

    shutdown_result?;
    unmount_result?;

    Ok(())
}

async fn prepare_mountpoint(mountpoint: &PathBuf) -> io::Result<bool> {
    match tokio::fs::try_exists(mountpoint).await {
        Ok(exists) => Ok(!exists),
        Err(error) if is_disconnected_fuse_mount(&error) => {
            unmount_stale_mountpoint(mountpoint).await?;
            Ok(true)
        }
        Err(error) => Err(error),
    }
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
    _project: locusfs_plugin_project::ProjectPluginHandle,
}

async fn default_graph() -> Result<(DynamicGraph, PluginHandles)> {
    let graph = DynamicGraph::new();
    let project = locusfs_plugin_project::register(&graph).await?;
    let dbus = locusfs_plugin_dbus::register(&graph).await?;
    let niri = locusfs_plugin_niri::register(&graph).await?;
    Ok((
        graph,
        PluginHandles {
            _dbus: dbus,
            _niri: niri,
            _project: project,
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

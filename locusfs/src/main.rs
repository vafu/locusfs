use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use locusfs::fuse::{FuseMountConfig, mount};
use locusfs::graph::{DynamicGraph, InMemoryProvider, NodeKind, Result};

mod watch;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("locusfs: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let command = parse_command()?;

    if let Command::Watch { path } = command {
        return watch::watch_path(&path).map_err(Into::into);
    }

    let Command::Mount { mountpoint } = command else {
        unreachable!("all commands are handled above");
    };
    fs::create_dir_all(&mountpoint)?;
    install_signal_handlers()?;

    let graph = default_graph()?;
    let _mount = mount(FuseMountConfig::new(&mountpoint), graph)?;

    eprintln!("locusfs mounted at {}", mountpoint.display());
    eprintln!("press Ctrl-C to unmount");

    while !SHUTDOWN.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }

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

fn default_graph() -> Result<DynamicGraph> {
    let kind = NodeKind::new("node")?;
    let provider = InMemoryProvider::new(kind.clone());
    let graph = DynamicGraph::new();
    graph.register_node_provider(provider.clone())?;
    graph.register_node_mutation_provider(kind.clone(), provider.clone())?;
    graph.register_property_provider(kind.clone(), provider.clone())?;
    graph.register_property_mutation_provider(kind.clone(), provider.clone())?;
    graph.register_relation_provider(kind.clone(), provider.clone())?;
    graph.register_relation_mutation_provider(kind, provider)?;
    locusfs_plugin_niri::register(&graph)?;
    Ok(graph)
}

fn install_signal_handlers() -> std::result::Result<(), String> {
    unsafe {
        if libc::signal(
            libc::SIGINT,
            handle_signal as *const () as libc::sighandler_t,
        ) == libc::SIG_ERR
        {
            return Err("failed to install SIGINT handler".to_string());
        }
        if libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        ) == libc::SIG_ERR
        {
            return Err("failed to install SIGTERM handler".to_string());
        }
    }
    Ok(())
}

extern "C" fn handle_signal(_signal: libc::c_int) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

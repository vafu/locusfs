use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use locusfs::fuse::{FuseMountConfig, mount};
use locusfs::graph::{DynamicGraph, InMemoryProvider, NodeKind, Result};

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
    let mountpoint = parse_mountpoint()?;
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

fn parse_mountpoint() -> std::result::Result<PathBuf, String> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_else(|| "locusfs".into());
    let Some(mountpoint) = args.next() else {
        return Err(format!("usage: {} <mountpoint>", program.to_string_lossy()));
    };

    if args.next().is_some() {
        return Err(format!("usage: {} <mountpoint>", program.to_string_lossy()));
    }

    Ok(PathBuf::from(mountpoint))
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

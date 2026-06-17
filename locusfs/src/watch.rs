use std::io::{self, Write};
use std::path::Path;

use locusfs_client::Watch;

pub fn watch_path(path: &Path) -> io::Result<()> {
    let mut watcher = Watch::open(path)?;

    print_value(watcher.read()?)?;
    loop {
        print_value(watcher.wait_and_read()?)?;
    }
}

fn print_value(value: Vec<u8>) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(&value)?;
    if !value.ends_with(b"\n") {
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
}

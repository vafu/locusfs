use std::io;
use std::path::Path;

use locusfs_client::Watch;
use tokio::io::{AsyncWriteExt, stdout};

pub async fn watch_path(path: &Path) -> io::Result<()> {
    let mut watcher = Watch::open(path).await?;
    if tokio::fs::metadata(path).await?.is_dir() {
        loop {
            print_value(watcher.wait_event().await?).await?;
        }
    }

    print_value(watcher.read().await?).await?;
    loop {
        print_value(watcher.wait_and_read().await?).await?;
    }
}

async fn print_value(value: Vec<u8>) -> io::Result<()> {
    let mut stdout = stdout();
    stdout.write_all(&value).await?;
    if !value.ends_with(b"\n") {
        stdout.write_all(b"\n").await?;
    }
    stdout.flush().await
}

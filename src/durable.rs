use std::fs::File;
use std::io;
use std::path::Path;

/// Flush a file created by this tool. Windows requires a writable handle for
/// FlushFileBuffers, while Unix can flush through a read-only handle.
pub(crate) fn sync_existing_file(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    let file = File::options().write(true).open(path)?;
    #[cfg(not(windows))]
    let file = File::open(path)?;
    file.sync_all()
}

/// Sync directory entries where std exposes a usable directory fsync.
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if std::fs::metadata(path)?.is_dir() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a directory",
            ))
        }
    }
    #[cfg(not(windows))]
    {
        File::open(path)?.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::{sync_directory, sync_existing_file};
    use std::fs;

    #[test]
    fn syncs_a_written_file_and_its_parent_directory() {
        let root = std::env::temp_dir().join(format!("kanade-durable-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let file = root.join("payload");
        fs::write(&file, b"durable").unwrap();
        sync_existing_file(&file).unwrap();
        sync_directory(&root).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"durable");
        fs::remove_dir_all(root).unwrap();
    }
}

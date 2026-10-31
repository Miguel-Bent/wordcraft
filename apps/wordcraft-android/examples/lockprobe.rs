//! Diagnostic: what `File::try_lock` reports on this device (the recovery copy's lock).
//! Usage: lockprobe <path>. Prints the outcome and the raw errno.
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/data/local/tmp/probe.lock".into());
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(&path);
    let file = match file {
        Ok(f) => f,
        Err(e) => {
            println!("open failed: {e} (raw {:?})", e.raw_os_error());
            return;
        }
    };
    match file.try_lock() {
        Ok(()) => println!("try_lock: locked"),
        Err(std::fs::TryLockError::WouldBlock) => println!("try_lock: would block"),
        Err(std::fs::TryLockError::Error(e)) => println!("try_lock error: {e} (raw {:?}, kind {:?})", e.raw_os_error(), e.kind()),
    }
}

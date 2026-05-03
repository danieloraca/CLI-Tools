use anyhow::{Result, anyhow};
use std::{
    io::{self, IsTerminal, Write},
    thread,
    time::Duration,
};

const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];

pub fn run_with_spinner<T, F>(message: &str, operation: F) -> Result<T>
where
    T: Send,
    F: FnOnce() -> Result<T> + Send,
{
    if !io::stderr().is_terminal() {
        return operation();
    }

    thread::scope(|scope| -> Result<T> {
        let handle = scope.spawn(operation);
        let mut index = 0usize;

        while !handle.is_finished() {
            let frame = FRAMES[index % FRAMES.len()];
            index = index.wrapping_add(1);
            eprint!("\r{frame} {message}");
            io::stderr().flush()?;
            thread::sleep(Duration::from_millis(120));
        }

        eprint!("\r\x1b[2K");
        io::stderr().flush()?;

        handle
            .join()
            .map_err(|_| anyhow!("{message} task panicked"))?
    })
}

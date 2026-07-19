use std::{
    io,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::Sender,
    thread,
};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

use crate::publisher;

pub struct RatholeProcess {
    pid: u32,
    running: bool,
}

impl RatholeProcess {
    pub fn start(
        binary: &Path,
        config: &Path,
        generation: u64,
        sender: Sender<publisher::Command>,
    ) -> io::Result<Self> {
        let mut command = Command::new(binary);
        command
            .arg("--client")
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        #[cfg(target_os = "linux")]
        {
            let parent = std::process::id();
            // SAFETY: pre_exec calls only async-signal-safe libc functions and
            // constructs no heap-backed values in the child process.
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getppid() as u32 != parent {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "adapter exited while starting Rathole",
                        ));
                    }
                    Ok(())
                });
            }
        }

        let mut child = command.spawn()?;
        let pid = child.id();
        let waiter = thread::Builder::new()
            .name("rathole-wait".to_owned())
            .spawn(move || {
                let mut status = 0;
                let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
                let success = waited == pid as libc::pid_t
                    && libc::WIFEXITED(status)
                    && libc::WEXITSTATUS(status) == 0;
                let _ = sender.send(publisher::Command::ChildExited {
                    generation,
                    success,
                });
            });
        if let Err(error) = waiter {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        drop(child);
        Ok(Self { pid, running: true })
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if !self.running {
            return Ok(());
        }
        let result = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        self.running = false;
        Ok(())
    }

    pub fn mark_exited(&mut self) {
        self.running = false;
    }
}

impl Drop for RatholeProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

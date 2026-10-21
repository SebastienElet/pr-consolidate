use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

fn drain(stream: &mut UnixStream, output: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut buffer = [0u8; 65536];
    for _ in 0..64 {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub fn capture(
    command: &mut Command,
    input: Option<&str>,
    timeout: Duration,
    merge: bool,
) -> std::io::Result<Output> {
    let (mut stdout, out_writer) = UnixStream::pair()?;
    let (mut stderr, err_writer) = UnixStream::pair()?;
    let (mut stdin, in_reader) = UnixStream::pair()?;
    stdout.set_nonblocking(true)?;
    stderr.set_nonblocking(true)?;
    stdin.set_nonblocking(true)?;
    command.stdin(if input.is_some() {
        Stdio::from(OwnedFd::from(in_reader))
    } else {
        Stdio::inherit()
    });
    command.stdout(Stdio::from(OwnedFd::from(out_writer.try_clone()?)));
    let error_output = if merge {
        drop(err_writer);
        out_writer
    } else {
        drop(out_writer);
        err_writer
    };
    command.stderr(Stdio::from(OwnedFd::from(error_output)));
    command.process_group(0);
    let mut child = command.spawn()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut remaining = input.unwrap_or("").as_bytes();
    let mut status = None;
    let mut stdout_closed = false;
    let mut stderr_closed = merge;
    let outcome = (|| {
        loop {
            if !stdout_closed {
                stdout_closed = drain(&mut stdout, &mut out)?;
            }
            if !stderr_closed {
                stderr_closed = drain(&mut stderr, &mut err)?;
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if let Some(status) = status {
                if stdout_closed && stderr_closed {
                    return Ok((status, false));
                }
            }
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                child.kill().ok();
                let status = match status {
                    Some(status) => status,
                    None => child.wait()?,
                };
                drain(&mut stdout, &mut out)?;
                drain(&mut stderr, &mut err)?;
                return Ok((status, true));
            }
            if remaining.is_empty() {
                stdin.shutdown(Shutdown::Write).ok();
            } else {
                match stdin.write(remaining) {
                    Ok(count) => remaining = &remaining[count..],
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => remaining = &[],
                    Err(error) => return Err(error),
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    let (status, timed_out) = match outcome {
        Ok(value) => value,
        Err(error) => {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            child.kill().ok();
            child.wait().ok();
            return Err(error);
        }
    };
    Ok(Output {
        code: if timed_out {
            124
        } else {
            status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
        },
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        timed_out,
    })
}

pub fn sanitize_git(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("GIT_CONFIG")
            || matches!(
                name.as_ref(),
                "GIT_DIR"
                    | "GIT_COMMON_DIR"
                    | "GIT_WORK_TREE"
                    | "GIT_INDEX_FILE"
                    | "GIT_OBJECT_DIRECTORY"
                    | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                    | "GIT_NAMESPACE"
                    | "GIT_SHALLOW_FILE"
                    | "GIT_REPLACE_REF_BASE"
            )
        {
            command.env_remove(key);
        }
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
}

pub fn redact(value: &str) -> String {
    let token = std::env::var("NPM_TOKEN").unwrap_or_default();
    if token.is_empty() {
        value.to_owned()
    } else {
        value.replace(&token, "[REDACTED]")
    }
}
pub fn mask(value: &str) -> String {
    regex::Regex::new(r"//[^/\s@]+@")
        .unwrap()
        .replace_all(&redact(value), "//***@")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_stdin_and_output_do_not_deadlock_or_truncate() {
        let input = "data\n".repeat(100_000);
        let mut command = Command::new("cat");
        let result = capture(&mut command, Some(&input), Duration::from_secs(3), false).unwrap();
        assert_eq!(result.code, 0);
        assert_eq!(result.stdout, input);
    }
    #[test]
    fn merged_output_preserves_stdout_stderr_order_and_signal_exit() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf first; printf second >&2; kill -TERM $$"]);
        let result = capture(&mut command, None, Duration::from_secs(3), true).unwrap();
        assert_eq!(result.code, 143);
        assert_eq!(result.stdout, "firstsecond");
    }
}

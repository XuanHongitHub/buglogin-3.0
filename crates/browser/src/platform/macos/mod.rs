use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use buglogin_domain::errors::Problem;

use crate::platform::{OwnedProcessTree, ProcessTreeIdentity, ProcessTreeStatus};
use crate::process::ProcessIdentity;

const GROUP_HELPER: &str = r#"
owner_pid="$BUGLOGIN_OWNER_PID"
owner_start="$BUGLOGIN_OWNER_START"
"$@" &
child=$!
pgid=$$
child_start=$(/bin/ps -p "$child" -o lstart= | /usr/bin/sed 's/^ *//;s/ *$//')
printf 'BUGLOGIN %s %s %s\n' "$child" "$pgid" "$child_start"
cleanup() {
  trap '' TERM INT HUP
  /bin/kill -TERM -"$pgid" 2>/dev/null || true
  /bin/sleep 2
  /bin/kill -KILL -"$pgid" 2>/dev/null || true
}
trap cleanup TERM INT HUP
while /bin/kill -0 "$child" 2>/dev/null; do
  current_owner=$(/bin/ps -p "$owner_pid" -o lstart= | /usr/bin/sed 's/^ *//;s/ *$//')
  if [ "$current_owner" != "$owner_start" ]; then cleanup; fi
  /bin/sleep 0.2
done
wait "$child" 2>/dev/null || true
cleanup
"#;

#[derive(Clone, Copy, Debug)]
pub struct StopPolicy {
    pub graceful_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for StopPolicy {
    fn default() -> Self {
        Self {
            graceful_timeout: Duration::from_secs(8),
            poll_interval: Duration::from_millis(50),
        }
    }
}

pub struct MacOsProcessGroupController {
    stop_policy: StopPolicy,
}

impl MacOsProcessGroupController {
    pub fn system() -> Self {
        Self {
            stop_policy: StopPolicy::default(),
        }
    }

    pub fn with_stop_policy(mut self, stop_policy: StopPolicy) -> Self {
        self.stop_policy = stop_policy;
        self
    }

    pub fn launch(&self, executable: &Path, args: &[String]) -> Result<MacOsProcessGroup, Problem> {
        let executable = executable
            .canonicalize()
            .map_err(|_| executable_invalid())?;
        let owner_pid = std::process::id();
        let owner_start = process_start(owner_pid).ok_or_else(identity_unverified)?;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", GROUP_HELPER, "buglogin-process-group"])
            .arg(&executable)
            .args(args)
            .env("BUGLOGIN_OWNER_PID", owner_pid.to_string())
            .env("BUGLOGIN_OWNER_START", owner_start)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let mut helper = command.spawn().map_err(|_| launch_failed())?;
        let stdout = helper.stdout.take().ok_or_else(launch_failed)?;
        let line = match read_first_line(stdout, Duration::from_secs(10)) {
            Ok(line) => line,
            Err(error) => {
                let _ = helper.kill();
                let _ = helper.wait();
                return Err(error);
            }
        };
        let Some((pid, pgid, start)) = parse_launch_line(&line) else {
            let _ = helper.kill();
            let _ = helper.wait();
            return Err(launch_failed());
        };
        Ok(MacOsProcessGroup {
            helper,
            identity: ProcessTreeIdentity {
                root: ProcessIdentity {
                    pid,
                    executable,
                    token: format!("macos:{start}"),
                },
                tree_id: pgid,
            },
            stop_policy: self.stop_policy,
        })
    }

    pub fn inspect(&self, identity: &ProcessTreeIdentity) -> ProcessTreeStatus {
        inspect_process(identity)
    }

    pub fn terminate(&self, identity: &ProcessTreeIdentity) -> Result<(), Problem> {
        if self.inspect(identity) != ProcessTreeStatus::Running {
            return Err(identity_unverified());
        }
        signal_group(identity.tree_id, "KILL")
    }
}

pub struct MacOsProcessGroup {
    helper: Child,
    identity: ProcessTreeIdentity,
    stop_policy: StopPolicy,
}

impl OwnedProcessTree for MacOsProcessGroup {
    fn identity(&self) -> &ProcessTreeIdentity {
        &self.identity
    }

    fn status(&mut self) -> Result<ProcessTreeStatus, Problem> {
        if self
            .helper
            .try_wait()
            .map_err(|_| status_failed())?
            .is_some()
        {
            return Ok(ProcessTreeStatus::Exited);
        }
        Ok(inspect_process(&self.identity))
    }

    fn stop(&mut self) -> Result<(), Problem> {
        if self
            .helper
            .try_wait()
            .map_err(|_| status_failed())?
            .is_some()
        {
            return Ok(());
        }
        match inspect_process(&self.identity) {
            // The browser already exited (window closed, crash): only its
            // helpers and the group supervisor are left to reap.
            ProcessTreeStatus::Exited => {}
            ProcessTreeStatus::Running => {
                // TERM only the browser so it can flush profile state; a group
                // TERM would also trigger the supervisor's 2 s kill timer.
                signal_process(self.identity.root.pid, "TERM")?;
                let deadline = Instant::now() + self.stop_policy.graceful_timeout;
                while Instant::now() < deadline
                    && inspect_process(&self.identity) == ProcessTreeStatus::Running
                {
                    thread::sleep(self.stop_policy.poll_interval);
                }
            }
            ProcessTreeStatus::Unknown => return Err(identity_unverified()),
        }
        let _ = signal_group(self.identity.tree_id, "KILL");
        self.helper.wait().map_err(|_| stop_failed())?;
        Ok(())
    }
}

impl Drop for MacOsProcessGroup {
    fn drop(&mut self) {
        if self.helper.try_wait().ok().flatten().is_none() {
            let _ = signal_group(self.identity.tree_id, "KILL");
            let _ = self.helper.wait();
        }
    }
}

fn inspect_process(identity: &ProcessTreeIdentity) -> ProcessTreeStatus {
    let Some((pgid, start)) = process_group_fingerprint(identity.root.pid) else {
        return ProcessTreeStatus::Exited;
    };
    if pgid == identity.tree_id
        && identity.root.token.strip_prefix("macos:") == Some(start.as_str())
        && process_executable(identity.root.pid).as_deref()
            == Some(identity.root.executable.as_path())
    {
        ProcessTreeStatus::Running
    } else {
        ProcessTreeStatus::Unknown
    }
}

fn process_start(pid: u32) -> Option<String> {
    process_group_fingerprint(pid).map(|(_, start)| start)
}

fn process_group_fingerprint(pid: u32) -> Option<(u32, String)> {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pgid=", "-o", "lstart="])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut fields = text.trim().splitn(2, char::is_whitespace);
    Some((
        fields.next()?.parse().ok()?,
        fields.next()?.trim().to_owned(),
    ))
}

fn process_executable(pid: u32) -> Option<PathBuf> {
    let output = Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "txt", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok())
}

fn signal_process(pid: u32, signal: &str) -> Result<(), Problem> {
    let status = Command::new("/bin/kill")
        .args([format!("-{signal}"), pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .map_err(|_| stop_failed())?;
    if status.success() {
        Ok(())
    } else {
        Err(stop_failed())
    }
}

fn signal_group(pgid: u32, signal: &str) -> Result<(), Problem> {
    let status = Command::new("/bin/kill")
        .args([format!("-{signal}"), format!("-{pgid}")])
        .stderr(Stdio::null())
        .status()
        .map_err(|_| stop_failed())?;
    if status.success() {
        Ok(())
    } else {
        Err(stop_failed())
    }
}

fn read_first_line(
    stdout: impl Read + Send + 'static,
    timeout: Duration,
) -> Result<String, Problem> {
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(timeout)
        .map_err(|_| launch_failed())?
        .map_err(|_| launch_failed())
}

fn parse_launch_line(line: &str) -> Option<(u32, u32, String)> {
    let mut fields = line.trim().splitn(4, char::is_whitespace);
    if fields.next()? != "BUGLOGIN" {
        return None;
    }
    Some((
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.trim().to_owned(),
    ))
}

fn executable_invalid() -> Problem {
    problem(
        "runtime_executable_invalid",
        "Runtime executable is invalid",
        400,
    )
}
fn launch_failed() -> Problem {
    problem(
        "process_tree_launch_failed",
        "Process tree launch failed",
        500,
    )
}
fn identity_unverified() -> Problem {
    problem(
        "process_identity_unverified",
        "Process identity could not be verified",
        409,
    )
}
fn status_failed() -> Problem {
    problem(
        "process_tree_status_failed",
        "Process tree status failed",
        500,
    )
}
fn stop_failed() -> Problem {
    problem("process_tree_stop_failed", "Process tree stop failed", 500)
}
fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "browser")
}

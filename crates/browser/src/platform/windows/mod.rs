use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use buglogin_domain::errors::Problem;

use crate::platform::{OwnedProcessTree, ProcessTreeIdentity, ProcessTreeStatus};
use crate::process::ProcessIdentity;

const JOB_HELPER: &str = r#"
$ErrorActionPreference = 'Stop'
$process = $null
try {
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class BugLoginJob {
    const uint KILL_ON_CLOSE = 0x00002000;
    static IntPtr job;
    [StructLayout(LayoutKind.Sequential)] struct IO_COUNTERS { public ulong a,b,c,d,e,f; }
    [StructLayout(LayoutKind.Sequential)] struct BASIC_LIMITS {
        public long a,b; public uint flags; public UIntPtr c,d; public uint e; public UIntPtr f; public uint g,h;
    }
    [StructLayout(LayoutKind.Sequential)] struct EXTENDED_LIMITS {
        public BASIC_LIMITS basic; public IO_COUNTERS io; public UIntPtr a,b,c,d;
    }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetInformationJobObject(IntPtr job, int kind, IntPtr info, uint length);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    public static void Create() {
        job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        var limits = new EXTENDED_LIMITS(); limits.basic.flags = KILL_ON_CLOSE;
        int size = Marshal.SizeOf(limits); IntPtr buffer = Marshal.AllocHGlobal(size);
        try {
            Marshal.StructureToPtr(limits, buffer, false);
            if (!SetInformationJobObject(job, 9, buffer, (uint)size)) throw new Win32Exception(Marshal.GetLastWin32Error());
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    public static void OwnProcess(Process process) {
        if (!AssignProcessToJobObject(job, process.Handle)) throw new Win32Exception(Marshal.GetLastWin32Error());
    }
}
'@
$null = [BugLoginJob]::Create()
$process = Start-Process -FilePath $env:BUGLOGIN_EXECUTABLE -ArgumentList $env:BUGLOGIN_ARGUMENTS -PassThru
$null = [BugLoginJob]::OwnProcess($process)
$ticks = $process.StartTime.ToUniversalTime().Ticks
[Console]::Out.WriteLine("BUGLOGIN {0} {1}", $process.Id, $ticks)
[Console]::Out.Flush()
$ownerPid = [int]$env:BUGLOGIN_OWNER_PID
$ownerTicks = [long]$env:BUGLOGIN_OWNER_TICKS
while (!$process.HasExited) {
    try {
        $owner = Get-Process -Id $ownerPid -ErrorAction Stop
        if ($owner.StartTime.ToUniversalTime().Ticks -ne $ownerTicks) { break }
    } catch { break }
    Start-Sleep -Milliseconds 200
    $process.Refresh()
}
} catch {
    if ($null -ne $process -and !$process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
"#;

#[derive(Clone, Copy, Debug)]
pub struct StopPolicy {
    pub graceful_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for StopPolicy {
    fn default() -> Self {
        Self {
            graceful_timeout: Duration::from_secs(3),
            poll_interval: Duration::from_millis(50),
        }
    }
}

pub struct WindowsJobController {
    powershell: PathBuf,
    taskkill: PathBuf,
    stop_policy: StopPolicy,
}

impl WindowsJobController {
    pub fn system() -> Result<Self, Problem> {
        let root = PathBuf::from(std::env::var_os("SystemRoot").ok_or_else(platform_unavailable)?);
        let powershell = root.join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let taskkill = root.join("System32/taskkill.exe");
        if !powershell.is_file() || !taskkill.is_file() {
            return Err(platform_unavailable());
        }
        Ok(Self {
            powershell,
            taskkill,
            stop_policy: StopPolicy::default(),
        })
    }

    pub fn with_stop_policy(mut self, stop_policy: StopPolicy) -> Self {
        self.stop_policy = stop_policy;
        self
    }

    pub fn launch(&self, executable: &Path, args: &[String]) -> Result<WindowsJobProcess, Problem> {
        let executable = executable
            .canonicalize()
            .map_err(|_| executable_invalid())?;
        let executable_value = executable.to_str().ok_or_else(executable_invalid)?;
        let owner_pid = std::process::id();
        let owner_ticks = query_process(&self.powershell, owner_pid)
            .map(|(_, ticks)| ticks)
            .ok_or_else(identity_unverified)?;
        let mut helper = Command::new(&self.powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                JOB_HELPER,
            ])
            .env("BUGLOGIN_EXECUTABLE", executable_value)
            .env("BUGLOGIN_ARGUMENTS", windows_command_line(args))
            .env("BUGLOGIN_OWNER_PID", owner_pid.to_string())
            .env("BUGLOGIN_OWNER_TICKS", owner_ticks)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| launch_failed())?;
        let stdout = helper.stdout.take().ok_or_else(launch_failed)?;
        let line = match read_first_line(stdout, Duration::from_secs(60)) {
            Ok(line) => line,
            Err(error) => {
                let _ = helper.kill();
                let _ = helper.wait();
                return Err(error);
            }
        };
        let Some((pid, ticks)) = parse_launch_line(&line) else {
            let _ = helper.kill();
            let _ = helper.wait();
            return Err(launch_failed());
        };
        let tree_id = helper.id();
        Ok(WindowsJobProcess {
            helper,
            identity: ProcessTreeIdentity {
                root: ProcessIdentity {
                    pid,
                    executable,
                    token: format!("windows:{ticks}"),
                },
                tree_id,
            },
            powershell: self.powershell.clone(),
            taskkill: self.taskkill.clone(),
            stop_policy: self.stop_policy,
        })
    }

    pub fn inspect(&self, identity: &ProcessTreeIdentity) -> ProcessTreeStatus {
        inspect_process(&self.powershell, identity)
    }

    pub fn terminate(&self, identity: &ProcessTreeIdentity) -> Result<(), Problem> {
        if self.inspect(identity) != ProcessTreeStatus::Running {
            return Err(identity_unverified());
        }
        run_taskkill(&self.taskkill, identity.root.pid, true)
    }
}

pub struct WindowsJobProcess {
    helper: Child,
    identity: ProcessTreeIdentity,
    powershell: PathBuf,
    taskkill: PathBuf,
    stop_policy: StopPolicy,
}

impl WindowsJobProcess {
    fn wait_for_exit(&mut self, timeout: Duration) -> Result<bool, Problem> {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .helper
                .try_wait()
                .map_err(|_| status_failed())?
                .is_some()
            {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(self.stop_policy.poll_interval);
        }
    }

    fn force_close_job(&mut self) -> Result<(), Problem> {
        match self.helper.kill() {
            Ok(()) => {
                self.helper.wait().map_err(|_| stop_failed())?;
                Ok(())
            }
            Err(_) if self.helper.try_wait().ok().flatten().is_some() => Ok(()),
            Err(_) => Err(stop_failed()),
        }
    }
}

impl OwnedProcessTree for WindowsJobProcess {
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
        Ok(inspect_process(&self.powershell, &self.identity))
    }

    fn stop(&mut self) -> Result<(), Problem> {
        if inspect_process(&self.powershell, &self.identity) != ProcessTreeStatus::Running {
            return Err(identity_unverified());
        }
        let _ = run_taskkill(&self.taskkill, self.identity.root.pid, false);
        if self.wait_for_exit(self.stop_policy.graceful_timeout)? {
            return Ok(());
        }
        self.force_close_job()
    }
}

impl Drop for WindowsJobProcess {
    fn drop(&mut self) {
        if self.helper.try_wait().ok().flatten().is_none() {
            let _ = self.helper.kill();
            let _ = self.helper.wait();
        }
    }
}

fn inspect_process(powershell: &Path, identity: &ProcessTreeIdentity) -> ProcessTreeStatus {
    let Some((path, ticks)) = query_process(powershell, identity.root.pid) else {
        return ProcessTreeStatus::Exited;
    };
    if identity.root.token.strip_prefix("windows:") == Some(ticks.as_str())
        && paths_equal(&path, &identity.root.executable)
    {
        ProcessTreeStatus::Running
    } else {
        ProcessTreeStatus::Unknown
    }
}

fn query_process(powershell: &Path, pid: u32) -> Option<(PathBuf, String)> {
    const SCRIPT: &str = "$p=Get-Process -Id ([int]$env:BUGLOGIN_PID) -ErrorAction Stop; [Console]::Out.WriteLine($p.StartTime.ToUniversalTime().Ticks); [Console]::Out.WriteLine($p.Path)";
    let output = run_output(
        Command::new(powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                SCRIPT,
            ])
            .env("BUGLOGIN_PID", pid.to_string()),
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines();
    let ticks = lines.next()?.trim().to_owned();
    let path = PathBuf::from(lines.next()?.trim()).canonicalize().ok()?;
    Some((path, ticks))
}

fn run_taskkill(taskkill: &Path, pid: u32, force: bool) -> Result<(), Problem> {
    let mut command = Command::new(taskkill);
    command.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        command.arg("/F");
    }
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| stop_failed())?;
    if status.success() {
        Ok(())
    } else {
        Err(stop_failed())
    }
}

fn run_output(command: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            let mut stdout = Vec::new();
            child.stdout.take()?.read_to_end(&mut stdout).ok()?;
            let status = child.wait().ok()?;
            return Some(std::process::Output {
                status,
                stdout,
                stderr: Vec::new(),
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
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

fn parse_launch_line(line: &str) -> Option<(u32, String)> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "BUGLOGIN" {
        return None;
    }
    Some((fields.next()?.parse().ok()?, fields.next()?.to_owned()))
}

fn windows_command_line(args: &[String]) -> OsString {
    args.iter()
        .map(|argument| quote_windows_argument(argument))
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

fn quote_windows_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return argument.to_owned();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        quoted.push_str(&"\\".repeat(if character == '"' {
            backslashes * 2 + 1
        } else {
            backslashes
        }));
        backslashes = 0;
        quoted.push(character);
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn platform_unavailable() -> Problem {
    problem(
        "platform_process_control_unavailable",
        "Platform process control is unavailable",
        503,
    )
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

#[cfg(test)]
mod tests {
    use super::quote_windows_argument;

    #[test]
    fn quotes_windows_arguments_without_shell_interpretation() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument(""), "\"\"");
        assert_eq!(quote_windows_argument("two words"), "\"two words\"");
        assert_eq!(quote_windows_argument(r#"a\"b"#), "\"a\\\\\\\"b\"");
        assert_eq!(
            quote_windows_argument(r#"C:\path with space\"#),
            r#""C:\path with space\\""#
        );
    }
}

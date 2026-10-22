use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use buglogin_domain::errors::Problem;
use sha2::{Digest, Sha256};

use crate::platform::{OwnedProcessTree, ProcessTreeIdentity, ProcessTreeStatus};
use crate::process::ProcessIdentity;

const JOB_HELPER: &str = r#"
$ErrorActionPreference = 'Stop'
$process = $null
try {
Add-Type -ReferencedAssemblies @('System.dll', 'System.Drawing.dll') -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Drawing;
using System.Runtime.InteropServices;
public static class BugLoginJob {
    const uint KILL_ON_CLOSE = 0x00002000;
    const uint WM_SETICON = 0x0080;
    static IntPtr job;
    static IntPtr largeIcon;
    static IntPtr smallIcon;
    delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);
    [StructLayout(LayoutKind.Sequential)] struct IO_COUNTERS { public ulong a,b,c,d,e,f; }
    [StructLayout(LayoutKind.Sequential)] struct BASIC_LIMITS {
        public long a,b; public uint flags; public UIntPtr c,d; public uint e; public UIntPtr f; public uint g,h;
    }
    [StructLayout(LayoutKind.Sequential)] struct EXTENDED_LIMITS {
        public BASIC_LIMITS basic; public IO_COUNTERS io; public UIntPtr a,b,c,d;
    }
    [StructLayout(LayoutKind.Sequential)] struct BASIC_ACCOUNTING {
        public long userTime, kernelTime, periodUserTime, periodKernelTime;
        public uint pageFaults, totalProcesses, activeProcesses, terminatedProcesses;
    }
    [StructLayout(LayoutKind.Sequential)] struct PROPERTYKEY { public Guid fmtid; public uint pid; }
    [StructLayout(LayoutKind.Explicit)] struct PROPVARIANT {
        [FieldOffset(0)] public ushort vt;
        [FieldOffset(8)] public IntPtr pointer;
    }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore {
        [PreserveSig] int GetCount(out uint count);
        [PreserveSig] int GetAt(uint index, out PROPERTYKEY key);
        [PreserveSig] int GetValue(ref PROPERTYKEY key, out PROPVARIANT value);
        [PreserveSig] int SetValue(ref PROPERTYKEY key, ref PROPVARIANT value);
        [PreserveSig] int Commit();
    }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetInformationJobObject(IntPtr job, int kind, IntPtr info, uint length);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool QueryInformationJobObject(IntPtr job, int kind, IntPtr info, uint length, IntPtr returned);
    [DllImport("shell32.dll", CharSet=CharSet.Unicode)]
    static extern uint ExtractIconEx(string file, int index, out IntPtr large, out IntPtr small, uint count);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    static extern uint PrivateExtractIcons(string file, int index, int width, int height, out IntPtr icon, out uint iconId, uint count, uint flags);
    [DllImport("user32.dll")]
    static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
    [DllImport("user32.dll")]
    static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")]
    static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")]
    static extern IntPtr SendMessageTimeout(IntPtr window, uint message, IntPtr wParam, IntPtr lParam, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll")]
    static extern bool DestroyIcon(IntPtr icon);
    [DllImport("shell32.dll")]
    static extern int SHGetPropertyStoreForWindow(IntPtr window, ref Guid iid, out IPropertyStore store);
    [DllImport("ole32.dll")]
    static extern int PropVariantClear(ref PROPVARIANT value);
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
    public static uint ActiveProcesses() {
        int size = Marshal.SizeOf(typeof(BASIC_ACCOUNTING)); IntPtr buffer = Marshal.AllocHGlobal(size);
        try {
            if (!QueryInformationJobObject(job, 1, buffer, (uint)size, IntPtr.Zero)) throw new Win32Exception(Marshal.GetLastWin32Error());
            return Marshal.PtrToStructure<BASIC_ACCOUNTING>(buffer).activeProcesses;
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    public static uint[] ProcessIds() {
        const int capacity = 4096;
        int size = 8 + IntPtr.Size * capacity;
        IntPtr buffer = Marshal.AllocHGlobal(size);
        try {
            if (!QueryInformationJobObject(job, 3, buffer, (uint)size, IntPtr.Zero)) throw new Win32Exception(Marshal.GetLastWin32Error());
            uint count = unchecked((uint)Marshal.ReadInt32(buffer, 4));
            if (count > capacity) throw new Win32Exception("The browser process tree exceeds the supported icon update limit.");
            var processIds = new uint[checked((int)count)];
            for (int index = 0; index < processIds.Length; index++) {
                processIds[index] = unchecked((uint)Marshal.ReadIntPtr(buffer, 8 + index * IntPtr.Size).ToInt64());
            }
            return processIds;
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    static string ProfileInitials(string profileName) {
        var words = profileName.Trim().Split(new char[] {' ', '\t', '-', '_'}, StringSplitOptions.RemoveEmptyEntries);
        if (words.Length == 0) return "?";
        if (words.Length > 1) return (words[0][0].ToString() + words[1][0].ToString()).ToUpperInvariant();
        var value = words[0];
        return value.Substring(0, Math.Min(2, value.Length)).ToUpperInvariant();
    }
    static IntPtr CreateProfileIcon(string executable, string profileName) {
        IntPtr sourceHandle = IntPtr.Zero;
        uint iconId;
        if (PrivateExtractIcons(executable, 0, 256, 256, out sourceHandle, out iconId, 1, 0) == 0 || sourceHandle == IntPtr.Zero) {
            IntPtr unusedSmall;
            if (ExtractIconEx(executable, 0, out sourceHandle, out unusedSmall, 1) == 0) return IntPtr.Zero;
            if (unusedSmall != IntPtr.Zero) DestroyIcon(unusedSmall);
        }
        try {
            using (var source = Icon.FromHandle(sourceHandle).ToBitmap())
            using (var bitmap = new Bitmap(256, 256, System.Drawing.Imaging.PixelFormat.Format32bppArgb))
            using (var graphics = Graphics.FromImage(bitmap)) {
                graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
                graphics.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
                graphics.PixelOffsetMode = System.Drawing.Drawing2D.PixelOffsetMode.HighQuality;
                graphics.DrawImage(source, 0, 0, 256, 256);
                const int badgeSize = 92;
                const int badgeX = 256 - badgeSize - 2;
                const int badgeY = 256 - badgeSize - 2;
                using (var border = new SolidBrush(Color.White))
                using (var fill = new SolidBrush(Color.FromArgb(214, 59, 43)))
                using (var font = new Font("Segoe UI", 43, FontStyle.Bold, GraphicsUnit.Pixel))
                using (var text = new SolidBrush(Color.White)) {
                    graphics.FillEllipse(border, badgeX - 4, badgeY - 4, badgeSize + 8, badgeSize + 8);
                    graphics.FillEllipse(fill, badgeX, badgeY, badgeSize, badgeSize);
                    var initials = ProfileInitials(profileName);
                    var measured = graphics.MeasureString(initials, font);
                    graphics.DrawString(initials, font, text, badgeX + (badgeSize - measured.Width) / 2, badgeY + (badgeSize - measured.Height) / 2 - 2);
                }
                return bitmap.GetHicon();
            }
        } finally { DestroyIcon(sourceHandle); }
    }
    public static void ApplyWindowIcon(uint[] processIds, string executable, string profileName) {
        if (largeIcon == IntPtr.Zero || smallIcon == IntPtr.Zero) {
            IntPtr extractedLarge, extractedSmall;
            largeIcon = CreateProfileIcon(executable, profileName);
            if (largeIcon == IntPtr.Zero) {
                if (ExtractIconEx(executable, 0, out extractedLarge, out extractedSmall, 1) == 0) return;
                largeIcon = extractedLarge;
                smallIcon = extractedSmall;
            } else {
                smallIcon = CreateProfileIcon(executable, profileName);
                if (smallIcon == IntPtr.Zero) smallIcon = largeIcon;
            }
        }
        var processSet = new System.Collections.Generic.HashSet<uint>(processIds);
        EnumWindows((window, parameter) => {
            uint ownerProcessId;
            GetWindowThreadProcessId(window, out ownerProcessId);
            if (processSet.Contains(ownerProcessId) && IsWindowVisible(window)) {
                IntPtr result;
                SendMessageTimeout(window, WM_SETICON, new IntPtr(1), largeIcon, 0x0002, 500, out result);
                SendMessageTimeout(window, WM_SETICON, IntPtr.Zero, smallIcon, 0x0002, 500, out result);
                SendMessageTimeout(window, WM_SETICON, new IntPtr(2), smallIcon, 0x0002, 500, out result);
            }
            return true;
        }, IntPtr.Zero);
    }
    static bool ApplyWindowAppUserModelId(IntPtr window, string appUserModelId) {
        var iid = new Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99");
        IPropertyStore store;
        if (SHGetPropertyStoreForWindow(window, ref iid, out store) < 0) return false;
        var key = new PROPERTYKEY {
            fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"),
            pid = 5
        };
        var value = new PROPVARIANT { vt = 31, pointer = Marshal.StringToCoTaskMemUni(appUserModelId) };
        try { return store.SetValue(ref key, ref value) >= 0 && store.Commit() >= 0; }
        finally { PropVariantClear(ref value); Marshal.ReleaseComObject(store); }
    }
    public static void ApplyWindowIdentity(uint[] processIds, string executable, string appUserModelId, string profileName) {
        ApplyWindowIcon(processIds, executable, profileName);
        var processSet = new System.Collections.Generic.HashSet<uint>(processIds);
        EnumWindows((window, parameter) => {
            uint ownerProcessId;
            GetWindowThreadProcessId(window, out ownerProcessId);
            if (processSet.Contains(ownerProcessId) && IsWindowVisible(window)) {
                ApplyWindowAppUserModelId(window, appUserModelId);
            }
            return true;
        }, IntPtr.Zero);
    }
    public static void ReleaseWindowIcons() {
        if (largeIcon != IntPtr.Zero) DestroyIcon(largeIcon);
        if (smallIcon != IntPtr.Zero && smallIcon != largeIcon) DestroyIcon(smallIcon);
        largeIcon = IntPtr.Zero;
        smallIcon = IntPtr.Zero;
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
while ([BugLoginJob]::ActiveProcesses() -gt 0) {
    try {
        $owner = Get-Process -Id $ownerPid -ErrorAction Stop
        if ($owner.StartTime.ToUniversalTime().Ticks -ne $ownerTicks) { break }
    } catch { break }
    if ($env:BUGLOGIN_OWNER_EXECUTABLE) {
        try { [BugLoginJob]::ApplyWindowIdentity([BugLoginJob]::ProcessIds(), $env:BUGLOGIN_OWNER_EXECUTABLE, $env:BUGLOGIN_APP_USER_MODEL_ID, $env:BUGLOGIN_PROFILE_NAME) } catch { }
    }
    Start-Sleep -Milliseconds 200
    $process.Refresh()
}
[BugLoginJob]::ReleaseWindowIcons()
} catch {
    if ($null -ne $process -and !$process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
"#;

fn profile_app_user_model_id(profile_path: &Path) -> String {
    let normalized = profile_path.to_string_lossy().to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("BugLogin.Wayfern.{suffix}")
}

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

    pub fn launch(
        &self,
        executable: &Path,
        args: &[String],
        use_buglogin_window_icon: bool,
        profile_path: &Path,
    ) -> Result<WindowsJobProcess, Problem> {
        let executable = executable
            .canonicalize()
            .map_err(|_| executable_invalid())?;
        let executable_value = executable.to_str().ok_or_else(executable_invalid)?;
        let owner_pid = std::process::id();
        let owner_ticks = query_process(&self.powershell, owner_pid)
            .map(|(_, ticks)| ticks)
            .ok_or_else(identity_unverified)?;
        let owner_executable = if use_buglogin_window_icon {
            std::env::current_exe()
                .ok()
                .and_then(|path| path.to_str().map(str::to_owned))
                .unwrap_or_default()
        } else {
            String::new()
        };
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
            .env("BUGLOGIN_OWNER_EXECUTABLE", owner_executable)
            .env(
                "BUGLOGIN_APP_USER_MODEL_ID",
                profile_app_user_model_id(profile_path),
            )
            .env(
                "BUGLOGIN_PROFILE_NAME",
                args.iter()
                    .find_map(|argument| argument.strip_prefix("--wayfern-profile-label="))
                    .unwrap_or("Browser"),
            )
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
        // The root executable can exit after handing control to browser child
        // processes. The helper stays alive while the Job Object owns any process.
        Ok(ProcessTreeStatus::Running)
    }

    fn stop(&mut self) -> Result<(), Problem> {
        if self
            .helper
            .try_wait()
            .map_err(|_| status_failed())?
            .is_some()
        {
            // The helper owns the Job Object with KILL_ON_CLOSE. Once it exits,
            // the owned process tree is already gone and stopping is complete.
            return Ok(());
        }
        if inspect_process(&self.powershell, &self.identity) == ProcessTreeStatus::Running {
            let _ = run_taskkill(&self.taskkill, self.identity.root.pid, false);
        }
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
    use super::{profile_app_user_model_id, quote_windows_argument};
    use std::path::Path;

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

    #[test]
    fn wayfern_app_user_model_id_is_stable_per_profile_without_exposing_path() {
        let first = profile_app_user_model_id(Path::new(r"C:\Profiles\profile-one"));
        let same_profile = profile_app_user_model_id(Path::new(r"c:\profiles\PROFILE-ONE"));
        let second = profile_app_user_model_id(Path::new(r"C:\Profiles\profile-two"));

        assert_eq!(first, same_profile);
        assert_ne!(first, second);
        assert!(first.starts_with("BugLogin.Wayfern."));
        assert!(!first.contains("profile-one"));
    }
}

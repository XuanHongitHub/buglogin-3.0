//! Opt-in native configuration transport for the independent Chromium backend.
//! The ordinary Wayfern job helper is left byte-for-byte unchanged.

use buglogin_domain::errors::Problem;

const START: &str = "$process = Start-Process -FilePath $env:BUGLOGIN_EXECUTABLE -ArgumentList $launchArguments -PassThru";
const METHOD: &str = "    public static void Create() {";

pub(super) fn configured_helper(original: &str) -> Result<String, Problem> {
    if original.matches(START).count() != 1 || original.matches(METHOD).count() != 1 {
        return Err(Problem::new(
            "fingerprint_transport_unavailable",
            "Native fingerprint transport is unavailable",
            503,
            "browser-process",
        ));
    }
    Ok(original
        .replacen(METHOD, &format!("{NATIVE_METHODS}\n{METHOD}"), 1)
        .replacen(
            START,
            "$process = [BugLoginJob]::StartWithFingerprint($env:BUGLOGIN_EXECUTABLE, $launchArguments, $env:BUGLOGIN_FINGERPRINT_FILE)",
            1,
        ))
}

// Only the read-only configuration handle is inherited. The browser starts
// suspended and joins the existing kill-on-close job before any child can run.
// No configuration contents or secret are passed in argv or the environment.
const NATIVE_METHODS: &str = r#"
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct STARTUPINFO {
        public uint cb; public IntPtr reserved, desktop, title;
        public uint x,y,xSize,ySize,xChars,yChars,fill,flags;
        public ushort showWindow, reservedSize;
        public IntPtr reservedBytes, input, output, error;
    }
    [StructLayout(LayoutKind.Sequential)] struct STARTUPINFOEX {
        public STARTUPINFO startup; public IntPtr attributes;
    }
    [StructLayout(LayoutKind.Sequential)] struct PROCESS_INFORMATION {
        public IntPtr process, thread; public uint pid, tid;
    }
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, uint flags, ref UIntPtr size);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute, IntPtr value, UIntPtr size, IntPtr previous, IntPtr returned);
    [DllImport("kernel32.dll")]
    static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetHandleInformation(IntPtr handle, uint mask, uint flags);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool CreateProcessW(string application, System.Text.StringBuilder command, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint flags, IntPtr environment, string directory, ref STARTUPINFOEX startup, out PROCESS_INFORMATION information);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr handle);

    public static Process StartWithFingerprint(string executable, string arguments, string file) {
        using (var input = new System.IO.FileStream(file, System.IO.FileMode.Open, System.IO.FileAccess.Read, System.IO.FileShare.Read)) {
            IntPtr handle = input.SafeFileHandle.DangerousGetHandle();
            if (!SetHandleInformation(handle, 1, 1)) throw new Win32Exception(Marshal.GetLastWin32Error());
            UIntPtr size = UIntPtr.Zero;
            InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
            if (size == UIntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            IntPtr attributes = Marshal.AllocHGlobal(checked((int)size.ToUInt64()));
            IntPtr handles = Marshal.AllocHGlobal(IntPtr.Size);
            bool initialized = false;
            var info = new PROCESS_INFORMATION();
            try {
                if (!InitializeProcThreadAttributeList(attributes, 1, 0, ref size)) throw new Win32Exception(Marshal.GetLastWin32Error());
                initialized = true;
                Marshal.WriteIntPtr(handles, handle);
                if (!UpdateProcThreadAttribute(attributes, 0, new IntPtr(0x20002), handles, (UIntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero)) throw new Win32Exception(Marshal.GetLastWin32Error());
                var startup = new STARTUPINFOEX();
                startup.startup.cb = (uint)Marshal.SizeOf(typeof(STARTUPINFOEX));
                startup.attributes = attributes;
                uint slot = unchecked((uint)handle.ToInt64());
                var command = new System.Text.StringBuilder("\"" + executable + "\" " + arguments + " --fury-fp-file-handle=" + slot.ToString(System.Globalization.CultureInfo.InvariantCulture));
                const uint flags = 0x00080000 | 0x00000004 | 0x08000000;
                if (!CreateProcessW(executable, command, IntPtr.Zero, IntPtr.Zero, true, flags, IntPtr.Zero, null, ref startup, out info)) throw new Win32Exception(Marshal.GetLastWin32Error());
                if (!AssignProcessToJobObject(job, info.process)) throw new Win32Exception(Marshal.GetLastWin32Error());
                var process = Process.GetProcessById(checked((int)info.pid));
                if (ResumeThread(info.thread) == UInt32.MaxValue) throw new Win32Exception(Marshal.GetLastWin32Error());
                return process;
            } catch {
                if (info.process != IntPtr.Zero) TerminateProcess(info.process, 1);
                throw;
            } finally {
                if (info.thread != IntPtr.Zero) CloseHandle(info.thread);
                if (info.process != IntPtr.Zero) CloseHandle(info.process);
                if (initialized) DeleteProcThreadAttributeList(attributes);
                Marshal.FreeHGlobal(handles);
                Marshal.FreeHGlobal(attributes);
                SetHandleInformation(handle, 1, 0);
            }
        }
    }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_helper_requires_exact_transport_insertion_points() {
        assert!(configured_helper("unrelated helper").is_err());
        let source = format!("{METHOD}\n{START}");
        let configured = configured_helper(&source).unwrap();
        assert!(!configured.contains(START));
        assert!(configured.contains("StartWithFingerprint($env:BUGLOGIN_EXECUTABLE"));
        assert!(configured.contains("AssignProcessToJobObject(job, info.process)"));
        assert!(configured_helper(&format!("{source}\n{START}")).is_err());
    }
}

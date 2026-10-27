//! Disposable native probe used by the independent Chromium smoke harness.
use buglogin_browser::{
    process::{ApprovedExecutable, ProcessController, ProcessStatus, SystemProcessController},
    runtime::{
        RuntimeAdapter, RuntimeBinary,
        independent_chromium::{ChromiumIdentity, IndependentChromiumAdapter},
    },
};
use buglogin_domain::entities::{FingerprintOperatingSystem, ProfileFingerprintTarget};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let binary_path = PathBuf::from(args.next().ok_or("missing binary path")?);
    let version = args.next().ok_or("missing version")?;
    let checksum = args.next().ok_or("missing executable checksum")?;
    let profile_path = PathBuf::from(args.next().ok_or("missing disposable profile path")?);
    fs::create_dir_all(&profile_path)?;
    let identity = match ChromiumIdentity::load(&profile_path)? {
        Some(identity) => identity,
        None => ChromiumIdentity::generate(
            version.clone(),
            ProfileFingerprintTarget {
                operating_system: if cfg!(target_os = "windows") {
                    FingerprintOperatingSystem::Windows
                } else if cfg!(target_os = "macos") {
                    FingerprintOperatingSystem::Macos
                } else {
                    FingerprintOperatingSystem::Linux
                },
                operating_system_version: None,
            },
        )?,
    };
    let adapter = IndependentChromiumAdapter::new(identity)?;
    let binary = adapter.validate_binary(&RuntimeBinary {
        path: binary_path.clone(),
        version,
        expected_checksum: checksum,
    })?;
    let specification =
        adapter.launch_specification(&binary, &profile_path, &args.collect::<Vec<_>>())?;
    let controller = SystemProcessController::new([ApprovedExecutable::new(&binary_path)?]);
    let mut child = controller.launch(&specification)?;
    let initial_status = child.status()?;
    #[cfg(target_os = "macos")]
    if initial_status == ProcessStatus::Unknown {
        for (program, arguments) in [
            (
                "/bin/ps",
                vec![
                    "-p".to_owned(),
                    child.identity().pid.to_string(),
                    "-o".into(),
                    "comm=".into(),
                    "-o".into(),
                    "stat=".into(),
                    "-o".into(),
                    "pgid=".into(),
                ],
            ),
            (
                "/usr/sbin/lsof",
                vec![
                    "-a".to_owned(),
                    "-p".into(),
                    child.identity().pid.to_string(),
                    "-d".into(),
                    "txt".into(),
                    "-Fn".into(),
                ],
            ),
        ] {
            let output = std::process::Command::new(program)
                .args(arguments)
                .output()?;
            eprintln!(
                "native identity diagnostic {program}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
    println!(
        "{}",
        serde_json::json!({"pid": child.identity().pid, "status": "launched", "processState": format!("{initial_status:?}")})
    );
    io::stdout().flush()?;
    let mut command = String::new();
    let _ = io::stdin().read_line(&mut command);
    if command.trim() == "closed" {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && child.status()? != ProcessStatus::Exited {
            thread::sleep(Duration::from_millis(50));
        }
    }
    child.stop()?;
    if child.status()? != ProcessStatus::Exited {
        return Err("native process tree did not stop".into());
    }
    println!("{}", serde_json::json!({"status": "stopped"}));
    Ok(())
}

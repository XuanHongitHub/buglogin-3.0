# Windows profile process isolation design

## Required behavior

Stopping one BugLogin profile must terminate only the browser process tree that was launched for that profile. Other running profiles must remain alive, retain their process identity and lock, and continue to be usable. A stop result is successful only after the requested tree is confirmed stopped.

## Authority and boundaries

- Rust process and lifecycle services own launch, process-tree containment, stop, and profile-lock state.
- Each launched profile receives an independent Windows Job Object with kill-on-close behavior.
- Profile IDs and canonical profile paths select the process tree to stop.
- Tests must distinguish the helper process, browser root process, and descendants; they must not treat a transient root PID as proof that the whole tree is alive or dead.
- This packet covers the reproduced Windows multi-profile lifecycle defect. It does not promote a release or change stable feeds.

## Acceptance evidence

- A real Wayfern Windows test launches two isolated profile directories, proves both process trees are alive, stops A, proves every A process is gone and B's root plus at least one owned browser process remains alive, then stops B and verifies cleanup.
- The same test is repeatable without leaking processes, helpers, profile locks, or temporary data.
- `cargo test -p buglogin-browser --test platform_windows -- --test-threads=1` and the targeted desktop native lifecycle test pass in strict mode with the installed Wayfern binary.
- The test result is not a macOS or release-readiness claim.

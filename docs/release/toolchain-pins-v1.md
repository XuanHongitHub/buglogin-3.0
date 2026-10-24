# Toolchain Pins v1

These are the implementation baseline pins. CI must fail on drift.

| Component | Pin | Scope |
|---|---|---|
| Node.js | `24.18.0` | Desktop, web, scripts |
| pnpm | `11.10.0` | All JavaScript workspaces |
| Rust | `1.95.0` | All Rust crates and binaries |
| Windows runner | `windows-2025` | Windows x64 build/smoke |
| macOS Apple Silicon runner | `macos-14` | macOS arm64 build/smoke |
| macOS Intel runner | `macos-15-intel` | macOS x86_64 build/smoke |

Runner labels are the CI contract. Availability, image digest, SDK/toolchain contents, and signing capacity remain infrastructure prerequisites and must be recorded by the release owner before platform implementation.

The checked-in upstream baseline is DonutBrowser `0.34.0`; its package manager declaration is pnpm `12.8.1`, which is reference evidence, not a BugLogin toolchain pin.

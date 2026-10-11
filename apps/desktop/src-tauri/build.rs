fn main() {
    println!(
        "cargo:rustc-env=BUGLOGIN_TARGET_TRIPLE={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    tauri_build::build();
}

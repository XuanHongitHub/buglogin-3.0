fn main() {
    println!("cargo:rerun-if-changed=runtime-trust-keys.json");
    println!("cargo:rerun-if-env-changed=BUGLOGIN_RUNTIME_TRUST_KEY_ID");
    println!("cargo:rerun-if-env-changed=BUGLOGIN_RUNTIME_PUBLIC_KEY_HEX");
    println!("cargo:rerun-if-env-changed=BUGLOGIN_RUNTIME_TRUST_KEYS_JSON");
    println!(
        "cargo:rustc-env=BUGLOGIN_TARGET_TRIPLE={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    tauri_build::build();
}

fn main() {
    // These values are consumed through `option_env!` in cutover.rs. Cargo does
    // not otherwise know that changing them must invalidate this crate, so an
    // incremental desktop build could silently reuse a Production-class runner
    // after DUCKLE_ENTRY_POINT_CLASS was changed to `test` (or vice versa).
    println!("cargo:rerun-if-env-changed=DUCKLE_ENTRY_POINT_CLASS");
    println!("cargo:rerun-if-env-changed=DUCKLE_CUTOVER_EVIDENCE_JSON");

    let entry_point = std::env::var("DUCKLE_ENTRY_POINT_CLASS")
        .unwrap_or_else(|_| "production".to_string());
    match entry_point.trim().to_ascii_lowercase().as_str() {
        "production" | "release-ci" | "release_ci" | "test" | "compatibility" => {}
        value => panic!("invalid DUCKLE_ENTRY_POINT_CLASS: {value}"),
    }

    // Kept visible in Cargo/Tauri logs so a packaged executable can never be
    // mistaken for a Quack test build merely because the shell variable was set.
    println!(
        "cargo:warning=duckle-db-runner compiled entry point class: {}",
        entry_point.trim()
    );
}

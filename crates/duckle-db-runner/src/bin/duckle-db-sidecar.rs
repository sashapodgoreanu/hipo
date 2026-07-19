//! Trusted local Quack sidecar entrypoint.

#[cfg(windows)]
fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if duckle_db_runner::local_quack_sidecar::run_windows_sidecar(&args).is_err() {
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    // Unix inherited-pipe containment is intentionally not implemented yet.
    // Keep the binary inert rather than accepting a weaker bootstrap channel.
    std::process::exit(1);
}

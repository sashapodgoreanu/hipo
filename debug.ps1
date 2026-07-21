# Launch Duckle in Tauri development mode with verbose Rust diagnostics.
#
# This deliberately delegates to dev.ps1 so the normal Vite + Tauri startup
# contract remains the single source of truth. Use this script when attaching
# the Visual Studio Code native debugger to duckle.exe or duckle-db-sidecar.exe.
$ErrorActionPreference = 'Stop'

if (-not $env:RUST_BACKTRACE) {
    $env:RUST_BACKTRACE = 'full'
}
if (-not $env:RUST_LOG) {
    $env:RUST_LOG = 'duckle_desktop=debug,duckle_duckdb_engine=trace,duckle_db_runner=trace,info'
}

& "$PSScriptRoot\dev.ps1"

fn main() {
    // Quack is provisioned by Duckle's DuckDB CLI installation flow together
    // with the other extensions used by the product. The database sidecar loads
    // that installed extension at runtime and lets DuckDB perform its own binary
    // compatibility checks.
    println!("cargo:rerun-if-changed=build.rs");
}

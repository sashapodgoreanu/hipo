fn main() {
    // Cutover evidence is retained as release documentation only. Changing it
    // must still invalidate the crate so packaged metadata stays current.
    println!("cargo:rerun-if-env-changed=DUCKLE_CUTOVER_EVIDENCE_JSON");
    println!("cargo:warning=duckle-db-runner runtime: quack");
}

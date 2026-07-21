fn main() {
    // These values are consumed through `option_env!` in cutover.rs. Cargo does
    // not otherwise know that changing them must invalidate this crate, so an
    // incremental desktop build could silently reuse a Production-class runner
    // after DUCKLE_ENTRY_POINT_CLASS was changed to `test` (or vice versa).
    println!("cargo:rerun-if-env-changed=DUCKLE_ENTRY_POINT_CLASS");
    println!("cargo:rerun-if-env-changed=DUCKLE_CUTOVER_EVIDENCE_JSON");
}

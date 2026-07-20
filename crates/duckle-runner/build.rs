use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const QUACK_EXTENSION_FILE: &str = "quack.duckdb_extension";
const QUACK_VERSION: &str = "1.5.4";
const DUCKDB_VERSION: &str = "1.5.4";
const QUACK_WINDOWS_AMD64_SHA256: &str =
    "3274bac6becc0f750497726a73f9ae858606cec7ec1a935d83a5b84ee0402122";
const QUACK_MACOS_AMD64_SHA256: &str =
    "85a48992d0b940f7cf1c55bbe4efd02f46c9724b67e238a990df3f3244d8e970";
const QUACK_LINUX_AMD64_SHA256: &str =
    "decb78a4d953ff9cc65c300cf2c8d3f3d8f4732851205684565c922113bc2b9e";

fn expected_sha256(target_os: &str, target_arch: &str) -> Option<&'static str> {
    match (target_os, target_arch) {
        ("windows", "x86_64") => Some(QUACK_WINDOWS_AMD64_SHA256),
        ("macos", "x86_64") => Some(QUACK_MACOS_AMD64_SHA256),
        ("linux", "x86_64") => Some(QUACK_LINUX_AMD64_SHA256),
        _ => None,
    }
}

fn profile_dir(out_dir: &Path) -> Option<PathBuf> {
    // OUT_DIR is target/<target?>/<profile>/build/<package-hash>/out.
    out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

fn staged_candidates(manifest_dir: &Path, out_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("DUCKLE_QUACK_EXTENSION") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(manifest_dir.join("bin").join(QUACK_EXTENSION_FILE));
    candidates.push(
        manifest_dir
            .join("..")
            .join("..")
            .join("apps")
            .join("desktop")
            .join("bin")
            .join(QUACK_EXTENSION_FILE),
    );
    if let Some(profile) = profile_dir(out_dir) {
        candidates.push(profile.join(QUACK_EXTENSION_FILE));
    }
    candidates
}

fn verify(path: &Path, expected: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        return Err(format!(
            "Quack extension checksum mismatch for {}: expected {}, got {}",
            path.display(), expected, actual
        ));
    }
    Ok(bytes)
}

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let embedded = out_dir.join("embedded-quack-extension.bin");

    println!("cargo:rerun-if-env-changed=DUCKLE_QUACK_EXTENSION");
    for candidate in staged_candidates(&manifest_dir, &out_dir) {
        println!("cargo:rerun-if-changed={}", candidate.display());
    }

    let expected = expected_sha256(&target_os, &target_arch);
    let source = staged_candidates(&manifest_dir, &out_dir)
        .into_iter()
        .find(|candidate| candidate.is_file());

    match (expected, source) {
        (Some(expected), Some(source)) => {
            let bytes = verify(&source, expected)
                .unwrap_or_else(|error| panic!("official runner staging rejected: {error}"));
            std::fs::write(&embedded, &bytes)
                .unwrap_or_else(|error| panic!("write {}: {error}", embedded.display()));

            // Keep the verified pair adjacent in target/<profile>. Desktop's
            // build script and release staging can locate both binaries without
            // consulting PATH or the network.
            if let Some(profile) = profile_dir(&out_dir) {
                let adjacent = profile.join(QUACK_EXTENSION_FILE);
                if adjacent != source {
                    std::fs::write(&adjacent, &bytes).unwrap_or_else(|error| {
                        panic!("write verified {}: {error}", adjacent.display())
                    });
                }
            }
        }
        (Some(_), None) => {
            std::fs::write(&embedded, [])
                .unwrap_or_else(|error| panic!("write {}: {error}", embedded.display()));
            println!(
                "cargo:warning={} not staged; duckle-db-sidecar will report runner_unavailable until the verified offline extension is supplied",
                QUACK_EXTENSION_FILE
            );
        }
        (None, Some(source)) => {
            panic!(
                "no approved DuckDB/Quack bundle for {}-{}, but {} was staged",
                target_os,
                target_arch,
                source.display()
            );
        }
        (None, None) => {
            std::fs::write(&embedded, [])
                .unwrap_or_else(|error| panic!("write {}: {error}", embedded.display()));
            println!(
                "cargo:warning=official runner is unavailable on unsupported target {}-{}",
                target_os, target_arch
            );
        }
    }

    println!("cargo:rustc-env=DUCKLE_EMBEDDED_QUACK_EXTENSION={}", embedded.display());
    println!("cargo:rustc-env=DUCKLE_RUNNER_DUCKDB_VERSION={DUCKDB_VERSION}");
    println!("cargo:rustc-env=DUCKLE_QUACK_VERSION={QUACK_VERSION}");
    println!("cargo:rustc-env=DUCKLE_QUACK_EXTENSION_FILE={QUACK_EXTENSION_FILE}");
}

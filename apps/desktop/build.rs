mod legacy {
    include!("build_base.rs");

    pub fn embed_common_sidecars() {
        embed_runner();
        embed_runner_linux();
        embed_mcp();
        embed_lance();
    }

    pub fn compress(src: &std::path::Path, dst: &std::path::Path) {
        compress_to(src, dst);
    }

    pub fn verify_quack(
        extension: &std::path::Path,
        target_os: &str,
        target_arch: &str,
    ) -> Result<&'static str, String> {
        verify_staged_quack_extension(extension, target_os, target_arch)
    }

    pub fn write_pin_manifest(
        out_dir: &std::path::Path,
        sha256: &str,
    ) -> std::path::PathBuf {
        write_runner_pin_manifest(out_dir, sha256)
    }

    pub fn finish_tauri_build() {
        tauri_build::build();
    }
}

const RUNNER_DUCKDB_VERSION: &str = "1.5.4";
const QUACK_VERSION: &str = "1.5.4";
const QUACK_LICENSE: &str = "MIT";
const QUACK_PROVENANCE: &str = "duckdb/duckdb-quack";
const QUACK_EXTENSION_FILE: &str = "quack.duckdb_extension";

fn main() {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=DUCKLE_BUILD_EPOCH={epoch}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_base.rs");
    println!("cargo:rerun-if-changed=.duckle-always-restamp-build-epoch");

    legacy::embed_common_sidecars();
    embed_db_sidecar_pair();
    legacy::finish_tauri_build();
}

fn write_empty(path: &std::path::Path) {
    std::fs::write(path, [])
        .unwrap_or_else(|error| panic!("write empty {}: {error}", path.display()));
}

fn embed_db_sidecar_pair() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let sidecar_name = if target_os == "windows" {
        "duckle-db-sidecar.exe"
    } else {
        "duckle-db-sidecar"
    };

    let staged_dir = manifest_dir.join("bin");
    let profile_dir = out_dir.ancestors().nth(3).map(std::path::Path::to_path_buf);
    let staged_sidecar = staged_dir.join(sidecar_name);
    let staged_extension = staged_dir.join(QUACK_EXTENSION_FILE);

    println!("cargo:rerun-if-changed={}", staged_sidecar.display());
    println!("cargo:rerun-if-changed={}", staged_extension.display());
    if let Some(profile) = &profile_dir {
        println!("cargo:rerun-if-changed={}", profile.join(sidecar_name).display());
        println!(
            "cargo:rerun-if-changed={}",
            profile.join(QUACK_EXTENSION_FILE).display()
        );
    }

    let pair = if staged_sidecar.is_file() {
        if !staged_extension.is_file() {
            panic!(
                "explicit desktop staging is incomplete: apps/desktop/bin/{sidecar_name} requires apps/desktop/bin/{QUACK_EXTENSION_FILE}"
            );
        }
        Some((staged_sidecar, staged_extension))
    } else if let Some(profile) = profile_dir {
        let sidecar = profile.join(sidecar_name);
        let extension = profile.join(QUACK_EXTENSION_FILE);
        if sidecar.is_file() && extension.is_file() {
            Some((sidecar, extension))
        } else {
            if sidecar.is_file() || extension.is_file() {
                println!(
                    "cargo:warning=ignoring incomplete local official-runner fallback in {}; sidecar and Quack extension must be adjacent",
                    profile.display()
                );
            }
            None
        }
    } else {
        None
    };

    let embedded_sidecar = out_dir.join("embedded-db-sidecar.bin");
    let embedded_extension = out_dir.join("embedded-quack-extension.bin");
    let pin_manifest = out_dir.join("official-runner-pin.json");

    match pair {
        Some((sidecar, extension)) => {
            let checksum = legacy::verify_quack(&extension, &target_os, &target_arch)
                .unwrap_or_else(|error| panic!("official runner staging rejected: {error}"));
            legacy::compress(&sidecar, &embedded_sidecar);
            legacy::compress(&extension, &embedded_extension);
            let manifest = legacy::write_pin_manifest(&out_dir, checksum);
            println!(
                "cargo:rustc-env=DUCKLE_OFFICIAL_RUNNER_PIN={}",
                manifest.display()
            );
            println!(
                "cargo:warning=packaged verified official runner pair from {}",
                sidecar.parent().unwrap_or(&sidecar).display()
            );
        }
        None => {
            write_empty(&embedded_sidecar);
            write_empty(&embedded_extension);
            write_empty(&pin_manifest);
            println!(
                "cargo:rustc-env=DUCKLE_OFFICIAL_RUNNER_PIN={}",
                pin_manifest.display()
            );
            println!(
                "cargo:warning=verified duckle-db-sidecar/Quack pair not staged; official runner remains unavailable"
            );
        }
    }

    println!(
        "cargo:rustc-env=DUCKLE_EMBEDDED_DB_SIDECAR={}",
        embedded_sidecar.display()
    );
    println!(
        "cargo:rustc-env=DUCKLE_EMBEDDED_QUACK_EXTENSION={}",
        embedded_extension.display()
    );
    println!("cargo:rustc-env=DUCKLE_RUNNER_DUCKDB_VERSION={RUNNER_DUCKDB_VERSION}");
    println!("cargo:rustc-env=DUCKLE_QUACK_VERSION={QUACK_VERSION}");
    println!("cargo:rustc-env=DUCKLE_QUACK_LICENSE={QUACK_LICENSE}");
    println!("cargo:rustc-env=DUCKLE_QUACK_PROVENANCE={QUACK_PROVENANCE}");
}

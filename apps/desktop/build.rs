// Shared packaging helpers retained until the final source-file cleanup.
#[allow(dead_code)]
mod packaging {
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
        const WINDOWS_AMD64: &str =
            "3274bac6becc0f750497726a73f9ae858606cec7ec1a935d83a5b84ee0402122";
        const MACOS_AMD64: &str =
            "85a48992d0b940f7cf1c55bbe4efd02f46c9724b67e238a990df3f3244d8e970";
        const LINUX_AMD64: &str =
            "decb78a4d953ff9cc65c300cf2c8d3f3d8f4732851205684565c922113bc2b9e";

        let expected = match (target_os, target_arch) {
            ("windows", "x86_64") => WINDOWS_AMD64,
            ("macos", "x86_64") => MACOS_AMD64,
            ("linux", "x86_64") => LINUX_AMD64,
            _ => {
                return Err(format!(
                    "no verified Quack {} bundle for {}-{}",
                    QUACK_VERSION, target_os, target_arch
                ))
            }
        };

        let bytes = std::fs::read(extension)
            .map_err(|error| format!("read staged {}: {error}", extension.display()))?;
        let actual = format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(&bytes)
        );
        if actual != expected {
            return Err(format!(
                "staged Quack checksum mismatch for {}-{}: expected {}, got {}",
                target_os, target_arch, expected, actual
            ));
        }
        Ok(expected)
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

// Bump when the staged sidecar contract changes so an installed desktop app
// cannot retain a same-sized older executable in AppData.
const DB_SIDECAR_PACKAGE_REVISION: usize = 1;

fn main() {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=DUCKLE_BUILD_EPOCH={epoch}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_base.rs");
    println!("cargo:rerun-if-changed=.duckle-always-restamp-build-epoch");

    packaging::embed_common_sidecars();
    embed_db_sidecar_pair();
    packaging::finish_tauri_build();
}

fn compress_db_sidecar(source: &std::path::Path, destination: &std::path::Path) {
    let mut bytes = std::fs::read(source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    bytes.extend(std::iter::repeat_n(
        0_u8,
        DB_SIDECAR_PACKAGE_REVISION,
    ));
    let compressed = zstd::encode_all(std::io::Cursor::new(bytes), 19)
        .unwrap_or_else(|error| panic!("zstd compress {}: {error}", source.display()));
    std::fs::write(destination, compressed)
        .unwrap_or_else(|error| panic!("write {}: {error}", destination.display()));
}

fn pair_in_directory(
    directory: &std::path::Path,
    sidecar_name: &str,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let sidecar = directory.join(sidecar_name);
    let extension = directory.join(QUACK_EXTENSION_FILE);
    println!("cargo:rerun-if-changed={}", sidecar.display());
    println!("cargo:rerun-if-changed={}", extension.display());

    if sidecar.is_file() && extension.is_file() {
        Some((sidecar, extension))
    } else {
        if sidecar.is_file() || extension.is_file() {
            println!(
                "cargo:warning=ignoring incomplete Quack runner pair in {}; sidecar and extension must be adjacent",
                directory.display()
            );
        }
        None
    }
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
    let active_profile_dir = out_dir.ancestors().nth(3).map(std::path::Path::to_path_buf);
    let release_profile_dir = active_profile_dir
        .as_ref()
        .and_then(|profile| profile.parent())
        .map(|target| target.join("release"));

    let staged_sidecar = staged_dir.join(sidecar_name);
    let staged_extension = staged_dir.join(QUACK_EXTENSION_FILE);
    println!("cargo:rerun-if-changed={}", staged_sidecar.display());
    println!("cargo:rerun-if-changed={}", staged_extension.display());

    let pair = if staged_sidecar.is_file() {
        if !staged_extension.is_file() {
            panic!(
                "desktop staging is incomplete: apps/desktop/bin/{sidecar_name} requires apps/desktop/bin/{QUACK_EXTENSION_FILE}"
            );
        }
        Some((staged_sidecar, staged_extension))
    } else {
        [active_profile_dir, release_profile_dir]
            .into_iter()
            .flatten()
            .find_map(|directory| pair_in_directory(&directory, sidecar_name))
    }
    .unwrap_or_else(|| {
        panic!(
            "the desktop app requires {sidecar_name} and {QUACK_EXTENSION_FILE} as an adjacent verified pair in apps/desktop/bin, the active Cargo profile, or target/release"
        )
    });

    let embedded_sidecar = out_dir.join("embedded-db-sidecar.bin");
    let embedded_extension = out_dir.join("embedded-quack-extension.bin");
    let (sidecar, extension) = pair;
    let checksum = packaging::verify_quack(&extension, &target_os, &target_arch)
        .unwrap_or_else(|error| panic!("Quack runner staging rejected: {error}"));
    compress_db_sidecar(&sidecar, &embedded_sidecar);
    packaging::compress(&extension, &embedded_extension);
    let manifest = packaging::write_pin_manifest(&out_dir, checksum);

    println!(
        "cargo:rustc-env=DUCKLE_OFFICIAL_RUNNER_PIN={}",
        manifest.display()
    );
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
    println!("cargo:warning=desktop database runtime: quack");
}

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

    pub fn finish_tauri_build() {
        tauri_build::build();
    }
}

// Bump when the staged sidecar contract changes so an installed desktop app
// cannot retain a same-sized older executable in AppData.
const DB_SIDECAR_PACKAGE_REVISION: usize = 2;

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
    embed_db_sidecar();
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

fn embed_db_sidecar() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
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
    println!("cargo:rerun-if-changed={}", staged_sidecar.display());

    let sidecar = if staged_sidecar.is_file() {
        staged_sidecar
    } else {
        [active_profile_dir, release_profile_dir]
            .into_iter()
            .flatten()
            .map(|directory| directory.join(sidecar_name))
            .inspect(|candidate| println!("cargo:rerun-if-changed={}", candidate.display()))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| {
                panic!(
                    "the desktop app requires {sidecar_name} in apps/desktop/bin, the active Cargo profile, or target/release; build it first with cargo build -p duckle-runner --bin duckle-db-sidecar"
                )
            })
    };

    let embedded_sidecar = out_dir.join("embedded-db-sidecar.bin");
    compress_db_sidecar(&sidecar, &embedded_sidecar);

    println!(
        "cargo:rustc-env=DUCKLE_EMBEDDED_DB_SIDECAR={}",
        embedded_sidecar.display()
    );
    println!("cargo:warning=desktop database runtime: quack from CLI-managed extensions");
}

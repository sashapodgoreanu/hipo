const RUNNER_DUCKDB_VERSION: &str = "1.5.4";
const QUACK_VERSION: &str = "1.5.4";
const QUACK_LICENSE: &str = "MIT";
const QUACK_PROVENANCE: &str = "duckdb/duckdb-quack";
const QUACK_EXTENSION_FILE: &str = "quack.duckdb_extension";

fn write_runner_pin_manifest(out_dir: &std::path::Path, sha256: &str) -> std::path::PathBuf {
    let manifest = format!(
        concat!(
            "{{\n",
            "  \"schemaVersion\": 1,\n",
            "  \"duckdbVersion\": \"{}\",\n",
            "  \"quackVersion\": \"{}\",\n",
            "  \"quackSha256\": \"{}\",\n",
            "  \"license\": \"{}\",\n",
            "  \"provenance\": \"{}\",\n",
            "  \"extensionFile\": \"{}\"\n",
            "}}\n"
        ),
        RUNNER_DUCKDB_VERSION,
        QUACK_VERSION,
        sha256,
        QUACK_LICENSE,
        QUACK_PROVENANCE,
        QUACK_EXTENSION_FILE
    );
    let path = out_dir.join("official-runner-pin.json");
    std::fs::write(&path, manifest)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    path
}

fn compress_to(src: &std::path::Path, dst: &std::path::Path) {
    if let (Ok(source_modified), Ok(destination_modified)) = (
        src.metadata().and_then(|metadata| metadata.modified()),
        dst.metadata().and_then(|metadata| metadata.modified()),
    ) {
        if destination_modified >= source_modified {
            return;
        }
    }

    let raw = std::fs::read(src)
        .unwrap_or_else(|error| panic!("read {}: {error}", src.display()));
    let compressed = zstd::encode_all(std::io::Cursor::new(&raw), 19)
        .unwrap_or_else(|error| panic!("zstd compress {}: {error}", src.display()));
    std::fs::write(dst, compressed)
        .unwrap_or_else(|error| panic!("write {}: {error}", dst.display()));
}

fn staged_or_profile_binary(
    manifest_dir: &std::path::Path,
    out_dir: &std::path::Path,
    name: &str,
) -> Option<std::path::PathBuf> {
    let staged = manifest_dir.join("bin").join(name);
    if staged.is_file() {
        return Some(staged);
    }

    out_dir
        .ancestors()
        .nth(3)
        .map(|profile| profile.join(name))
        .filter(|candidate| candidate.is_file())
}

fn embed_lance() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let name = if target_os == "windows" {
        "duckle-lance.exe"
    } else {
        "duckle-lance"
    };
    let staged = manifest_dir.join("bin").join(name);
    let destination = out_dir.join("embedded-lance.bin");

    if staged.is_file() {
        compress_to(&staged, &destination);
    } else {
        std::fs::write(&destination, [])
            .unwrap_or_else(|error| panic!("write empty embedded-lance: {error}"));
        println!(
            "cargo:warning=duckle-lance not staged (apps/desktop/bin/{name}); LanceDB nodes will need a duckle-lance on PATH or DUCKLE_LANCE_BIN. CI stages it."
        );
    }

    println!(
        "cargo:rustc-env=DUCKLE_EMBEDDED_LANCE={}",
        destination.display()
    );
    println!("cargo:rerun-if-changed={}", staged.display());
}

fn embed_mcp() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let name = if target_os == "windows" {
        "duckle-mcp.exe"
    } else {
        "duckle-mcp"
    };
    let destination = out_dir.join("embedded-mcp.bin");

    match staged_or_profile_binary(&manifest_dir, &out_dir, name) {
        Some(source) => compress_to(&source, &destination),
        None => {
            std::fs::write(&destination, [])
                .unwrap_or_else(|error| panic!("write empty embedded-mcp: {error}"));
            println!(
                "cargo:warning=duckle-mcp not staged (apps/desktop/bin/{name}); the in-app MCP popup will report no bundled server. Stage it: cargo build --profile release-runner -p duckle-mcp"
            );
        }
    }

    println!(
        "cargo:rustc-env=DUCKLE_EMBEDDED_MCP={}",
        destination.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("bin").join(name).display()
    );
}

fn embed_runner() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let name = if target_os == "windows" {
        "duckle-runner.exe"
    } else {
        "duckle-runner"
    };
    let source = staged_or_profile_binary(&manifest_dir, &out_dir, name).unwrap_or_else(|| {
        panic!(
            "duckle-runner not found; build it first with `cargo build --profile release-runner -p duckle-runner`, or stage it at apps/desktop/bin/{name}`"
        )
    });
    let destination = out_dir.join("embedded-runner.bin");

    compress_to(&source, &destination);
    println!(
        "cargo:rustc-env=DUCKLE_EMBEDDED_RUNNER={}",
        destination.display()
    );
    println!("cargo:rerun-if-changed={}", source.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("bin").join(name).display()
    );
}

fn embed_runner_linux() {
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let name = "duckle-runner-linux-x64";
    let staged = manifest_dir.join("bin").join(name);
    let destination = out_dir.join("embedded-runner-linux-x64.bin");

    if staged.is_file() {
        compress_to(&staged, &destination);
    } else {
        std::fs::write(&destination, [])
            .unwrap_or_else(|error| panic!("write empty embedded Linux runner: {error}"));
        println!(
            "cargo:warning=Linux runner not staged (apps/desktop/bin/{name}); Build Pipeline will not be able to target Linux from this build. Stage it: bash scripts/build-runner-linux.sh"
        );
    }

    println!(
        "cargo:rustc-env=DUCKLE_EMBEDDED_RUNNER_LINUX_X64={}",
        destination.display()
    );
    println!("cargo:rerun-if-changed={}", staged.display());
}

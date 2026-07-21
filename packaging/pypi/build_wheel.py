#!/usr/bin/env python3
"""Build a platform-tagged `duckle` wheel around a prebuilt duckle-runner.

The wheel carries a compiled Rust binary, so it is platform specific but not
Python-version specific: the tag is py3-none-<platform>, one wheel per OS and
architecture, which is the same shape ruff, uv and duckdb-cli publish.

    python build_wheel.py --binary <path-to-duckle-runner> --platform <tag>

Example platform tags:
    manylinux_2_17_x86_64.manylinux2014_x86_64
    manylinux_2_17_aarch64.manylinux2014_aarch64
    macosx_11_0_x86_64
    macosx_11_0_arm64
    win_amd64
    win_arm64
"""

import argparse
import re
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))

# Where repo-relative links have to point once the README is served by PyPI.
RAW_BASE = "https://raw.githubusercontent.com/slothflowlabs/duckle/main/"
BLOB_BASE = "https://github.com/slothflowlabs/duckle/blob/main/"
# Top-level directories the README links into.
REPO_DIRS = "docs|website|crates|apps|frontend|packaging|scripts|benchmarks"


def absolutize_links(text):
    """Rewrite repo-relative links so the README renders away from GitHub.

    PyPI serves the description standalone, so `src="docs/assets/x.png"`
    resolves against pypi.org and 404s. Images point at raw.githubusercontent
    (which serves real image content types) and document links at the blob
    view (which is what a reader actually wants to land on). Anchor links like
    `#whats-new-in-v056` are left alone; PyPI keeps heading anchors.
    """
    n = 0

    def sub(pattern, repl, s):
        nonlocal n
        s, k = re.subn(pattern, repl, s)
        n += k
        return s

    # HTML <img src="docs/...">
    text = sub(
        r'(<img[^>]*\ssrc=")(?:\./)?(' + REPO_DIRS + r')/',
        lambda m: m.group(1) + RAW_BASE + m.group(2) + "/",
        text,
    )
    # HTML <a href="docs/...">
    text = sub(
        r'(<a[^>]*\shref=")(?:\./)?(' + REPO_DIRS + r')/',
        lambda m: m.group(1) + BLOB_BASE + m.group(2) + "/",
        text,
    )
    # Markdown image ![alt](docs/...)
    text = sub(
        r'(!\[[^\]]*\]\()(?:\./)?(' + REPO_DIRS + r')/',
        lambda m: m.group(1) + RAW_BASE + m.group(2) + "/",
        text,
    )
    # Markdown link [text](docs/...) - after images, so the ! form is taken first.
    text = sub(
        r'(?<!!)(\[[^\]]*\]\()(?:\./)?(' + REPO_DIRS + r')/',
        lambda m: m.group(1) + BLOB_BASE + m.group(2) + "/",
        text,
    )
    # Root-level files and dirs the README points at: CONTRIBUTING.md,
    # SPONSORS.md, samples/, .github/workflows/, .gitlab-ci.yml.
    text = sub(
        r'(?<!!)(\[[^\]]*\]\()(?:\./)?((?:\.github|\.gitlab-ci\.yml|samples|[A-Z][A-Za-z0-9_]*\.md)(?:[/#][^)]*)?)\)',
        lambda m: m.group(1) + BLOB_BASE + m.group(2) + ")",
        text,
    )
    return text, n


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True, help="path to the built duckle-runner")
    ap.add_argument("--platform", required=True, help="wheel platform tag")
    ap.add_argument("--outdir", default=os.path.join(HERE, "dist"))
    args = ap.parse_args()

    if not os.path.isfile(args.binary):
        sys.exit("build_wheel: no such binary: {}".format(args.binary))

    is_windows_tag = args.platform.startswith("win")
    target_name = "duckle-runner.exe" if is_windows_tag else "duckle-runner"

    # Build in a scratch copy so a stale binary from a previous platform can
    # never be picked up by include-package-data.
    with tempfile.TemporaryDirectory() as tmp:
        stage = os.path.join(tmp, "pkg")
        shutil.copytree(HERE, stage, ignore=shutil.ignore_patterns("dist", "build", "*.egg-info", "__pycache__"))
        for stale in ("duckle-runner", "duckle-runner.exe"):
            p = os.path.join(stage, "duckle", stale)
            if os.path.exists(p):
                os.remove(p)
        dest = os.path.join(stage, "duckle", target_name)
        shutil.copyfile(args.binary, dest)
        os.chmod(dest, 0o755)

        size_mb = os.path.getsize(dest) / (1024 * 1024)
        print("staged {} ({:.1f} MB)".format(target_name, size_mb))

        # The package README is the PyPI page. It is written with absolute
        # URLs already, but run the rewriter anyway so a repo-relative link
        # added later cannot silently ship as a 404 on pypi.org.
        pkg_readme = os.path.join(stage, "README.md")
        if os.path.isfile(pkg_readme):
            with open(pkg_readme, encoding="utf-8") as fh:
                body = fh.read()
            body, rewritten = absolutize_links(body)
            with open(pkg_readme, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(body)
            print("README.md {:.0f} KB, {} relative link(s) absolutized".format(
                len(body.encode("utf-8")) / 1024, rewritten))

        cmd = [
            sys.executable, "-m", "build", "--wheel",
            "--outdir", os.path.abspath(args.outdir),
            "--config-setting=--build-option=--plat-name={}".format(args.platform),
            stage,
        ]
        rc = subprocess.call(cmd)
        if rc != 0:
            # setuptools' --plat-name plumbing differs across versions; fall
            # back to wheel's own retagging, which is version independent.
            print("build with --plat-name failed, retagging instead", file=sys.stderr)
            rc = subprocess.call([sys.executable, "-m", "build", "--wheel",
                                  "--outdir", os.path.abspath(args.outdir), stage])
            if rc != 0:
                sys.exit("build_wheel: wheel build failed")
            for name in sorted(os.listdir(args.outdir)):
                if name.endswith("-any.whl"):
                    subprocess.check_call([
                        sys.executable, "-m", "wheel", "tags",
                        "--platform-tag", args.platform, "--remove",
                        os.path.join(args.outdir, name),
                    ])

    print("\nwheels in {}:".format(args.outdir))
    for name in sorted(os.listdir(args.outdir)):
        p = os.path.join(args.outdir, name)
        print("  {}  ({:.1f} MB)".format(name, os.path.getsize(p) / (1024 * 1024)))


if __name__ == "__main__":
    main()

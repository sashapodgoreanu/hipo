"""Console entry point: hand off to the bundled duckle-runner binary.

The wheel ships a real compiled executable next to this module. This shim
exists only so the command lands on PATH as `duckle` rather than under the
Cargo target name, and so the DuckDB CLI that `duckdb-cli` installs is found
without the user configuring anything.

On POSIX it execs, replacing this process, so signals, exit codes and stdio
behave exactly as if the binary had been invoked directly. Windows has no
exec that preserves the process, so there it subprocesses and forwards the
return code.
"""

import os
import shutil
import subprocess
import sys

_EXE = ".exe" if os.name == "nt" else ""
_BIN_NAME = "duckle-runner" + _EXE


def _binary_path(stem="duckle-runner"):
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), stem + _EXE)
    if not os.path.exists(path):
        sys.stderr.write(
            "duckle: bundled {} not found at {}\n"
            "This wheel appears to be built for a different platform, or was built\n"
            "without that binary.\n"
            "Report at https://github.com/slothflowlabs/duckle/issues\n".format(stem, path)
        )
        raise SystemExit(2)
    return path


def _find_duckdb():
    """Locate the DuckDB CLI that `duckdb-cli` installed alongside us.

    PATH alone is not enough. `duckle` is frequently invoked by absolute path
    rather than through an activated environment - CI steps, cron entries,
    pipx and uvx all do this - and then the venv's scripts directory is not on
    PATH and `duckdb` is invisible even though pip installed it right next to
    this entry point. So look in our own environment first and treat PATH as
    the fallback, not the primary.
    """
    exe = "duckdb.exe" if os.name == "nt" else "duckdb"
    candidates = [
        # The scripts/bin dir of the interpreter running this shim.
        os.path.join(os.path.dirname(os.path.abspath(sys.executable)), exe),
        # sys.argv[0] is the generated `duckle` launcher, so its directory is
        # the scripts dir even when a different interpreter is in play.
        os.path.join(os.path.dirname(os.path.abspath(sys.argv[0])), exe),
    ]
    for path in candidates:
        if os.path.isfile(path) and os.access(path, os.X_OK):
            return path
    return shutil.which("duckdb")


def _engine_env():
    """Point the runner at the DuckDB CLI without the user configuring it.

    An explicit DUCKLE_DUCKDB_BIN always wins, so a user pinning their own
    build is never overridden.
    """
    env = os.environ.copy()
    if env.get("DUCKLE_DUCKDB_BIN"):
        return env
    found = _find_duckdb()
    if found:
        env["DUCKLE_DUCKDB_BIN"] = found
    return env


def _exec(stem, default_name):
    binary = _binary_path(stem)
    # argv[0] is what the child sees as its own name, and the runner renders
    # help under it. Passing the bundled binary's path would make a pip user
    # read "duckle-runner --pipeline ..." for a command they do not have, so
    # pass the launcher's name instead.
    invoked = os.path.splitext(os.path.basename(sys.argv[0]))[0] or default_name
    argv = [invoked] + sys.argv[1:]
    env = _engine_env()
    if os.name == "nt":
        # No exec that preserves the process on Windows: subprocess and
        # forward the child's exit code so CI still sees 0 / 1 / 2.
        # `executable` says what to launch while argv[0] says what the child
        # believes it is called. Without it, subprocess would try to launch
        # argv[0] ("duckle") as a path and fail with WinError 2.
        raise SystemExit(subprocess.call(argv, executable=binary, env=env))
    # execve already takes the binary separately from argv, so argv[0] is
    # free to be the friendly name.
    os.execve(binary, argv, env)


def main():
    _exec("duckle-runner", "duckle")


def mcp_main():
    """Entry point for the `duckle-mcp` console script.

    A stdio JSON-RPC MCP server: an MCP client spawns it and talks over
    stdin/stdout, so this must not print anything of its own. It resolves the
    DuckDB engine exactly like the runner does, and finds duckle-runner (which
    build_pipeline shells out to) by looking beside itself, which is satisfied
    because both binaries ship in this package directory.
    """
    _exec("duckle-mcp", "duckle-mcp")


if __name__ == "__main__":
    main()

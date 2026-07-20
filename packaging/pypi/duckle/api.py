"""A tiny fluent API for building and running Duckle pipelines from Python.

    import duckle

    (duckle.read_csv("orders.csv")
        .where("amount >= 20 and region in ('EU', 'UK')")
        .derive(total="round(amount * 1.2, 2)", label="f'{region}-{id}'")
        .write_parquet("out.parquet")
        .run())

Two properties make this different from a dataframe library.

**No data passes through Python.** Every method here only appends a node to a
pipeline graph. Nothing executes until `.run()`, and then the whole graph is
handed to the Duckle engine, which compiles it to SQL and executes it inside
DuckDB. Python builds a plan; DuckDB moves the rows. A billion-row job costs
the same here as it does on the canvas, because it is the same execution.

**The expressions are Python, and they compile to SQL.** `amount * 1.2`,
`f'{a}-{b}'`, `x if c else y`, `name.strip().upper()`, `email is None` are all
translated to vectorized DuckDB SQL at plan time. An expression that has no
exact SQL equivalent is rejected with the construct named, rather than being
quietly executed somewhere slower.

The graph is the same JSON the desktop canvas reads, so a pipeline written
here opens in the studio, and one drawn in the studio runs from here.
"""

import json
import os
import subprocess
import sys
import tempfile
import uuid

__all__ = [
    "Pipeline",
    "read_csv", "read_parquet", "read_json", "read_postgres", "read_duckdb",
    "from_json",
]


class DuckleError(RuntimeError):
    """A pipeline failed to compile or to run."""


def _nid():
    return "n_" + uuid.uuid4().hex[:8]


class Pipeline:
    """An immutable-ish builder over a Duckle pipeline graph.

    Each call appends a node and returns self, so chains read top to bottom in
    the order the data flows.
    """

    def __init__(self, name="pipeline"):
        self.name = name
        self._nodes = []
        self._edges = []
        self._last = None
        self._x = 0

    # ---------------------------------------------------------- graph building

    def _add(self, kind, component, properties, label=None, connect=True):
        node_id = _nid()
        self._nodes.append({
            "id": node_id,
            "type": kind,
            "position": {"x": self._x, "y": 0},
            "data": {
                "label": label or component,
                "componentId": component,
                "properties": properties,
            },
        })
        self._x += 220
        if connect and self._last is not None:
            self._edges.append({
                "id": "e_" + uuid.uuid4().hex[:8],
                "source": self._last,
                "target": node_id,
                "sourceHandle": "main",
                "targetHandle": "main",
                "data": {"connectionType": "main"},
            })
        self._last = node_id
        return self

    # ------------------------------------------------------------------ sources

    def read_csv(self, path, **opts):
        return self._add("source", "src.csv", dict(path=path, **opts), "CSV")

    def read_parquet(self, path, **opts):
        return self._add("source", "src.parquet", dict(path=path, **opts), "Parquet")

    def read_json(self, path, **opts):
        return self._add("source", "src.json", dict(path=path, **opts), "JSON")

    def read_postgres(self, table, **opts):
        return self._add("source", "src.postgres", dict(table=table, **opts), "Postgres")

    def read_duckdb(self, path, table, **opts):
        return self._add("source", "src.duckdb", dict(path=path, table=table, **opts), "DuckDB")

    def source(self, component, **props):
        """Escape hatch: any of Duckle's source components by id."""
        return self._add("source", component, props, component)

    # --------------------------------------------------------------- transforms

    def derive(self, **columns):
        """Add columns from Python expressions, compiled to SQL.

        .derive(total="round(amount * 1.2, 2)", tag="f'{region}-{id}'")
        """
        if not columns:
            raise DuckleError("derive() needs at least one column")
        cols = [{"name": k, "expr": _expr_src(v)} for k, v in columns.items()]
        return self._add("transform", "xf.pyexpr", {"columns": cols}, "Derive")

    def where(self, expr):
        """Keep rows where a Python expression is true.

        Compiled to a SQL predicate, so filtering happens inside DuckDB.
        """
        return self._add(
            "transform", "xf.filter",
            {"predicate": {"mode": "python", "expr": _expr_src(expr)}},
            "Filter",
        )

    filter = where

    def select(self, *columns):
        return self._add("transform", "xf.select", {"columns": list(columns)}, "Select")

    def rename(self, **mapping):
        pairs = [{"from": k, "to": v} for k, v in mapping.items()]
        return self._add("transform", "xf.rename", {"renames": pairs}, "Rename")

    def sort(self, *columns, desc=False):
        order = [{"column": c, "direction": "desc" if desc else "asc"} for c in columns]
        return self._add("transform", "xf.sort", {"orderBy": order}, "Sort")

    def limit(self, n):
        return self._add("transform", "xf.limit", {"count": int(n)}, "Limit")

    def dedupe(self, *columns):
        return self._add("transform", "xf.distinct", {"columns": list(columns)}, "Dedupe")

    def transform(self, component, **props):
        """Escape hatch: any of Duckle's transform components by id."""
        return self._add("transform", component, props, component)

    # -------------------------------------------------------------------- sinks

    def write_csv(self, path, **opts):
        return self._add("sink", "snk.csv", dict(path=path, **opts), "CSV out")

    def write_parquet(self, path, **opts):
        return self._add("sink", "snk.parquet", dict(path=path, **opts), "Parquet out")

    def write_json(self, path, **opts):
        return self._add("sink", "snk.json", dict(path=path, **opts), "JSON out")

    def sink(self, component, **props):
        """Escape hatch: any of Duckle's sink components by id."""
        return self._add("sink", component, props, component)

    # ------------------------------------------------------------------ outputs

    def to_dict(self):
        return {"name": self.name, "nodes": self._nodes, "edges": self._edges}

    def to_json(self, indent=2):
        return json.dumps(self.to_dict(), indent=indent)

    def save(self, path):
        """Write the pipeline JSON. Opens directly in the Duckle studio."""
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(self.to_json())
        return self

    def validate(self):
        """Compile-check without touching a source or a sink.

        Needs no engine, no credentials and no network. Raises on failure.
        """
        out = self._invoke(["validate", "--json", "{pipeline}"])
        try:
            report = json.loads(out)
        except ValueError:
            raise DuckleError(out.strip() or "validate produced no output")
        if not report.get("ok"):
            problems = [r.get("error", "?") for r in report.get("results", []) if not r.get("ok")]
            raise DuckleError("; ".join(problems) or "pipeline did not compile")
        return self

    def sql(self):
        """Return the compiled SQL, one entry per stage.

        The whole point of compiling to SQL is that the result is readable.
        This runs the compiler only: no source is opened and no sink written.
        """
        out = self._invoke(["validate", "--json", "--sql", "{pipeline}"])
        try:
            report = json.loads(out)
        except ValueError:
            raise DuckleError(out.strip() or "could not compile")
        results = report.get("results") or []
        if not results or not results[0].get("ok"):
            problems = [r.get("error", "?") for r in results if not r.get("ok")]
            raise DuckleError("; ".join(problems) or "pipeline did not compile")
        return results[0].get("sql", [])

    def explain(self):
        """Print the compiled SQL."""
        for stage in self.sql():
            name = stage.get("name") or stage.get("node_id") or "stage"
            text = (stage.get("sql") or "").strip()
            if text:
                print("-- {}\n{}\n".format(name, text))
        return self

    def run(self, quiet=False):
        """Execute the pipeline. Rows are moved by DuckDB, never by Python."""
        out = self._invoke(["--pipeline", "{pipeline}"], check=True)
        if not quiet:
            sys.stdout.write(out)
        return self

    # ---------------------------------------------------------------- internals

    def _invoke(self, argv_template, check=False):
        from .__main__ import _binary_path, _engine_env
        tmp = tempfile.mkdtemp(prefix="duckle-")
        path = os.path.join(tmp, "{}.json".format(self.name))
        self.save(path)
        argv = [_binary_path()] + [
            a.replace("{pipeline}", path) for a in argv_template
        ]
        proc = subprocess.run(
            argv, env=_engine_env(), capture_output=True, text=True,
        )
        combined = (proc.stdout or "") + (proc.stderr or "")
        # Exit 2 means the runner could not start; 1 means a real finding.
        if proc.returncode == 2:
            raise DuckleError(combined.strip() or "the runner could not start")
        if check and proc.returncode != 0:
            raise DuckleError(combined.strip() or "pipeline failed")
        return combined


def _expr_src(expr):
    """Accept either a string or a `col`-built Expr and return source text.

    Both spell the same thing. The translation to SQL lives in the engine
    (Rust) so the canvas, the CLI and this API cannot drift apart; here the
    expression is only carried.
    """
    from .expr import Expr
    if isinstance(expr, Expr):
        return str(expr)
    if not isinstance(expr, str):
        raise DuckleError(
            "expected a string or a col expression, got {}".format(type(expr).__name__)
        )
    return expr


# Module-level conveniences so a one-liner does not need to name a Pipeline.
def _start(method, *args, **kwargs):
    p = Pipeline()
    return getattr(p, method)(*args, **kwargs)


def read_csv(path, **opts):
    return _start("read_csv", path, **opts)


def read_parquet(path, **opts):
    return _start("read_parquet", path, **opts)


def read_json(path, **opts):
    return _start("read_json", path, **opts)


def read_postgres(table, **opts):
    return _start("read_postgres", table, **opts)


def read_duckdb(path, table, **opts):
    return _start("read_duckdb", path, table, **opts)


def from_json(path_or_dict):
    """Load a pipeline the studio wrote, so it can be run or extended here."""
    p = Pipeline()
    doc = path_or_dict
    if isinstance(path_or_dict, str):
        with open(path_or_dict, encoding="utf-8") as fh:
            doc = json.load(fh)
    p.name = doc.get("name", "pipeline")
    p._nodes = doc.get("nodes", [])
    p._edges = doc.get("edges", [])
    if p._nodes:
        p._last = p._nodes[-1]["id"]
        p._x = max((n.get("position", {}).get("x", 0) for n in p._nodes), default=0) + 220
    return p

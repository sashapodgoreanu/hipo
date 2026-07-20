"""Namespaces that expose every Duckle component to Python.

Component ids map straight onto attribute paths, so palette knowledge
transfers with no translation layer:

    src.salesforce        ->  duckle.src.salesforce(...)
    xf.geo.reproject      ->  duckle.xf.geo.reproject(...)
    snk.salesforce.bulk   ->  duckle.snk.salesforce.bulk(...)

Every parameter is keyword-only with no default. The catalog marks only
18.6% of fields `required`, and 43 components mark none at all, so absence
of that flag means "nobody annotated it" rather than "optional". Inventing
mandatory arguments from it would reject valid pipelines; the engine already
reports a genuinely missing setting with a precise message, so that is left
to the engine.

Unknown keyword names are not silently accepted. They raise, with a
did-you-mean drawn from the component's own field list, because a mistyped
property does not error at run time: the builder simply does not find the
key it wants and falls back to a default. That is how a filter with
`condition=` instead of `predicate=` passes every row rather than failing.
"""

import difflib
import warnings

from ._components import COMPONENTS

__all__ = ["Namespace", "component_ids", "describe", "root_namespaces"]


def component_ids(kind=None, contains=None):
    """List component ids, optionally filtered by kind or substring."""
    out = []
    for cid, meta in sorted(COMPONENTS.items()):
        if kind and meta.get("kind") != kind:
            continue
        if contains and contains not in cid:
            continue
        out.append(cid)
    return out


def describe(component_id):
    """Human-readable description of a component and its settings."""
    meta = COMPONENTS.get(component_id)
    if not meta:
        near = difflib.get_close_matches(component_id, COMPONENTS, n=3, cutoff=0.5)
        hint = "  Did you mean: {}".format(", ".join(near)) if near else ""
        return "unknown component {!r}.{}".format(component_id, hint)
    lines = ["{}  ({})".format(component_id, meta.get("kind", "?"))]
    if meta.get("summary"):
        lines.append("  {}".format(meta["summary"]))
    params = meta.get("params") or []
    lines.append("  settings: {}".format(", ".join(params) if params else "(none catalogued)"))
    unver = meta.get("unverified") or []
    if unver:
        lines.append(
            "  not read by the engine (accepted, but has no effect): {}".format(", ".join(unver))
        )
    return "\n".join(lines)


def _check_kwargs(component_id, kwargs):
    """Warn about a setting the catalog does not list. Never block.

    This warns rather than raising because the catalog is known to be an
    incomplete description, not an allowlist. It models a repeating group by
    its inner fields and loses the outer key: xf.addcol catalogues
    `name, expression, type` while the builder actually reads a `columns`
    array holding objects of that shape. Treating the catalog as authoritative
    would reject correct pipelines.

    A warning still earns its place, because a mistyped setting does not fail
    at run time. The builder does not find the key it wants and quietly falls
    back to a default, so `condition=` instead of `predicate=` yields a filter
    that passes every row. Surfacing that at build time is the whole point;
    refusing to build on it is a step too far.
    """
    meta = COMPONENTS.get(component_id) or {}
    known = set(meta.get("params") or []) | set(meta.get("unverified") or [])
    if not known:
        return  # nothing catalogued; do not second-guess the caller
    unknown = [k for k in kwargs if k not in known]
    if not unknown:
        return
    problems = []
    for k in unknown:
        near = difflib.get_close_matches(k, sorted(known), n=3, cutoff=0.6)
        if near:
            problems.append("{!r} (did you mean {}?)".format(k, " or ".join(repr(n) for n in near)))
        else:
            problems.append("{!r}".format(k))
    warnings.warn(
        "{}: {} not listed for this component. It is still passed through, but "
        "if the name is wrong the engine will ignore it silently rather than "
        "fail. Catalogued settings: {}".format(
            component_id, ", ".join(problems), ", ".join(sorted(known))
        ),
        stacklevel=3,
    )


class Namespace:
    """A node in the component-id tree, e.g. `duckle.xf` or `duckle.xf.geo`.

    Bound to a Pipeline it appends and returns the pipeline, so chains keep
    reading top to bottom. Unbound it starts a new pipeline.
    """

    __slots__ = ("_prefix", "_pipeline")

    def __init__(self, prefix="", pipeline=None):
        object.__setattr__(self, "_prefix", prefix)
        object.__setattr__(self, "_pipeline", pipeline)

    def _bind(self, pipeline):
        return Namespace(self._prefix, pipeline)

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        path = "{}.{}".format(self._prefix, name) if self._prefix else name
        # A leaf that is itself a prefix (snk.salesforce is both a component
        # and the parent of snk.salesforce.bulk) stays callable AND traversable.
        if path in COMPONENTS or any(c.startswith(path + ".") for c in COMPONENTS):
            return _Node(path, self._pipeline)
        near = difflib.get_close_matches(path, list(COMPONENTS), n=3, cutoff=0.5)
        hint = "\n  Did you mean: {}".format(", ".join(near)) if near else ""
        raise AttributeError("no Duckle component {!r}{}".format(path, hint))

    def __dir__(self):
        seen = set()
        plen = len(self._prefix) + 1 if self._prefix else 0
        for cid in COMPONENTS:
            if self._prefix and not cid.startswith(self._prefix + "."):
                continue
            rest = cid[plen:]
            if rest:
                seen.add(rest.split(".")[0])
        return sorted(seen)

    def __repr__(self):
        return "<duckle namespace {!r}: {} components>".format(
            self._prefix or "(root)", len(self.__dir__())
        )


class _Node(Namespace):
    """A namespace path that is also a real component, so it can be called."""

    __slots__ = ()

    def __call__(self, **props):
        cid = self._prefix
        if cid not in COMPONENTS:
            raise TypeError(
                "{!r} is a group, not a component. Options: {}".format(cid, ", ".join(self.__dir__()))
            )
        _check_kwargs(cid, props)
        meta = COMPONENTS[cid]
        kind = {"source": "source", "sink": "sink"}.get(meta.get("kind"), "transform")
        pipeline = self._pipeline
        if pipeline is None:
            from .api import Pipeline
            pipeline = Pipeline()
        label = cid.split(".", 1)[-1]
        return pipeline._add(kind, cid, dict(props), label)

    @property
    def help(self):
        return describe(self._prefix)

    def __repr__(self):
        if self._prefix in COMPONENTS:
            return "<duckle component {!r}>".format(self._prefix)
        return Namespace.__repr__(self)


def root_namespaces():
    """The top-level id segments: src, xf, snk, qa, ctl, code, ..."""
    roots = sorted({cid.split(".")[0] for cid in COMPONENTS})
    return {r: Namespace(r) for r in roots}

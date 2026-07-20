"""Column expressions built with real Python operators.

    from duckle import col

    (col.amount * 1.2).round(2)
    (col.amount >= 20) & col.region.isin(["EU", "UK"])
    col.email.is_null()

Why this exists alongside plain strings. A string like "amount * 1.2" is the
most faithful "just write Python" surface: it supports `and`, `or`, `not`, the
conditional expression and f-strings, none of which Python lets a library
overload. What it cannot give you is editor completion, or a fragment you can
name once and reuse.

So both are supported and both compile to the same DuckDB SQL. Use a string
when you want ternaries and f-strings; use `col` when you want completion and
reusable pieces:

    eu_only = col.region.isin(["EU", "UK"])      # name it once
    p.where(eu_only & (col.amount >= 20))        # reuse anywhere

Note the operator spellings. Python cannot overload `and` / `or` / `not`, so
`col` uses `&` / `|` / `~`, and because those bind tighter than comparison you
must parenthesise each side. If that reads badly, write a string instead - it
compiles to exactly the same SQL.

Nothing here evaluates data. An expression is a small tree that renders to a
Python-expression string, which the engine compiles to vectorized SQL at plan
time.
"""

__all__ = ["Expr", "col", "lit", "when"]


def _render(value):
    """Render a Python value or Expr as Duckle expression source."""
    if isinstance(value, Expr):
        return value._src
    if value is None:
        return "None"
    if isinstance(value, bool):
        return "True" if value else "False"
    if isinstance(value, (int, float)):
        return repr(value)
    if isinstance(value, str):
        # repr gives a correctly quoted and escaped Python literal.
        return repr(value)
    if isinstance(value, (list, tuple)):
        return "(" + ", ".join(_render(v) for v in value) + ("," if len(value) == 1 else "") + ")"
    raise TypeError(
        "cannot use {!r} in a Duckle expression; supported: numbers, strings, "
        "bools, None, lists/tuples of those, and other expressions".format(value)
    )


class Expr:
    """A column expression. Combine with operators; render to source text."""

    __slots__ = ("_src",)

    def __init__(self, src):
        self._src = src

    # ---- rendering -------------------------------------------------------

    def __str__(self):
        return self._src

    def __repr__(self):
        return "Expr({!r})".format(self._src)

    def _bin(self, op, other, reverse=False):
        a, b = (_render(other), self._src) if reverse else (self._src, _render(other))
        return Expr("({} {} {})".format(a, op, b))

    # ---- arithmetic ------------------------------------------------------

    def __add__(self, o): return self._bin("+", o)
    def __radd__(self, o): return self._bin("+", o, True)
    def __sub__(self, o): return self._bin("-", o)
    def __rsub__(self, o): return self._bin("-", o, True)
    def __mul__(self, o): return self._bin("*", o)
    def __rmul__(self, o): return self._bin("*", o, True)
    def __truediv__(self, o): return self._bin("/", o)
    def __rtruediv__(self, o): return self._bin("/", o, True)
    def __floordiv__(self, o): return self._bin("//", o)
    def __mod__(self, o): return self._bin("%", o)
    def __pow__(self, o): return self._bin("**", o)
    def __neg__(self): return Expr("(-{})".format(self._src))

    # ---- comparison ------------------------------------------------------

    def __eq__(self, o): return self._bin("==", o)
    def __ne__(self, o): return self._bin("!=", o)
    def __lt__(self, o): return self._bin("<", o)
    def __le__(self, o): return self._bin("<=", o)
    def __gt__(self, o): return self._bin(">", o)
    def __ge__(self, o): return self._bin(">=", o)

    __hash__ = None  # __eq__ builds an expression, so this is not hashable

    # ---- boolean ---------------------------------------------------------
    # Python reserves `and` / `or` / `not` for truthiness and will not call a
    # dunder for them, so these are the bitwise operators instead.

    def __and__(self, o): return Expr("({} and {})".format(self._src, _render(o)))
    def __rand__(self, o): return Expr("({} and {})".format(_render(o), self._src))
    def __or__(self, o): return Expr("({} or {})".format(self._src, _render(o)))
    def __ror__(self, o): return Expr("({} or {})".format(_render(o), self._src))
    def __invert__(self): return Expr("(not {})".format(self._src))

    def __bool__(self):
        raise TypeError(
            "a Duckle expression has no truth value. Use & | ~ rather than "
            "and / or / not, and parenthesise each side: "
            "(col.a > 1) & (col.b < 2)"
        )

    # ---- null handling ---------------------------------------------------

    def is_null(self): return Expr("({} is None)".format(self._src))
    def is_not_null(self): return Expr("({} is not None)".format(self._src))

    # ---- membership ------------------------------------------------------

    def isin(self, values):
        if not values:
            raise ValueError("isin() needs at least one value")
        return Expr("({} in {})".format(self._src, _render(list(values))))

    def not_in(self, values):
        if not values:
            raise ValueError("not_in() needs at least one value")
        return Expr("({} not in {})".format(self._src, _render(list(values))))

    # ---- numeric ---------------------------------------------------------

    def round(self, digits=0): return Expr("round({}, {})".format(self._src, int(digits)))
    def abs(self): return Expr("abs({})".format(self._src))

    # ---- strings ---------------------------------------------------------

    def upper(self): return Expr("{}.upper()".format(self._src))
    def lower(self): return Expr("{}.lower()".format(self._src))
    def strip(self): return Expr("{}.strip()".format(self._src))
    def title(self): return Expr("{}.title()".format(self._src))
    def length(self): return Expr("len({})".format(self._src))

    def replace(self, old, new):
        return Expr("{}.replace({}, {})".format(self._src, _render(old), _render(new)))

    def startswith(self, prefix):
        return Expr("{}.startswith({})".format(self._src, _render(prefix)))

    def endswith(self, suffix):
        return Expr("{}.endswith({})".format(self._src, _render(suffix)))

    def zfill(self, width):
        return Expr("{}.zfill({})".format(self._src, int(width)))

    # ---- casts -----------------------------------------------------------

    def as_int(self): return Expr("int({})".format(self._src))
    def as_float(self): return Expr("float({})".format(self._src))
    def as_str(self): return Expr("str({})".format(self._src))


class _ColFactory:
    """`col.name` or `col["name with spaces"]` -> an Expr."""

    def __getattr__(self, name):
        if name.startswith("__"):
            raise AttributeError(name)
        return Expr(name)

    def __getitem__(self, name):
        return Expr(str(name))


col = _ColFactory()


def lit(value):
    """A literal, for when a bare Python value would be ambiguous."""
    return Expr(_render(value))


def when(condition, then, otherwise):
    """The conditional expression, which `if/else` cannot express via operators.

        when(col.amount > 50, "high", "low")
    """
    return Expr("({} if {} else {})".format(
        _render(then), _render(condition), _render(otherwise)
    ))

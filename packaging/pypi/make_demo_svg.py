#!/usr/bin/env python3
"""Render the terminal demos used on the PyPI page.

Self-contained SVG: no asciinema, no recorder, no runtime dependency.

The transcript is ALWAYS fully visible and only the cursor blinks. A
progressive line-by-line reveal was built first and rejected: embedded
through an <img> tag, which is exactly how PyPI and GitHub render it, the
animation was repeatedly observed sitting on its first frame, leaving a
terminal that showed one line and nothing else. Content a reader needs must
never depend on an animation actually running, least of all on the hero
image of a package page. The cursor blink is CSS rather than script because
GitHub serves raw SVG under
`Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'`,
so inline style survives where script would not.

Every line below is real captured output, not a mock-up.

    python packaging/pypi/make_demo_svg.py
"""

import os

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.abspath(os.path.join(HERE, "..", "..", "docs", "assets"))

# Duckle palette. Success reads maya, never green (brand rule).
BG = "#0f1720"
CHROME = "#18222e"
FG = "#c9d5e1"
DIM = "#7d8ea1"
LEMON = "#f5d90a"
MAYA = "#5bc8f5"
ORANGE = "#f59f0a"
RED = "#f2555a"

CHAR_W = 8.4
LINE_H = 21
PAD_X = 18
TOP = 44


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def render(path, title, lines):
    """lines: list of (kind, text); kind picks the row colour."""
    width = max(660, int(PAD_X * 2 + CHAR_W * max(len(t) for _, t in lines) + 10))
    height = TOP + LINE_H * (len(lines) + 1) + 16

    body = []
    for i, (kind, text) in enumerate(lines):
        y = TOP + LINE_H * (i + 1)
        if kind == "cmd":
            body.append(
                '<text x="{x}" y="{y}" class="m">'
                '<tspan class="p">$</tspan> <tspan class="c">{t}</tspan></text>'.format(
                    x=PAD_X, y=y, t=esc(text)
                )
            )
        elif text:
            body.append(
                '<text x="{x}" y="{y}" class="m {k}">{t}</text>'.format(
                    x=PAD_X, y=y, k=kind, t=esc(text).replace(" ", "&#160;")
                )
            )

    cursor_y = TOP + LINE_H * (len(lines) + 1) - 11
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" '
        'viewBox="0 0 {w} {h}" role="img" aria-label="{title}">\n'
        "<style>\n"
        ".m{{font-family:'SFMono-Regular',Consolas,'Liberation Mono',Menlo,monospace;"
        "font-size:13.5px;white-space:pre}}\n"
        ".p{{fill:{lemon};font-weight:700}}\n"
        ".c{{fill:{fg};font-weight:600}}\n"
        ".out{{fill:{fg}}}\n"
        ".dim{{fill:{dim}}}\n"
        ".ok{{fill:{maya}}}\n"
        ".err{{fill:{red}}}\n"
        ".key{{fill:{lemon}}}\n"
        ".cur{{animation:blink 1.06s steps(1,end) infinite}}\n"
        "@keyframes blink{{0%,50%{{opacity:1}}50.01%,100%{{opacity:0}}}}\n"
        "</style>\n"
        '<rect width="{w}" height="{h}" rx="9" fill="{bg}"/>\n'
        '<rect width="{w}" height="30" rx="9" fill="{chrome}"/>\n'
        '<rect y="21" width="{w}" height="9" fill="{chrome}"/>\n'
        '<circle cx="19" cy="15" r="5" fill="{red}"/>\n'
        '<circle cx="37" cy="15" r="5" fill="{orange}"/>\n'
        '<circle cx="55" cy="15" r="5" fill="{maya}"/>\n'
        '<text x="{tx}" y="19.5" class="m dim" font-size="11.5">{title}</text>\n'
        "{body}\n"
        '<rect class="cur" x="{cx}" y="{cy}" width="8" height="15" fill="{lemon}"/>\n'
        "</svg>\n"
    ).format(
        w=width, h=height, title=esc(title), body="\n".join(body),
        bg=BG, chrome=CHROME, fg=FG, dim=DIM, lemon=LEMON, maya=MAYA,
        orange=ORANGE, red=RED, tx=max(70, width // 2 - len(title) * 3),
        cx=PAD_X, cy=cursor_y,
    )
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(svg)
    print("wrote {} ({:.1f} KB, {}x{})".format(
        os.path.basename(path), len(svg) / 1024, width, height))


# --------------------------------------------------------------- demo 1
# The actual new-user path: one command, from nothing to real rows.
# Captured verbatim from a pip-installed duckle with DUCKLE_DUCKDB_BIN unset.
INSTALL = [
    ("cmd", "pip install duckle"),
    ("ok",  "Successfully installed duckdb-cli-1.5.4 duckle-0.5.8"),
    ("out", ""),
    ("cmd", "duckle quickstart"),
    ("out", ""),
    ("key", "Duckle quickstart"),
    ("out", ""),
    ("dim", "  created  orders.csv  (8 rows of sample data)"),
    ("dim", "  created  pipelines/quickstart.json"),
    ("out", ""),
    ("out", "Running pipelines/quickstart.json ..."),
    ("out", ""),
    ("out", "status   : ok"),
    ("dim", "duration : 563 ms"),
    ("ok",  "  csv        ok (8 rows)"),
    ("ok",  "  filter     ok (5 rows)"),
    ("ok",  "  derive     ok (5 rows)"),
    ("ok",  "  out        ok (5 rows)"),
    ("out", ""),
    ("out", "out.csv:"),
    ("out", ""),
    ("dim", "  id,region,customer,amount,total,tag"),
    ("out", "  2,EU,Globex,25,30.0,EU-Globex"),
    ("out", "  3,US,Initech,40,48.0,US-Initech"),
    ("out", "  4,UK,Umbrella,30,36.0,UK-Umbrella"),
    ("out", "  6,EU,Soylent,55,66.0,EU-Soylent"),
    ("out", "  7,UK,Vehement,22,26.4,UK-Vehement"),
    ("out", ""),
    ("dim", "# One command. Scaffolded, compiled to SQL, executed by DuckDB."),
]

# --------------------------------------------------------------- demo 2
# The CI gate: compile-checks with no engine, no credentials, no network.
VALIDATE = [
    ("cmd", "duckle validate"),
    ("err", "FAIL  pipelines/broken.json"),
    ("dim", "      config: Filter (xf.filter / filter): missing main input"),
    ("ok",  "ok    pipelines/orders.json  (4 stages)"),
    ("out", ""),
    ("out", "2 pipeline(s) checked, 1 failed"),
    ("out", ""),
    ("cmd", "echo $?"),
    ("key", "1"),
    ("out", ""),
    ("dim", "# 0 clean | 1 a real finding | 2 the runner could not start"),
    ("dim", "# No DuckDB, no credentials, no network: validate only compiles."),
]


# --------------------------------------------------------------- demo 3
# The agent loop. Opens with the one command a first-time user types, then
# follows the conversation. Tool results are verbatim from a real MCP session.
AGENT = [
    ("cmd", "claude mcp add duckle -- uvx duckle mcp"),
    ("ok",  "Added stdio MCP server duckle"),
    ("out", ""),
    ("cmd", "claude"),
    ("out", ""),
    ("key", "> load orders.csv into parquet, keep orders over 20"),
    ("out", ""),
    ("dim", "  duckle - get_component_schema(\"xf.filter\")"),
    ("out", "      predicate, rejectOnError"),
    ("out", ""),
    ("dim", "  duckle - create_pipeline(\"big-orders\")"),
    ("ok",  "      ok   3 stages compiled"),
    ("out", ""),
    ("dim", "  duckle - run_pipeline(\"big-orders.json\")"),
    ("ok",  "      ok   169 ms"),
    ("ok",  "        csv      ok (4 rows)"),
    ("ok",  "        filter   ok (3 rows)"),
    ("ok",  "        parquet  ok (3 rows)"),
    ("out", ""),
    ("out", "  Wrote big_orders.parquet. The pipeline is saved as JSON,"),
    ("out", "  so you can open it on the canvas and see what I built."),
    ("out", ""),
    ("dim", "# Compile-checked before it ran. No pip install, no engine setup."),
]


if __name__ == "__main__":
    os.makedirs(OUT_DIR, exist_ok=True)
    render(os.path.join(OUT_DIR, "pypi-demo-install.svg"),
           "pip install duckle", INSTALL)
    render(os.path.join(OUT_DIR, "pypi-demo-validate.svg"),
           "duckle validate  -  the CI gate", VALIDATE)
    render(os.path.join(OUT_DIR, "pypi-demo-agent.svg"),
           "duckle as an MCP server", AGENT)

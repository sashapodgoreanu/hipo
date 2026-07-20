"""Duckle: local-first ETL/ELT pipelines that run on DuckDB.

    import duckle

    (duckle.read_csv("orders.csv")
        .where("amount >= 20")
        .derive(total="round(amount * 1.2, 2)")
        .write_parquet("out.parquet")
        .run())

Python builds the plan; DuckDB moves the rows. Nothing here pulls data into
the interpreter, and the Python expressions above are compiled to vectorized
SQL before the run starts.
"""

from .api import (  # noqa: F401
    DuckleError,
    Pipeline,
    from_json,
    read_csv,
    read_duckdb,
    read_json,
    read_parquet,
    read_postgres,
)

__version__ = "0.5.7"
__all__ = [
    "Pipeline", "DuckleError", "from_json",
    "read_csv", "read_parquet", "read_json", "read_postgres", "read_duckdb",
]

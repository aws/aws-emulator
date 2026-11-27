// SPDX-License-Identifier: CC0-1.0

//! `pyspark.sql.statements`: the STATEMENTS a Spark session runs, and where they are run.
//!
//! A Spark driver does not interpret a statement in its Python client: it RESOLVES the names
//! the statement carries in the session's catalog — on Amazon Web Services the AWS Glue Data
//! Catalog, for an environment whose configuration selects it as the Hive metastore — reads
//! the data those tables point at, and plans and runs the statement over it. That work belongs
//! to the engine, so this module is the client half alone: it submits the statement text and
//! the database an unqualified name resolves in, and reads back the columns and rows the
//! engine answered.
//!
//! The ENGINE is the one the environment the program runs in supplies, reached through the
//! seam its interpreter registers ([`super::HOST_MODULE`]) — the same shape of guest-to-host
//! seam the SDK this runtime supplies a guest takes. What a statement answers is therefore
//! what that engine computed over real stored data, never a value read off the statement's own
//! text; and a statement the engine refused raises the class the engine named, so the driver's
//! traceback carries the failure a client branches on — an analysis failure over a name the
//! catalog does not hold apart from the engine's own failure.

/// The source of `pyspark/sql/statements.py`.
pub(super) const STATEMENTS_SOURCE: &str = r#"
"""The statements a Spark session submits, and what the engine answered for them."""

# The module the interpreter registers this session's ENGINE on, and the one function on it:
# the statement in, what the engine answered out. Both names are the runtime's own
# (`service_common::spark_runtime::HOST_MODULE` and `HOST_QUERY_FUNCTION`), and the engine
# behind them is the calling service's.
HOST_MODULE = "@@HOST_MODULE@@"
HOST_FUNCTION = "@@HOST_FUNCTION@@"

# The failure classes the engine names its refusals by, mapped onto the class each is raised
# as. A class outside this table is raised as the engine's own failure carrying its message,
# so a refusal this client has no class for is still reported as what the engine said.
ANALYSIS = "@@ANALYSIS_CLASS@@"
ENGINE = "@@ENGINE_CLASS@@"


def _engine():
    """The engine this session's statements are planned and run by.

    Imported HERE and not at module scope, the way every other seam of this runtime is
    reached: a program that runs no statement runs in an environment it never asks for.
    """
    import importlib

    return importlib.import_module(HOST_MODULE)


def _raised(error):
    """The failure a refused statement raises: the class the engine named, its message."""
    from pyspark.filesystem import AnalysisException
    from pyspark.sql import SparkException

    classes = {ANALYSIS: AnalysisException, ENGINE: SparkException}
    held = (error or {}).get("class")
    raised = classes.get(str(held or ""), SparkException)
    return raised(str((error or {}).get("message") or ""))


def run(text, database):
    """What the engine answered for the statement `text`, run with `database` current.

    The answer is `(columns, rows)`: one entry per column of the answer, carrying the name the
    engine reports it under and the type it holds where the engine states one, and the rows
    themselves in the order the engine answered them. A statement that changed the catalog
    rather than reading answers no columns and no rows, which is the empty dataset Spark
    answers such a statement with.
    """
    import json

    submitted = json.dumps({"query": str(text), "database": str(database or "")})
    answered = json.loads(getattr(_engine(), HOST_FUNCTION)(submitted))
    error = answered.get("error")
    if error:
        raise _raised(error)
    return (list(answered.get("columns") or []), list(answered.get("rows") or []))
"#;

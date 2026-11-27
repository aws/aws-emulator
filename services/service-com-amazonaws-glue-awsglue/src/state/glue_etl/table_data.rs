// SPDX-License-Identifier: CC0-1.0

//! `awsglue._table_data`: READING a Data Catalog table, for the frame a script reads
//! `from_catalog`.
//!
//! The table's metadata comes from the Data Catalog's own API and its data from the object
//! store, both over the AWS SDK the runtime supplies the guest — so both calls are signed as
//! the role the run is performed as and are authorized exactly as any other client's are.
//! What a script gets is therefore what this emulator really holds for that table: a read of
//! a governed table the role holds no Lake Formation permission on is refused by the catalog
//! and the refusal travels out as the script's own failure, and a read the catalog answers
//! resolves the objects that were really put at the table's location into one record per row.
//!
//! The service's own host-side reading of a table's rows ([`crate::state::table_rows`])
//! serves the work this SERVICE performs over a table (a column statistics task run, a data
//! quality evaluation run). This is the reading a SCRIPT performs, which has to happen in the
//! script's own process because the script's own functions compute over the records — so it
//! goes through the public API the way any other client's reader would.

/// The source of `awsglue/_table_data.py`.
pub(super) const TABLE_DATA_SOURCE: &str = r#"
"""Reading the rows of a Data Catalog table, through the catalog and the object store."""

import csv
import io
import json

import boto3
from awsglue import _bookmark
from pyspark import filesystem

# How much of a table ONE read takes. The bounds that decide what a RUN may spend are the
# interpreter's, in host code; these keep one read of a table from asking this emulator for
# an unbounded amount of it, and each is checked BEFORE the work it bounds is done — an
# object against the size the listing reports for it, a row against the rows already read.
MAX_OBJECTS = 1000
MAX_ROWS = 200000
MAX_BYTES = 64 * 1024 * 1024
MAX_PARTITIONS = 1000
# How WIDE a schema one read assembles. A record's field names are content the objects
# carry, so the number of distinct columns one read can be made to hold is bounded here the
# way the rows and the objects are.
MAX_COLUMNS = 1000
# How many CELLS one read MATERIALISES: every record carries one entry per column, so the
# two dimensions above multiply, and a width stated by catalog metadata or by one header row
# costs nothing of the byte budget — a narrow source read under a wide schema would hold
# `rows * columns` entries while passing every other bound. So what one read may hold is
# bounded on the PRODUCT as well, checked before each record is kept.
MAX_CELLS = 2000000

# The names a Spark output writes beside its data, which hold none of the table's rows.
MARKER_PREFIXES = ("_", ".")

# The classifications this emulator reads a table's data in: delimited text and JSON, the two
# text layouts. A table in a binary layout is one whose rows this emulator cannot resolve, and
# a read of it says so rather than answering rows nothing read.
TEXT_CLASSIFICATIONS = ("csv", "tsv", "text", "")
JSON_CLASSIFICATIONS = ("json", "jsonl", "ion")

# Hive's own default for a cell that holds no value, which a text-format table writes NULL as.
DEFAULT_NULL_FORMAT = "\\N"


class UnreadableTableData(Exception):
    """A table whose data layout this emulator cannot resolve into rows."""


class TableTooLarge(Exception):
    """A table holding more data than one read of it takes."""


# The format options a read of objects under a path HONOURS: the delimiter between cells,
# the quote character around one, and whether the objects carry a header line.
HONOURED_FORMAT_OPTIONS = ("separator", "delimiter", "quoteChar", "withHeader")


def truthy(value):
    """A connection option's flag, whether it arrived as a boolean or as its text.

    A client names such an option either way — `{"recurse": True}` and `{"recurse": "true"}`
    ask for the same thing — so both are read here, and anything else is the flag unset.
    """
    if isinstance(value, bool):
        return value
    if value is None:
        return False
    return str(value).strip().lower() in ("true", "1")


def refuse_unhonoured(given, honoured, what):
    """Refuse the options `given` that are not among `honoured`.

    An option changes what a read or a write DOES, so one this library performs nothing for
    is refused naming it rather than dropped — which would report a success for work that
    was not done.
    """
    extra = sorted(str(name) for name in (given or {}) if str(name) not in honoured)
    if extra:
        raise ValueError(
            "this emulator %s; the options %r are not ones it honours" % (what, extra)
        )


def glue_client():
    """The Data Catalog client this library makes its catalog calls through.

    It is the client the RUNTIME supplies the guest: its transport is this emulator's own
    in-process callback, signed as the role the run is performed as, so it resolves no
    endpoint, opens no connection and reads no credential of the host. Built here, once, so
    every catalog call this library makes travels the same seam.
    """
    return boto3.client("glue")


def _glue():
    return glue_client()


def _s3():
    """The object store client, built through the same runtime-supplied SDK."""
    return boto3.client("s3")


def table_of(database, table_name, catalog_id=None):
    """The stored table definition, read through the Data Catalog's own API."""
    request = {"DatabaseName": database, "Name": table_name}
    if catalog_id:
        request["CatalogId"] = catalog_id
    return _glue().get_table(**request)["Table"]


def _split_uri(location):
    """The (bucket, prefix) an Amazon S3 location names.

    Read by the shared Spark filesystem's own URI reading, which is the one place this
    workspace resolves an `s3://` address into the object it names.
    """
    scheme, authority, within = filesystem.split_uri(location)
    if scheme is None or scheme.lower() not in filesystem.S3_SCHEMES or not authority:
        raise UnreadableTableData(
            "the table's location %r does not name an Amazon S3 prefix" % (str(location),)
        )
    return (authority, within)


def _columns(descriptor):
    """The (name, type) pairs a storage descriptor declares, in declaration order."""
    declared = []
    for column in (descriptor or {}).get("Columns") or []:
        declared.append((column.get("Name") or "", column.get("Type") or "string"))
    return declared


def _layout(table, descriptor):
    """How the objects behind one location are laid out, as the table says they are.

    The property names are the Data Catalog's own, and the service reads the same ones
    host-side where IT computes over a table's data (`state::column_statistics_source`,
    `state::crawl`) — so a rule added there belongs here too, and the other way about.
    """
    parameters = table.get("Parameters") or {}
    serde = ((descriptor or {}).get("SerdeInfo") or {}).get("Parameters") or {}
    library = ((descriptor or {}).get("SerdeInfo") or {}).get("SerializationLibrary") or ""
    classification = str(parameters.get("classification") or "").lower()
    if not classification:
        if "json" in library.lower():
            classification = "json"
        elif "parquet" in library.lower():
            classification = "parquet"
        elif "orc" in library.lower():
            classification = "orc"
        elif "avro" in library.lower():
            classification = "avro"
    delimiter = (
        serde.get("field.delim")
        or serde.get("separatorChar")
        or ("\t" if classification == "tsv" else ",")
    )
    header_lines = 0
    for name in ("skip.header.line.count", "header.line.count"):
        value = parameters.get(name) or serde.get(name)
        if value:
            try:
                header_lines = int(value)
            except ValueError:
                header_lines = 0
            break
    return {
        "classification": classification,
        "delimiter": delimiter[:1] or ",",
        "quote": (serde.get("quoteChar") or '"')[:1] or '"',
        "header_lines": max(header_lines, 0),
        "null_format": serde.get("serialization.null.format", DEFAULT_NULL_FORMAT),
    }


def _within(prefix, key):
    """Where `key` sits under `prefix`: the part of it below the prefix's own level."""
    rest = key[len(prefix):]
    return rest[1:] if rest.startswith("/") else rest


def _data_keys(bucket, prefix, left, recurse=True, source=None):
    """The `(key, size, modified)` objects under one prefix that hold data, at most `left`.

    The SIZE comes from the listing, so how much a read is about to take is known before
    anything is fetched (see `_object_text`), and the MODIFIED instant is what a bookmarked
    read tells an object it has already processed from a new one (`_bookmark`).

    `recurse` is whether the read descends the sub-prefixes under the prefix it was given:
    with it unset only the objects at that prefix's own level are read, which is what a read
    that was not asked to recurse takes.

    `source` is the path the READ was given, which is the path a failure to resolve it names;
    the prefix being listed is what names it for a read that was given none.
    """
    keys = []
    token = None
    while True:
        request = {"Bucket": bucket, "Prefix": prefix}
        if token:
            request["ContinuationToken"] = token
        try:
            listed = _s3().list_objects_v2(**request)
        except Exception as error:
            # A listing the store REFUSES tells this read nothing about what is under the
            # location, which is what a location holding nothing tells it too: the read
            # resolved no file there, and that is the one failure the filesystem answers
            # with (`filesystem.missing_source`).
            if filesystem.is_denial(error):
                raise filesystem.missing_source(
                    source or filesystem.uri(bucket, prefix)
                )
            raise
        for entry in listed.get("Contents") or []:
            key = entry.get("Key") or ""
            name = key.rsplit("/", 1)[-1]
            if not name or name.startswith(MARKER_PREFIXES):
                continue
            if not recurse and "/" in _within(prefix, key):
                continue
            size = int(entry.get("Size") or 0)
            if size <= 0:
                continue
            # A table holding more objects than one read takes is a table this read cannot
            # cover: it says so rather than answering the first `left` of them, which would
            # be a frame missing rows reported as a complete one.
            if len(keys) >= left:
                raise TableTooLarge(
                    "one read of a table takes at most %d of the objects at its location"
                    % (MAX_OBJECTS,)
                )
            keys.append((key, size, _modified_at(entry.get("LastModified"))))
        token = listed.get("NextContinuationToken")
        if not token or not listed.get("IsTruncated"):
            return keys


def _modified_at(value):
    """The instant a listing reports for one object, as the whole epoch seconds a bookmarked
    read orders objects by (`_bookmark`).

    The SDK this runtime calls hands a member the called service's model types as a timestamp
    over as a native date-time, so the instant is read off that value rather than from a
    number; a listing carrying none reports the epoch, which is before every object.
    """
    if value is None:
        return 0
    if hasattr(value, "timestamp"):
        return int(value.timestamp())
    return int(value)


def _object_text(bucket, key, size, left_bytes, source=None):
    """The text of one object, and how many bytes of the read's budget it took.

    What one read may take is checked BEFORE the object is fetched: the listing's own size
    says how large it is, so an object the budget does not cover is refused rather than
    buffered and then rejected.

    `source` is the path the READ was given, as `_data_keys` takes it.
    """
    if size > left_bytes:
        raise TableTooLarge(
            "one read of a table takes at most %d bytes of it" % (MAX_BYTES,)
        )
    try:
        read = _s3().get_object(Bucket=bucket, Key=key)
    except Exception as error:
        # As with the listing above: a read the store refuses resolved no file at that path.
        if filesystem.is_denial(error):
            raise filesystem.missing_source(source or filesystem.uri(bucket, key))
        raise
    body = read["Body"].read()
    if isinstance(body, bytes) or isinstance(body, bytearray):
        return (bytes(body).decode("utf-8"), len(body))
    text = str(body)
    return (text, len(text.encode("utf-8")))


def _cast(value, declared):
    """One cell as the type the table declares for its column.

    A cell the declared type cannot hold is kept as the text it is, which is what a frame
    does with a value whose type is not the one declared: the record carries the value that
    was really there, and the script decides what to do about it.
    """
    if value is None:
        return None
    kind = base_hive_type(declared)
    text = str(value)
    if kind in ("int", "integer", "bigint", "smallint", "tinyint", "long"):
        try:
            return int(text)
        except ValueError:
            return text
    if kind in ("double", "float", "decimal", "real"):
        try:
            return float(text)
        except ValueError:
            return text
    if kind == "boolean":
        folded = text.strip().lower()
        if folded in ("true", "false"):
            return folded == "true"
        return text
    return text


def base_hive_type(declared):
    """The TYPE a declaration names, folded and without its parameter or member list.

    A table states its column types in Hive type syntax (`int`, `varchar(64)`,
    `decimal(10,2)`, `array<string>`), and neither the parameter list of a parameterised name
    nor the member list of a complex one changes WHICH type is named. Read in ONE place, so
    the cast of a cell ([`_cast`]) and the question of whether a value is one of its column's
    own type ([`holds_declared`]) cannot disagree about a declaration.
    """
    return str(declared or "string").lower().split("<")[0].split("(")[0].strip()


def holds_declared(value, declared):
    """Whether `value` is a value of the Hive type `declared`.

    A cell the declared type could not be read from is kept as the TEXT it was ([`_cast`]),
    so this is what tells such a value from one the column really holds — the two read the
    declaration through the same [`base_hive_type`], so a value `_cast` resolved is one this
    accepts. A declared type whose values this library does not resolve into a native one — a
    date, a timestamp, a nested type — holds whatever the source carried for it, so nothing
    is judged there.
    """
    if value is None:
        return True
    kind = base_hive_type(declared)
    if kind in ("int", "integer", "bigint", "smallint", "tinyint", "long"):
        return isinstance(value, int) and not isinstance(value, bool)
    if kind in ("double", "float", "decimal", "real"):
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if kind == "boolean":
        return isinstance(value, bool)
    return True


def _text_rows(text, columns, layout):
    """The rows of one delimited-text object, ONE AT A TIME as they are read.

    Yielded rather than collected, so a caller that has reached the rows one read may take
    stops part way through an object instead of after building every record in it.
    """
    reader = csv.reader(
        io.StringIO(text),
        delimiter=layout["delimiter"],
        quotechar=layout["quote"],
    )
    for index, cells in enumerate(reader):
        if index < layout["header_lines"]:
            continue
        if not cells or (len(cells) == 1 and cells[0] == ""):
            continue
        record = {}
        for position, (name, declared) in enumerate(columns):
            cell = cells[position] if position < len(cells) else None
            if cell is not None and cell == layout["null_format"]:
                cell = None
            record[name] = _cast(cell, declared)
        yield record


def _json_rows(text, columns, layout):
    """The rows of one JSON object, one record per document in it, as they are read."""
    declared = dict(columns)
    for line in text.split("\n"):
        stripped = line.strip()
        if not stripped:
            continue
        document = json.loads(stripped)
        if not isinstance(document, dict):
            raise UnreadableTableData(
                "a JSON row of this table is %s rather than an object"
                % (type(document).__name__,)
            )
        record = {}
        for name, _type in columns:
            record[name] = _cast(document.get(name), declared.get(name))
        for name, value in document.items():
            if name not in record:
                record[name] = value
        yield record


def _read_budget(columns, partition_keys):
    """What ONE read of a table may materialise, spent as its rows are kept.

    The catalog's own declared width is checked here, BEFORE any object is fetched: a record
    carries one entry per declared column, so the declared width multiplies every row the read
    builds and it is metadata the byte and row budgets do not count. The row allowance is the
    smaller of the row bound and what the cell bound leaves at that width ([`MAX_CELLS`]), so a
    narrow source read under a wide declared schema stops at the same ceiling as a wide source
    does.

    The rest is spent per record, because the declared width is not the whole width: a JSON
    document's own fields are CONTENT the objects carry, so what the records materialise
    together and how many distinct names they name are counted as they are kept
    (`_rows_under`), exactly as the read of the objects under a path counts them
    (`read_s3_paths`). `known` is the names that cost no further width — the declared columns
    and partition keys, plus every name admitted beyond them — and `extra` is those admitted
    ones in the order they were first seen, which is what the read reports as the part of the
    schema the records themselves named.
    """
    width = len(columns) + len(partition_keys)
    if width > MAX_COLUMNS:
        raise TableTooLarge(
            "one read of a table takes at most %d of its columns" % (MAX_COLUMNS,)
        )
    known = {}
    for name, _declared in list(columns) + list(partition_keys):
        known[name] = True
    return {
        "rows": min(MAX_ROWS, MAX_CELLS // max(width, 1)),
        "cells": MAX_CELLS,
        "bytes": MAX_BYTES,
        "known": known,
        "extra": [],
    }


def _rows_under(location, columns, layout, budget, transformation_ctx="", partition_keys=()):
    """Every row the objects under one location hold, within what is left of the bounds.

    `budget` is what the whole read may still materialise (`_read_budget`), spent here: the
    bytes each object takes, the rows kept, the entries those rows hold TOGETHER and the
    distinct names they name — `partition_keys` being the entries the caller adds to each row
    of a partitioned table after this returns, counted here because this is where the record is
    admitted.

    `transformation_ctx` is the context the read was named under, which is what a bookmarked
    run resumes against: with the bookmark in force the read takes only the objects that are
    new to that context (`_bookmark.unread`).
    """
    bucket, prefix = _split_uri(location)
    keys = _bookmark.unread(
        transformation_ctx, _data_keys(bucket, prefix, MAX_OBJECTS, True, location)
    )
    classification = layout["classification"]
    if classification in JSON_CLASSIFICATIONS:
        parse = _json_rows
    elif classification in TEXT_CLASSIFICATIONS:
        parse = _text_rows
    else:
        raise UnreadableTableData(
            "this emulator reads a table whose data is delimited text or JSON; %r is not a "
            "layout it can resolve into rows" % (classification,)
        )
    rows = []
    for key, size, _modified in keys:
        text, taken = _object_text(bucket, key, size, budget["bytes"], location)
        budget["bytes"] -= taken
        # The rows come one at a time, and every bound is checked BEFORE the record that
        # would spend it is kept — so a read that has taken all it may stops part way
        # through the object rather than after every record in it has been built.
        for row in parse(text, columns, layout):
            if budget["rows"] <= 0:
                raise TableTooLarge(
                    "one read of a table takes at most %d rows and %d cells of it"
                    % (MAX_ROWS, MAX_CELLS)
                )
            held = len(row) + len(partition_keys)
            if held > budget["cells"]:
                raise TableTooLarge(
                    "one read of a table materialises at most %d cells of it" % (MAX_CELLS,)
                )
            # A name the declared schema does not carry is content this record brought, so
            # it costs width the way a declared column does.
            for name in row:
                if name in budget["known"]:
                    continue
                if len(budget["known"]) >= MAX_COLUMNS:
                    raise TableTooLarge(
                        "one read of a table takes at most %d of its columns"
                        % (MAX_COLUMNS,)
                    )
                budget["known"][name] = True
                budget["extra"].append(name)
            budget["cells"] -= held
            budget["rows"] -= 1
            rows.append(row)
    return rows


def _partitions(database, table_name, catalog_id, predicate):
    """The partitions of one table, as the catalog holds them."""
    held = []
    token = None
    while True:
        request = {"DatabaseName": database, "TableName": table_name}
        if catalog_id:
            request["CatalogId"] = catalog_id
        if predicate:
            request["Expression"] = predicate
        if token:
            request["NextToken"] = token
        listed = _glue().get_partitions(**request)
        for partition in listed.get("Partitions") or []:
            # As with the objects at a location: a table holding more partitions than one
            # read covers is refused rather than read in part.
            if len(held) >= MAX_PARTITIONS:
                raise TableTooLarge(
                    "one read of a table takes at most %d of its partitions"
                    % (MAX_PARTITIONS,)
                )
            held.append(partition)
        token = listed.get("NextToken")
        if not token:
            return held


def read_catalog_table(
    database, table_name, catalog_id=None, push_down_predicate="",
    additional_options=None, transformation_ctx=""
):
    """The columns and the records of one catalog table.

    The table's metadata is read through the Data Catalog, and its data from the objects at
    the location the table (or each of its partitions) names. A partitioned table's records
    carry its partition keys too, with the values of the partition they were read from, which
    is what a frame read from a partitioned table holds.
    """
    # The options a read can be GIVEN beyond the table it names — a bounded-execution
    # option, a schema to read the data under — change what the read does, and this reader
    # performs none of them: it says so rather than reading the table as if the options had
    # not been passed.
    if additional_options:
        raise ValueError(
            "this emulator reads a catalog table as the catalog describes it; the "
            "additional options %r are not ones it honours" % (sorted(additional_options),)
        )
    table = table_of(database, table_name, catalog_id)
    descriptor = table.get("StorageDescriptor") or {}
    columns = _columns(descriptor)
    partition_keys = [
        (key.get("Name") or "", key.get("Type") or "string")
        for key in table.get("PartitionKeys") or []
    ]
    layout = _layout(table, descriptor)
    # What this read may materialise, and the declared width checked before any object is
    # fetched ([`_read_budget`]).
    budget = _read_budget(columns, partition_keys)
    records = []
    if partition_keys:
        for partition in _partitions(database, table_name, catalog_id, push_down_predicate):
            partition_descriptor = partition.get("StorageDescriptor") or descriptor
            location = partition_descriptor.get("Location") or descriptor.get("Location")
            if not location:
                continue
            values = partition.get("Values") or []
            held = dict(
                (name, _cast(values[index] if index < len(values) else None, declared))
                for index, (name, declared) in enumerate(partition_keys)
            )
            rows = _rows_under(
                location,
                columns,
                _layout(table, partition_descriptor),
                budget,
                transformation_ctx,
                partition_keys,
            )
            for row in rows:
                row.update(held)
                records.append(row)
    else:
        location = descriptor.get("Location")
        if not location:
            raise UnreadableTableData(
                "the table %s.%s declares no location to read its data from"
                % (database, table_name)
            )
        records = _rows_under(location, columns, layout, budget, transformation_ctx)
    # The schema the frame holds: what the catalog declares, its partition keys, and the names
    # the records themselves named beyond them — the fields of a JSON document are content the
    # objects carry, so a record carrying one is reported under a column for it rather than
    # under a schema that does not describe it.
    named = [(name, "string") for name in budget["extra"]]
    return (columns + partition_keys + named, records)


def table_format(table):
    """The layout the stored table declares, which is the format a write of it is in."""
    descriptor = table.get("StorageDescriptor") or {}
    return _layout(table, descriptor)["classification"] or "csv"


def read_s3_paths(
    paths, format=None, format_options=None, recurse=False, transformation_ctx=""
):
    """The columns and records the objects under `paths` hold, in the format named.

    The columns of delimited text are the ones its header names when the read was told the
    objects carry one, and `col0`, `col1`, … otherwise — which is what the Glue reader calls
    the columns of headerless text.

    `recurse` is whether the read descends the sub-prefixes under each path, and
    `transformation_ctx` the context a bookmarked run resumes against — with the bookmark in
    force the read takes only the objects that are new to that context.

    A path the read RESOLVED NO FILE at — the store holds no data object under it, or the
    role the run is performed as is refused the objects there — is the filesystem's own
    missing-path failure, naming that path (`filesystem.missing_source`), rather than a frame
    of no records reported as a read of the path.
    """
    options = dict(format_options or {})
    refuse_unhonoured(
        options,
        HONOURED_FORMAT_OPTIONS,
        "reads the objects under a path as delimited text or as JSON documents",
    )
    kind = str(format or "csv").lower()
    delimiter = str(options.get("separator") or options.get("delimiter") or ",")[:1] or ","
    quote = str(options.get("quoteChar") or '"')[:1] or '"'
    with_header = bool(options.get("withHeader"))
    columns = []
    # The names `columns` already holds, for an O(1) answer to "is this column known": a
    # record's field names come from the objects' own content, so the membership test may not
    # be a walk of every name read so far.
    known = {}
    records = []
    left_bytes = MAX_BYTES
    # How many entries the records kept so far hold together ([`MAX_CELLS`]).
    cells = 0
    for path in list(paths or []):
        bucket, prefix = _split_uri(path)
        resolved = _data_keys(bucket, prefix, MAX_OBJECTS, recurse, path)
        # A path this read resolved no file at is the filesystem's own missing-path failure,
        # naming the path the read was GIVEN: the store holding nothing under it and the
        # store refusing the role that is reading it are the same answer, so neither is a
        # frame of no records reported as a read of the path.
        if not resolved:
            raise filesystem.missing_source(path)
        listed = _bookmark.unread(transformation_ctx, resolved)
        for key, size, _modified in listed:
            text, taken = _object_text(bucket, key, size, left_bytes, path)
            left_bytes -= taken
            if kind in JSON_CLASSIFICATIONS:
                read = _json_documents(text)
            elif kind in TEXT_CLASSIFICATIONS:
                read = _delimited_documents(text, delimiter, quote, with_header, columns)
            else:
                raise UnreadableTableData(
                    "this emulator reads delimited text or JSON; %r is not a format it can "
                    "resolve into records" % (kind,)
                )
            # One record at a time, and every bound checked BEFORE the thing it bounds is
            # kept, as the catalog-table read does it: the rows already read, and the
            # COLUMNS they have named so far — a record's field names are content the
            # objects carry, so how wide a schema one read assembles is bounded too, and
            # whether a name is already known is answered from a mapping rather than by
            # walking the names read so far.
            for record in read:
                if len(records) >= MAX_ROWS:
                    raise TableTooLarge(
                        "one read takes at most %d rows of the objects it reads" % (MAX_ROWS,)
                    )
                # And what the rows MATERIALISE together, which is the product of the two
                # dimensions ([`MAX_CELLS`]): the entries this record would add are counted
                # before it is kept, so a narrow source read under a wide header stops at
                # the same ceiling as a wide source does.
                cells += len(record)
                if cells > MAX_CELLS:
                    raise TableTooLarge(
                        "one read materialises at most %d cells of the objects it reads"
                        % (MAX_CELLS,)
                    )
                for name in record:
                    if name in known:
                        continue
                    if len(columns) >= MAX_COLUMNS:
                        raise TableTooLarge(
                            "one read takes at most %d columns of the objects it reads"
                            % (MAX_COLUMNS,)
                        )
                    known[name] = True
                    columns.append(name)
                records.append(record)
    return ([(name, "string") for name in columns], records)


def _json_documents(text):
    """One record per JSON document of an object, as each is read."""
    for line in text.split("\n"):
        stripped = line.strip()
        if not stripped:
            continue
        document = json.loads(stripped)
        if not isinstance(document, dict):
            raise UnreadableTableData(
                "a JSON record is %s rather than an object" % (type(document).__name__,)
            )
        yield document


def _delimited_documents(text, delimiter, quote, with_header, known_columns):
    """One record per row of a delimited-text object, as each is read.

    The columns are the ones the object's own header names when the read was told it carries
    one, else the ones already known, else `col0`, `col1`, … for as many cells as the first
    row holds.
    """
    reader = csv.reader(io.StringIO(text), delimiter=delimiter, quotechar=quote)
    names = list(known_columns)
    pending_header = bool(with_header)
    for cells in reader:
        if not cells or (len(cells) == 1 and cells[0] == ""):
            continue
        # How many columns a row may name is bounded for the reason `MAX_COLUMNS` gives: a
        # header row is content the object carries, so one row may not decide how wide every
        # record this read builds is.
        if len(cells) > MAX_COLUMNS:
            raise TableTooLarge(
                "one read takes at most %d columns of the objects it reads" % (MAX_COLUMNS,)
            )
        if pending_header:
            names = [str(cell) for cell in cells]
            pending_header = False
            continue
        if not names:
            names = ["col%d" % (position,) for position in range(len(cells))]
        record = {}
        for position, name in enumerate(names):
            record[name] = cells[position] if position < len(cells) else None
        yield record
"#;

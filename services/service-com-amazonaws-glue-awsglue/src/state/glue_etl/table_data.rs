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
from pyspark import filesystem

# How much of a table ONE read takes. The bounds that decide what a RUN may spend are the
# interpreter's, in host code; these keep one read of a table from asking this emulator for
# an unbounded amount of it, and each is checked BEFORE the work it bounds is done — an
# object against the size the listing reports for it, a row against the rows already read.
MAX_OBJECTS = 1000
MAX_ROWS = 200000
MAX_BYTES = 64 * 1024 * 1024
MAX_PARTITIONS = 1000

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


def _data_keys(bucket, prefix, left):
    """The `(key, size)` pairs under one prefix that hold a table's data, at most `left`.

    The SIZE comes from the listing, so how much a read is about to take is known before
    anything is fetched (see `_object_text`).
    """
    keys = []
    token = None
    while True:
        request = {"Bucket": bucket, "Prefix": prefix}
        if token:
            request["ContinuationToken"] = token
        listed = _s3().list_objects_v2(**request)
        for entry in listed.get("Contents") or []:
            key = entry.get("Key") or ""
            name = key.rsplit("/", 1)[-1]
            if not name or name.startswith(MARKER_PREFIXES):
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
            keys.append((key, size))
        token = listed.get("NextContinuationToken")
        if not token or not listed.get("IsTruncated"):
            return keys


def _object_text(bucket, key, size, left_bytes):
    """The text of one object, and how many bytes of the read's budget it took.

    What one read may take is checked BEFORE the object is fetched: the listing's own size
    says how large it is, so an object the budget does not cover is refused rather than
    buffered and then rejected.
    """
    if size > left_bytes:
        raise TableTooLarge(
            "one read of a table takes at most %d bytes of it" % (MAX_BYTES,)
        )
    read = _s3().get_object(Bucket=bucket, Key=key)
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
    kind = str(declared or "string").lower().split("<")[0].strip()
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


def _rows_under(location, columns, layout, left_rows, left_bytes):
    """Every row the objects under one location hold, within what is left of the bounds."""
    bucket, prefix = _split_uri(location)
    keys = _data_keys(bucket, prefix, MAX_OBJECTS)
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
    for key, size in keys:
        text, taken = _object_text(bucket, key, size, left_bytes)
        left_bytes -= taken
        # The rows come one at a time, and the bound is checked BEFORE each is kept — so a
        # read that has taken all the rows it may stops part way through the object rather
        # than after every record in it has been built.
        for row in parse(text, columns, layout):
            if len(rows) >= left_rows:
                raise TableTooLarge(
                    "one read of a table takes at most %d rows of it" % (MAX_ROWS,)
                )
            rows.append(row)
    return (rows, left_bytes)


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
    database, table_name, catalog_id=None, push_down_predicate="", additional_options=None
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
    left_rows = MAX_ROWS
    left_bytes = MAX_BYTES
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
            rows, left_bytes = _rows_under(
                location,
                columns,
                _layout(table, partition_descriptor),
                left_rows - len(records),
                left_bytes,
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
        records, left_bytes = _rows_under(location, columns, layout, left_rows, left_bytes)
    return (columns + partition_keys, records)


def table_format(table):
    """The layout the stored table declares, which is the format a write of it is in."""
    descriptor = table.get("StorageDescriptor") or {}
    return _layout(table, descriptor)["classification"] or "csv"


def read_s3_paths(paths, format=None, format_options=None):
    """The columns and records the objects under `paths` hold, in the format named.

    The columns of delimited text are the ones its header names when the read was told the
    objects carry one, and `col0`, `col1`, … otherwise — which is what the Glue reader calls
    the columns of headerless text.
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
    records = []
    left_bytes = MAX_BYTES
    for path in list(paths or []):
        bucket, prefix = _split_uri(path)
        for key, size in _data_keys(bucket, prefix, MAX_OBJECTS):
            text, taken = _object_text(bucket, key, size, left_bytes)
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
            # One record at a time, and the bound checked before each is kept, as the
            # catalog-table read does it.
            for record in read:
                if len(records) >= MAX_ROWS:
                    raise TableTooLarge(
                        "one read takes at most %d rows of the objects it reads" % (MAX_ROWS,)
                    )
                for name in record:
                    if name not in columns:
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

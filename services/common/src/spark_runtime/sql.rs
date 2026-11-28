// SPDX-License-Identifier: CC0-1.0

//! `pyspark.sql`: the session a program starts and the dataset of rows it hands out.

/// The source of `pyspark/sql/__init__.py`.
pub(super) const SQL_SOURCE: &str = r#"
"""The Spark session of one application, and the rows a dataset of them holds."""

from pyspark import catalog, filesystem, parquet
from pyspark.conf import SparkConf
from pyspark.context import SparkContext
from pyspark.jvm import JavaSparkContext, SPARK_VERSION, gateway
from pyspark.rdd import RDD, slices

# Whether a write leaves the marker object beside its part files: the setting of the
# environment's OWN output committer, which is not the same for every environment that runs a
# Spark program ([`pyspark.release`]).
from pyspark.release import MARKS_SUCCESSFUL_JOBS

# The type system a schema is declared in lives where PySpark's own does, and `pyspark.sql`
# re-exports it, so a program that imports a type from either resolves it.
from pyspark.sql.types import (
    BIGINT,
    BINARY,
    BOOLEAN,
    DATE,
    DOUBLE,
    STRING,
    TIMESTAMP,
    VOID,
    ArrayType,
    BinaryType,
    BooleanType,
    ByteType,
    DataType,
    DateType,
    DecimalType,
    DoubleType,
    FloatType,
    IntegerType,
    LongType,
    MapType,
    NullType,
    Row,
    ShortType,
    StringType,
    StructField,
    StructType,
    TimestampNTZType,
    TimestampType,
    type_by_name,
)

# The failure a read that cannot be planned raises, which is the filesystem's one reading of
# that class.
AnalysisException = filesystem.AnalysisException


class SparkException(Exception):
    """A failure of the engine itself, as Spark raises one."""


# The column names a read gives the columns of a source that names none, as Spark names them.
UNNAMED_COLUMN = "_c%d"

# The KIND of join each spelling of `DataFrame.join`'s `how` names, mapped onto the one this
# engine matches by. Source: `DataFrame.join`'s own `how` — "Must be one of: inner, cross,
# outer, full, fullouter, full_outer, left, leftouter, left_outer, right, rightouter,
# right_outer, semi, leftsemi, left_semi, anti, leftanti, left_anti" — read here with the
# underscores taken out, so each documented spelling of a kind resolves to it. A `cross` named
# with a join condition keeps the rows that satisfy it, which is the inner join of them.
JOIN_KINDS = {
    "inner": "inner",
    "cross": "inner",
    "outer": "outer",
    "full": "outer",
    "fullouter": "outer",
    "left": "left",
    "leftouter": "left",
    "right": "right",
    "rightouter": "right",
    "semi": "semi",
    "leftsemi": "semi",
    "anti": "anti",
    "leftanti": "anti",
}

# What a join whose `how` names no kind of join is refused with, as Spark words that refusal.
UNSUPPORTED_JOIN = (
    "Unsupported join type '%s'. Supported join types include: 'Inner', 'LeftOuter', "
    "'RightOuter', 'FullOuter', 'LeftSemi', 'LeftAnti', 'Cross'."
)

# How a read treats a record it cannot read under the schema it was given, as the `mode`
# option names it: the default nulls the field it could not read, `DROPMALFORMED` drops the
# whole record, and `FAILFAST` fails the read. A mode outside these is one Spark reads as the
# default.
PARSE_PERMISSIVE = "permissive"
PARSE_DROPMALFORMED = "dropmalformed"
PARSE_FAILFAST = "failfast"
PARSE_MODES = (PARSE_PERMISSIVE, PARSE_DROPMALFORMED, PARSE_FAILFAST)

# What a read in the failing-fast mode says about the record it could not read: the fixed
# text of that failure, naming the RECORD it was reading, the mode in force, and the mode that
# would have read the record as a null instead.
MALFORMED_RECORD = (
    "Malformed records are detected in record parsing: [%s]. Parse Mode: FAILFAST. To "
    "process malformed records as null result, try setting the option 'mode' as 'PERMISSIVE'."
)

# The name the Hive layout gives the partition directory of a value that is not there, which
# is what a partition column holding no value is written under and read back from.
HIVE_NULL_PARTITION = "__HIVE_DEFAULT_PARTITION__"

# How a part file of a write is NAMED: the `part-` prefix, the zero-padded index of the task
# that wrote it, the UUID the write job was minted with, the `-c` file counter of that task,
# and the extension of the format the part was written in. That is the template Spark's file
# output committer lands a part under, and it is what lets a client attribute a file to the
# write that produced it and select the parts of a format by their suffix.
PART_NAME = "part-%05d-%s-c%03d%s"

# The counter a part carries: it tells apart the files ONE task of ONE write leaves for the
# same output directory, and a task that writes its rows as a single file leaves the first.
FILE_COUNTER = 0

# The extension a part carries for the format that wrote it, where the format is not named by
# its own extension: the text source writes `.txt` files. Every other format's parts carry the
# name of the format itself (`.json`, `.csv`, `.parquet`).
FORMAT_SUFFIXES = {"text": ".txt"}

# The expression surface a projection, a filter and a grouping are given, and the primitives
# for reading a value as a type and for folding a column's values into one: stated ONCE,
# where PySpark's own column expressions live, and used by both this engine and the function
# catalog over it ([`pyspark.sql.column`], [`pyspark.sql.functions`]).
from pyspark.sql.column import (
    CAST_TYPES,
    Column,
    aggregate as _aggregate_of,
    cast_value as _cast_value,
    field as _field,
    sort_key as _sort_key,
    type_of as _type_of,
)

# The SQL EXPRESSION GRAMMAR a projection's expressions are read by, stated once beside the
# expression surface it builds ([`pyspark.sql.expression`]).
from pyspark.sql.expression import read_sql_expression

# WHERE a statement this session submits is planned and run: the engine the environment the
# program runs in supplies, reached as [`pyspark.sql.statements`] reaches it.
from pyspark.sql.statements import run as _submitted_statement


def _range_aggregate(function, span):
    """The aggregate of an arithmetic progression, from the progression's own definition.

    The values of a `range` are an arithmetic progression, so its count, its bounds and its
    sum follow from its first value, its last and how many there are — exactly, in whole
    numbers, and without walking the progression. That is what lets an aggregate over a
    dataset of two hundred million values be answered at all: the answer is the one walking it
    would give, computed from the definition of the values rather than by adding them one by
    one, which is also why it is EXACT where a float accumulation would not be.
    """
    count = len(span)
    if function == "count":
        return count, BIGINT
    if count == 0:
        # Every aggregate of no rows is null, as Spark reports one.
        return None, BIGINT
    first = span[0]
    last = span[count - 1]
    if function == "first":
        return first, BIGINT
    if function == "last":
        return last, BIGINT
    # The product is always even: with an odd count the bounds sum to an even number, so the
    # halving below is exact in whole numbers whichever way round the progression runs.
    total = (first + last) * count // 2
    if function == "sum":
        return total, BIGINT
    if function == "min":
        return min(first, last), BIGINT
    if function == "max":
        return max(first, last), BIGINT
    if function in ("avg", "mean"):
        return float(total) / count, DOUBLE
    raise AnalysisException("Undefined function: %s" % (function,))


def _truth(value):
    """A reader option's value as the boolean Spark reads it as."""
    if isinstance(value, bool):
        return value
    return str(value).strip().lower() in ("true", "yes", "1")



def _typed(value):
    """One field as the value an inferred schema gives it."""
    text = value.strip()
    if not text:
        return None
    try:
        return int(text)
    except ValueError:
        pass
    try:
        return float(text)
    except ValueError:
        return value


def _hidden(path):
    """Whether a file is one a read of a directory passes over.

    The files whose name begins with `_` or `.` are the markers and the staging entries the
    Hadoop output committers leave (`_SUCCESS` among them), which is why a read of an output
    directory does not take them for data.
    """
    return path.rstrip("/").rsplit("/", 1)[-1][:1] in ("_", ".")


def _partition_values(path, held):
    """The partition columns the path of one file carries, by the Hive layout's spelling.

    A partitioned write holds the value of each partition column in a `<name>=<value>` path
    segment rather than in the files, so these are the columns a read of that directory
    recovers from the segments between the path it was asked for and the file itself. The
    value is read as the value its text spells, which is the inference Spark makes over a
    partition column, and the name Hive gives the partition of a null is read back as one.
    """
    root = filesystem._text(path).rstrip("/")
    if not held.startswith(root):
        return {}
    values = {}
    for segment in held[len(root) :].strip("/").split("/")[:-1]:
        if "=" not in segment:
            continue
        name, _marker, value = segment.partition("=")
        if name:
            held_value = filesystem.unescape_path_name(value)
            values[filesystem.unescape_path_name(name)] = (
                None if held_value == HIVE_NULL_PARTITION else _typed(held_value)
            )
    return values


def _write_job_uuid():
    """The UUID of ONE write job, which every part file that write leaves carries.

    Spark mints it per write — not per part and not per application — so the parts of one
    write share it and the parts of another do not: that is what attributes a file to the
    write that produced it, and what keeps a later write of the same output from taking the
    names of an earlier one's files.
    """
    import uuid

    return str(uuid.uuid4())


def _format_suffix(kind):
    """The extension a part file written in the format `kind` carries ([`FORMAT_SUFFIXES`])."""
    return FORMAT_SUFFIXES.get(kind, ".%s" % (kind,))


def _part_name(index, job, suffix):
    """The name one part file of a write takes ([`PART_NAME`]).

    `index` is the task that wrote it — the index of the dataset partition whose rows it
    holds — `job` the UUID of the write job, and `suffix` the extension of the format it was
    written in, which for a columnar part carries the codec its pages are stored under ahead
    of the format itself.
    """
    return PART_NAME % (index, job, FILE_COUNTER, suffix)


def _partition_text(value):
    """One partition column's value, as the Hive layout spells it in a path.

    A boolean is spelled in the lower case the layout's own grammar writes it in, a value
    the row does not hold is the partition Hive names for the absent value, and every other
    value is the text it spells — through the layout's own escaping
    ([`pyspark.filesystem.escape_path_name`]), because the value is DATA: a cell spelling a
    path separator or a parent directory may not name an object outside the path the write
    was given.
    """
    if value is None:
        return HIVE_NULL_PARTITION
    if isinstance(value, bool):
        return "true" if value else "false"
    return filesystem.escape_path_name(str(value))


def _partition_of(within, column):
    """The value one partition column holds in the path a partitioned write laid rows under.

    The path is the Hive layout's `<name>=<value>` segments ([`DataFrameWriter._grouped`]), so
    the value is the one the segment of that column spells — read back through the layout's own
    escaping, and left as the name Hive gives the partition of an absent value where that is
    what the segment carries, because that is the value the catalog holds for it.
    """
    for segment in str(within).strip("/").split("/"):
        name, _marker, value = segment.partition("=")
        if filesystem.unescape_path_name(name) == column:
            return filesystem.unescape_path_name(value)
    return ""


class DataFrameReader(object):
    """Where a dataset of rows is READ from: the objects its source path names.

    "Returns a DataFrameReader that can be used to read data in as a DataFrame"
    (`SparkSession.read`). A path naming one object reads that object and a path naming a
    prefix reads every object under it, which is how a Spark source addresses a directory of
    part files; the read is made through the filesystem, so it is made as the identity the
    program runs as and a refused one raises that refusal.
    """

    def __init__(self, session=None):
        self._session = session
        self._options = {}
        self._format = None
        self._columns = None
        self._declared = {}

    def format(self, source):
        self._format = str(source).strip().lower()
        return self

    def option(self, key, value):
        self._options[str(key).strip().lower()] = value
        return self

    def options(self, **named):
        for key, value in named.items():
            self.option(key, value)
        return self

    def schema(self, schema):
        """The schema the read is made under: the column names, and the types declared.

        A schema states what each column HOLDS, so a value a read takes in is read as the
        type its own column declares rather than as the text it arrived as — which is what
        makes a read under a schema answer the same values whatever format carried them. A
        schema given as names alone declares no types, and a value then stands as it came.
        """
        self._declared = {}
        if hasattr(schema, "fields"):
            self._columns = [field.name for field in schema.fields]
            for field in schema.fields:
                self._declared[field.name] = field.dataType.simpleString()
        elif hasattr(schema, "fieldNames"):
            self._columns = list(schema.fieldNames())
        elif isinstance(schema, (list, tuple)):
            self._columns = [str(column) for column in schema]
        elif isinstance(schema, str):
            # The data-definition spelling of a schema: `<name> <type>` per column.
            self._columns = []
            for field in schema.split(","):
                stated = field.strip().split()
                if not stated:
                    continue
                self._columns.append(stated[0])
                if len(stated) > 1:
                    self._declared[stated[0]] = " ".join(stated[1:]).lower()
        return self

    def _declared_value(self, name, value):
        """One value as the type its column declares holds it.

        A value the declared type cannot be read from is null, which is what a read in the
        permissive mode Spark reads in by default answers for a malformed one. A type this
        engine reads no value as leaves the value as it came.
        """
        kind = self._declared.get(name)
        if kind is None or kind not in CAST_TYPES:
            return value
        return _cast_value(value, kind)

    def _declared_types(self, columns):
        """The type each declared column carries, by the name a schema spells it."""
        held = {}
        for name in columns or []:
            kind = self._declared.get(name)
            if kind in CAST_TYPES:
                held[name] = CAST_TYPES[kind][0]
        return held

    def _read(self, kind, path):
        if path is None:
            raise AnalysisException("a read names no path")
        read = filesystem.source_objects(path)
        if kind in ("csv", "com.databricks.spark.csv"):
            return self._csv(read)
        if kind == "json":
            return self._json(read)
        if kind == "text":
            return self._text(read)
        if kind == "parquet":
            return self._parquet(read, path)
        raise AnalysisException(
            "This emulator reads a Spark source in the csv, json, text and parquet "
            "formats; %s names another one" % (kind,)
        )

    def _csv(self, read):
        import csv
        import io

        header = _truth(self._options.get("header", False))
        infer = _truth(self._options.get("inferschema", False))
        separator = str(self._options.get("sep", self._options.get("delimiter", ",")))
        mode = self._parse_mode()
        columns = list(self._columns) if self._columns else None
        rows = []
        for _held, body in read:
            fields = csv.reader(
                io.StringIO(body.decode("utf-8")), delimiter=separator or ","
            )
            lines = [line for line in fields if line]
            if not lines:
                continue
            start = 0
            if header:
                if columns is None:
                    columns = [str(name).strip() for name in lines[0]]
                start = 1
            for line in lines[start:]:
                values = [_typed(value) for value in line] if infer else list(line)
                named = columns or [
                    UNNAMED_COLUMN % (index,) for index in range(len(values))
                ]
                if columns is None:
                    columns = list(named)
                record = dict(zip(named, values))
                malformed = False
                for name in list(record):
                    held = self._declared_value(name, record[name])
                    if held is None and self._unreadable(name, record[name]):
                        malformed = True
                    record[name] = held
                if malformed:
                    # A record the schema cannot be read over is treated as the read's own
                    # mode says: the failing-fast mode fails the read naming the record it
                    # was reading and the mode in force, the dropping one leaves the record
                    # out, and the default one keeps it with the field it could not read as a
                    # null.
                    if mode == PARSE_FAILFAST:
                        raise SparkException(
                            MALFORMED_RECORD
                            % ((separator or ",").join(str(value) for value in line),)
                        )
                    if mode == PARSE_DROPMALFORMED:
                        continue
                rows.append(Row(**record))
        return DataFrame(rows, columns or [], 1, None, self._declared_types(columns))

    def _parse_mode(self):
        """How this read treats a record it cannot read, as its `mode` option names it."""
        held = str(self._options.get("mode", PARSE_PERMISSIVE)).strip().lower()
        if held not in PARSE_MODES:
            # A mode Spark does not know is one it reads in the default mode.
            return PARSE_PERMISSIVE
        return held

    def _unreadable(self, name, value):
        """Whether `value` is a value the type column `name` declares cannot be read from.

        A field the source holds NOTHING in is a null rather than a malformed one — that is
        what an empty cell means — so only a value that is there and cannot be read as the
        declared type makes the record a malformed one.
        """
        if name not in self._declared or self._declared[name] not in CAST_TYPES:
            return False
        if value is None:
            return False
        return str(value).strip() != ""

    def _json(self, read):
        import json

        rows = []
        columns = list(self._columns) if self._columns else []
        for _held, body in read:
            for line in body.decode("utf-8").splitlines():
                if not line.strip():
                    continue
                record = json.loads(line)
                if not isinstance(record, dict):
                    record = {UNNAMED_COLUMN % (0,): record}
                for name in record:
                    if name not in columns:
                        columns.append(name)
                rows.append(Row(**record))
        return DataFrame(rows, columns)

    def _text(self, read):
        rows = []
        for _held, body in read:
            for line in body.decode("utf-8").splitlines():
                rows.append(Row(**{"value": line}))
        return DataFrame(rows, ["value"])

    def _parquet(self, read, path):
        """The dataset the Parquet files under a path hold, partition columns and all.

        Each file is READ as the Parquet file it is ([`pyspark.parquet`]), so the rows are the
        rows it holds and each column carries the type its own schema states. A directory
        written out PARTITIONED holds its partition columns in the path rather than in the
        files, as the Hive layout puts them, so a read of such a directory recovers each one
        from the `<name>=<value>` segments under the path it was asked for — which is what
        makes a partitioned write read back as the dataset that was written.
        """
        columns = []
        types = {}
        rows = []
        for held, body in read:
            if _hidden(held):
                continue
            try:
                named, kinds, values = parquet.read_file(body)
            except parquet.ParquetError as error:
                raise AnalysisException("%s: %s" % (held, error))
            partitioned = _partition_values(path, held)
            for index, name in enumerate(named):
                if name not in columns:
                    columns.append(name)
                if name not in types:
                    types[name] = kinds[index]
            for name in partitioned:
                if name not in columns:
                    columns.append(name)
                if name not in types and partitioned[name] is not None:
                    types[name] = _type_of(partitioned[name])
            for row in values:
                record = dict(zip(named, row))
                record.update(partitioned)
                rows.append(Row(**record))
        for name in columns:
            if name not in types:
                types[name] = STRING
        return DataFrame(rows, columns, 1, None, types)

    def load(self, path=None, format=None, schema=None, **named):
        if schema is not None:
            self.schema(schema)
        self.options(**named)
        # The format a read names NOTHING for is the session's default source format,
        # `spark.sql.sources.default`, which is `parquet`.
        return self._read(format or self._format or "parquet", path)

    def csv(self, path, schema=None, **named):
        if schema is not None:
            self.schema(schema)
        self.options(**named)
        return self._read("csv", path)

    def json(self, path, schema=None, **named):
        if schema is not None:
            self.schema(schema)
        self.options(**named)
        return self._read("json", path)

    def text(self, path, **named):
        self.options(**named)
        return self._read("text", path)

    def parquet(self, *paths, **named):
        self.options(**named)
        if not paths:
            raise AnalysisException("a read names no path")
        read = self._read("parquet", paths[0])
        for path in paths[1:]:
            read = read.union(self._read("parquet", path))
        return read


# The save modes a write may name, as `DataFrameWriter.mode` documents them — `append`,
# `overwrite`, `error`/`errorifexists` and `ignore` — with the `default` spelling Spark's own
# reading of the member maps onto `errorifexists` beside them. A value outside this set names no
# behaviour, which is what makes it a refusal rather than a choice.
SAVE_MODES = ("append", "overwrite", "error", "errorifexists", "default", "ignore")

# The modes that REFUSE an output already there, which is what a write naming no mode does.
REFUSING_MODES = ("error", "errorifexists", "default")


class DataFrameWriter(object):
    """Where a dataset of rows is WRITTEN: through the filesystem its path resolves in.

    The output is the layout Spark leaves: one part file per non-empty partition of the
    dataset under the path, named as the write job that produced it names one
    ([`_part_name`]), the `_SUCCESS` marker beside them, and — where the write names
    columns to partition BY — one `name=value` directory level per such column, holding the
    part files of the rows whose values those are. The partition columns are in the
    directories rather than in the files, which is what a read of the output resolves them
    from again.

    What a write does with an output ALREADY at that destination is its SAVE MODE's
    ([`mode`], [`_committed`]): every write of this class goes through that, so a path write
    and a table write treat what is there the same way.
    """

    def __init__(self, frame):
        self._frame = frame
        self._mode = "errorifexists"
        self._format = None
        self._options = {}
        self._partitions = []

    def mode(self, saveMode):
        """The save mode this write names, which decides what an output ALREADY THERE means.

        "Specifies the behavior when data or table already exists" (`DataFrameWriter.mode`):
        `append`, `overwrite`, `error`/`errorifexists` (the default) and `ignore`. A value
        outside them names no behaviour at all, so it is REFUSED the way Spark refuses one
        rather than read as any of them — a misspelled `overwrite` must not be what deletes a
        table's data.
        """
        held = str(saveMode).lower()
        if held not in SAVE_MODES:
            raise ValueError(
                "Unknown save mode: %s. Accepted save modes are 'overwrite', 'append', "
                "'ignore', 'error', 'errorifexists'." % (saveMode,)
            )
        self._mode = held
        return self

    def format(self, source):
        self._format = str(source).lower()
        return self

    def option(self, key, value):
        self._options[str(key).strip().lower()] = value
        return self

    def options(self, **named):
        for key, value in named.items():
            self.option(key, value)
        return self

    def partitionBy(self, *columns):
        """The columns the output is PARTITIONED by, as `partitionBy` names them."""
        named = []
        for column in columns:
            if isinstance(column, (list, tuple)):
                named.extend(str(held) for held in column)
            else:
                named.append(str(column))
        for column in named:
            if column not in self._frame.columns:
                raise AnalysisException(
                    "Partition column `%s` not found in schema %s"
                    % (column, self._frame.schema.simpleString())
                )
        self._partitions = named
        return self

    def _rendered(self, kind, rows):
        if kind == "json":
            import json

            return [
                json.dumps(row.asDict()) if isinstance(row, Row) else json.dumps(row)
                for row in rows
            ]
        return [",".join(str(value) for value in row) for row in rows]

    def _grouped(self):
        """The rows by the values of the partition columns, in the order they first appear.

        A write with no partition columns is the one group of every row, written at the path
        itself; one with them leaves a directory per combination of their values, which is
        the Hive layout a partitioned write lands.
        """
        rows = self._frame.collect()
        if not self._partitions:
            return [("", rows)]
        order = []
        grouped = {}
        for row in rows:
            within = "/".join(
                "%s=%s" % (filesystem.escape_path_name(column), _partition_text(row[column]))
                for column in self._partitions
            )
            if within not in grouped:
                grouped[within] = []
                order.append(within)
            grouped[within].append(row)
        return [(within, grouped[within]) for within in order]

    def _committed(self, kind, path, mode=None):
        """Write this dataset out at `path`, as the save mode in force treats what is THERE.

        "Specifies the behavior when data or table already exists" (`DataFrameWriter.mode`),
        and what is already at a destination is the objects under it — whatever wrote them,
        and whether or not anything registered a table over them. So `overwrite` REPLACES
        them: the objects the destination holds are DELETED before this write's own are
        committed, which is what makes a second overwrite write of one output leave exactly
        one copy of its rows instead of a copy beside the one already there. `append` leaves
        this write's parts beside them ([`_write`]'s `part_offset`), `ignore` leaves the
        output as it stands and writes nothing, and the refusing modes fail the write naming
        the destination. A destination holding nothing is the first write of it, whichever
        mode it names.

        `mode` is the mode to honour where the caller resolves one against something else as
        well ([`saveAsTable`]), and defaults to the one the write itself names. The answer is
        [`_write`]'s — the directories the rows were laid under — or None for a write a mode
        left undone.
        """
        if path is None:
            raise AnalysisException("a write names no path")
        prefix = filesystem._text(path).rstrip("/")
        held = mode or self._mode
        stored = filesystem.list_objects(prefix)
        if not stored:
            return self._write(kind, prefix)
        if held in REFUSING_MODES:
            raise AnalysisException("path %s already exists." % (prefix,))
        if held == "ignore":
            return None
        if held == "append":
            return self._write(
                kind, prefix, len([key for key, _size in stored if not _hidden(key)])
            )
        bucket, _key = filesystem.prefix_of(prefix)
        store = filesystem.FileSystem.get(prefix)
        for key, _size in stored:
            store.delete(filesystem.uri(bucket, key))
        return self._write(kind, prefix)

    def _write(self, kind, path, part_offset=0):
        if path is None:
            raise AnalysisException("a write names no path")
        prefix = filesystem._text(path).rstrip("/")
        # A partition column is held in the PATH of the output rather than in its files,
        # which is what the Hive layout a partitioned write lands means.
        held = [
            column for column in self._frame.columns if column not in self._partitions
        ]
        # ONE write is ONE write job, so its parts all carry the SAME write-job UUID — the
        # one minted here, before the first of them is written ([`_write_job_uuid`]).
        job = _write_job_uuid()
        written = []
        for within, rows in self._grouped():
            at = "%s/%s" % (prefix, within) if within else prefix
            if kind == "parquet":
                self._parquet_parts(at, held, rows, job, part_offset)
            else:
                self._text_parts(kind, at, held, rows, job, part_offset)
            written.append((within, at))
        if MARKS_SUCCESSFUL_JOBS:
            filesystem.write_text("%s/_SUCCESS" % (prefix,), "")
        # The directories this write laid its rows out under, which is what a PARTITIONED
        # table's partitions are registered from ([`DataFrameWriter.saveAsTable`]).
        return written

    def _text_parts(self, kind, at, held, rows, job, part_offset=0):
        """One part file per PARTITION of the dataset, as text.

        That is the output layout a write leaves: `coalesce` and `repartition` are what a
        program names that count with, so a write of a dataset coalesced to one partition
        leaves one part file, and a write of one repartitioned leaves a part per partition
        the rows were spread over. `part_offset` is where the numbering starts, so a write
        that APPENDS to a path leaves its parts beside the ones already there instead of
        over them. Each part is named as the format that wrote it names one
        ([`_part_name`]), so the extension a reader selects by is the format's own.
        """
        lines = self._rendered(kind, [_without(row, held) for row in rows])
        suffix = _format_suffix(kind)
        for index, part in enumerate(slices(lines, self._frame.partitions())):
            filesystem.write_text(
                "%s/%s" % (at, _part_name(index + part_offset, job, suffix)),
                "".join("%s\n" % (line,) for line in part),
            )

    def _parquet_parts(self, at, held, rows, job, part_offset=0):
        """The same part files, each one an Apache Parquet file of its own rows.

        The bytes are the format's own ([`pyspark.parquet`]): the values themselves in the
        file's pages, and the row count, the schema and each column's bounds in its footer.
        The part carries the suffix of the codec its pages are stored under ahead of the
        format's own, which is how Spark names a Parquet part file.
        """
        declared = dict(self._frame.dtypes)
        compression = self._options.get("compression", "snappy")
        _codec, suffix = parquet.codec_of(compression)
        for index, part in enumerate(slices(rows, self._frame.partitions())):
            body = parquet.file_bytes(
                held,
                [[row[column] for column in held] for row in part],
                declared,
                compression,
            )
            filesystem.write_bytes(
                "%s/%s"
                % (at, _part_name(index + part_offset, job, "%s.parquet" % (suffix,))),
                body,
            )

    def csv(self, path, **named):
        self.options(**named)
        self._committed("csv", path)
        return None

    def text(self, path, **named):
        self.options(**named)
        self._committed("text", path)
        return None

    def json(self, path, **named):
        self.options(**named)
        self._committed("json", path)
        return None

    def parquet(self, path, mode=None, partitionBy=None, compression=None, **named):
        if mode is not None:
            self.mode(mode)
        if partitionBy is not None:
            self.partitionBy(partitionBy)
        if compression is not None:
            self.option("compression", compression)
        self.options(**named)
        self._committed("parquet", path)
        return None

    def save(self, path=None, format=None, mode=None, partitionBy=None, **named):
        if format is not None:
            self.format(format)
        if mode is not None:
            self.mode(mode)
        if partitionBy is not None:
            self.partitionBy(partitionBy)
        self.options(**named)
        self._committed(self._format or "parquet", path)
        return None

    def saveAsTable(self, name, format=None, mode=None, partitionBy=None, **named):
        """Write this dataset as the TABLE `name` of the session's data catalog.

        "Saves the content of the DataFrame as the specified table" (`saveAsTable`): the rows
        are written as the table's data and the TABLE ITSELF is registered in the catalog —
        its columns, the partition keys it is laid out by and the location its data lives at —
        so another engine over the same catalog resolves the table a program wrote. Both
        halves are real here: the data is written through the filesystem the path resolves in
        ([`pyspark.filesystem`]) and the registration is a call to the catalog itself
        ([`pyspark.catalog`]), signed as the identity the program runs as, so a program whose
        role may not register a table in that catalog fails with the catalog's own refusal.

        WHERE the data goes is resolved the way Spark resolves a table's location: the `path`
        option the write names, else the location the registered table already has, else the
        database's own location with the table's name under it, else the session's warehouse
        directory ([`pyspark.catalog.table_location`]).

        The `mode` decides what a table that is ALREADY there means: the default fails,
        `ignore` leaves it alone, `overwrite` replaces its data and its definition, and
        `append` adds this dataset's rows to it. A table the catalog holds NONE of is created
        here, and the data it is created with is this dataset: the location's own output is
        replaced, so a write whose earlier attempt left files under that location registers a
        table holding one copy of these rows rather than those files as well.
        """
        if format is not None:
            self.format(format)
        if mode is not None:
            self.mode(mode)
        if partitionBy is not None:
            self.partitionBy(partitionBy)
        self.options(**named)
        kind = self._format or "parquet"
        session = SparkSession.getActiveSession()
        current = session.catalog.currentDatabase() if session is not None else None
        database_name, table_name = catalog.resolve(name, current)
        # The catalog is asked about the database and the table BEFORE anything is written:
        # where the data goes is read off them, and a mode that forbids replacing a table
        # that is there must not have written its rows first.
        database_record = catalog.database(database_name)
        existing = catalog.table(database_name, table_name)
        if existing is not None and self._mode in REFUSING_MODES:
            raise AnalysisException(
                "Table `%s`.`%s` already exists." % (database_name, table_name)
            )
        if existing is not None and self._mode == "ignore":
            return None
        warehouse = None
        if session is not None:
            warehouse = session.conf.get(catalog.WAREHOUSE_PROPERTY)
        location = catalog.table_location(
            self._options.get("path"),
            existing,
            database_record,
            warehouse,
            database_name,
            table_name,
        )
        if location is None:
            raise AnalysisException(
                "Table `%s`.`%s` names no location: neither the write, the table nor the "
                "database `%s` states one and the session configures no %s."
                % (
                    database_name,
                    table_name,
                    database_name,
                    catalog.WAREHOUSE_PROPERTY,
                )
            )
        held = [
            column for column in self._frame.columns if column not in self._partitions
        ]
        types = dict(self._frame.dtypes)
        # WHAT the write then does with an output already at that location follows from the
        # TABLE: a table the catalog HOLDS is added to by an append — this dataset's parts go
        # beside the ones already there, so the table holds both — and replaced by anything
        # else, "existing data is expected to be overwritten by the contents of the
        # DataFrame". A table the catalog holds NONE of is being created here, so its data is
        # this dataset alone: whatever an earlier write left under the location is replaced
        # rather than served as rows of the table now being registered over it.
        written = self._committed(
            kind,
            location,
            "append" if (self._mode == "append" and existing is not None) else "overwrite",
        )
        definition = catalog.table_definition(
            table_name,
            held,
            types,
            self._partitions,
            location,
            kind,
            existing,
        )
        if existing is None:
            catalog.create_table(database_name, definition)
        else:
            catalog.update_table(database_name, definition)
        if self._partitions:
            # Each directory the write laid rows under is one partition of the table, which a
            # partitioned table holds in the catalog beside its definition.
            inputs = []
            for within, at in written:
                values = [
                    _partition_of(within, column) for column in self._partitions
                ]
                inputs.append(
                    catalog.partition_input(values, at, kind, held, types)
                )
            catalog.create_partitions(database_name, table_name, inputs)
        return None


def _without(row, held):
    """One row of the columns `held` names, in that order."""
    if list(row.fields()) == list(held):
        return row
    return Row(**dict((column, row[column]) for column in held))


def _matched(columns, keys, left, right):
    """One row of a join: the keys once, then what each side holds, null where it matched none.

    A side that matched nothing is `None`, so every column of it is null in the row — which is
    what an outer join leaves for an unmatched row.
    """
    values = {}
    for name in columns:
        value = None
        if name in keys:
            held = left if left is not None else right
            value = held[name]
        elif left is not None and name in left.fields():
            value = left[name]
        elif right is not None and name in right.fields():
            value = right[name]
        values[name] = value
    return Row(**values)



class DataFrame(object):
    """A dataset of rows: the rows themselves, and the computations over them.

    A dataset the application named by its VALUES holds those rows. One named as a span of
    numbers (`SparkSession.range`) holds the span itself instead and makes its rows only when
    something asks for them: a span is a progression, so what an aggregate over it comes to
    follows from its definition ([`_range_aggregate`]) and a dataset far larger than this
    process could hold rows for is still one every aggregate answers for.
    """

    def __init__(self, rows, columns, numPartitions=1, span=None, types=None):
        self._span = span
        self._rows = None if span is not None else list(rows)
        self._columns = list(columns)
        self._partitions = max(int(numPartitions or 1), 1)
        self._types = dict(types or {})

    def _frame_rows(self):
        """The rows of this dataset, made from its span the first time one is asked for."""
        if self._rows is None:
            name = self._columns[0] if self._columns else "id"
            self._rows = [Row(**{name: value}) for value in self._span]
        return self._rows

    @property
    def columns(self):
        return list(self._columns)

    def partitions(self):
        """How many partitions this dataset holds, which is what a write of it leaves."""
        return self._partitions

    @property
    def schema(self):
        """This dataset's schema: one field per column, carrying that column's own type."""
        return StructType(
            [
                StructField(name, type_by_name(self._column_type(name)) or NullType())
                for name in self._columns
            ]
        )

    @property
    def dtypes(self):
        """The (column, type) pairs of this dataset, as `DataFrame.dtypes` reports them."""
        return [(name, self._column_type(name)) for name in self._columns]

    def printSchema(self):
        print(self.schema.simpleString())
        return None

    def __getattr__(self, name):
        """The column this dataset holds under `name`.

        `DataFrame.__getattr__` "returns the Column denoted by name", which is how an
        application writes `frame.id` for the column `id`. A name this dataset holds no
        column under is no attribute of it, so it raises exactly as any other missing
        attribute does — a program asking `hasattr` about one is told the truth.
        """
        if name.startswith("_"):
            raise AttributeError("'DataFrame' object has no attribute %r" % (name,))
        if name in (self.__dict__.get("_columns") or []):
            return _field(name)
        raise AttributeError("'DataFrame' object has no attribute %r" % (name,))

    def __getitem__(self, item):
        """What `DataFrame.__getitem__` answers for each of the forms it takes: a NAME or a
        POSITION is the column it names, a LIST of them is the projection of those columns,
        and a column EXPRESSION is the rows it holds for (`frame[frame.id > 3]`).
        """
        if isinstance(item, Column):
            return self.filter(item)
        if isinstance(item, (list, tuple)):
            return self.select(*item)
        if isinstance(item, bool):
            raise AnalysisException("a column is named by a name, a position or an expression")
        if isinstance(item, int):
            if item < 0 or item >= len(self._columns):
                raise AnalysisException(
                    "this dataset holds no column at position %d" % (item,)
                )
            return _field(self._columns[item])
        return self._resolved(str(item))

    def toJSON(self, use_unicode=True):
        """This dataset as the strings `DataFrame.toJSON` answers: "a new RDD of JSON
        strings", one per row, each the row's own columns as a JSON document.
        """
        import json

        return RDD(
            slices(
                [json.dumps(row.asDict(), default=str) for row in self._frame_rows()],
                self._partitions,
            )
        )

    def _column_type(self, name):
        """The type the column `name` carries: the one it was built with, or the one the first
        value of it that is not null has."""
        if name in self._types:
            return self._types[name]
        if self._span is not None and self._columns and name == self._columns[0]:
            return BIGINT
        for row in self._frame_rows():
            value = row[name]
            if value is not None:
                return _type_of(value)
        return VOID

    @property
    def rdd(self):
        return RDD(slices(self._frame_rows(), self._partitions))

    def coalesce(self, numPartitions):
        """The same rows over FEWER partitions — a coalesce never raises the count."""
        asked = max(int(numPartitions), 1)
        return self._over(min(self._partitions, asked))

    def repartition(self, numPartitions=None, *columns):
        """The same rows over the partitions asked for, as many as were named."""
        if numPartitions is None or not isinstance(numPartitions, int):
            return self._over(self._partitions)
        return self._over(max(int(numPartitions), 1))

    def _over(self, partitions):
        """The same dataset over `partitions` partitions, span and all."""
        if self._span is not None:
            return DataFrame([], self._columns, partitions, self._span, self._types)
        return DataFrame(self._rows, self._columns, partitions, None, self._types)

    @property
    def write(self):
        return DataFrameWriter(self)

    def collect(self):
        return list(self._frame_rows())

    def union(self, other):
        """The rows of this dataset followed by the rows of `other`, as `union` takes it.

        A union is positional: the columns of the two datasets are matched by position, as
        Spark's own `union` matches them, so the answer carries this dataset's names.
        """
        rows = list(self._frame_rows())
        for row in other.collect():
            held = list(row)
            rows.append(Row(**dict(zip(self._columns, held))))
        types = dict(other._types)
        types.update(self._types)
        return DataFrame(rows, self._columns, self._partitions, None, types)

    def unionAll(self, other):
        return self.union(other)

    def unionByName(self, other, allowMissingColumns=False):
        """The same union, matching the two datasets' columns by NAME rather than position."""
        rows = list(self._frame_rows())
        for row in other.collect():
            record = row.asDict()
            missing = [name for name in record if name not in self._columns]
            if missing and not allowMissingColumns:
                raise AnalysisException(
                    "Cannot resolve column name %s among (%s)"
                    % (missing[0], ", ".join(self._columns))
                )
            rows.append(
                Row(**dict((name, record.get(name)) for name in self._columns))
            )
        types = dict(other._types)
        types.update(self._types)
        return DataFrame(rows, self._columns, self._partitions, None, types)

    def count(self):
        if self._span is not None:
            return len(self._span)
        return len(self._rows)

    def join(self, other, on=None, how="inner"):
        """The two datasets matched on the columns `on` names.

        "Joins with another DataFrame, using the given join expression" (`DataFrame.join`).
        What is matched here is the EQUI-join on the column names both datasets carry, which
        is what `on` names — one name, or a list of them; `how` is the kind of join
        ([`JOIN_KINDS`]), and an unmatched side contributes its own row with the other side's
        columns null wherever that kind keeps it. A join named by anything else — an `on`
        that is not those names, or a `how` naming no kind of join — is REFUSED rather than
        answered with the rows of some other join.

        The result is a shuffle, so it is left over as many partitions as this dataset holds.
        """
        if isinstance(on, str):
            names = [on]
        elif isinstance(on, (list, tuple)) and on and all(
            isinstance(value, str) for value in on
        ):
            names = [str(value) for value in on]
        else:
            raise AnalysisException(
                "a join of this engine is made on the column names the two datasets share; "
                "%r names none of them" % (on,)
            )
        for name in names:
            if name not in self._columns:
                raise AnalysisException("cannot resolve the column %s" % (name,))
            if name not in other.columns:
                raise AnalysisException("cannot resolve the column %s" % (name,))
        spelled = str(how or "inner").lower().replace("_", "")
        if spelled not in JOIN_KINDS:
            raise AnalysisException(UNSUPPORTED_JOIN % (how,))
        kind = JOIN_KINDS[spelled]
        if kind in ("semi", "anti"):
            columns = list(self._columns)
        else:
            # The join keys once, then each side's own columns, which is the order Spark
            # leaves a join on names in.
            columns = names + [name for name in self._columns if name not in names]
            columns = columns + [name for name in other.columns if name not in columns]
        matching = {}
        for row in other.collect():
            matching.setdefault(tuple(row[name] for name in names), []).append(row)
        matched = set()
        rows = []
        for row in self._frame_rows():
            keys = tuple(row[name] for name in names)
            found = matching.get(keys) or []
            if kind == "semi":
                if found:
                    rows.append(row)
                continue
            if kind == "anti":
                if not found:
                    rows.append(row)
                continue
            if found:
                matched.add(keys)
                for right in found:
                    rows.append(_matched(columns, names, row, right))
            elif kind in ("left", "outer"):
                rows.append(_matched(columns, names, row, None))
        if kind in ("right", "outer"):
            for row in other.collect():
                if tuple(row[name] for name in names) not in matched:
                    rows.append(_matched(columns, names, None, row))
        types = dict(other._types)
        types.update(self._types)
        return DataFrame(
            rows,
            columns,
            self._partitions,
            None,
            dict((name, kind_of) for name, kind_of in types.items() if name in columns),
        )

    def crossJoin(self, other):
        """Every row of this dataset beside every row of `other`.

        A column name the two sides SHARE is one column of the product here, carrying this
        dataset's value — the product of two datasets with a name in common is a dataset with
        that name twice, which a row of this engine cannot hold.
        """
        columns = list(self._columns) + [
            name for name in other.columns if name not in self._columns
        ]
        rows = []
        for row in self._frame_rows():
            for right in other.collect():
                rows.append(_matched(columns, [], row, right))
        types = dict(other._types)
        types.update(self._types)
        return DataFrame(rows, columns, self._partitions, None, types)

    def take(self, num):
        if self._span is not None:
            name = self._columns[0] if self._columns else "id"
            return [
                Row(**{name: value}) for value in self._span[: max(int(num), 0)]
            ]
        return self._rows[: max(int(num), 0)]

    def first(self):
        taken = self.take(1)
        if not taken:
            return None
        return taken[0]

    def toLocalIterator(self):
        return iter(self._frame_rows())

    def head(self, n=None):
        """The first row, or the first `n` of them, as `head` answers either."""
        if n is None:
            return self.first()
        return self.take(n)

    def isEmpty(self):
        return self.count() == 0

    def dropDuplicates(self, subset=None):
        """The rows of this dataset once each, told apart by the columns `subset` names."""
        names = [str(name) for name in (subset or self._columns)]
        for name in names:
            if name not in self._columns:
                raise AnalysisException("cannot resolve the column %s" % (name,))
        seen = set()
        rows = []
        for row in self._frame_rows():
            keys = tuple(row[name] for name in names)
            if keys in seen:
                continue
            seen.add(keys)
            rows.append(row)
        return DataFrame(rows, self._columns, self._partitions, None, self._types)

    def drop_duplicates(self, subset=None):
        return self.dropDuplicates(subset)

    def cache(self):
        """The dataset itself: its rows are already held in this process.

        "Persists the DataFrame with the default storage level" — a storage level is where a
        distributed engine keeps the partitions it has computed, and this engine holds them
        here, so there is nowhere else to put them and nothing for a program to observe but
        the same dataset.
        """
        return self

    def persist(self, storageLevel=None):
        return self

    def unpersist(self, blocking=False):
        return self

    def select(self, *columns):
        """The dataset the expressions project, as `select` takes them.

        An argument is either the NAME of a column of this dataset or a column expression
        over it ([`pyspark.sql.column`]), and what the projection answers is what those
        expressions COMPUTE over this dataset's own rows: one row per row of it, or the one
        row every aggregate answers when every expression is an aggregate. The name `*`
        stands for every column of the dataset, as it does in a projection's own grammar.
        """
        named = []
        for column in _flattened(columns):
            if isinstance(column, str) and column.strip() == "*":
                named.extend(self._columns)
            else:
                named.append(column)
        held = [self._resolved(column) for column in named]
        if not held:
            raise AnalysisException("a projection names no expression")
        folding = [column for column in held if column.folds()]
        if folding and len(folding) != len(held):
            raise AnalysisException(
                "grouping expressions sequence is empty, and %s is not an aggregate function"
                % (held[0]._name,)
            )
        names = [column._name for column in held]
        if folding:
            return self._folded(held)
        rows = [
            Row(**dict((column._name, column.of(row)) for column in held))
            for row in self._frame_rows()
        ]
        return DataFrame(rows, names, self._partitions, None, self._carried(held))

    def _resolved(self, column):
        """One argument of a projection as the expression it names.

        A NAME is the column of this dataset it names, and anything else is the expression
        it already is — which is how Spark reads the two forms `select` takes.
        """
        if isinstance(column, Column):
            return column
        name = str(column)
        if name not in self._columns:
            raise AnalysisException("cannot resolve the column %s" % (name,))
        return _field(name)

    def _carried(self, columns):
        """The types a projection carries over: those of the columns it refers to.

        An expression that is a bare reference projects the column it names, so the type
        that column already carries is the type of the projected one; one that READS that
        column as another type carries the type the cast names; anything else is typed by
        the values it computes.
        """
        held = {}
        for column in columns:
            if column._cast is not None:
                held[column._name] = CAST_TYPES[column._cast][0]
            elif column._field is not None and column._field in self._columns:
                held[column._name] = self._column_type(column._field)
        return held

    def _folded(self, columns, rows=None):
        """The ONE row every aggregate answers, over `rows` or over this whole dataset."""
        values = {}
        kinds = {}
        # The rows are made only where an aggregate really needs them: one answered from a
        # SPAN's own definition needs none, which is what lets a dataset far larger than this
        # process could hold rows for be aggregated at all.
        held = rows
        for column in columns:
            inner = column._inner
            over_the_span = (
                rows is None
                and self._span is not None
                and bool(self._columns)
                and (
                    # `count(*)` counts the values of the span, which is how many it holds.
                    (inner is None and column._aggregate == "count")
                    or (inner is not None and inner._field == self._columns[0])
                )
            )
            if over_the_span:
                # The span's own definition answers it, so no row of it is ever made.
                value, kind = _range_aggregate(column._aggregate, self._span)
                if inner is not None and inner._cast is not None:
                    # A cast inside the aggregate reads every value as that type, so the
                    # aggregate's own type is that one and its value is read as it.
                    value = _cast_value(value, inner._cast)
                    kind = CAST_TYPES[inner._cast][0]
            else:
                if held is None:
                    held = self._frame_rows()
                value, kind = column.over(held)
            values[column._name] = value
            kinds[column._name] = kind
        names = [column._name for column in columns]
        return DataFrame([Row(**values)], names, 1, None, kinds)

    def withColumn(self, colName, col):
        """The same dataset with the column `colName` holding what `col` computes."""
        held = self._resolved(col)
        name = str(colName)
        names = list(self._columns)
        if name not in names:
            names.append(name)
        rows = []
        for row in self._frame_rows():
            record = row.asDict()
            record[name] = held.of(row)
            rows.append(Row(**dict((key, record.get(key)) for key in names)))
        types = dict(self._types)
        if name in types:
            del types[name]
        carried = self._carried([held])
        if held._name in carried:
            types[name] = carried[held._name]
        return DataFrame(rows, names, self._partitions, None, types)

    def withColumnRenamed(self, existing, new):
        """The same dataset with one column under another name."""
        old = str(existing)
        if old not in self._columns:
            return self
        names = [str(new) if name == old else name for name in self._columns]
        rows = []
        for row in self._frame_rows():
            record = row.asDict()
            rows.append(
                Row(
                    **dict(
                        (
                            str(new) if name == old else name,
                            record.get(name),
                        )
                        for name in self._columns
                    )
                )
            )
        types = {}
        for name in self._columns:
            types[str(new) if name == old else name] = self._column_type(name)
        return DataFrame(rows, names, self._partitions, None, types)

    def drop(self, *columns):
        """The same dataset without the columns named."""
        dropped = [
            column._name if isinstance(column, Column) else str(column)
            for column in _flattened(columns)
        ]
        names = [name for name in self._columns if name not in dropped]
        rows = [
            Row(**dict((name, row[name]) for name in names)) for row in self._frame_rows()
        ]
        types = dict(
            (name, kind) for name, kind in self._types.items() if name in names
        )
        return DataFrame(rows, names, self._partitions, None, types)

    def filter(self, condition):
        """The rows of this dataset the condition holds for.

        `DataFrame.filter` takes "a Column of types.BooleanType or a string of SQL
        expressions", so a condition spelled as SQL text is read by the same expression
        grammar `selectExpr` reads ([`pyspark.sql.expression`]) and then applied to each row
        exactly as a column expression is; a function of a row is applied the same way.
        """
        if isinstance(condition, str):
            condition = read_sql_expression(condition)
        if isinstance(condition, Column):
            kept = [row for row in self._frame_rows() if condition.of(row)]
        elif callable(condition):
            kept = [row for row in self._frame_rows() if condition(row)]
        else:
            raise AnalysisException(
                "a filter takes a column expression or a function of a row; %r is neither"
                % (condition,)
            )
        return DataFrame(kept, self._columns, self._partitions, None, self._types)

    def where(self, condition):
        return self.filter(condition)

    def orderBy(self, *columns):
        """The same rows, ordered by the expressions named, each ascending or descending."""
        held = [self._resolved(column) for column in _flattened(columns)]
        rows = list(self._frame_rows())
        for column in reversed(held):
            rows = sorted(
                rows,
                key=lambda row, held=column: _sort_key(held.of(row)),
                reverse=column._descending,
            )
        return DataFrame(rows, self._columns, self._partitions, None, self._types)

    def sort(self, *columns):
        return self.orderBy(*columns)

    def distinct(self):
        """The rows of this dataset, each held once."""
        kept = []
        for row in self._frame_rows():
            if row not in kept:
                kept.append(row)
        return DataFrame(kept, self._columns, self._partitions, None, self._types)

    def limit(self, num):
        return DataFrame(
            self.take(num), self._columns, self._partitions, None, self._types
        )

    def groupBy(self, *columns):
        """The groups the rows fall into, by the expressions named."""
        return GroupedData(self, [self._resolved(column) for column in _flattened(columns)])

    def groupby(self, *columns):
        return self.groupBy(*columns)

    def agg(self, *exprs):
        """The aggregates over this whole dataset, which is the one group of every row."""
        return GroupedData(self, []).agg(*exprs)

    def toDF(self, *names):
        """The same rows under the column names given."""
        held = [str(name) for name in _flattened(names)]
        if not held:
            return self
        rows = [
            Row(**dict(zip(held, [row[name] for name in self._columns])))
            for row in self._frame_rows()
        ]
        types = {}
        for index, name in enumerate(held):
            if index < len(self._columns):
                types[name] = self._column_type(self._columns[index])
        return DataFrame(rows, held, self._partitions, None, types)

    def selectExpr(self, *exprs):
        """The dataset the SQL expressions project, as `selectExpr` takes them.

        Each expression is read by the SQL expression grammar
        ([`pyspark.sql.expression`]) and then computed over this dataset exactly as the
        column expressions `select` takes are: a projection answers one row per row of it,
        and an aggregate answers the one row every aggregate answers — over the span's own
        definition where this dataset is a span, and over its values otherwise. The name `*`
        stands for every column of the dataset, as it does in a projection's own grammar.
        """
        held = []
        for expr in _flattened(exprs):
            spelled = str(expr)
            held.append("*" if spelled.strip() == "*" else read_sql_expression(spelled))
        if not held:
            raise AnalysisException("a projection names no expression")
        return self.select(*held)

    def show(self, n=20, truncate=True, vertical=False):
        for row in self.take(n):
            print(row)
        return None


def _flattened(columns):
    """The arguments of a call that takes several columns, lists and tuples of them too."""
    held = []
    for column in columns:
        if isinstance(column, (list, tuple)):
            held.extend(column)
        else:
            held.append(column)
    return held


def _frame_of(answered):
    """The dataset the engine's answer to a statement IS: its columns, their types, its rows.

    The engine names each column of its answer and states the type it holds where it knows
    one — a column of a table carries the type the catalog declares for it, while a computed
    one has none stated and takes the type of the values the engine answered. A statement that
    changed the catalog rather than reading answers neither columns nor rows, which is the
    empty dataset Spark answers such a statement with.
    """
    columns, rows = answered
    names = [str(column.get("name") or "") for column in columns]
    types = {}
    for column in columns:
        declared = type_by_name(str(column.get("type") or ""))
        if declared is not None:
            types[str(column.get("name") or "")] = declared.simpleString()
    built = [Row(**dict(zip(names, list(row)))) for row in rows]
    return DataFrame(built, names, 1, None, types)


class GroupedData(object):
    """The groups a dataset's rows fall into, and the aggregates over each of them.

    `groupBy` answers one of these: the keys are the expressions the rows are grouped by, and
    an aggregate over it answers ONE row per group — the key values, and what each aggregate
    folded that group's own rows into. A grouping by nothing at all is the one group of every
    row, which is what `DataFrame.agg` asks for.
    """

    def __init__(self, frame, keys):
        self._frame = frame
        self._keys = keys

    def _groups(self):
        """The (key values, rows) of each group, in the order the keys first appear."""
        if not self._keys:
            return [((), self._frame.collect())]
        order = []
        grouped = {}
        for row in self._frame.collect():
            values = tuple(key.of(row) for key in self._keys)
            if values not in grouped:
                grouped[values] = []
                order.append(values)
            grouped[values].append(row)
        return [(values, grouped[values]) for values in order]

    def agg(self, *exprs):
        """The dataset of one row per group, holding the keys and these aggregates."""
        columns = []
        for expr in _flattened(exprs):
            if isinstance(expr, dict):
                # The mapping form: the column to fold, by the function that folds it.
                for name in expr:
                    columns.append(_aggregate_of(str(expr[name]).lower(), _field(name)))
            else:
                columns.append(expr)
        if not columns:
            raise AnalysisException("an aggregation names no aggregate")
        for column in columns:
            if not isinstance(column, Column) or not column.folds():
                raise AnalysisException(
                    "%s is not an aggregate function" % (column,)
                )
        if not self._keys:
            return self._frame._folded(columns)
        names = [key._name for key in self._keys] + [
            column._name for column in columns
        ]
        rows = []
        types = {}
        for values, group in self._groups():
            record = {}
            for index, key in enumerate(self._keys):
                record[key._name] = values[index]
            for column in columns:
                value, kind = column.over(group)
                record[column._name] = value
                types[column._name] = kind
            rows.append(Row(**dict((name, record.get(name)) for name in names)))
        for key in self._keys:
            if key._field is not None:
                types[key._name] = self._frame._column_type(key._field)
        return DataFrame(
            rows, names, _shuffle_partitions(self._frame._partitions), None, types
        )

    def count(self):
        """How many rows each group holds, under the name Spark gives that column."""
        return self.agg(Column("count", None, "count", None))

    def sum(self, *columns):
        return self._over("sum", columns)

    def min(self, *columns):
        return self._over("min", columns)

    def max(self, *columns):
        return self._over("max", columns)

    def avg(self, *columns):
        return self._over("avg", columns)

    def mean(self, *columns):
        return self._over("avg", columns)

    def _over(self, function, columns):
        """One aggregate over the columns named, or over every numeric column.

        A grouped aggregate that names no column folds the NUMERIC columns of the dataset,
        which is what Spark's own `sum`, `min`, `max` and `avg` of a grouping do.
        """
        named = [str(column) for column in _flattened(columns)]
        if not named:
            named = [
                name
                for name in self._frame.columns
                if self._frame._column_type(name) in (BIGINT, DOUBLE)
                and name not in [key._name for key in self._keys]
            ]
        return self.agg([_aggregate_of(function, _field(name)) for name in named])


# "spark.sql.shuffle.partitions": "the default number of partitions to use when shuffling
# data for joins or aggregations" (the Spark SQL configuration documentation), which is what
# a GROUPED aggregate's own partitioning is.
SHUFFLE_PARTITIONS_PROPERTY = "spark.sql.shuffle.partitions"


def _shuffle_partitions(held):
    """How many partitions a SHUFFLE of this engine answers over.

    The property above is what the environment sets it to, so a grouping answers that many
    partitions where the environment the program runs in sets one — which is what a write of
    the aggregate then leaves.

    Where NOTHING sets it the answer is `held`, the partitioning of the dataset that was
    grouped, and that is a DEVIATION stated here rather than Spark's own default of 200: this
    engine computes a whole group in one process, so there is no exchange to spread a grouping
    across and 200 partitions would be 199 empty ones in every write. A program that wants a
    width names it, the way one does on a real cluster, and gets it.
    """
    session = SparkSession.getActiveSession()
    if session is None:
        return held
    spelled = session.conf.get(SHUFFLE_PARTITIONS_PROPERTY)
    if spelled is None:
        return held
    try:
        asked = int(str(spelled))
    except ValueError:
        return held
    return asked if asked > 0 else held


class SparkSession(object):
    """The session an application runs in, as `SparkSession.builder` hands one out."""

    class Builder(object):
        """The builder of one session: what it is named, where it runs, how configured."""

        def __init__(self):
            self._conf = SparkConf()

        def appName(self, name):
            self._conf.setAppName(name)
            return self

        def master(self, master):
            self._conf.setMaster(master)
            return self

        def config(self, key=None, value=None, conf=None, **named):
            if conf is not None:
                self._conf.setAll(conf.getAll())
            if key is not None:
                self._conf.set(key, value)
            for held, setting in named.items():
                self._conf.set(held, setting)
            return self

        def enableHiveSupport(self):
            return self.config("spark.sql.catalogImplementation", "hive")

        def getOrCreate(self):
            if SparkSession._active is None or SparkSession._active._stopped:
                SparkSession._active = SparkSession(
                    SparkContext.getOrCreate(self._conf)
                )
            return SparkSession._active

    _active = None

    def __init__(self, sparkContext):
        self._context = sparkContext
        self._jvm_gateway = gateway()
        self._java_context = JavaSparkContext(sparkContext.getConf())
        self._stopped = False
        self._catalog = None
        SparkSession._active = self

    @property
    def sparkContext(self):
        return self._context

    @property
    def version(self):
        return SPARK_VERSION

    @property
    def conf(self):
        return self._context.getConf()

    @property
    def read(self):
        """Returns a DataFrameReader that can be used to read data in as a DataFrame."""
        return DataFrameReader(self)

    @property
    def catalog(self):
        """The interface through which the user may create, drop, alter or query underlying
        databases, tables and functions — the session's data catalog
        ([`pyspark.catalog`]), which is the account's own AWS Glue Data Catalog.

        One per session, because the current database is a property of the session: a
        program that sets it sees it set for every later table name it resolves.
        """
        if self._catalog is None:
            self._catalog = catalog.Catalog(self)
        return self._catalog

    # The gateway and the Java context an application reaches the Hadoop filesystem
    # through, under the names PySpark gives them.
    @property
    def _jvm(self):
        return self._jvm_gateway

    @property
    def _jsc(self):
        return self._java_context

    @property
    def _sc(self):
        return self._context

    def sql(self, sqlQuery, args=None, **kwargs):
        """Returns a `DataFrame` representing the result of the given query.

        The statement is resolved and run where a driver resolves and runs one: against the
        catalog this session's table names resolve in and the data its tables point at, by the
        engine the environment supplies ([`pyspark.sql.statements`]). What this answers is the
        columns and rows that engine computed. A name the statement spells without a database
        is resolved in the session's current database, exactly as every other name this session
        resolves is.
        """
        if args is not None:
            raise SparkException(
                "this engine runs a statement as it stands, so a parameterised one is not run"
            )
        return _frame_of(_submitted_statement(sqlQuery, self.catalog.currentDatabase()))

    def table(self, tableName):
        """Returns the specified table as a `DataFrame`.

        The table is the one the session's catalog holds under that name — the database it
        names, or the session's current one — and its rows are read the way the statement that
        names it reads them.
        """
        database, name = catalog.resolve(tableName, self.catalog.currentDatabase())
        return self.sql("SELECT * FROM `%s`.`%s`" % (database, name))

    def createDataFrame(self, data, schema=None, **named):
        """The dataset the values hold, under the schema one was stated as.

        A stated schema names the columns and, where it states them, the types they carry —
        as a `StructType`, as the data-definition text `<name> <type>` per column, or as the
        names alone; a dataset named without one takes its columns from the values themselves
        and its types from what they are.
        """
        rows = list(data)
        columns = []
        types = {}
        if hasattr(schema, "fields"):
            for field in schema.fields:
                columns.append(field.name)
                types[field.name] = field.dataType.simpleString()
        elif hasattr(schema, "fieldNames"):
            columns = list(schema.fieldNames())
        elif isinstance(schema, (list, tuple)):
            columns = [str(column) for column in schema]
        elif isinstance(schema, str):
            # The data-definition spelling of a schema, which `DataFrameReader.schema` takes
            # too: `<name> <type>` per column.
            for field in schema.split(","):
                stated = field.strip().split()
                if not stated:
                    continue
                columns.append(stated[0])
                if len(stated) > 1:
                    held = type_by_name(" ".join(stated[1:]))
                    if held is not None:
                        types[stated[0]] = held.simpleString()
        if not columns and rows:
            first = rows[0]
            if isinstance(first, Row):
                columns = list(first.fields())
            elif isinstance(first, dict):
                columns = list(first.keys())
            elif isinstance(first, (list, tuple)):
                columns = ["_%d" % (index + 1,) for index in range(len(first))]
            else:
                columns = ["_1"]
        built = []
        for row in rows:
            if isinstance(row, Row):
                values = row.asDict()
            elif isinstance(row, dict):
                values = dict(row)
            elif isinstance(row, (list, tuple)):
                values = dict(zip(columns, list(row)))
            else:
                values = dict(zip(columns, [row]))
            built.append(Row(**dict((name, values.get(name)) for name in columns)))
        return DataFrame(built, columns, 1, None, types)

    @classmethod
    def getActiveSession(cls):
        """The session this application is running in, or None where it started none."""
        if cls._active is not None and not cls._active._stopped:
            return cls._active
        return None

    def range(self, start, end=None, step=1, numPartitions=None):
        """The dataset of one bigint column over a span of numbers, as `range` names one.

        The span itself is what the dataset holds: its rows are made only when something asks
        for them, and an aggregate over it is answered from the span's own definition
        ([`DataFrame`]).
        """
        if end is None:
            start, end = 0, start
        span = range(int(start), int(end), int(step or 1))
        return DataFrame([], ["id"], numPartitions or 1, span, {"id": BIGINT})

    def stop(self):
        self._stopped = True
        self._context.stop()
        if SparkSession._active is self:
            SparkSession._active = None
        return None

    def newSession(self):
        return SparkSession(self._context)

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.stop()
        return False


# `SparkSession.builder`, the attribute an application starts from.
SparkSession.builder = SparkSession.Builder()

__all__ = [
    "AnalysisException",
    "ArrayType",
    "BinaryType",
    "BooleanType",
    "ByteType",
    "Column",
    "DataFrame",
    "DataFrameReader",
    "DataFrameWriter",
    "DataType",
    "DateType",
    "DecimalType",
    "DoubleType",
    "FloatType",
    "GroupedData",
    "IntegerType",
    "LongType",
    "MapType",
    "NullType",
    "Row",
    "ShortType",
    "SparkSession",
    "SparkException",
    "StringType",
    "StructField",
    "StructType",
    "TimestampNTZType",
    "TimestampType",
]
"#;

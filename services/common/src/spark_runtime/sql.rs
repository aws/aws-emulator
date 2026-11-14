// SPDX-License-Identifier: CC0-1.0

//! `pyspark.sql`: the session a program starts and the dataset of rows it hands out.

/// The source of `pyspark/sql/__init__.py`.
pub(super) const SQL_SOURCE: &str = r#"
"""The Spark session of one application, and the rows a dataset of them holds."""

from pyspark import filesystem
from pyspark.conf import SparkConf
from pyspark.context import SparkContext
from pyspark.jvm import JavaSparkContext, SPARK_VERSION, gateway
from pyspark.rdd import RDD, slices


class Row(object):
    """One row: its fields by name and by position, as `Row` holds them."""

    def __init__(self, *values, **named):
        if values and named:
            raise ValueError("a Row is built from positions or from names, not both")
        if named:
            self._fields = list(named.keys())
            self._values = [named[field] for field in self._fields]
        else:
            self._fields = ["_%d" % (index + 1,) for index in range(len(values))]
            self._values = list(values)

    def __getitem__(self, item):
        if isinstance(item, str):
            return self._values[self._fields.index(item)]
        return self._values[item]

    def __getattr__(self, name):
        fields = object.__getattribute__(self, "_fields")
        if name in fields:
            return object.__getattribute__(self, "_values")[fields.index(name)]
        raise AttributeError("no such field: %s" % (name,))

    def __len__(self):
        return len(self._values)

    def __iter__(self):
        return iter(self._values)

    def __eq__(self, other):
        if isinstance(other, Row):
            return self._fields == other._fields and self._values == other._values
        return tuple(self._values) == other

    def __repr__(self):
        pairs = ", ".join(
            "%s=%r" % (field, value) for field, value in zip(self._fields, self._values)
        )
        return "Row(%s)" % (pairs,)

    def asDict(self, recursive=False):
        return dict(zip(self._fields, self._values))


class DataFrameWriter(object):
    """Where a dataset of rows is WRITTEN: through the filesystem its path resolves in."""

    def __init__(self, frame):
        self._frame = frame
        self._mode = "errorifexists"
        self._format = None

    def mode(self, saveMode):
        self._mode = str(saveMode).lower()
        return self

    def format(self, source):
        self._format = str(source).lower()
        return self

    def option(self, key, value):
        return self

    def options(self, **named):
        return self

    def _rendered(self, kind):
        if kind == "json":
            import json

            return [
                json.dumps(row.asDict()) if isinstance(row, Row) else json.dumps(row)
                for row in self._frame.collect()
            ]
        return [
            ",".join(str(value) for value in row) for row in self._frame.collect()
        ]

    def _write(self, kind, path):
        lines = self._rendered(kind)
        prefix = filesystem._text(path).rstrip("/")
        filesystem.write_text(
            "%s/part-00000" % (prefix,), "".join("%s\n" % (line,) for line in lines)
        )
        filesystem.write_text("%s/_SUCCESS" % (prefix,), "")
        return None

    def csv(self, path, **named):
        return self._write("csv", path)

    def text(self, path, **named):
        return self._write("text", path)

    def json(self, path, **named):
        return self._write("json", path)

    def save(self, path=None, **named):
        return self._write(self._format or "csv", path)


class DataFrame(object):
    """A dataset of rows: the rows themselves, and the computations over them."""

    def __init__(self, rows, columns):
        self._rows = list(rows)
        self._columns = list(columns)

    @property
    def columns(self):
        return list(self._columns)

    @property
    def rdd(self):
        return RDD(slices(self._rows, 1))

    @property
    def write(self):
        return DataFrameWriter(self)

    def collect(self):
        return list(self._rows)

    def count(self):
        return len(self._rows)

    def take(self, num):
        return self._rows[: max(int(num), 0)]

    def first(self):
        if not self._rows:
            return None
        return self._rows[0]

    def toLocalIterator(self):
        return iter(self._rows)

    def select(self, *columns):
        names = [str(column) for column in columns]
        for name in names:
            if name not in self._columns:
                raise ValueError("no such column: %s" % (name,))
        rows = [
            Row(**dict((name, row[name]) for name in names)) for row in self._rows
        ]
        return DataFrame(rows, names)

    def show(self, n=20, truncate=True, vertical=False):
        for row in self._rows[: max(int(n), 0)]:
            print(row)
        return None


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

    def createDataFrame(self, data, schema=None, **named):
        rows = list(data)
        columns = []
        if isinstance(schema, (list, tuple)):
            columns = [str(column) for column in schema]
        elif rows:
            first = rows[0]
            if isinstance(first, Row):
                columns = list(first._fields)
            elif isinstance(first, dict):
                columns = list(first.keys())
            else:
                columns = ["_%d" % (index + 1,) for index in range(len(first))]
        built = []
        for row in rows:
            if isinstance(row, Row):
                built.append(row)
            elif isinstance(row, dict):
                built.append(Row(**row))
            else:
                built.append(Row(**dict(zip(columns, list(row)))))
        return DataFrame(built, columns)

    def range(self, start, end=None, step=1, numPartitions=None):
        if end is None:
            start, end = 0, start
        rows = [Row(**{"id": value}) for value in range(start, end, step)]
        return DataFrame(rows, ["id"])

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

__all__ = ["DataFrame", "Row", "SparkSession"]
"#;

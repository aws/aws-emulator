// SPDX-License-Identifier: CC0-1.0

//! `pyspark.context` and `pyspark.jvm`: the context a program hands its work to, and the
//! gateway it reaches Java classes through.

/// The source of `pyspark/context.py`.
pub(super) const CONTEXT_SOURCE: &str = r#"
"""The Spark context of one application: where its datasets come from."""

from pyspark import filesystem
from pyspark.conf import SparkConf
from pyspark.rdd import RDD, slices

# How many partitions a collection is sliced into when the application names no number.
# CHOSEN: the work runs in this one process, so the default parallelism is one.
DEFAULT_PARALLELISM = 1


class Broadcast(object):
    """A value broadcast to the work: the value itself, read back through `value`."""

    def __init__(self, value):
        self._value = value

    @property
    def value(self):
        return self._value

    def unpersist(self, blocking=False):
        return None

    def destroy(self):
        return None


class Accumulator(object):
    """A value the work adds to, as `Accumulator` holds one."""

    def __init__(self, value):
        self._value = value

    def add(self, term):
        self._value = self._value + term
        return None

    @property
    def value(self):
        return self._value

    def __iadd__(self, term):
        self.add(term)
        return self


class SparkContext(object):
    """The application's context. One per application, as `getOrCreate` hands it out."""

    _active = None

    def __init__(self, master=None, appName=None, conf=None, **named):
        self._conf = conf or SparkConf()
        if master:
            self._conf.setMaster(master)
        if appName:
            self._conf.setAppName(appName)
        self._stopped = False
        SparkContext._active = self

    @classmethod
    def getOrCreate(cls, conf=None):
        if cls._active is None or cls._active._stopped:
            cls._active = SparkContext(conf=conf)
        return cls._active

    @property
    def appName(self):
        return self._conf.get("spark.app.name", "")

    @property
    def master(self):
        return self._conf.get("spark.master", "local")

    @property
    def applicationId(self):
        return self._conf.get("spark.app.id", "local-application")

    @property
    def defaultParallelism(self):
        return DEFAULT_PARALLELISM

    def getConf(self):
        return self._conf

    def setLogLevel(self, logLevel):
        return None

    def setJobGroup(self, groupId, description, interruptOnCancel=False):
        return None

    def setLocalProperty(self, key, value):
        return None

    def parallelize(self, c, numSlices=None):
        return RDD(slices(c, numSlices or DEFAULT_PARALLELISM))

    def emptyRDD(self):
        return RDD([[]])

    def range(self, start, end=None, step=1, numSlices=None):
        if end is None:
            start, end = 0, start
        return self.parallelize(list(range(start, end, step)), numSlices)

    def textFile(self, name, minPartitions=None, use_unicode=True):
        """The lines of the object `name` names, as a dataset of them."""
        text = filesystem.read_text(name)
        lines = text.split("\n")
        if lines and lines[-1] == "":
            lines = lines[:-1]
        return RDD(slices(lines, minPartitions or DEFAULT_PARALLELISM))

    def wholeTextFiles(self, path, minPartitions=None, use_unicode=True):
        return RDD(
            slices(
                [(filesystem._text(path), filesystem.read_text(path))],
                minPartitions or DEFAULT_PARALLELISM,
            )
        )

    def broadcast(self, value):
        return Broadcast(value)

    def accumulator(self, value, accum_param=None):
        return Accumulator(value)

    def stop(self):
        self._stopped = True
        if SparkContext._active is self:
            SparkContext._active = None
        return None

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.stop()
        return False
"#;

/// The source of `pyspark/jvm.py`: the gateway a Spark program reaches Java classes
/// through.
///
/// A PySpark program reaches the Hadoop filesystem the way it does on a cluster —
/// `spark._jvm.org.apache.hadoop.fs.Path`, `FileSystem.get`, the configuration off
/// `spark._jsc.hadoopConfiguration()` — so the gateway hands out exactly the classes this
/// emulator can perform, and a class it does not have raises the way an absent attribute
/// does.
pub(super) const JVM_SOURCE: &str = r#"
"""The Java classes a Spark application reaches through its gateway."""

from pyspark import filesystem
from pyspark.filesystem import FileSystem, FileStatus

# The Spark the application is running on: the version the environment's own release ships,
# as the per-run module `pyspark.release` carries it.
from pyspark.release import VERSION as SPARK_VERSION


class URI(object):
    """`java.net.URI`: the address a filesystem is resolved from."""

    def __init__(self, text):
        self._text = str(text)

    def toString(self):
        return self._text

    def getScheme(self):
        scheme, _, _ = filesystem.split_uri(self._text)
        return scheme

    def getHost(self):
        _, authority, _ = filesystem.split_uri(self._text)
        return authority

    def getPath(self):
        _, _, within = filesystem.split_uri(self._text)
        return "/" + within

    def __str__(self):
        return self._text


class Configuration(object):
    """`org.apache.hadoop.conf.Configuration`: the settings a filesystem is opened with."""

    def __init__(self, loadDefaults=True):
        self._settings = {}

    def get(self, name, defaultValue=None):
        return self._settings.get(str(name), defaultValue)

    def set(self, name, value):
        self._settings[str(name)] = str(value)
        return None

    def setBoolean(self, name, value):
        self.set(name, "true" if value else "false")
        return None

    def getBoolean(self, name, defaultValue=False):
        held = self.get(name)
        if held is None:
            return defaultValue
        return held == "true"

    def unset(self, name):
        self._settings.pop(str(name), None)
        return None


class Path(object):
    """`org.apache.hadoop.fs.Path`: a path in a filesystem."""

    def __init__(self, first, child=None):
        base = filesystem._text(first)
        if child is None:
            self._text = base
        else:
            self._text = "%s/%s" % (base.rstrip("/"), filesystem._text(child).lstrip("/"))

    def toString(self):
        return self._text

    def toUri(self):
        return URI(self._text)

    def getName(self):
        return self._text.rstrip("/").rsplit("/", 1)[-1]

    def getParent(self):
        trimmed = self._text.rstrip("/")
        if "/" not in trimmed:
            return None
        return Path(trimmed.rsplit("/", 1)[0])

    def getFileSystem(self, conf=None):
        return FileSystem.get(self._text, conf)

    def isAbsolute(self):
        return "://" in self._text or self._text.startswith("/")

    def __str__(self):
        return self._text


class JavaPackage(object):
    """One package of the gateway: the classes under it, by name."""

    def __init__(self, name, members):
        self._name = name
        self._members = members

    def __getattr__(self, name):
        members = object.__getattribute__(self, "_members")
        if name in members:
            return members[name]
        held = object.__getattribute__(self, "_name")
        raise AttributeError(
            "%s.%s is not a class this emulator supplies" % (held, name)
        )


def gateway():
    """The root of the gateway: what `spark._jvm` answers."""
    hadoop_fs = JavaPackage(
        "org.apache.hadoop.fs",
        {"Path": Path, "FileSystem": FileSystem, "FileStatus": FileStatus},
    )
    hadoop_conf = JavaPackage(
        "org.apache.hadoop.conf", {"Configuration": Configuration}
    )
    hadoop = JavaPackage(
        "org.apache.hadoop", {"fs": hadoop_fs, "conf": hadoop_conf}
    )
    apache = JavaPackage("org.apache", {"hadoop": hadoop})
    org = JavaPackage("org", {"apache": apache})
    net = JavaPackage("java.net", {"URI": URI})
    java = JavaPackage("java", {"net": net})
    return JavaPackage("jvm", {"org": org, "java": java})


class JavaSparkContext(object):
    """`spark._jsc`: the Java context an application reads its Hadoop settings off."""

    def __init__(self, conf=None):
        self._configuration = Configuration()

    def hadoopConfiguration(self):
        return self._configuration

    def version(self):
        return SPARK_VERSION

    def stop(self):
        return None
"#;

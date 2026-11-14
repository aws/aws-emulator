// SPDX-License-Identifier: CC0-1.0

//! `awsglue.gluetypes` and `awsglue.dynamicframe`: the schema a frame reports, and the frame
//! itself — the records, and the transformations and actions a script computes over them.

/// The source of `awsglue/gluetypes.py`.
pub(super) const GLUETYPES_SOURCE: &str = r#"
"""The types a DynamicFrame's schema is written in."""


class DataType(object):
    """One type of a frame's schema, named as the Glue ETL library names it."""

    def __init__(self, name):
        self.name = str(name)

    def typeName(self):
        return self.name

    def jsonValue(self):
        return {"dataType": self.name}

    def __eq__(self, other):
        return isinstance(other, DataType) and other.name == self.name

    def __repr__(self):
        return self.name


class StringType(DataType):
    def __init__(self):
        DataType.__init__(self, "string")


class IntegerType(DataType):
    def __init__(self):
        DataType.__init__(self, "int")


class LongType(DataType):
    def __init__(self):
        DataType.__init__(self, "long")


class DoubleType(DataType):
    def __init__(self):
        DataType.__init__(self, "double")


class BooleanType(DataType):
    def __init__(self):
        DataType.__init__(self, "boolean")


class DateType(DataType):
    def __init__(self):
        DataType.__init__(self, "date")


class TimestampType(DataType):
    def __init__(self):
        DataType.__init__(self, "timestamp")


class NullType(DataType):
    def __init__(self):
        DataType.__init__(self, "null")


# The Hive types a catalog column declares, as the frame types they resolve to. SOURCE: the
# `Column$Type` values the Data Catalog holds ("the data type of the Column") — the Hive type
# names a table declares its columns in.
_BY_HIVE_NAME = {
    "string": StringType,
    "varchar": StringType,
    "char": StringType,
    "int": IntegerType,
    "integer": IntegerType,
    "smallint": IntegerType,
    "tinyint": IntegerType,
    "bigint": LongType,
    "long": LongType,
    "double": DoubleType,
    "float": DoubleType,
    "decimal": DoubleType,
    "boolean": BooleanType,
    "date": DateType,
    "timestamp": TimestampType,
}


def of_hive_type(declared):
    """The frame type a catalog column's declared type resolves to."""
    kind = str(declared or "string").lower().split("<")[0].split("(")[0].strip()
    built = _BY_HIVE_NAME.get(kind)
    if built is None:
        return StringType()
    return built()


class Field(object):
    """One field of a frame's schema: its name and its type."""

    def __init__(self, name, dataType, properties=None):
        self.name = str(name)
        self.dataType = dataType
        self.properties = properties or {}

    def jsonValue(self):
        return {"name": self.name, "container": self.dataType.jsonValue()}

    def __repr__(self):
        return "%s: %s" % (self.name, self.dataType)


class StructType(DataType):
    """The schema of a frame: its fields, in order."""

    def __init__(self, fields=None):
        DataType.__init__(self, "struct")
        self.fields = list(fields or [])

    def add(self, field):
        self.fields.append(field)
        return self

    def field(self, name):
        for held in self.fields:
            if held.name == name:
                return held
        return None

    def fieldNames(self):
        return [held.name for held in self.fields]

    def jsonValue(self):
        return {
            "dataType": "struct",
            "fields": [held.jsonValue() for held in self.fields],
        }

    def __repr__(self):
        return "struct(%s)" % (", ".join(repr(held) for held in self.fields),)
"#;

/// The source of `awsglue/dynamicframe.py`.
///
/// `DynamicFrame` is "a distributed collection of self-describing DynamicRecords", and every
/// transformation and action here is COMPUTED over the records the frame really holds: a
/// count counts them, a filter calls the script's own predicate on each, a mapping renames
/// and re-types the fields it names, and a write leaves real objects behind.
pub(super) const DYNAMICFRAME_SOURCE: &str = r#"
"""The frame a Glue ETL script reads, transforms and writes."""

from awsglue import gluetypes


class DynamicFrame(object):
    """A collection of self-describing records, and the computations over them."""

    def __init__(self, records=None, glue_ctx=None, name="", columns=None):
        self._records = [dict(record) for record in (records or [])]
        self.glue_ctx = glue_ctx
        self.name = str(name or "")
        # The columns the frame's source declared, in declaration order: the schema reports
        # them even for a frame holding no record.
        self._columns = list(columns or [])
        if not self._columns:
            declared = []
            for record in self._records:
                for field in record:
                    if field not in declared:
                        declared.append(field)
            self._columns = [(field, "string") for field in declared]

    # --- what the frame IS --------------------------------------------------------------

    def count(self):
        return len(self._records)

    def schema(self):
        return gluetypes.StructType(
            [
                gluetypes.Field(name, gluetypes.of_hive_type(declared))
                for name, declared in self._columns
            ]
        )

    def printSchema(self):
        print("root")
        for field in self.schema().fields:
            print("|-- %s: %s" % (field.name, field.dataType))
        return None

    def show(self, num_rows=20):
        for record in self._records[: max(int(num_rows), 0)]:
            print(record)
        return None

    def toDF(self, options=None):
        """This frame as a Spark dataset of rows, which is what `toDF` hands out.

        The options a conversion takes name how a field of more than one type is resolved
        on the way; every record here carries the value that was really read, under the type
        the table declares, so there is none to resolve and the rows are the frame's own.
        """
        from pyspark.sql import DataFrame, Row

        columns = [name for name, _declared in self._columns]
        rows = []
        for record in self._records:
            rows.append(Row(**dict((name, record.get(name)) for name in columns)))
        return DataFrame(rows, columns)

    @classmethod
    def fromDF(cls, dataframe, glue_ctx, name):
        """A frame of the rows a Spark dataset holds."""
        columns = list(dataframe.columns)
        records = []
        for row in dataframe.collect():
            if hasattr(row, "asDict"):
                records.append(row.asDict())
            elif isinstance(row, dict):
                records.append(dict(row))
            else:
                records.append(dict(zip(columns, list(row))))
        return cls(records, glue_ctx, name, [(column, "string") for column in columns])

    @property
    def records(self):
        return [dict(record) for record in self._records]

    def toPandas(self):
        raise AttributeError(
            "a frame is not handed out as a pandas dataframe by this emulator"
        )

    # --- transformations: new frames over the same records ------------------------------

    def _derived(self, records, columns, name):
        return DynamicFrame(records, self.glue_ctx, name or self.name, columns)

    def select_fields(self, paths, transformation_ctx="", info="", stageThreshold=0,
                      totalThreshold=0):
        wanted = [str(path) for path in paths or []]
        columns = [pair for pair in self._columns if pair[0] in wanted]
        records = [
            dict((name, record.get(name)) for name in wanted if name in record)
            for record in self._records
        ]
        return self._derived(records, columns, self.name)

    def drop_fields(self, paths, transformation_ctx="", info="", stageThreshold=0,
                    totalThreshold=0):
        dropped = [str(path) for path in paths or []]
        columns = [pair for pair in self._columns if pair[0] not in dropped]
        records = [
            dict((name, value) for name, value in record.items() if name not in dropped)
            for record in self._records
        ]
        return self._derived(records, columns, self.name)

    def rename_field(self, oldName, newName, transformation_ctx="", info="",
                     stageThreshold=0, totalThreshold=0):
        old = str(oldName)
        new = str(newName)
        columns = [
            (new if name == old else name, declared) for name, declared in self._columns
        ]
        records = []
        for record in self._records:
            renamed = {}
            for name, value in record.items():
                renamed[new if name == old else name] = value
            records.append(renamed)
        return self._derived(records, columns, self.name)

    def apply_mapping(self, mappings, case_sensitive=False, transformation_ctx="", info="",
                      stageThreshold=0, totalThreshold=0):
        """The fields `mappings` names, under the names and types it gives them.

        One mapping is `(source, source_type, target, target_type)`, which is the form AWS
        documents for `ApplyMapping`. `case_sensitive` is how the SOURCE field is matched:
        with it unset — the library's own default — a mapping naming `ID` resolves the field
        the records carry as `id`, and with it set the name has to match as written.
        """
        resolved = []
        for mapping in mappings or []:
            entry = list(mapping)
            while len(entry) < 4:
                entry.append("string")
            resolved.append((str(entry[0]), str(entry[2]), str(entry[3])))
        columns = [(target, target_type) for _source, target, target_type in resolved]
        records = []
        for record in self._records:
            mapped = {}
            # The field a mapping names, matched as the mapping asked for it: as written, or
            # whatever its case, which is what the library matches by default.
            folded = {}
            if not case_sensitive:
                for name, value in record.items():
                    folded.setdefault(str(name).lower(), value)
            for source, target, target_type in resolved:
                if source in record or case_sensitive:
                    value = record.get(source)
                else:
                    value = folded.get(source.lower())
                mapped[target] = _as_type(value, target_type)
            records.append(mapped)
        return self._derived(records, columns, self.name)

    def filter(self, f, transformation_ctx="", info="", stageThreshold=0, totalThreshold=0):
        kept = [record for record in self._records if f(record)]
        return self._derived(kept, self._columns, self.name)

    def map(self, f, transformation_ctx="", info="", stageThreshold=0, totalThreshold=0):
        mapped = []
        for record in self._records:
            answered = f(record)
            if answered is None:
                continue
            mapped.append(dict(answered))
        columns = []
        for record in mapped:
            for field in record:
                if field not in [name for name, _declared in columns]:
                    columns.append((field, "string"))
        return self._derived(mapped, columns or self._columns, self.name)

    def resolveChoice(self, specs=None, choice="", transformation_ctx="", info="",
                      stageThreshold=0, totalThreshold=0):
        """Resolve the fields the specs name, as `ResolveChoice` resolves them.

        One spec is `(path, action)`. The actions this library PERFORMS are the two that are
        computations over the records it holds: `cast:<type>`, the field's values as that
        type, and `project:<type>`, only the values that are of that type with the rest
        resolved to no value. `choice` names one action to apply to every field, which is
        what a frame-wide resolution is.

        `make_cols`, `make_struct` and `match_catalog` RESTRUCTURE a frame — a column per
        type, a struct of them, the catalog's own type — and this library does not perform
        them, so a spec asking for one raises rather than answering the frame unresolved.
        """
        actions = [
            (str(path), str(action)) for path, action in (specs or []) if str(action)
        ]
        if choice:
            actions = actions + [
                (name, str(choice)) for name, _declared in self._columns
            ]
        if not actions:
            return self
        columns = list(self._columns)
        records = [dict(record) for record in self._records]
        held = [name for name, _declared in columns]
        for path, action in actions:
            if path not in held:
                raise ValueError(
                    "the frame holds no field %r to resolve, it holds %r" % (path, held)
                )
            if action.startswith("cast:"):
                target = action[len("cast:"):]
                for record in records:
                    record[path] = _as_type(record.get(path), target)
            elif action.startswith("project:"):
                target = action[len("project:"):]
                for record in records:
                    value = record.get(path)
                    record[path] = value if _is_type(value, target) else None
            else:
                raise ValueError(
                    "this emulator resolves a choice by casting or projecting a field; %r "
                    "is not an action it performs" % (action,)
                )
            target = action.split(":", 1)[1]
            columns = [
                (name, target if name == path else declared)
                for name, declared in columns
            ]
        return self._derived(records, columns, self.name)

    def repartition(self, numPartitions):
        # How many partitions a frame is held in decides how many files a write of it
        # leaves. Every frame here is the records of this one process and a write of one
        # leaves a single part file, so there is nothing to spread: the frame itself is the
        # answer, as it is for a request to hold it in memory.
        return self

    def coalesce(self, numPartitions, shuffle=False):
        return self

    def unbox(self, *rest, **named):
        raise AttributeError("unbox is not a transformation this emulator supplies")

    def relationalize(self, *rest, **named):
        raise AttributeError(
            "relationalize is not a transformation this emulator supplies"
        )

    # --- actions ------------------------------------------------------------------------

    def write(self, connection_type=None, connection_options=None, format=None,
              format_options=None, accumulator_size=0):
        """Write this frame out, through the context that handed it out.

        The accumulable size bounds how much of a write's own diagnostics a run gathers,
        which is not part of what the write leaves behind.
        """
        if self.glue_ctx is None:
            raise ValueError("a frame with no GlueContext cannot be written")
        return self.glue_ctx.write_dynamic_frame_from_options(
            self,
            connection_type=connection_type,
            connection_options=connection_options or {},
            format=format,
            format_options=format_options or {},
        )


def _as_type(value, target_type):
    """One value as the type a mapping names for it."""
    if value is None:
        return None
    kind = str(target_type or "string").lower()
    try:
        if kind in ("int", "integer", "smallint", "tinyint"):
            return int(value)
        if kind in ("long", "bigint"):
            return int(value)
        if kind in ("double", "float", "decimal"):
            return float(value)
        if kind == "boolean":
            if isinstance(value, bool):
                return value
            return str(value).strip().lower() == "true"
        if kind == "string":
            return value if isinstance(value, str) else str(value)
    except (TypeError, ValueError):
        return value
    return value


def _is_type(value, target_type):
    """Whether one value IS of the type a projection names, as the records carry types."""
    if value is None:
        return False
    kind = str(target_type or "string").lower()
    if kind in ("int", "integer", "smallint", "tinyint", "long", "bigint"):
        return isinstance(value, int) and not isinstance(value, bool)
    if kind in ("double", "float", "decimal"):
        return isinstance(value, float)
    if kind == "boolean":
        return isinstance(value, bool)
    if kind == "string":
        return isinstance(value, str)
    raise ValueError(
        "this emulator projects a field to a type it reads a record's values under; %r is "
        "not one of them" % (target_type,)
    )


class DynamicFrameCollection(object):
    """The frames one transformation handed out, by name."""

    def __init__(self, dynamic_frames=None, glue_ctx=None):
        self._frames = dict(dynamic_frames or {})
        self.glue_ctx = glue_ctx

    def __getitem__(self, key):
        return self._frames[key]

    def __len__(self):
        return len(self._frames)

    def __iter__(self):
        return iter(self._frames)

    def keys(self):
        return list(self._frames.keys())

    def values(self):
        return list(self._frames.values())

    def select(self, key, transformation_ctx=""):
        return self._frames[key]


class DynamicFrameReader(object):
    """`glueContext.create_dynamic_frame`: where a frame is READ from."""

    def __init__(self, glue_context):
        self._glue_context = glue_context

    def from_catalog(self, database=None, table_name=None, redshift_tmp_dir="",
                     transformation_ctx="", push_down_predicate="", additional_options=None,
                     catalog_id=None, **kwargs):
        return self._glue_context.create_dynamic_frame_from_catalog(
            database=database,
            table_name=table_name,
            redshift_tmp_dir=redshift_tmp_dir,
            transformation_ctx=transformation_ctx,
            push_down_predicate=push_down_predicate,
            additional_options=additional_options,
            catalog_id=catalog_id,
            **kwargs
        )

    def from_options(self, connection_type=None, connection_options=None, format=None,
                     format_options=None, transformation_ctx="", push_down_predicate="",
                     **kwargs):
        return self._glue_context.create_dynamic_frame_from_options(
            connection_type=connection_type,
            connection_options=connection_options,
            format=format,
            format_options=format_options,
            transformation_ctx=transformation_ctx,
            push_down_predicate=push_down_predicate,
            **kwargs
        )

    def from_rdd(self, data, name, schema=None, sampleRatio=None):
        return self._glue_context.create_dynamic_frame_from_rdd(data, name, schema)


class DynamicFrameWriter(object):
    """`glueContext.write_dynamic_frame`: where a frame is WRITTEN to."""

    def __init__(self, glue_context):
        self._glue_context = glue_context

    def from_options(self, frame, connection_type=None, connection_options=None,
                     format=None, format_options=None, transformation_ctx="", **kwargs):
        return self._glue_context.write_dynamic_frame_from_options(
            frame,
            connection_type=connection_type,
            connection_options=connection_options,
            format=format,
            format_options=format_options,
            transformation_ctx=transformation_ctx,
            **kwargs
        )

    def from_catalog(self, frame, database=None, table_name=None, redshift_tmp_dir="",
                     transformation_ctx="", additional_options=None, catalog_id=None,
                     **kwargs):
        return self._glue_context.write_dynamic_frame_from_catalog(
            frame,
            database=database,
            table_name=table_name,
            transformation_ctx=transformation_ctx,
            additional_options=additional_options,
            catalog_id=catalog_id,
            **kwargs
        )

    def from_jdbc_conf(self, *rest, **named):
        raise AttributeError(
            "a frame is not written to a JDBC connection by this emulator"
        )
"#;

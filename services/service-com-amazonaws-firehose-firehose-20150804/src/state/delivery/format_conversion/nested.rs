// SPDX-License-Identifier: CC0-1.0

//! The column information of the bound Glue table as the TREE a Hive data type declares, and
//! the records of a flush written against it.
//!
//! A Glue column's `Type` is a Hive data type ("the data type of the Column" — `Column$Type`),
//! and Hive's data types are not all scalar: `array<…>`, `struct<…>` and `map<…,…>` declare a
//! column whose values nest. The conversion deserializes a record "from JSON and then
//! serialize[s] it to the Parquet or ORC format" against those columns whatever they declare,
//! so this module reads a declared type into a tree, builds the Parquet message type that
//! tree is written as — the groups the format's own LIST and MAP annotations describe — and
//! shreds each record's values into the leaves of that message type with the definition and
//! repetition level of every value, which is how Parquet carries a null, an empty list and a
//! list of several items.
//!
//! WHAT A FAILURE IS. A record is read against the tree and either produces a row or does
//! not: a value that is not one the column at that point of the tree holds makes the record a
//! failed record naming WHERE in the tree it failed ([`super::Rejection::Malformed`]), while
//! bytes that are not one JSON object at all are a failure of a different class
//! ([`super::Rejection::Unparseable`]) — the distinction a consumer of the destination's
//! `ErrorOutputPrefix` tells a schema mismatch from a JSON parse failure by.

use super::*;

use ::parquet::basic::{ConvertedType, Repetition};
use ::parquet::data_type::{
    BoolType, ByteArray, ByteArrayType, DoubleType, FloatType, Int32Type, Int64Type,
};
use ::parquet::file::writer::SerializedFileWriter;
use ::parquet::schema::types::Type as SchemaType;

/// How deeply a declared Glue column type may nest.
///
/// A CHOSEN bound on a string the Data Catalog hands this reader: the reader descends once per
/// `<…>` it opens, so this is what keeps a declared type from deciding how deep that descent
/// goes. Far deeper than a schema a client would really declare.
const MAX_NESTING_DEPTH: usize = 16;

/// How many Parquet leaves one conversion writes.
///
/// A CHOSEN bound, beside [`MAX_CONVERTED_COLUMNS`]: a column that nests holds more than one
/// leaf, so the columns alone no longer bound the cells one conversion holds (rows x leaves).
pub(super) const MAX_CONVERTED_LEAVES: usize = 4_000;

/// The name of the repeated group of a Parquet LIST, which is the group one element of the
/// list is repeated as.
const LIST_GROUP: &str = "list";

/// The name of the field one element of a Parquet LIST is written as.
const LIST_ELEMENT: &str = "element";

/// The name of the repeated group of a Parquet MAP, which is the group one entry of the map
/// is repeated as.
const MAP_GROUP: &str = "key_value";

/// The name of the field one entry's key is written as.
const MAP_KEY: &str = "key";

/// The name of the field one entry's value is written as.
const MAP_VALUE: &str = "value";

/// One Glue column's declared type: a scalar, or the tree a Hive complex type declares.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum HiveType {
    /// A scalar Hive type, written as one Parquet leaf.
    Primitive(ColumnType),
    /// `struct<name:type,…>`: a group of named fields, each of its own type.
    Struct(Vec<HiveField>),
    /// `array<type>`: any number of elements of one type.
    List(Box<HiveType>),
    /// `map<key,value>`: any number of entries, whose key is a scalar — Hive admits no other
    /// kind of map key — and whose value is of one type.
    Map(ColumnType, Box<HiveType>),
}

/// One field of a `struct<…>`: the name it is declared under and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct HiveField {
    pub(super) name: String,
    pub(super) declared: HiveType,
}

impl HiveType {
    /// The type a Glue column whose `Type` is `declared` holds, or `None` for a Hive type this
    /// module has no Parquet writing for.
    ///
    /// Hive type names are case-insensitive, so the head of each type is folded; the names of
    /// a `struct<…>`'s fields are NOT, because they are the names a record's keys are matched
    /// against.
    pub(super) fn of_declared(declared: &str) -> Option<Self> {
        read_declared(declared, 0)
    }

    /// The Parquet leaves this type is written as, in the order the message type declares
    /// them, each with the path from the column's own value down to it.
    pub(super) fn leaves(&self) -> Vec<Leaf> {
        let mut leaves = Vec::new();
        collect_leaves(self, &mut Vec::new(), 0, &mut leaves);
        leaves
    }
}

/// The type `declared` holds, read at `depth` groups below the column itself.
fn read_declared(declared: &str, depth: usize) -> Option<HiveType> {
    if depth > MAX_NESTING_DEPTH {
        return None;
    }
    let declared = declared.trim();
    let Some((head, inner)) = parameters(declared) else {
        return ColumnType::of_declared(declared).map(HiveType::Primitive);
    };
    match head.as_str() {
        "array" => Some(HiveType::List(Box::new(read_declared(inner, depth + 1)?))),
        "struct" => {
            let mut fields = Vec::new();
            for member in top_level_parts(inner)? {
                let (name, declared) = member.split_once(':')?;
                let name = name.trim();
                if name.is_empty() {
                    return None;
                }
                fields.push(HiveField {
                    name: name.to_string(),
                    declared: read_declared(declared, depth + 1)?,
                });
            }
            (!fields.is_empty()).then_some(HiveType::Struct(fields))
        }
        "map" => {
            let parts = top_level_parts(inner)?;
            let [key, value] = parts.as_slice() else {
                return None;
            };
            // Hive declares a map's key as a scalar type, so a key this module has no leaf for
            // is a type it cannot write rather than a key it reads some other way.
            let key = ColumnType::of_declared(key.trim())?;
            Some(HiveType::Map(
                key,
                Box::new(read_declared(value, depth + 1)?),
            ))
        }
        // Any other parameterised type — `uniontype<…>` among them — is one this module has
        // no Parquet writing for, like a scalar outside [`ColumnType::of_declared`].
        _ => None,
    }
}

/// `declared` split into its folded head and the text between its angle brackets, or `None`
/// for a type that carries no such parameters — a scalar, whose own `(n)` parameters belong to
/// it.
fn parameters(declared: &str) -> Option<(String, &str)> {
    let (head, rest) = declared.split_once('<')?;
    let inner = rest.trim_end().strip_suffix('>')?;
    Some((head.trim().to_ascii_lowercase(), inner))
}

/// The comma-separated parts of a declared type's parameters, split only where the comma is
/// NOT inside a nested type's own parameters.
///
/// `None` when the brackets do not balance, which is not a type this reader can place.
fn top_level_parts(inner: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in inner.char_indices() {
        match character {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                parts.push(&inner[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    parts.push(&inner[start..]);
    Some(parts)
}

/// One step from a column's own value down toward one Parquet leaf of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Into the field at this index of a `struct<…>`, written as an OPTIONAL group.
    Field(usize),
    /// Into the elements of an `array<…>`, written as an OPTIONAL group holding a REPEATED
    /// one.
    Element,
    /// Into the keys of a `map<…,…>`, whose key field is REQUIRED within the repeated group.
    Key,
    /// Into the values of a `map<…,…>`.
    Value,
}

/// One Parquet leaf a column is written as.
#[derive(Debug, Clone)]
pub(super) struct Leaf {
    /// The scalar type the leaf carries.
    column_type: ColumnType,
    /// The steps from the column's own value down to this leaf.
    path: Vec<Step>,
    /// Whether the leaf itself is OPTIONAL, which every leaf is except a map's key.
    optional: bool,
    /// How many REPEATED groups stand above this leaf, which is the greatest repetition level
    /// one of its values carries — and `0` for a leaf no list or map stands above, whose
    /// values carry no repetition levels at all.
    max_repetition: i16,
}

/// Collect the leaves of `declared` into `into`, in the order the message type declares them.
fn collect_leaves(
    declared: &HiveType,
    path: &mut Vec<Step>,
    repetitions: i16,
    into: &mut Vec<Leaf>,
) {
    match declared {
        HiveType::Primitive(column_type) => into.push(Leaf {
            column_type: *column_type,
            path: path.clone(),
            optional: true,
            max_repetition: repetitions,
        }),
        HiveType::Struct(fields) => {
            for (index, field) in fields.iter().enumerate() {
                path.push(Step::Field(index));
                collect_leaves(&field.declared, path, repetitions, into);
                path.pop();
            }
        }
        HiveType::List(element) => {
            path.push(Step::Element);
            collect_leaves(element, path, repetitions + 1, into);
            path.pop();
        }
        HiveType::Map(key, value) => {
            path.push(Step::Key);
            into.push(Leaf {
                column_type: *key,
                path: path.clone(),
                optional: false,
                max_repetition: repetitions + 1,
            });
            path.pop();
            path.push(Step::Value);
            collect_leaves(value, path, repetitions + 1, into);
            path.pop();
        }
    }
}

/// How many Parquet leaves the columns of a bound table are written as.
pub(super) fn leaf_count(columns: &[(String, HiveType)]) -> usize {
    columns
        .iter()
        .map(|(_, declared)| declared.leaves().len())
        .sum()
}

/// The Parquet message type of the bound table: one OPTIONAL field per Glue column, because a
/// column of a Glue table holds no value wherever a record named none, and the groups a column
/// that nests is written as.
pub(super) fn message_type(
    table: &str,
    columns: &[(String, HiveType)],
) -> Result<SchemaType, String> {
    /// The name of the message type of a conversion whose binding names no table — a
    /// configuration the create refuses, so this only keeps the writer from being handed an
    /// empty name.
    const UNNAMED_TABLE: &str = "record";

    let mut fields = Vec::with_capacity(columns.len());
    for (name, declared) in columns {
        fields.push(::std::sync::Arc::new(schema_field(
            name,
            declared,
            Repetition::OPTIONAL,
        )?));
    }
    let name = if table.is_empty() {
        UNNAMED_TABLE
    } else {
        table
    };
    SchemaType::group_type_builder(name)
        .with_fields(fields)
        .build()
        .map_err(|error| error.to_string())
}

/// The Parquet field one column — or one field, element, key or value within it — is written
/// as.
fn schema_field(
    name: &str,
    declared: &HiveType,
    repetition: Repetition,
) -> Result<SchemaType, String> {
    match declared {
        HiveType::Primitive(column_type) => {
            let (physical, converted) = column_type.physical();
            SchemaType::primitive_type_builder(name, physical)
                .with_repetition(repetition)
                .with_converted_type(converted)
                .build()
                .map_err(|error| error.to_string())
        }
        HiveType::Struct(fields) => {
            let mut children = Vec::with_capacity(fields.len());
            for field in fields {
                children.push(::std::sync::Arc::new(schema_field(
                    &field.name,
                    &field.declared,
                    Repetition::OPTIONAL,
                )?));
            }
            group(name, repetition, ConvertedType::NONE, children)
        }
        // The three-level LIST the format's own annotation describes: the annotated group, the
        // REPEATED group one element is repeated as, and the element itself.
        HiveType::List(element) => {
            let element = ::std::sync::Arc::new(schema_field(
                LIST_ELEMENT,
                element,
                Repetition::OPTIONAL,
            )?);
            let repeated = ::std::sync::Arc::new(group(
                LIST_GROUP,
                Repetition::REPEATED,
                ConvertedType::NONE,
                vec![element],
            )?);
            group(name, repetition, ConvertedType::LIST, vec![repeated])
        }
        // The MAP the format's own annotation describes: the annotated group, the REPEATED
        // group one entry is repeated as, and that entry's REQUIRED key beside its value —
        // an entry of a map always has a key, which is what makes that field required.
        HiveType::Map(key, value) => {
            let key = ::std::sync::Arc::new(schema_field(
                MAP_KEY,
                &HiveType::Primitive(*key),
                Repetition::REQUIRED,
            )?);
            let value =
                ::std::sync::Arc::new(schema_field(MAP_VALUE, value, Repetition::OPTIONAL)?);
            let repeated = ::std::sync::Arc::new(group(
                MAP_GROUP,
                Repetition::REPEATED,
                ConvertedType::NONE,
                vec![key, value],
            )?);
            group(name, repetition, ConvertedType::MAP, vec![repeated])
        }
    }
}

/// One group of the message type, with the annotation and the children it carries.
fn group(
    name: &str,
    repetition: Repetition,
    converted: ConvertedType,
    fields: Vec<::std::sync::Arc<SchemaType>>,
) -> Result<SchemaType, String> {
    SchemaType::group_type_builder(name)
        .with_repetition(repetition)
        .with_converted_type(converted)
        .with_fields(fields)
        .build()
        .map_err(|error| error.to_string())
}

/// One record's value for one column, read as that column's own declared type.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    /// The record named no value here, or named a JSON null. A Glue column is nullable, so
    /// this is not a failure.
    Null,
    /// A scalar's own cell.
    Cell(Cell),
    /// A `struct<…>`'s fields, one value per declared field in declaration order.
    Struct(Vec<Value>),
    /// An `array<…>`'s elements, in the order the record wrote them.
    List(Vec<Value>),
    /// A `map<…,…>`'s entries, in the order the record wrote them.
    Map(Vec<(Cell, Value)>),
}

/// How many cells one row group of a conversion may read a record's values into.
///
/// A CHOSEN bound, and the one that caps what the shredding ALLOCATES: a record's own JSON
/// decides how many entries it carries in a list or a map, the bound table decides how many
/// fields a struct declares, and the values a chunk holds are the PRODUCT of the two — a tiny
/// element of a list of wide structs still holds one value per declared field — so neither
/// factor on its own bounds it. Charged BEFORE each of those allocations and over the whole
/// chunk, so what one row group holds is this whatever a producer puts and whatever the
/// catalog declares. Far above the values a flat table of [`MAX_CONVERTED_COLUMNS`] columns
/// over [`MAX_ROWS_PER_ROW_GROUP`] rows holds, so a schema and a buffer a client would really
/// put are never refused by it.
pub(super) const MAX_CELLS_PER_ROW_GROUP: usize = 2_000_000;

/// The values one row group may still read, charged down as a record is read into it.
#[derive(Debug)]
pub(super) struct CellBudget {
    remaining: usize,
}

impl CellBudget {
    /// The budget one row group starts with.
    pub(super) fn of_row_group() -> Self {
        Self {
            remaining: MAX_CELLS_PER_ROW_GROUP,
        }
    }

    /// Charge `cells` to the budget, BEFORE they are allocated.
    ///
    /// `Err` once the chunk cannot take them, which is not a verdict on the record: the
    /// conversion is left for a later delivery rather than reporting a record failure the real
    /// service would have converted.
    fn charge(&mut self, cells: usize) -> Result<(), ValueFault> {
        self.remaining = self
            .remaining
            .checked_sub(cells)
            .ok_or(ValueFault::TooManyCells)?;
        Ok(())
    }
}

/// Why a record is not a row of the bound table's columns.
#[derive(Debug)]
pub(super) enum RowFault {
    /// The record itself: a failed record, under the conversion error code of its class.
    Rejected(Rejection),
    /// The record's values do not FIT one row group of this conversion
    /// ([`MAX_CELLS_PER_ROW_GROUP`]) — an emulator bound and not a statement about the record,
    /// so nothing of the delivery is written and the whole of it is owed again.
    TooManyCells,
}

/// Why one value within a record was not read: the same two, stated for one point of a
/// column's tree.
#[derive(Debug)]
enum ValueFault {
    Malformed(String),
    TooManyCells,
}

impl ValueFault {
    /// The same fault, stated from one level up the column's tree: `place` names where the
    /// value stood, which is what a consumer of the error output diagnoses the record by.
    fn within(self, place: impl FnOnce(&str) -> String) -> Self {
        match self {
            Self::Malformed(fault) => Self::Malformed(place(&fault)),
            Self::TooManyCells => Self::TooManyCells,
        }
    }
}

/// One record's row: the value of every column, in the table's column order.
///
/// `Err` carries WHY the record is not a row of these columns, in the class that decides the
/// conversion error code its error-output document names — or that it does not fit the row
/// group at all, which is no verdict on the record.
pub(super) fn row_of(
    data: &[u8],
    columns: &[(String, HiveType)],
    deserializer: &JsonDeserializer,
    budget: &mut CellBudget,
) -> Result<Vec<Value>, RowFault> {
    let document: ::serde_json::Value = ::serde_json::from_slice(data).map_err(|error| {
        RowFault::Rejected(Rejection::Unparseable(format!(
            "the record is not one JSON document: {error}"
        )))
    })?;
    let object = document.as_object().ok_or_else(|| {
        RowFault::Rejected(Rejection::Unparseable(
            "the record is not a JSON object".to_string(),
        ))
    })?;
    let keys = folded_keys(object, deserializer);
    // One value per column of the row, charged before the row is sized: the columns are the
    // first factor of what this record holds.
    charged(budget.charge(columns.len()))?;
    let mut row = Vec::with_capacity(columns.len());
    for (name, declared) in columns {
        let value = matched_value(&keys, name, deserializer);
        let read = match value {
            None => Value::Null,
            Some(value) => value_of(value, declared, deserializer, budget)
                .map_err(|fault| row_fault(fault.within(|fault| format!("column {name}: {fault}"))))?,
        };
        row.push(read);
    }
    Ok(row)
}

/// A fault about one value of a record, as the record's own verdict.
fn row_fault(fault: ValueFault) -> RowFault {
    match fault {
        ValueFault::Malformed(fault) => RowFault::Rejected(Rejection::Malformed(fault)),
        ValueFault::TooManyCells => RowFault::TooManyCells,
    }
}

/// A charge made while reading a row rather than one value of it.
fn charged(charge: Result<(), ValueFault>) -> Result<(), RowFault> {
    charge.map_err(row_fault)
}

/// A JSON object's members by the key the deserializer matches them under: folded and
/// rewritten once per object rather than once per column, so a wide group costs one pass over
/// it.
fn folded_keys<'j>(
    object: &'j ::serde_json::Map<String, ::serde_json::Value>,
    deserializer: &JsonDeserializer,
) -> BTreeMap<String, &'j ::serde_json::Value> {
    object
        .iter()
        .map(|(key, value)| (matched_key(key, deserializer), value))
        .collect()
}

/// The member of `keys` the column or field `name` reads, or `None` where the object named
/// none or named a JSON null.
fn matched_value<'j>(
    keys: &BTreeMap<String, &'j ::serde_json::Value>,
    name: &str,
    deserializer: &JsonDeserializer,
) -> Option<&'j ::serde_json::Value> {
    let json_key = deserializer.column_to_json_key.get(name).map_or(name, |key| key.as_str());
    keys.get(&matched_key(json_key, deserializer))
        .copied()
        .filter(|value| !value.is_null())
}

/// One JSON value as the value of a column of type `declared`.
///
/// `Err` carries why the value is not one that column holds, naming WHERE in the column's tree
/// it was refused — which is what makes a schema-mismatched record a failed record rather than
/// a row of nulls.
fn value_of(
    value: &::serde_json::Value,
    declared: &HiveType,
    deserializer: &JsonDeserializer,
    budget: &mut CellBudget,
) -> Result<Value, ValueFault> {
    match declared {
        HiveType::Primitive(column_type) => cell_of(value, *column_type, deserializer)
            .map(Value::Cell)
            .map_err(ValueFault::Malformed),
        HiveType::Struct(fields) => {
            let object = value
                .as_object()
                .ok_or_else(|| ValueFault::Malformed(format!("{} is not a struct", kind_of(value))))?;
            let keys = folded_keys(object, deserializer);
            // One value per DECLARED field, whatever the record names: charged before the
            // group is sized, because that fan-out is what a small record multiplies by.
            budget.charge(fields.len())?;
            let mut members = Vec::with_capacity(fields.len());
            for field in fields {
                let member = match matched_value(&keys, &field.name, deserializer) {
                    None => Value::Null,
                    Some(member) => value_of(member, &field.declared, deserializer, budget)
                        .map_err(|fault| {
                            fault.within(|fault| format!("field {}: {fault}", field.name))
                        })?,
                };
                members.push(member);
            }
            Ok(Value::Struct(members))
        }
        HiveType::List(element) => {
            let items = value
                .as_array()
                .ok_or_else(|| ValueFault::Malformed(format!("{} is not an array", kind_of(value))))?;
            budget.charge(items.len())?;
            let mut read = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                if item.is_null() {
                    read.push(Value::Null);
                    continue;
                }
                read.push(
                    value_of(item, element, deserializer, budget)
                        .map_err(|fault| fault.within(|fault| format!("element {index}: {fault}")))?,
                );
            }
            Ok(Value::List(read))
        }
        HiveType::Map(key_type, value_type) => {
            let object = value
                .as_object()
                .ok_or_else(|| ValueFault::Malformed(format!("{} is not a map", kind_of(value))))?;
            // Each entry holds its key beside its value.
            budget.charge(object.len().saturating_mul(2))?;
            let mut entries = Vec::with_capacity(object.len());
            for (key, member) in object {
                // A JSON object's key is written as text, so a map whose keys are declared as
                // another scalar reads each of them as that scalar reads a string.
                let read_key = cell_of(
                    &::serde_json::Value::String(key.clone()),
                    *key_type,
                    deserializer,
                )
                .map_err(|fault| ValueFault::Malformed(format!("key {key:?}: {fault}")))?;
                let read_value = if member.is_null() {
                    Value::Null
                } else {
                    value_of(member, value_type, deserializer, budget)
                        .map_err(|fault| fault.within(|fault| format!("key {key:?}: {fault}")))?
                };
                entries.push((read_key, read_value));
            }
            Ok(Value::Map(entries))
        }
    }
}

/// Write `rows` as ONE row group of the file: each leaf's cells as one batch of values, in the
/// order the message type declares the leaves.
///
/// A Parquet file is a sequence of row groups, so a buffer longer than
/// [`MAX_ROWS_PER_ROW_GROUP`] is written as several of them and the cells held at once are one
/// chunk's rather than the whole flush's.
///
/// ASYNC because shredding a chunk into a wide, deeply repeated schema is CPU-bound: the loop
/// hands the runtime back between leaves, so a timeout or a cancellation can land while a row
/// group is being written instead of the whole run holding the worker.
pub(super) async fn write_row_group(
    writer: &mut SerializedFileWriter<Vec<u8>>,
    columns: &[(String, HiveType)],
    rows: &[Vec<Value>],
) -> Result<(), String> {
    let mut row_group = writer.next_row_group().map_err(|error| error.to_string())?;
    for (index, (name, declared)) in columns.iter().enumerate() {
        for leaf in declared.leaves() {
            ::tokio::task::yield_now().await;
            let mut column = row_group
                .next_column()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("the Parquet schema lost the column {name}"))?;
            let mut cells = LeafCells::default();
            for row in rows {
                let value = row.get(index).unwrap_or(&Value::Null);
                emit(&leaf, &leaf.path, value, Levels::default(), &mut cells);
            }
            write_column(&mut column, &leaf, &cells).map_err(|error| error.to_string())?;
            column.close().map_err(|error| error.to_string())?;
        }
    }
    row_group.close().map_err(|error| error.to_string())?;
    Ok(())
}

/// Where in the record one value of a leaf stands.
#[derive(Debug, Clone, Copy, Default)]
struct Levels {
    /// The definition level the groups ABOVE this point have reached: how many of the optional
    /// and repeated groups on the way down are present.
    definition: i16,
    /// The repetition level the next value written carries: `0` for the first value of a
    /// record, and the level of the list or map being repeated for every later one.
    repetition: i16,
    /// How many REPEATED groups have been entered to get here, which IS the repetition level
    /// of the innermost of them.
    depth: i16,
}

/// One leaf's cells over the rows of one row group: the values that are present, with the
/// definition level of every row and the repetition level that says which repeated group each
/// value belongs to.
#[derive(Debug, Default)]
struct LeafCells {
    values: Vec<Cell>,
    definitions: Vec<i16>,
    repetitions: Vec<i16>,
}

impl LeafCells {
    /// Record one value the leaf carries.
    fn push(&mut self, cell: Cell, definition: i16, repetition: i16) {
        self.values.push(cell);
        self.definitions.push(definition);
        self.repetitions.push(repetition);
    }

    /// Record that the leaf carries NO value here, at the level the present groups above it
    /// reached: a null, an absent group, or a list or map with no entries at all.
    fn push_absent(&mut self, definition: i16, repetition: i16) {
        self.definitions.push(definition);
        self.repetitions.push(repetition);
    }
}

/// Append the cells `value` holds for `leaf` — the Dremel shredding of one record's value into
/// one leaf, which is what carries a null, an empty list and a list of several items in a
/// columnar file.
///
/// `path` is what is left of the leaf's own path from this point down, and `levels` where in
/// the record this point stands.
fn emit(leaf: &Leaf, path: &[Step], value: &Value, levels: Levels, out: &mut LeafCells) {
    let Some((step, rest)) = path.split_first() else {
        match value {
            // A present value is defined one level deeper than its groups where the leaf
            // itself is optional, and at their level where it is required (a map's key).
            Value::Cell(cell) => out.push(
                cell.clone(),
                levels.definition + i16::from(leaf.optional),
                levels.repetition,
            ),
            _ => out.push_absent(levels.definition, levels.repetition),
        }
        return;
    };
    match (step, value) {
        (Step::Field(index), Value::Struct(members)) => {
            let member = members.get(*index).unwrap_or(&Value::Null);
            emit(
                leaf,
                rest,
                member,
                Levels {
                    definition: levels.definition + 1,
                    ..levels
                },
                out,
            );
        }
        (Step::Element, Value::List(items)) => {
            // A list that is present but EMPTY is defined at the annotated group's own level
            // and carries no element below it, which is how the format tells it from an
            // absent list.
            if items.is_empty() {
                out.push_absent(levels.definition + 1, levels.repetition);
                return;
            }
            for (index, item) in items.iter().enumerate() {
                emit(leaf, rest, item, repeated(levels, index), out);
            }
        }
        (Step::Key | Step::Value, Value::Map(entries)) => {
            if entries.is_empty() {
                out.push_absent(levels.definition + 1, levels.repetition);
                return;
            }
            for (index, (key, member)) in entries.iter().enumerate() {
                let levels = repeated(levels, index);
                match step {
                    Step::Key => emit(leaf, rest, &Value::Cell(key.clone()), levels, out),
                    _ => emit(leaf, rest, member, levels, out),
                }
            }
        }
        // The record named no value for the group this step descends into, so every leaf below
        // it carries none either, at the level the groups above it reached.
        _ => out.push_absent(levels.definition, levels.repetition),
    }
}

/// The levels one entry of a list or a map stands at: two definition levels deeper (the
/// annotated group and the repeated one), and — for every entry but the FIRST, which continues
/// the record it was reached from — repeated at the level of this list or map.
fn repeated(levels: Levels, index: usize) -> Levels {
    let depth = levels.depth + 1;
    Levels {
        definition: levels.definition + 2,
        repetition: if index == 0 { levels.repetition } else { depth },
        depth,
    }
}

/// Write one leaf's cells, each with its definition level — and, for a leaf a list or a map
/// stands above, the repetition level that says which entry it belongs to.
fn write_column(
    column: &mut ::parquet::file::writer::SerializedColumnWriter<'_>,
    leaf: &Leaf,
    cells: &LeafCells,
) -> Result<(), ::parquet::errors::ParquetError> {
    let definitions = Some(cells.definitions.as_slice());
    // A leaf no list or map stands above carries no repetition levels at all: every one of its
    // values is the first of its record.
    let repetitions = (leaf.max_repetition > 0).then_some(cells.repetitions.as_slice());
    match leaf.column_type {
        ColumnType::Boolean => {
            let present: Vec<bool> = cells
                .values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Boolean(flag) => Some(*flag),
                    _ => None,
                })
                .collect();
            column
                .typed::<BoolType>()
                .write_batch(&present, definitions, repetitions)?;
        }
        ColumnType::Integer(_) | ColumnType::Date => {
            let present: Vec<i32> = cells
                .values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Int32(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<Int32Type>()
                .write_batch(&present, definitions, repetitions)?;
        }
        ColumnType::BigInt | ColumnType::Timestamp => {
            let present: Vec<i64> = cells
                .values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Int64(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<Int64Type>()
                .write_batch(&present, definitions, repetitions)?;
        }
        ColumnType::Float => {
            let present: Vec<f32> = cells
                .values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Float(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<FloatType>()
                .write_batch(&present, definitions, repetitions)?;
        }
        ColumnType::Double => {
            let present: Vec<f64> = cells
                .values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Double(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<DoubleType>()
                .write_batch(&present, definitions, repetitions)?;
        }
        ColumnType::Text | ColumnType::Binary => {
            let present: Vec<ByteArray> = cells
                .values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Text(text) => Some(ByteArray::from(text.clone().into_bytes())),
                    Cell::Bytes(bytes) => Some(ByteArray::from(bytes.clone())),
                    _ => None,
                })
                .collect();
            column
                .typed::<ByteArrayType>()
                .write_batch(&present, definitions, repetitions)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declared types, each with the tree it is read as.
    ///
    /// The names are Hive's own data types, which is what a Glue column's `Type` carries
    /// (`Column$Type`: "the data type of the Column"), and the complex ones among them are
    /// `array<…>`, `struct<…>`, `map<…,…>` and `uniontype<…>`.
    #[test]
    fn every_declared_hive_type_reads_as_its_tree() {
        let text = || HiveType::Primitive(ColumnType::Text);
        let integer = || HiveType::Primitive(ColumnType::Integer(IntegerWidth::Int));
        let cases: &[(&str, Option<HiveType>)] = &[
            ("string", Some(text())),
            ("array<string>", Some(HiveType::List(Box::new(text())))),
            // The head of a type is folded, because Hive type names are case-insensitive;
            // the names of a struct's fields are not, because a record's keys are matched
            // against them.
            ("ARRAY< String >", Some(HiveType::List(Box::new(text())))),
            (
                "struct<Sku:string,qty:int>",
                Some(HiveType::Struct(vec![
                    HiveField {
                        name: "Sku".to_string(),
                        declared: text(),
                    },
                    HiveField {
                        name: "qty".to_string(),
                        declared: integer(),
                    },
                ])),
            ),
            (
                "map<string,int>",
                Some(HiveType::Map(ColumnType::Text, Box::new(integer()))),
            ),
            (
                "array<struct<sku:string,qty:int>>",
                Some(HiveType::List(Box::new(HiveType::Struct(vec![
                    HiveField {
                        name: "sku".to_string(),
                        declared: text(),
                    },
                    HiveField {
                        name: "qty".to_string(),
                        declared: integer(),
                    },
                ])))),
            ),
            (
                "map<string,array<int>>",
                Some(HiveType::Map(
                    ColumnType::Text,
                    Box::new(HiveType::List(Box::new(integer()))),
                )),
            ),
            // A type this module has no Parquet writing for, at the top and within a complex
            // one: not a tree, so the buffer is owed again rather than written wrongly.
            ("uniontype<int,string>", None),
            ("array<decimal(10,2)>", None),
            ("struct<a:uniontype<int,string>>", None),
            // Neither is a declaration that is not one: no fields, a field with no type, a
            // map with one parameter, brackets that do not balance.
            ("struct<>", None),
            ("struct<a>", None),
            ("map<string>", None),
            ("array<string", None),
            ("", None),
        ];
        for (declared, expected) in cases {
            let read = HiveType::of_declared(declared);
            assert_eq!(
                read, *expected,
                "case {declared:?}: expected {expected:?}, got {read:?}"
            );
        }
        // A declaration that nests deeper than this reader descends is read as no tree at
        // all, whatever its length: the depth is what bounds the descent, so a declaration
        // the catalog holds cannot decide how deep it goes.
        for depth in [MAX_NESTING_DEPTH + 1, MAX_NESTING_DEPTH + 8] {
            let declared = format!("{}int{}", "array<".repeat(depth), ">".repeat(depth));
            let read = HiveType::of_declared(&declared);
            assert!(
                read.is_none(),
                "case {depth} levels deep: expected no tree, got {read:?}"
            );
        }
        // The deepest declaration within the bound still reads, so the bound is where it says
        // it is.
        let within = format!(
            "{}int{}",
            "array<".repeat(MAX_NESTING_DEPTH),
            ">".repeat(MAX_NESTING_DEPTH)
        );
        let read = HiveType::of_declared(&within);
        assert!(
            read.is_some(),
            "case {MAX_NESTING_DEPTH} levels deep: expected a tree, got {read:?}"
        );
    }

    /// What the columns of a bound table are WRITTEN as is what the conversion is bounded by:
    /// one leaf per scalar wherever it stands in a column's tree, counted over every column.
    #[test]
    fn the_leaves_of_a_table_are_counted_over_every_column() {
        let columns: Vec<(String, HiveType)> = [
            ("marker", "string"),
            ("tags", "array<struct<sku:string,qty:int>>"),
            ("labels", "map<string,int>"),
        ]
        .iter()
        .map(|(name, declared)| {
            (
                (*name).to_string(),
                HiveType::of_declared(declared).expect("each is a type this module writes"),
            )
        })
        .collect();
        // One for the scalar, two for the struct's fields, two for the map's key and value.
        let counted = leaf_count(&columns);
        assert_eq!(
            counted, 5,
            "case one scalar, one list of structs and one map: expected 5 leaves, got {counted}"
        );
        // A nesting column holds more than one leaf, so the leaves of a table can pass the
        // bound while its COLUMNS are well within theirs: that is what the leaf bound catches.
        let nesting: Vec<(String, HiveType)> = (0..MAX_CONVERTED_LEAVES)
            .map(|ordinal| {
                (
                    format!("column_{ordinal}"),
                    HiveType::of_declared("map<string,int>").expect("a map of integers"),
                )
            })
            .collect();
        let counted = leaf_count(&nesting);
        assert_eq!(
            counted,
            MAX_CONVERTED_LEAVES * 2,
            "case one map per column: expected two leaves per column, got {counted}"
        );
    }

    /// A column that nests is shredded into its leaves with the levels the columnar format
    /// carries: the definition level that tells a null from an EMPTY list or map and from an
    /// absent one, and the repetition level that says which entry of a list or map each value
    /// belongs to.
    #[test]
    fn a_nesting_column_shreds_into_the_levels_the_format_carries() {
        let columns = vec![
            (
                "tags".to_string(),
                HiveType::of_declared("array<struct<sku:string,qty:int>>")
                    .expect("the element type is one this module writes"),
            ),
            (
                "labels".to_string(),
                HiveType::of_declared("map<string,int>")
                    .expect("the key and value types are ones this module writes"),
            ),
        ];
        let deserializer = JsonDeserializer::default();
        // Three records of one chunk: one carrying entries, one carrying an EMPTY list and
        // map, and one carrying neither column at all.
        let rows: Vec<Vec<Value>> = [
            &br#"{"tags": [{"sku": "a", "qty": 1}, {"sku": "b"}], "labels": {"x": 2}}"#[..],
            &br#"{"tags": [], "labels": {}}"#[..],
            &br#"{}"#[..],
        ]
        .iter()
        .map(|record| {
            row_of(
                record,
                &columns,
                &deserializer,
                &mut CellBudget::of_row_group(),
            )
            .expect("every record is one of these rows")
        })
        .collect();
        // Each leaf of each column, with the values it carries and the levels of every one.
        struct ShreddedLeaf {
            case: &'static str,
            values: Vec<Cell>,
            definitions: Vec<i16>,
            repetitions: Vec<i16>,
        }
        let expected = [
            ShreddedLeaf {
                case: "tags.sku",
                values: vec![Cell::Text("a".to_string()), Cell::Text("b".to_string())],
                definitions: vec![4, 4, 1, 0],
                repetitions: vec![0, 1, 0, 0],
            },
            ShreddedLeaf {
                case: "tags.qty",
                values: vec![Cell::Int32(1)],
                definitions: vec![4, 3, 1, 0],
                repetitions: vec![0, 1, 0, 0],
            },
            ShreddedLeaf {
                case: "labels.key",
                values: vec![Cell::Text("x".to_string())],
                definitions: vec![2, 1, 0],
                repetitions: vec![0, 0, 0],
            },
            ShreddedLeaf {
                case: "labels.value",
                values: vec![Cell::Int32(2)],
                definitions: vec![3, 1, 0],
                repetitions: vec![0, 0, 0],
            },
        ];

        let mut shredded = Vec::new();
        for (index, (name, declared)) in columns.iter().enumerate() {
            for leaf in declared.leaves() {
                let mut cells = LeafCells::default();
                for row in &rows {
                    let value = row.get(index).unwrap_or(&Value::Null);
                    emit(&leaf, &leaf.path, value, Levels::default(), &mut cells);
                }
                shredded.push((name.clone(), leaf, cells));
            }
        }
        assert_eq!(
            shredded.len(),
            expected.len(),
            "case leaf count: expected {} leaves, got {:?}",
            expected.len(),
            shredded
                .iter()
                .map(|(name, leaf, _)| (name.clone(), leaf.path.clone()))
                .collect::<Vec<_>>()
        );
        for (expected, (_, leaf, cells)) in expected.iter().zip(&shredded) {
            let ShreddedLeaf {
                case,
                values,
                definitions,
                repetitions,
            } = expected;
            assert_eq!(
                &cells.values, values,
                "case {case}: expected the values {values:?}, got {:?}",
                cells.values
            );
            assert_eq!(
                &cells.definitions, definitions,
                "case {case}: expected the definition levels {definitions:?}, got {:?}",
                cells.definitions
            );
            assert_eq!(
                &cells.repetitions, repetitions,
                "case {case}: expected the repetition levels {repetitions:?}, got {:?}",
                cells.repetitions
            );
            assert_eq!(
                leaf.max_repetition, 1,
                "case {case}: expected one repeated group above the leaf, got {}",
                leaf.max_repetition
            );
        }
    }

    /// A value the column at its point of the tree does not hold is a MALFORMED record naming
    /// where it failed, and bytes that are not one JSON object are an UNPARSEABLE one: the two
    /// classes the conversion error code of a failed record comes from.
    #[test]
    fn a_value_a_nested_column_does_not_hold_is_a_malformed_record() {
        let columns = vec![(
            "tags".to_string(),
            HiveType::of_declared("array<struct<sku:string,qty:int>>")
                .expect("the element type is one this module writes"),
        )];
        let deserializer = JsonDeserializer::default();
        let cases: &[(&[u8], &str, &[&str])] = &[
            // A value refused deep in the tree names the column, the element and the field it
            // stands in, so a consumer of the error output knows WHERE the record failed.
            (
                &br#"{"tags": [{"sku": "a", "qty": "not-an-int"}]}"#[..],
                "malformed",
                &["tags", "element 0", "qty"],
            ),
            (
                &br#"{"tags": [{"sku": "a", "qty": 1}, {"qty": 2.5}]}"#[..],
                "malformed",
                &["tags", "element 1", "qty"],
            ),
            // A column that declares a list holds no scalar, and one that declares a struct
            // holds no array.
            (&br#"{"tags": "a"}"#[..], "malformed", &["tags"]),
            (&br#"{"tags": [[1]]}"#[..], "malformed", &["element 0"]),
            (&b"not json at all"[..], "unparseable", &[]),
            (&br#"["tags"]"#[..], "unparseable", &[]),
            // The whole tree is nullable, so a record naming none of it is a row of nulls.
            (&br#"{"tags": null}"#[..], "converted", &[]),
            (&br#"{"tags": [null]}"#[..], "converted", &[]),
            (&br#"{"tags": [{}]}"#[..], "converted", &[]),
        ];
        for (record, expected, mentions) in cases {
            let read = row_of(
                record,
                &columns,
                &deserializer,
                &mut CellBudget::of_row_group(),
            );
            let class = match &read {
                Err(RowFault::Rejected(Rejection::Unparseable(_))) => "unparseable",
                Err(RowFault::Rejected(Rejection::Malformed(_))) => "malformed",
                Err(RowFault::TooManyCells) => "too many cells",
                Ok(_) => "converted",
            };
            let shown = String::from_utf8_lossy(record);
            assert_eq!(
                class, *expected,
                "case {shown:?}: expected {expected}, got {read:?}"
            );
            for mention in *mentions {
                let fault = match &read {
                    Err(RowFault::Rejected(rejection)) => rejection.fault(),
                    _ => "",
                };
                assert!(
                    fault.contains(mention),
                    "case {shown:?}: expected the fault to name {mention:?}, got {fault:?}"
                );
            }
        }
    }

    /// What a chunk reads a record's values into is bounded by the PRODUCT of the entries the
    /// record carries and the fields the bound table declares, charged before the values are
    /// allocated — so a tiny record under a wide declared struct is refused by the bound
    /// rather than read, and refused as no verdict on the record.
    #[test]
    fn a_records_values_are_bounded_by_the_entries_times_the_declared_fields() {
        /// How many fields the declared struct of this case holds.
        const FIELDS: usize = 2_000;

        let fields: Vec<String> = (0..FIELDS)
            .map(|ordinal| format!("field_{ordinal}:string"))
            .collect();
        let declared = format!("array<struct<{}>>", fields.join(","));
        let columns = vec![(
            "wide".to_string(),
            HiveType::of_declared(&declared).expect("a list of structs of strings"),
        )];
        let deserializer = JsonDeserializer::default();
        // Each element of the list holds one value per DECLARED field, whatever the element
        // itself carries: this many empty elements is what passes the bound.
        let elements = MAX_CELLS_PER_ROW_GROUP / FIELDS + 1;
        let record = format!(
            r#"{{"wide": [{}]}}"#,
            vec!["{}"; elements].join(",")
        );
        let read = row_of(
            record.as_bytes(),
            &columns,
            &deserializer,
            &mut CellBudget::of_row_group(),
        );
        assert!(
            matches!(read, Err(RowFault::TooManyCells)),
            "case {elements} empty elements under {FIELDS} declared fields: expected the row \
             group's bound to refuse it, got {read:?}"
        );
        // One row group's worth of the same record reads, so the bound is where it says it is
        // and a record a client would really put is not refused by it.
        let within = format!(
            r#"{{"wide": [{}]}}"#,
            vec!["{}"; elements / 2].join(",")
        );
        let read = row_of(
            within.as_bytes(),
            &columns,
            &deserializer,
            &mut CellBudget::of_row_group(),
        );
        assert!(
            read.is_ok(),
            "case {} empty elements under {FIELDS} declared fields: expected a row, got \
             {read:?}",
            elements / 2
        );
    }
}

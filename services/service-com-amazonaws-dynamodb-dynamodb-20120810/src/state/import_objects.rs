// SPDX-License-Identifier: CC0-1.0

//! The source objects one table import READS out of Amazon S3, and the items it makes of
//! them.
//!
//! "Imports table data from an S3 bucket" (the `ImportTable` doc): what an import reads
//! is the objects the request's `S3BucketSource` selects — every object of that bucket
//! under the key prefix it names — so they have to be objects Amazon S3 really holds,
//! which is why they are read through that service's own store on this same engine
//! ([`::service_com_amazonaws_s3_amazons3::state`]) rather than conjured here. An import
//! of a bucket nothing has written to reads nothing.
//!
//! The DATA the new table holds is therefore the source data itself, decoded from the
//! format the request names: nothing is synthesised, and an item the source carries in a
//! form the table cannot hold is counted as an error rather than stored as something
//! else.
//!
//! What ONE import reads is bounded by this emulator's own ceilings
//! ([`MAX_IMPORTED_ITEMS`], [`MAX_IMPORT_SOURCE_BYTES`]) — measured over the whole source
//! and over the DECODED bytes, so neither many objects nor one highly compressible object
//! can expand past it — and a source past them is refused BEFORE the table is created.

use std::io::Read;

use super::*;

/// How many items ONE import writes, at most.
///
/// This emulator's own ceiling on the work a single call can ask for, not an AWS quota:
/// DynamoDB states no bound on what one import may carry. An import reads every item its
/// source holds and writes it into the new table, so without a bound of its own one call's
/// work would be whatever a bucket has been filled with. It is the ceiling one export
/// renders ([`MAX_EXPORTED_ITEMS`]) read the other way round, since the two surfaces move
/// the same data. A source past it is refused BEFORE anything is created, with the
/// modelled `LimitExceededException` and [`import_too_large_message`].
pub const MAX_IMPORTED_ITEMS: i64 = MAX_EXPORTED_ITEMS;

/// How many bytes of SOURCE DATA one import reads, at most — the same ceiling measured
/// over the data rather than the item count, since one large item costs what many small
/// ones do. Measured over the DECODED bytes of the whole source ([`ImportBudget`]), so
/// neither the number of objects nor the compression ratio of any one of them can take an
/// import past it.
pub const MAX_IMPORT_SOURCE_BYTES: i64 = MAX_EXPORTED_BYTES;

/// How an import of more data than one call will read is refused.
///
/// The message names THE EMULATOR as what ran out and says the service itself imposes no
/// such limit, the way this store words its other own-exhaustion refusals, rather than
/// sending a client to fix input that is not wrong.
pub fn import_too_large_message(bucket: &str) -> String {
    format!(
        "This emulator cannot import from bucket {bucket}: one import reads at most \
         {MAX_IMPORTED_ITEMS} items and {MAX_IMPORT_SOURCE_BYTES} bytes of source data. \
         DynamoDB itself imposes no such limit; import a smaller source."
    )
}

/// One object an import reads: its key in the source bucket and the bytes stored under
/// it.
#[derive(Debug, Clone)]
pub struct ImportObject {
    /// The object's key, under the source's own prefix.
    pub key: String,
    /// The object's bytes, as the object store holds them.
    pub body: Vec<u8>,
}

/// Why an import's source could not be read at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSourceFault {
    /// The object store itself could not be reached on this engine.
    StoreUnavailable,
    /// The store holds no bucket under the name the source names, so there is nothing
    /// the import could read.
    NoSuchBucket,
    /// The objects the source selects come to more than one call of this emulator reads
    /// ([`MAX_IMPORT_SOURCE_BYTES`]), which is a request to be REFUSED
    /// ([`import_too_large_message`]) rather than a source that could not be read.
    TooLarge,
}

/// The objects the source selects, read out of Amazon S3's own store on this engine.
///
/// "The S3 bucket that is being imported from" (`S3BucketSource$S3Bucket`) and "The key
/// prefix shared by all S3 Objects that are being imported" (`$S3KeyPrefix`): every
/// current object of that bucket whose key starts with that prefix, in the ascending key
/// order S3 lists them in, so one import reads the same objects in the same order
/// whatever wrote them.
///
/// A bucket the store holds none of is [`ImportSourceFault::NoSuchBucket`]: an import
/// cannot read a source that does not exist, and that is the reason its task reports.
///
/// The ceiling is applied AS the objects are selected, not after: the bytes already taken
/// are counted before each further body is copied out of the object store, and a selection
/// that passes [`MAX_IMPORT_SOURCE_BYTES`] stops there with
/// [`ImportSourceFault::TooLarge`] — the request names only a bucket and a prefix, and the
/// empty prefix selects the whole bucket, so a reader that copied first and counted
/// afterwards would hold the whole of whatever that bucket already stores.
pub fn read_import_source<Eng: Send + Sync + 'static>(
    engine: &Eng,
    source: &S3BucketSource,
) -> Result<Vec<ImportObject>, ImportSourceFault> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)
        .ok_or(ImportSourceFault::StoreUnavailable)?;
    let store = state.lock();
    let bucket = store
        .bucket(&source.s3_bucket)
        .ok_or(ImportSourceFault::NoSuchBucket)?;
    let prefix = source.s3_key_prefix.as_deref().unwrap_or_default();
    let mut selected: Vec<ImportObject> = Vec::new();
    let mut taken: i64 = 0;
    for (key, object) in bucket.objects().filter(|(key, _)| key.starts_with(prefix)) {
        taken = taken.saturating_add(i64::try_from(object.body.len()).unwrap_or(i64::MAX));
        if taken > MAX_IMPORT_SOURCE_BYTES {
            return Err(ImportSourceFault::TooLarge);
        }
        selected.push(ImportObject {
            key: key.to_string(),
            body: object.body.clone(),
        });
    }
    Ok(selected)
}

/// What one import's task made of the objects it read.
#[derive(Debug, Clone, Default)]
pub struct ImportedData {
    /// The items the task decoded, in the order it read them. The caller keys them
    /// against the new table's own key schema, so an item the schema cannot key is
    /// counted among the errors there rather than here.
    pub items: Vec<Item>,
    /// "The total number of items processed from the source file"
    /// (`ImportTableDescription$ProcessedItemCount`): every record the task read,
    /// including the ones it could not decode.
    pub processed_item_count: i64,
    /// "The number of errors occurred on importing the source file into the target
    /// table" (`$ErrorCount`): the records the task read and could not decode.
    pub error_count: i64,
    /// "The total size of data processed from the source file, in Bytes"
    /// (`$ProcessedSizeBytes`): the bytes of the objects the task read, as the object
    /// store holds them.
    pub processed_size_bytes: i64,
}

/// Why an import could not be read even though its source exists: it asks for more than
/// one call of this emulator will read ([`import_too_large_message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportTooLarge;

/// What ONE import may still read, and what it has read already.
///
/// The budget is CUMULATIVE over the whole source and measured over the DECODED bytes,
/// not the stored ones: a compressed object expands, so a per-object ceiling would bound
/// each factor (object count, expansion per object) and not their product. Every decoded
/// object, and every item retained from one, is spent from here before it is held, so the
/// aggregate an import can hold in memory is [`MAX_IMPORT_SOURCE_BYTES`] whatever the
/// source is shaped like.
#[derive(Debug, Clone, Copy)]
struct ImportBudget {
    spent: i64,
}

impl ImportBudget {
    /// The whole budget, unspent.
    fn new() -> Self {
        Self { spent: 0 }
    }

    /// How many bytes are left of it, never below zero.
    fn remaining(self) -> i64 {
        MAX_IMPORT_SOURCE_BYTES.saturating_sub(self.spent).max(0)
    }

    /// Spend `bytes` of it, or refuse the import where that is more than is left.
    fn spend(&mut self, bytes: usize) -> Result<(), ImportTooLarge> {
        self.spent = self
            .spent
            .saturating_add(i64::try_from(bytes).unwrap_or(i64::MAX));
        if self.spent > MAX_IMPORT_SOURCE_BYTES {
            return Err(ImportTooLarge);
        }
        Ok(())
    }
}

/// The items `objects` carry, decoded from the format the request names.
///
/// `key_attribute_types` is the `AttributeType` each attribute the new table DECLARES is
/// declared with (`AttributeDefinitions`), which is what a CSV column's value is read as:
/// "DynamoDB imports CSV data with every field as a String, other than the ones the key
/// schema's attribute definitions type" — a column named by a definition carries that
/// type's value and every other column carries a string.
///
/// Decoding never fails as a whole: a record the source carries in a form the table
/// cannot hold is counted in [`ImportedData::error_count`] and left out of the items, the
/// way an import task logs an error for it and carries on. The one refusal is a source
/// past this emulator's own ceilings, which is [`ImportTooLarge`] — measured over the
/// whole source by [`ImportBudget`].
pub fn imported_items(
    objects: &[ImportObject],
    input_format: &str,
    input_compression_type: &str,
    input_format_options: Option<&InputFormatOptions>,
    key_attribute_types: &BTreeMap<String, String>,
) -> Result<ImportedData, ImportTooLarge> {
    let mut read = ImportedData::default();
    let mut budget = ImportBudget::new();
    for object in objects {
        // What the task reports having processed is the SOURCE's own size, as the object
        // store holds it; what it is allowed to read is the decoded volume below.
        read.processed_size_bytes = read
            .processed_size_bytes
            .saturating_add(i64::try_from(object.body.len()).unwrap_or(i64::MAX));
        let decoded = decompressed(&object.body, input_compression_type, budget.remaining())?;
        // An object whose compression the request named and whose bytes do not carry it
        // is one record's worth of error rather than a refusal: the task read it and
        // could not make items of it.
        let Some(decoded) = decoded else {
            read.processed_item_count = read.processed_item_count.saturating_add(1);
            read.error_count = read.error_count.saturating_add(1);
            continue;
        };
        budget.spend(decoded.len())?;
        match input_format {
            INPUT_FORMAT_CSV => read_csv_object(
                &decoded,
                input_format_options.and_then(|options| options.csv.as_ref()),
                key_attribute_types,
                &mut read,
                &mut budget,
            )?,
            // The DynamoDB JSON form is the one a table export writes, and the default
            // this reader falls back to: `InputFormat` is a modelled enum, so the only
            // values that reach here are the ones the model declares, and `ION` is
            // refused before the import is recorded.
            _ => read_dynamodb_json_object(&decoded, &mut read, &mut budget)?,
        }
    }
    Ok(read)
}

/// Keep one decoded item, or count it as an error where the table could not hold it.
///
/// The documented maximum item size is applied HERE, as each item is decoded, rather than
/// only where the items are keyed ([`keyed_items`]): an item past it is one no write could
/// store, so there is no reason to hold it in the meantime.
///
/// What IS held is spent from the import's own budget before it is kept, which is what makes
/// the aggregate this reader can hold [`MAX_IMPORT_SOURCE_BYTES`] whatever the source is
/// shaped like ([`ImportBudget`]): decoded bytes and retained bytes are not proportional — a
/// source whose every record repeats one long attribute name costs those bytes once in the
/// stream and once per item — so a reader that charged only the stream would hold as much as
/// the item COUNT allows rather than as much as the budget does.
fn retained(
    item: Item,
    read: &mut ImportedData,
    budget: &mut ImportBudget,
) -> Result<(), ImportTooLarge> {
    let size = item_size(&item);
    if size > MAX_ITEM_SIZE_BYTES {
        read.error_count = read.error_count.saturating_add(1);
        return Ok(());
    }
    budget.spend(usize::try_from(size).unwrap_or(usize::MAX))?;
    read.items.push(item);
    Ok(())
}

/// What one import's items became once keyed against the table they are written into.
#[derive(Debug, Clone, Default)]
pub struct KeyedImport {
    /// The items the new table HOLDS, keyed by their primary key, which is the map a
    /// table is created with.
    pub items: BTreeMap<ItemKey, Item>,
    /// The bytes of item data those items carry — the table's own stored total.
    pub stored_bytes: i64,
    /// "The number of items successfully imported into the new table"
    /// (`ImportTableDescription$ImportedItemCount`).
    pub imported_item_count: i64,
    /// The items that could not be written: one the table's key schema cannot key, one
    /// carrying a value the store does not hold, or one past the documented item size.
    pub error_count: i64,
}

/// The items `table` HOLDS once `items` are written into it, and how many of them could
/// not be.
///
/// Each item is held to exactly what a single-item write holds one to — its numbers
/// canonicalized and its values held to the documented domain
/// ([`canonicalize_item`]), the documented maximum item size
/// ([`MAX_ITEM_SIZE_BYTES`]) and the table's own key schema ([`Table::key_of_item`]) —
/// so an imported table holds only items a `PutItem` could have written. An item that
/// fails any of those is counted among the errors instead, the way an import task logs an
/// error for a record it could not write and carries on.
///
/// Two source records with the SAME primary key are one item, the later one: that is what
/// a key means, so the import reports the second as imported and the table holds one.
pub fn keyed_items(table: &Table, items: Vec<Item>) -> KeyedImport {
    let mut keyed = KeyedImport::default();
    for mut item in items {
        if canonicalize_item(&mut item).is_err() {
            keyed.error_count = keyed.error_count.saturating_add(1);
            continue;
        }
        let size = item_size(&item);
        let Ok(key) = table.key_of_item(&item) else {
            keyed.error_count = keyed.error_count.saturating_add(1);
            continue;
        };
        if size > MAX_ITEM_SIZE_BYTES {
            keyed.error_count = keyed.error_count.saturating_add(1);
            continue;
        }
        let replaced = keyed.items.insert(key, item);
        keyed.stored_bytes = keyed
            .stored_bytes
            .saturating_sub(replaced.as_ref().map_or(0, item_size))
            .saturating_add(size);
        keyed.imported_item_count = keyed.imported_item_count.saturating_add(1);
    }
    keyed
}

/// `body` with the compression the request named undone, `None` where the bytes do not
/// carry that compression at all.
///
/// "Type of compression to be used on the input coming from the imported table"
/// (`ImportTable$InputCompressionType`): `NONE` reads the stored bytes themselves, and
/// the two compressed forms are decoded with a reader bounded by `remaining` — what is
/// LEFT of this import's budget ([`ImportBudget`]) — so neither one object nor the source
/// as a whole can expand past it.
fn decompressed(
    body: &[u8],
    input_compression_type: &str,
    remaining: i64,
) -> Result<Option<Vec<u8>>, ImportTooLarge> {
    // One byte past what is left, so an expansion that reaches the bound is told from one
    // that exceeds it.
    let budget = u64::try_from(remaining)
        .unwrap_or(0)
        .saturating_add(1);
    let mut decoded = Vec::new();
    let read = match input_compression_type {
        "GZIP" => ::flate2::read::GzDecoder::new(body)
            .take(budget)
            .read_to_end(&mut decoded)
            .is_ok(),
        "ZSTD" => match ::ruzstd::decoding::StreamingDecoder::new(body) {
            Ok(decoder) => decoder.take(budget).read_to_end(&mut decoded).is_ok(),
            Err(_) => false,
        },
        // `NONE`, the value the member defaults to: the source bytes are the data, read
        // under the same bound.
        _ => {
            let allowed = usize::try_from(budget).unwrap_or(usize::MAX);
            decoded.extend(body.iter().copied().take(allowed));
            true
        }
    };
    if !read {
        return Ok(None);
    }
    if i64::try_from(decoded.len()).unwrap_or(i64::MAX) > remaining {
        return Err(ImportTooLarge);
    }
    Ok(Some(decoded))
}

/// Read one DYNAMODB_JSON source object: one item per line, each the item as DynamoDB
/// JSON — either under the `Item` member a table export writes it under
/// ([`exported_objects`]) or as the attribute map itself.
///
/// A line that is not an item of that form is counted as an error, which is what an
/// import task does with a record it cannot read.
fn read_dynamodb_json_object(
    body: &[u8],
    read: &mut ImportedData,
    budget: &mut ImportBudget,
) -> Result<(), ImportTooLarge> {
    for line in body.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        read.processed_item_count = read.processed_item_count.saturating_add(1);
        if read.processed_item_count > MAX_IMPORTED_ITEMS {
            return Err(ImportTooLarge);
        }
        match ::serde_json::from_slice::<::serde_json::Value>(line)
            .ok()
            .and_then(|value| item_from_json(&value))
        {
            Some(item) => retained(item, read, budget)?,
            None => read.error_count = read.error_count.saturating_add(1),
        }
    }
    Ok(())
}

/// Read one CSV source object: one item per row, each column an attribute of the item
/// named by its header.
///
/// "List of the headers used to specify a common header for all source CSV files being
/// imported. NOTE: If this field is specified then the first line of each CSV file is
/// treated as data instead of the header. If this field is not specified the the first
/// line of each CSV file is treated as the header" (`CsvOptions$HeaderList`), and "The
/// delimiter used for separating items in the CSV file being imported"
/// (`$Delimiter`), which defaults to the comma the format is named for.
///
/// An EMPTY cell is no attribute at all rather than an empty value: a CSV row carries one
/// cell per header whether that header applies to the row or not, so a row that leaves one
/// blank is an item without that attribute. A row that cannot be read, or whose cell count
/// does not match the headers, is counted as an error.
fn read_csv_object(
    body: &[u8],
    options: Option<&CsvOptions>,
    key_attribute_types: &BTreeMap<String, String>,
    read: &mut ImportedData,
    budget: &mut ImportBudget,
) -> Result<(), ImportTooLarge> {
    let headers: Option<Vec<String>> = options
        .and_then(|options| options.header_list.clone())
        .filter(|headers| !headers.is_empty());
    let delimiter = options
        .and_then(|options| options.delimiter.as_deref())
        .and_then(|delimiter| delimiter.as_bytes().first().copied())
        .unwrap_or(b',');
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(headers.is_none())
        .from_reader(body);
    // The names the columns carry: the ones the request named, or the file's own first
    // line where it named none.
    let names: Vec<String> = match headers {
        Some(named) => named,
        None => match reader.headers() {
            Ok(row) => row.iter().map(str::to_string).collect(),
            // A file whose first line cannot be read as a header row carries no item
            // this import can name the attributes of.
            Err(_) => return Ok(()),
        },
    };
    for row in reader.records() {
        read.processed_item_count = read.processed_item_count.saturating_add(1);
        if read.processed_item_count > MAX_IMPORTED_ITEMS {
            return Err(ImportTooLarge);
        }
        let Ok(row) = row else {
            read.error_count = read.error_count.saturating_add(1);
            continue;
        };
        if row.len() != names.len() {
            read.error_count = read.error_count.saturating_add(1);
            continue;
        }
        let mut item = Item::default();
        let mut readable = true;
        for (name, cell) in names.iter().zip(row.iter()) {
            if cell.is_empty() {
                continue;
            }
            match csv_attribute_value(cell, key_attribute_types.get(name.as_str())) {
                Some(value) => {
                    item.insert(name.clone(), value);
                }
                None => readable = false,
            }
        }
        if readable {
            retained(item, read, budget)?;
        } else {
            read.error_count = read.error_count.saturating_add(1);
        }
    }
    Ok(())
}

/// One CSV cell as the attribute value it stands for: the type the table DECLARES for
/// that attribute where it declares one, and a string otherwise — the only type a CSV
/// cell carries on its own.
///
/// `None` where the cell does not carry a value of the declared type, which makes the row
/// an error: a `B` attribute's cell is base64, as a blob is on the wire, and an `N`
/// attribute's cell is a number.
fn csv_attribute_value(cell: &str, declared_type: Option<&String>) -> Option<AttributeValue> {
    match declared_type.map(String::as_str) {
        Some("N") => number_attribute_value(cell),
        Some("B") => ::aws_smithy_types::base64::decode(cell)
            .ok()
            .map(AttributeValue::B),
        _ => Some(AttributeValue::S(cell.to_string())),
    }
}

/// One item as DynamoDB JSON carries it: the attribute map under the `Item` member a
/// table export writes it under, or the map itself.
///
/// `None` where the value is no item of either form, or carries an attribute the table
/// cannot hold.
fn item_from_json(value: &::serde_json::Value) -> Option<Item> {
    let members = value.as_object()?;
    // The envelope FIRST, and only where the whole value is that one member: an item of
    // its own may carry an attribute named `Item`, and such a line is read as the item it
    // is rather than as an envelope around one attribute value.
    if members.len() == 1 {
        if let Some(::serde_json::Value::Object(enveloped)) = members.get("Item") {
            if let Some(item) = attribute_map_from_json(enveloped, 1) {
                return Some(item);
            }
        }
    }
    attribute_map_from_json(members, 1)
}

/// One attribute map as DynamoDB JSON carries it, at `depth` levels of nesting.
fn attribute_map_from_json(
    members: &::serde_json::Map<String, ::serde_json::Value>,
    depth: usize,
) -> Option<Item> {
    let mut item = Item::default();
    for (name, value) in members {
        item.insert(name.clone(), attribute_value_from_json(value, depth)?);
    }
    Some(item)
}

/// One attribute value as DynamoDB JSON carries it: a single-member object whose name is
/// the attribute's TYPE (`S`, `N`, `B`, `SS`, `NS`, `BS`, `M`, `L`, `NULL`, `BOOL`) —
/// the inverse of [`attribute_value_json`], so an export's own data file reads back as
/// the items it was written from.
///
/// `None` for anything else, which is what makes the record carrying it an error. The
/// recursion is bounded by [`MAX_ATTRIBUTE_DEPTH`]: "DynamoDB supports nested attributes
/// up to 32 levels deep", so a value deeper than a stored one may be is not an item this
/// table could hold.
fn attribute_value_from_json(
    value: &::serde_json::Value,
    depth: usize,
) -> Option<AttributeValue> {
    if depth > MAX_ATTRIBUTE_DEPTH {
        return None;
    }
    let members = value.as_object()?;
    // "must contain exactly one of the supported datatypes"
    // (`crate::service::attribute_value_members`'s own wording): a value naming none or
    // several is no attribute value at all.
    if members.len() != 1 {
        return None;
    }
    let (datatype, carried) = members.iter().next()?;
    match datatype.as_str() {
        "S" => Some(AttributeValue::S(carried.as_str()?.to_string())),
        "N" => number_attribute_value(&json_number_text(carried)?),
        "B" => ::aws_smithy_types::base64::decode(carried.as_str()?)
            .ok()
            .map(AttributeValue::B),
        "SS" => Some(AttributeValue::Ss(
            carried
                .as_array()?
                .iter()
                .map(|member| member.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()?,
        )),
        "NS" => Some(AttributeValue::Ns(
            carried
                .as_array()?
                .iter()
                .map(|member| {
                    let text = json_number_text(member)?;
                    // Every member of a number set is a number, so one that is not makes
                    // the set no attribute value either.
                    matches!(number_attribute_value(&text), Some(AttributeValue::N(_)))
                        .then_some(text)
                })
                .collect::<Option<Vec<String>>>()?,
        )),
        "BS" => Some(AttributeValue::Bs(
            carried
                .as_array()?
                .iter()
                .map(|member| ::aws_smithy_types::base64::decode(member.as_str()?).ok())
                .collect::<Option<Vec<Vec<u8>>>>()?,
        )),
        "M" => Some(AttributeValue::M(attribute_map_from_json(
            carried.as_object()?,
            depth + 1,
        )?)),
        "L" => Some(AttributeValue::L(
            carried
                .as_array()?
                .iter()
                .map(|member| attribute_value_from_json(member, depth + 1))
                .collect::<Option<Vec<AttributeValue>>>()?,
        )),
        "NULL" => Some(AttributeValue::Null(carried.as_bool()?)),
        "BOOL" => Some(AttributeValue::Bool(carried.as_bool()?)),
        _ => None,
    }
}

/// The text a JSON value carries a NUMBER as: the string DynamoDB JSON writes an `N` in,
/// or the JSON number a source may carry it as instead.
fn json_number_text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// One source value as the `N` attribute it stands for, or `None` where it is no number.
///
/// Held to the same domain a written number is ([`canonicalize_attribute_value`] through
/// [`canonicalize_item`], which the caller applies to the whole item): this only decides
/// whether the text is a number at all, so a source carrying something else in a numeric
/// column is an error rather than a string in a number's place.
fn number_attribute_value(text: &str) -> Option<AttributeValue> {
    let mut value = AttributeValue::N(text.to_string());
    canonicalize_attribute_value(&mut value).ok().map(|()| value)
}

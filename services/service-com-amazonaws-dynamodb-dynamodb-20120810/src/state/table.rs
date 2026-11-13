// SPDX-License-Identifier: CC0-1.0

//! One table: the items it holds, the indexes over them, and the `TableDescription`
//! every reader of it projects.

use super::*;

/// How an operand a key CONDITION compares a key attribute with is refused when its type
/// is not the one the schema declares for that attribute: AWS's FIXED sentence for a
/// condition parameter, which states the rule and reports neither the attribute nor
/// either type. A key the request ADDRESSES an item by is a different member and keeps
/// its own sentence ([`Table::key_value`]).
const CONDITION_PARAMETER_TYPE_MESSAGE: &str =
    "One or more parameter values were invalid: Condition parameter type does not match \
     schema type";

/// The type a vector index requires of the attribute it indexes, as DynamoDB names it in
/// the refusals [`Table::check_vector_attributes`] states: "Each element is a 32-bit
/// IEEE-754 floating point number, provided in DynamoDB list format"
/// (`SearchVectors$SearchVector`), so the attribute is a LIST of that type and each of its
/// elements is one.
const VECTOR_PARAMETER_TYPE: &str = "32-bit floating point number";

/// How the item being held to the secondary-index key rules reached the service: in an
/// `Item` the request carried, or as what an UPDATE leaves behind.
///
/// The rules are the same for both; what differs is the sentence an empty index key
/// value is refused with, because an update states what to CHANGE rather than the whole
/// item (see [`Table::check_index_key_values_of_update`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexKeyWrite {
    Item,
    Update,
}

/// The least a write to a vector index meters: a vector write is denominated in BYTES
/// ("VectorWriteRequestBytes" — `ConsumedCapacity$VectorIndexes`) with a 1 KB floor, so a
/// small vector costs the floor rather than its own few bytes, the way a write of a small
/// item still costs one capacity unit.
pub const MIN_VECTOR_WRITE_BYTES: i64 = 1024;

/// What one dimension of a STORED vector costs: a vector attribute is a list of 32-bit
/// IEEE-754 floats ([`vector_values`]), so each dimension is four bytes once it is held
/// as the index holds it rather than as the decimal text the request sent.
pub const BYTES_PER_VECTOR_DIMENSION: i64 = 4;

impl VectorIndex {
    /// The bytes one write of an item this index covers meters on it, where `key_bytes` is
    /// the stored size of that item's primary KEY.
    ///
    /// What a vector write meters is the index ENTRY, not the item it was written from,
    /// and the entry is sized the way this store sizes an item: the key it is held under,
    /// plus the vector attribute — its name, and four bytes per dimension of the index
    /// ([`BYTES_PER_VECTOR_DIMENSION`]) for the vector itself rather than the decimal text
    /// the request spelled it with. The figure is then held to the per-write floor
    /// ([`MIN_VECTOR_WRITE_BYTES`]).
    ///
    /// So it grows with the index's `Dimensions`, a small vector meters the floor however
    /// large the item around it is, and the attributes that item carries beside the vector
    /// are metered on the table's own capacity rather than here.
    pub fn write_request_bytes(&self, key_bytes: i64) -> i64 {
        let attribute =
            i64::try_from(self.vector_attribute.attribute_name.len()).unwrap_or(i64::MAX);
        self.dimensions
            .saturating_mul(BYTES_PER_VECTOR_DIMENSION)
            .saturating_add(attribute)
            .saturating_add(key_bytes)
            .max(MIN_VECTOR_WRITE_BYTES)
    }
}

impl Table {
    /// The size a WRITE of `item` against this table is charged for: the item as the
    /// store holds it, with every attribute a vector index of this table indexes counted
    /// at four bytes per dimension ([`BYTES_PER_VECTOR_DIMENSION`]) instead of at the
    /// decimal text the request spelled that vector with.
    ///
    /// A vector is held as 32-bit floats, which is the form the index meters as well
    /// ([`VectorIndex::write_request_bytes`]), so one rule sizes a vector wherever it is
    /// metered: the per-dimension cost of writing it is part of what the item's own
    /// capacity is charged for, and it does not depend on how many digits the request
    /// spelled each coordinate with.
    ///
    /// An attribute several indexes index is counted once: what is being sized is the
    /// ITEM, not the entries the write fans out to. An item carrying no vector, and every
    /// item of a table with no vector index, is sized exactly as the store holds it.
    fn metered_item_size(&self, item: &Item) -> i64 {
        let mut size = item_size(item);
        let mut counted: Vec<&str> = Vec::new();
        for index in &self.vector_indexes {
            let name = index.vector_attribute.attribute_name.as_str();
            if counted.contains(&name) {
                continue;
            }
            let Some(value) = item.get(name) else {
                continue;
            };
            let AttributeValue::L(coordinates) = value else {
                continue;
            };
            counted.push(name);
            let stored = i64::try_from(coordinates.len())
                .unwrap_or(i64::MAX)
                .saturating_mul(BYTES_PER_VECTOR_DIMENSION);
            size = size
                .saturating_sub(attribute_value_size(value))
                .saturating_add(stored);
        }
        size
    }
}

impl Table {
    /// The status at `now`, derived from the stored instants.
    pub fn status(&self, now: SystemTime) -> TableStatus {
        if self.removed_at.is_some() {
            TableStatus::Deleting
        } else if now < self.active_at {
            TableStatus::Creating
        } else if self.updated_at.is_some_and(|due| now < due) {
            TableStatus::Updating
        } else {
            TableStatus::Active
        }
    }

    /// Whether the clock has already made this table's removal due.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.removed_at.is_some_and(|due| now >= due)
    }

    /// How long the clock still owes this table its `CREATING` -> `ACTIVE` transition at
    /// `now`, or `None` for a table whose creation has already fallen due.
    ///
    /// The creation is the transition that makes a table usable at all — "You can perform
    /// read and write operations only on an ACTIVE table" (the `CreateTable` doc) — so it
    /// is the one a client blocks on before it does anything else, with a poller whose
    /// interval is far longer than the span this emulator compresses the creation into
    /// ([`STATUS_TRANSITION`]). [`crate::ops::describe_table`] reports the table once this
    /// is no longer owed, so the first poll a client sends observes the settled table
    /// instead of waiting out its whole polling interval for a transition that was already
    /// a fraction of a second away. A table being deleted or updated is reported as it
    /// stands: it already serves (or has served) requests, so nothing is gated on it.
    pub fn creation_settles_in(&self, now: SystemTime) -> Option<Duration> {
        self.active_at
            .duration_since(now)
            .ok()
            .filter(|left| !left.is_zero())
    }

    /// The read and write capacity units this table has PROVISIONED: its own, plus that
    /// of every global secondary index on it.
    ///
    /// That sum is the unit both documented throughput quotas are stated in — the
    /// per-table one restricts "the sum of the provisioned capacity of the new table
    /// itself and all its global secondary indexes" (the `DescribeLimits` doc) — so one
    /// function answers it and every reader measures the same thing. A local secondary
    /// index shares the table's own capacity and so is already counted; a table in
    /// `PAY_PER_REQUEST` mode provisions no capacity at all ("you don't need to specify
    /// how much read and write throughput you expect" — `BillingMode`), and contributes
    /// nothing.
    pub fn provisioned_capacity(&self) -> (i64, i64) {
        ::std::iter::once(self.provisioned_throughput.as_ref())
            .chain(
                self.global_secondary_indexes
                    .iter()
                    .map(|index| index.provisioned_throughput.as_ref()),
            )
            .flatten()
            .fold((0_i64, 0_i64), |(read, write), throughput| {
                (
                    read.saturating_add(throughput.read_capacity_units),
                    write.saturating_add(throughput.write_capacity_units),
                )
            })
    }

    /// The ARN of the stream this table currently publishes to, for a table whose
    /// stream is ON.
    ///
    /// A DISABLED stream "takes no further records" — the table keeps reporting it as
    /// its latest stream, and a consumer can still drain what it captured — so a write
    /// publishes nothing to it.
    pub fn enabled_stream_arn(&self) -> Option<String> {
        self.stream
            .as_ref()
            .filter(|stream| stream.enabled)
            .map(|stream| stream.arn.clone())
    }

    /// "The primary key attribute(s) for the DynamoDB item that was modified"
    /// (`StreamRecord$Keys`): the key attributes of `item`, read through this table's
    /// own key schema, and nothing else.
    ///
    /// Read off the ITEM rather than rebuilt from the encoded [`ItemKey`], so the
    /// values a consumer sees are the ones the table stores.
    pub fn stream_keys(&self, item: &Item) -> Item {
        self.key_schema
            .iter()
            .filter_map(|element| {
                item.get_key_value(&element.attribute_name)
                    .map(|(name, value)| (name.clone(), value.clone()))
            })
            .collect()
    }

    /// The name of the `HASH` key attribute.
    pub fn partition_key_name(&self) -> &str {
        self.key_schema
            .iter()
            .find(|element| element.key_type == "HASH")
            .map_or("", |element| element.attribute_name.as_str())
    }

    /// The name of the `RANGE` key attribute, when the table declares one.
    pub fn sort_key_name(&self) -> Option<&str> {
        self.key_schema
            .iter()
            .find(|element| element.key_type == "RANGE")
            .map(|element| element.attribute_name.as_str())
    }

    /// The `AttributeType` (`S`/`N`/`B`) declared for `attribute`.
    pub(crate) fn declared_type(&self, attribute: &str) -> Option<&str> {
        self.attribute_definitions
            .iter()
            .find(|definition| definition.attribute_name == attribute)
            .map(|definition| definition.attribute_type.as_str())
    }

    /// Encode one key attribute, refusing a value whose type is not the one the
    /// table declared for it, whose `S`/`B` value is empty, or whose size is past the
    /// documented limit for the role the attribute plays
    /// ([`Self::check_key_limits`]).
    pub fn key_value(
        &self,
        attribute: &str,
        value: &AttributeValue,
    ) -> Result<AttributeValueKey, RequestError> {
        // CreateTable refuses a key schema whose attributes are not all defined, so
        // a key attribute always has a declared type; an undefined one is a bad
        // request rather than something to guess a type for.
        let declared = self.declared_type(attribute).ok_or_else(|| {
            RequestError::Validation(format!(
                "No AttributeDefinition was provided for key attribute {attribute}"
            ))
        })?;
        // "The AttributeValue for a key attribute cannot contain an empty string
        // value" / an empty binary value: a key attribute's value must be non-empty,
        // however the table declared its type, and the refusal names the key it was
        // supplied for. A value of ANOTHER type than the one declared is the type
        // mismatch below, whether or not it is also empty.
        if declared == attribute_value_type(value) {
            if let Some(kind) = empty_key_value_kind(value) {
                return Err(empty_key_attribute(attribute, kind));
            }
        }
        let encoded = match (declared, value) {
            ("S", AttributeValue::S(text)) => AttributeValueKey::S(text.clone()),
            ("B", AttributeValue::B(bytes)) => AttributeValueKey::B(bytes.clone()),
            ("N", AttributeValue::N(text)) => AttributeValueKey::N(
                // The numeric rules (38 significant digits, the documented exponent
                // range, and this emulator's own numeric-text limit) are the same ones
                // every `N` value in a request is held to; the refusal names the one
                // this text broke, inside the validation envelope this service states a
                // clause about a value a request carried in. A READ states it bare, as
                // it does every other refusal about a value it was handed
                // ([`unenveloped`] is where each reader does that).
                NumberKey::try_parse(text).map_err(|fault| number_rejection(text, fault))?,
            ),
            (_, actual) => {
                return Err(RequestError::Validation(format!(
                    "{INVALID_PARAMETER_CLAUSE}: Type mismatch for key {attribute} expected: \
                     {declared} actual: {}",
                    attribute_value_type(actual)
                )))
            }
        };
        self.check_key_limits(attribute, value)?;
        Ok(encoded)
    }

    /// Encode one operand a key CONDITION compares a key attribute with.
    ///
    /// The same encoding as [`Self::key_value`], with the refusal a key condition earns:
    /// an operand is a "condition parameter" rather than a key the request addresses an
    /// item by, so AWS states the type mismatch as the rule the parameter broke, naming
    /// neither the attribute nor the two types.
    pub fn key_condition_value(
        &self,
        attribute: &str,
        value: &AttributeValue,
    ) -> Result<AttributeValueKey, RequestError> {
        if self
            .declared_type(attribute)
            .is_some_and(|declared| declared != attribute_value_type(value))
        {
            return Err(RequestError::Validation(
                CONDITION_PARAMETER_TYPE_MESSAGE.to_string(),
            ));
        }
        self.key_value(attribute, value)
    }

    /// The policy slot a `*ResourcePolicy` call addressed on this table: the table's own
    /// policy, or its stream's.
    ///
    /// A stream is addressed by the ARN of the stream the table publishes to, so an ARN
    /// naming any OTHER stream label — a stream this table never had, or one it has
    /// replaced — names a resource that is not there, which is the not-found the Streams
    /// surface words over the ARN the request carried.
    pub fn policy_slot(
        &self,
        resource: &PolicyResource,
    ) -> Result<&ResourcePolicySlot, RequestError> {
        match resource.target {
            PolicyTarget::Table => Ok(&self.resource_policy),
            PolicyTarget::Stream => {
                self.check_addresses_stream(resource)?;
                Ok(&self.stream_resource_policy)
            }
        }
    }

    /// That same slot, for a call that WRITES it.
    pub fn policy_slot_mut(
        &mut self,
        resource: &PolicyResource,
    ) -> Result<&mut ResourcePolicySlot, RequestError> {
        match resource.target {
            PolicyTarget::Table => Ok(&mut self.resource_policy),
            PolicyTarget::Stream => {
                self.check_addresses_stream(resource)?;
                Ok(&mut self.stream_resource_policy)
            }
        }
    }

    /// Whether the ARN of that call names the stream THIS table publishes to.
    fn check_addresses_stream(&self, resource: &PolicyResource) -> Result<(), RequestError> {
        let names_it = self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.arn == resource.arn);
        if names_it {
            return Ok(());
        }
        Err(RequestError::NotFound(format!(
            "Requested resource not found: Stream: {} not found",
            resource.arn
        )))
    }

    /// The role `attribute` plays in a key schema of this table — its own, or one of
    /// its global secondary indexes — if it plays one at all.
    ///
    /// The table's own schema is consulted first: an attribute that is this table's
    /// sort key and some index's partition key is bounded as the sort key it is here.
    fn key_role(&self, attribute: &str) -> Option<KeyRole> {
        let role_in = |schema: &[KeySchemaElement]| {
            schema
                .iter()
                .find(|element| element.attribute_name == attribute)
                .map(|element| {
                    if element.key_type == "HASH" {
                        KeyRole::Partition
                    } else {
                        KeyRole::Sort
                    }
                })
        };
        role_in(&self.key_schema).or_else(|| {
            self.global_secondary_indexes
                .iter()
                .find_map(|index| role_in(&index.key_schema))
                .or_else(|| {
                    self.local_secondary_indexes
                        .iter()
                        .find_map(|index| role_in(&index.key_schema))
                })
        })
    }

    /// Refuse a key attribute value past the documented key-value size limit.
    ///
    /// The limit is the one for the role the attribute plays in a key schema
    /// ([`MAX_PARTITION_KEY_BYTES`] / [`MAX_SORT_KEY_BYTES`]). An attribute that is no
    /// key at all is the item's own business and is not bounded here; a key's empty
    /// `S`/`B` value is refused by [`Self::key_value`], which is the other half of the
    /// documented key-value contract.
    pub fn check_key_limits(
        &self,
        attribute: &str,
        value: &AttributeValue,
    ) -> Result<(), RequestError> {
        let Some(role) = self.key_role(attribute) else {
            return Ok(());
        };
        let size = attribute_value_size(value);
        if size > role.max_bytes() {
            return Err(RequestError::Validation(format!(
                "One or more parameter values were invalid: {}",
                role.oversize_message()
            )));
        }
        Ok(())
    }

    /// The primary key of an item being written: every key attribute the schema
    /// declares must be present, with its declared type. Non-key attributes are the
    /// item's own business.
    ///
    /// A missing key attribute is refused the way the service words it for an ITEM —
    /// "One or more parameter values were invalid: Missing the key <name> in the item",
    /// which NAMES the attribute ([`missing_item_key_message`]) — and that is not how it
    /// words the same gap in a `Key`-shaped member (see [`Self::key_of_key_map`]).
    pub fn key_of_item(&self, item: &Item) -> Result<ItemKey, RequestError> {
        self.key_from(item, KeySource::Item).map_err(Into::into)
    }

    /// The primary key of an item ONE ELEMENT of a BATCHED write carries (a
    /// `BatchWriteItem` `PutRequest`).
    ///
    /// That surface holds the element's item to the table's KEY SCHEMA as a whole, so
    /// every way it differs from that schema — a key attribute the item omits, or one
    /// carrying another type than the table declares for it — is the one mismatch
    /// sentence ([`KEY_SCHEMA_MISMATCH_MESSAGE`]), with no attribute name and no
    /// expected/actual type, whichever element and whichever of the two defects it was.
    /// The single-item write keeps its own per-defect wording
    /// ([`Self::key_of_item`]), a TRANSACTIONAL write's element states the type mismatch
    /// ([`Self::key_of_transact_item`]), and a `Key`-shaped member of either surface is
    /// read through [`Self::key_of_key_map`], which states the same mismatch sentence.
    pub fn key_of_multi_item(&self, item: &Item) -> Result<ItemKey, RequestError> {
        self.key_from(item, KeySource::BatchItem).map_err(Into::into)
    }

    /// The primary key named by a `Key` member: exactly the key attributes, no more.
    ///
    /// Unlike an `Item`, a `Key` IS the primary key, so every way it can differ
    /// from the schema — an attribute the schema does not declare, a key
    /// attribute it omits, a key attribute of another type — is the one mismatch
    /// AWS states with [`KEY_SCHEMA_MISMATCH_MESSAGE`].
    pub fn key_of_key_map(&self, key: &Item) -> Result<ItemKey, RequestError> {
        self.key_of_map(key, KeySource::Key).map_err(Into::into)
    }

    /// The primary key a paging member (`ExclusiveStartKey`) resumes from: read
    /// through the same key schema as a `Key`, and refused as the INVALID RESUME
    /// TOKEN it is when it is no key of that schema at all
    /// ([`START_KEY_INVALID_MESSAGE`]) — with the schema reason stated after it when the
    /// token's attributes ARE that schema's key attributes but one of them is stated
    /// wrongly, left out or carrying another TYPE than the schema declares
    /// ([`START_KEY_SCHEMA_MISMATCH_MESSAGE`]).
    pub fn key_of_start_key(&self, key: &Item) -> Result<ItemKey, RequestError> {
        self.key_of_map(key, KeySource::StartKey).map_err(Into::into)
    }

    /// The primary key of the ITEM a transactional `Put` carries, with WHICH side
    /// decided a refusal of it ([`KeyRefusal`]).
    ///
    /// The same walk as [`Self::key_of_multi_item`]; two things differ. A key attribute
    /// carrying another type than the table declares for it is stated as the mismatch
    /// between the two types, naming the attribute, the declared type and the supplied
    /// one — the sentence the transaction's cancellation reason for that element carries,
    /// as a single-item write's refusal states it ([`Self::key_of_item`]) — while a key
    /// attribute the item OMITS is the one mismatch sentence, as a batched element's is.
    /// And the caller also learns whether the key was refused against the table's key
    /// schema or against the request's own shape, which is what decides whether the
    /// transaction cancels that element or refuses the call.
    pub fn key_of_transact_item(&self, item: &Item) -> Result<ItemKey, KeyRefusal> {
        self.key_from(item, KeySource::TransactItem)
    }

    /// The primary key named by the `Key` member of a transactional `Delete`,
    /// `ConditionCheck` or `Update`, with WHICH side decided a refusal of it
    /// ([`KeyRefusal`]) — the classified half of [`Self::key_of_key_map`].
    pub fn key_of_transact_key_map(&self, key: &Item) -> Result<ItemKey, KeyRefusal> {
        self.key_of_map(key, KeySource::Key)
    }

    /// The one walk a `Key`-shaped map is read through, whichever member carried it:
    /// exactly the key attributes the schema declares, with their declared types.
    fn key_of_map(&self, key: &Item, source: KeySource) -> Result<ItemKey, KeyRefusal> {
        // A map carrying NOTHING names no key element of this schema to be held to it, so
        // it is not shaped like a key at all rather than a key stated wrongly.
        if key.is_empty() {
            return Err(KeyRefusal::Request(source.mismatch()));
        }
        for name in key.keys() {
            if name != self.partition_key_name() && Some(name.as_str()) != self.sort_key_name() {
                // A map carrying an attribute that is no key of this schema is not
                // shaped like a key, which is the request's own defect.
                return Err(KeyRefusal::Request(source.mismatch()));
            }
        }
        self.key_from(key, source)
    }

    /// The one walk over the key schema both `Key` and `Item` are read through:
    /// each declared key attribute is taken from `supplied` and encoded, and
    /// `source` decides only how a missing or wrongly-typed element is WORDED.
    fn key_from(&self, supplied: &Item, source: KeySource) -> Result<ItemKey, KeyRefusal> {
        let mut partition = None;
        let mut sort = None;
        for element in &self.key_schema {
            let name = element.attribute_name.as_str();
            let value = supplied
                .get(name)
                .ok_or_else(|| KeyRefusal::Request(source.absent(name)))?;
            // Read out of a member that names the key as a WHOLE, an element of the
            // wrong type does not name this key either, so it is the same mismatch
            // rather than a report of the declared and the supplied type.
            if source.hides_the_type_mismatch()
                && self
                    .declared_type(name)
                    .is_some_and(|declared| declared != attribute_value_type(value))
            {
                return Err(KeyRefusal::Schema(source.typed_against_schema()));
            }
            let encoded = self
                .key_value(name, value)
                .map_err(|error| self.classified_key_value_refusal(name, value, error))?;
            if element.key_type == "HASH" {
                partition = Some(encoded);
            } else {
                sort = Some(encoded);
            }
        }
        Ok(ItemKey {
            partition: partition
                .ok_or_else(|| KeyRefusal::Request(source.mismatch()))?,
            sort,
        })
    }

    /// WHICH side decided the refusal [`Self::key_value`] raised for one key attribute.
    ///
    /// The two classes the SCHEMA decides are read back through the very checks that
    /// raise them, so nothing about a type or a size is restated here: the declared type
    /// of the attribute, and the limit of the role the schema gives it
    /// ([`Self::check_key_limits`]). Everything else about a key value — a text that is
    /// no number, an empty `S`/`B` value, a key attribute the table declares no type for
    /// — is a fact of the value the request sent.
    fn classified_key_value_refusal(
        &self,
        attribute: &str,
        value: &AttributeValue,
        error: RequestError,
    ) -> KeyRefusal {
        let typed_against_schema = self
            .declared_type(attribute)
            .is_some_and(|declared| declared != attribute_value_type(value));
        if typed_against_schema || self.check_key_limits(attribute, value).is_err() {
            return KeyRefusal::Schema(error);
        }
        KeyRefusal::Request(error)
    }

    /// Hold every SECONDARY-INDEX key attribute the item carries to the type the
    /// table declared for it, to the non-emptiness a key value must have, and to the
    /// same key-value limits — the three rules below, in that order — and then every
    /// VECTOR-INDEX vector attribute it carries to the vector that index holds
    /// ([`Self::check_vector_attributes`]).
    ///
    /// Every write path that stores an item goes through here. The transactional
    /// write, whose answer depends on WHICH of the rules was broken, calls
    /// [`Self::check_index_key_types`], [`Self::check_index_key_values`] and
    /// [`Self::check_vector_attributes`] separately instead.
    pub fn check_index_keys(&self, item: &Item) -> Result<(), RequestError> {
        self.check_index_key_types(item)?;
        self.check_index_key_values(item)?;
        self.check_vector_attributes(item)
    }

    /// The same three rules for the item an UPDATE leaves behind, whose empty
    /// index-key value is worded as the update's own defect
    /// ([`Self::check_index_key_values_of_update`]).
    pub fn check_index_keys_of_update(&self, item: &Item) -> Result<(), RequestError> {
        self.check_index_key_types(item)?;
        self.check_index_key_values_of_update(item)?;
        self.check_vector_attributes(item)
    }

    /// Hold the item's VECTOR attributes to the shape every vector index over them was
    /// built for: "The number of dimensions in each vector" (`VectorIndex$Dimensions`),
    /// and "If multiple vector indexes reference the same attribute, they must all use
    /// the same number of dimensions" (`CreateVectorIndexAction$VectorAttribute`) — so an
    /// attribute an index indexes carries a list of exactly that many numbers.
    ///
    /// An item carrying something else under that name could not be indexed at all, so
    /// the write is refused rather than stored outside the index it was written for. An
    /// item that carries NO value for the attribute is simply not covered by the index
    /// ([`VectorIndex::covers`]) and is written as it stands.
    ///
    /// Each refusal is DynamoDB's parameter sentence for what the value broke, reporting
    /// the attribute as the PARAMETER at fault beside the index that requires it: a value
    /// of another type is told the type the parameter has to be, an element of another
    /// type is named by its position in the list and reports the type it arrived as, and a
    /// list of the wrong length reports the count the index was built for beside the count
    /// supplied — which is the only place a caller is told either figure.
    ///
    /// Every write path that stores an item reaches this, through
    /// [`Self::check_index_keys`] or beside it, so one rule decides what a table with a
    /// vector index accepts whichever operation the write arrived on.
    pub fn check_vector_attributes(&self, item: &Item) -> Result<(), RequestError> {
        for index in &self.vector_indexes {
            let name = index.vector_attribute.attribute_name.as_str();
            let Some(value) = item.get(name) else {
                continue;
            };
            // The TYPE of the attribute and of each of its elements before the COUNT of
            // them: a value that is no list of 32-bit floats has no dimensions to compare
            // with the index's, so the caller is told what the parameter has to be before
            // being told how many of them to send.
            let AttributeValue::L(elements) = value else {
                return Err(RequestError::Validation(format!(
                    "{INVALID_PARAMETER_CLAUSE}. Invalid type for parameter {name}, Expected: \
                     {VECTOR_PARAMETER_TYPE} list IndexName: {}",
                    index.name
                )));
            };
            for (position, element) in elements.iter().enumerate() {
                if vector_component(element).is_none() {
                    // WHICH element is at fault is reported here and nowhere else, so the
                    // position is part of the parameter named, beside the
                    // `AttributeValue` type the element arrived as.
                    return Err(RequestError::Validation(format!(
                        "{INVALID_PARAMETER_CLAUSE}. Invalid type for parameter \
                         {name}[{position}], Expected: {VECTOR_PARAMETER_TYPE}, Actual: {}. \
                         IndexName: {}",
                        attribute_value_type(element),
                        index.name
                    )));
                }
            }
            if i64::try_from(elements.len()).unwrap_or(i64::MAX) != index.dimensions {
                return Err(RequestError::Validation(format!(
                    "{INVALID_PARAMETER_CLAUSE}. Invalid size for parameter {name}, Expected: {}, \
                     Actual: {} IndexName: {}",
                    index.dimensions,
                    elements.len(),
                    index.name
                )));
            }
            // The search schema partitions and filters by the VALUE of each attribute it
            // names, so its type is the one the table declared for that attribute:
            // "Partition key and inline filter attributes defined in the search schema must
            // have data types matching the index schema definition" (the `PutItem$Item`
            // doc's own list of what a vector index requires of a write).
            for element in &index.search_schema {
                let attribute = element.attribute_name.as_str();
                let Some(supplied) = item.get(attribute) else {
                    continue;
                };
                if let Some(declared) = self.declared_type(attribute) {
                    let actual = attribute_value_type(supplied);
                    if declared != actual {
                        return Err(index_key_type_mismatch(
                            attribute, declared, actual, &index.name,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The TYPE rule for the item's secondary-index key attributes: "If you specify
    /// any attributes that are part of an index key, then the data types for those
    /// attributes must match those of the schema in the table's attribute definition"
    /// (the `PutItem$Item` doc), so a value whose `AttributeValue` member is not the
    /// `AttributeType` this table's `AttributeDefinitions` declare for that attribute
    /// is refused ([`index_key_type_mismatch`]) and the item is never written.
    ///
    /// This is decided against the TABLE's schema rather than against the request
    /// alone, which is why the transactional write reports it as the refusal of the
    /// element that carries the item.
    pub fn check_index_key_types(&self, item: &Item) -> Result<(), RequestError> {
        for (index_name, attribute, value) in self.index_key_values(item) {
            let actual = attribute_value_type(value);
            if let Some(declared) = self.declared_type(attribute) {
                if declared != actual {
                    return Err(index_key_type_mismatch(
                        attribute, declared, actual, index_name,
                    ));
                }
            }
        }
        Ok(())
    }

    /// The VALUE rules for the item's secondary-index key attributes:
    ///
    /// * the EMPTINESS: "Attribute values of type String and Binary must have a
    ///   length greater than zero if the attribute is used as a key attribute for a
    ///   table or index" (the `PutItem$Item` doc), so an empty `S`/`B` index key is
    ///   refused ([`empty_index_key_attribute`]);
    /// * the SIZE: the documented key-value limits are stated for a key attribute
    ///   "for a table or index", and an item that carries an index's key attributes
    ///   is projected into that index, so an oversize index key is refused on the
    ///   same write the table's own key is checked on.
    ///
    /// Both speak about the value the request carried rather than about the table's
    /// schema, so the transactional write answers them as the request-level refusals
    /// they are.
    pub fn check_index_key_values(&self, item: &Item) -> Result<(), RequestError> {
        self.index_key_values_held_to(item, IndexKeyWrite::Item)
    }

    /// The same rules for the item an UPDATE leaves behind.
    ///
    /// An update states what to CHANGE rather than the whole item, so an empty value it
    /// leaves in a secondary index key is refused as the defect of the UPDATE it is
    /// ([`empty_updated_index_key`]) rather than as a value the request carried in an
    /// item. Everything else about the two is the same rule and the same walk: the
    /// emptiness and the documented key-value size limits.
    pub fn check_index_key_values_of_update(&self, item: &Item) -> Result<(), RequestError> {
        self.index_key_values_held_to(item, IndexKeyWrite::Update)
    }

    /// The one walk both of the above are, over every secondary-index key attribute the
    /// item carries, with `wrote` deciding only how an empty value is WORDED.
    fn index_key_values_held_to(
        &self,
        item: &Item,
        wrote: IndexKeyWrite,
    ) -> Result<(), RequestError> {
        for (index_name, attribute, value) in self.index_key_values(item) {
            if let Some(kind) = empty_key_value_kind(value) {
                return Err(match wrote {
                    IndexKeyWrite::Item => empty_index_key_attribute(attribute, kind, index_name),
                    IndexKeyWrite::Update => empty_updated_index_key(kind),
                });
            }
            self.check_key_limits(attribute, value)?;
        }
        Ok(())
    }

    /// Every secondary-index key attribute `item` carries, as `(index name,
    /// attribute, value)`, over the global indexes first and then the local ones.
    ///
    /// An attribute the item does not carry is simply not projected into that index
    /// and yields nothing. An attribute that is part of the TABLE's own key schema is
    /// left out too: it is refused as the table key it is by [`Self::key_of_item`] —
    /// the attribute plays both roles whenever a local secondary index is defined,
    /// and the refusal AWS states names the role the table gave it.
    fn index_key_values<'a>(
        &'a self,
        item: &'a Item,
    ) -> impl Iterator<Item = (&'a str, &'a str, &'a AttributeValue)> + 'a {
        self.global_secondary_indexes
            .iter()
            .map(|index| (index.name.as_str(), &index.key_schema))
            .chain(
                self.local_secondary_indexes
                    .iter()
                    .map(|index| (index.name.as_str(), &index.key_schema)),
            )
            .flat_map(move |(index_name, schema)| {
                schema.iter().filter_map(move |element| {
                    let attribute = element.attribute_name.as_str();
                    if self.is_table_key(attribute) {
                        return None;
                    }
                    item.get(attribute)
                        .map(|value| (index_name, attribute, value))
                })
            })
    }

    /// Whether `attribute` is one of this table's OWN key attributes.
    ///
    /// `pub`: a `Query`'s `FilterExpression` "does not allow key attributes", so the
    /// handler judges the attributes an expression reads against this one reading of
    /// the key schema rather than a second copy of it.
    pub fn is_table_key(&self, attribute: &str) -> bool {
        self.key_schema
            .iter()
            .any(|element| element.attribute_name == attribute)
    }

    /// The key attributes of a stored item, as the `Key`-shaped map
    /// `LastEvaluatedKey` reports (and a later `ExclusiveStartKey` resumes from).
    pub fn key_attributes(&self, item: &Item) -> Item {
        self.key_schema
            .iter()
            .filter_map(|element| {
                item.get_key_value(&element.attribute_name)
                    .map(|(name, value)| (name.clone(), value.clone()))
            })
            .collect()
    }

    /// Whether the resource an `IndexName` addresses declares a SORT key: this table's own
    /// key schema where the request names no index, and the named secondary index's where it
    /// does.
    ///
    /// The two are different questions — an index declares its own `HASH` and `RANGE`
    /// elements, whichever ones the table declares — so a setting that is the INDEX's is
    /// decided against the index's schema ([`crate::state::contributor_insights`], where the
    /// sort-key half of a feature only exists for a resource that has a sort key to report
    /// on). `None` for an index this table does not carry, which declares no key at all.
    pub fn resource_declares_sort_key(&self, index_name: Option<&str>) -> bool {
        match index_name {
            None => self.sort_key_name().is_some(),
            Some(name) => self.secondary_index(name).is_some_and(|index| {
                index
                    .key_schema
                    .iter()
                    .any(|element| element.key_type != "HASH")
            }),
        }
    }

    /// The secondary index an `IndexName` names, whichever kind it is, or `None` when
    /// this table declares no index by that name.
    ///
    /// "This index can be any local secondary index or global secondary index"
    /// (`Scan$IndexName`), so both lists answer; an index name is unique across the two
    /// of them, because a create refuses a name either list already holds.
    pub fn secondary_index(&self, name: &str) -> Option<SecondaryIndex<'_>> {
        let global = self
            .global_secondary_indexes
            .iter()
            .find(|index| index.name == name)
            .map(|index| SecondaryIndex {
                name: index.name.as_str(),
                key_schema: &index.key_schema,
                projection: &index.projection,
                global: true,
            });
        global.or_else(|| {
            self.local_secondary_indexes
                .iter()
                .find(|index| index.name == name)
                .map(|index| SecondaryIndex {
                    name: index.name.as_str(),
                    key_schema: &index.key_schema,
                    projection: &index.projection,
                    global: false,
                })
        })
    }

    /// One index key schema's elements in the order the index orders its entries by:
    /// every `HASH` element the schema declares, then every `RANGE` one, each group in
    /// the order the schema declared it — "HASH - partition key ... RANGE - sort key"
    /// (`KeySchemaElement$KeyType`), and an index may declare several of each
    /// ([`MAX_INDEX_PARTITION_KEYS`]).
    fn index_key_elements(
        key_schema: &[KeySchemaElement],
    ) -> impl Iterator<Item = &KeySchemaElement> {
        key_schema
            .iter()
            .filter(|element| element.key_type == "HASH")
            .chain(key_schema.iter().filter(|element| element.key_type != "HASH"))
    }

    /// Where `item`, stored under the primary key `key`, sits in the order a read of
    /// `index` walks it — or `None` when the index holds no entry for it.
    ///
    /// Every index key value was held to this table's declared type for it, to
    /// non-emptiness and to the documented key-value limits when the item was written
    /// ([`Self::check_index_keys`]), so encoding one cannot fail for a stored item; one
    /// this table could not encode is not in the index, exactly as an item that leaves an
    /// index key attribute out is not ([`index_covers`]).
    pub fn index_item_key(
        &self,
        index: &SecondaryIndex<'_>,
        key: &ItemKey,
        item: &Item,
    ) -> Option<IndexItemKey> {
        let mut values = Vec::with_capacity(index.key_schema.len());
        for element in Self::index_key_elements(index.key_schema) {
            let name = element.attribute_name.as_str();
            values.push(self.key_value(name, item.get(name)?).ok()?);
        }
        Some(IndexItemKey {
            index: values,
            table: key.clone(),
        })
    }

    /// The key attributes a read of `index` reports as `LastEvaluatedKey`, and resumes
    /// from: the index's own key attributes together with this table's, which is what
    /// addresses one entry of a secondary index — the index key alone does not, since
    /// nothing makes it unique.
    pub fn index_key_attributes(&self, index: &SecondaryIndex<'_>, item: &Item) -> Item {
        Self::index_key_elements(index.key_schema)
            .chain(self.key_schema.iter())
            .filter_map(|element| {
                item.get_key_value(&element.attribute_name)
                    .map(|(name, value)| (name.clone(), value.clone()))
            })
            .collect()
    }

    /// The place in `index`'s order an `ExclusiveStartKey` resumes a read from.
    ///
    /// A resume token is read back through the two schemas the read reported it from, so
    /// a map that names an attribute neither declares or leaves one of them out is the
    /// INVALID RESUME TOKEN it is ([`START_KEY_INVALID_MESSAGE`]), and one carrying a key
    /// attribute with a type other than the table declared states the schema reason after
    /// it ([`START_KEY_SCHEMA_MISMATCH_MESSAGE`]) — as a base-table token does
    /// ([`Self::key_of_start_key`]), which words a DROPPED table key attribute that way
    /// too. An index cursor carries both schemas' key attributes, so one it does not carry
    /// in full is not that cursor at all.
    pub fn index_key_of_start_key(
        &self,
        index: &SecondaryIndex<'_>,
        key: &Item,
    ) -> Result<IndexItemKey, RequestError> {
        let named: BTreeSet<&str> = Self::index_key_elements(index.key_schema)
            .chain(self.key_schema.iter())
            .map(|element| element.attribute_name.as_str())
            .collect();
        if key.len() != named.len() || key.keys().any(|name| !named.contains(name.as_str())) {
            return Err(KeySource::StartKey.mismatch());
        }
        let mut values = Vec::with_capacity(index.key_schema.len());
        for element in Self::index_key_elements(index.key_schema) {
            let name = element.attribute_name.as_str();
            let value = key.get(name).ok_or_else(|| KeySource::StartKey.mismatch())?;
            if self
                .declared_type(name)
                .is_some_and(|declared| declared != attribute_value_type(value))
            {
                return Err(KeySource::StartKey.typed_against_schema());
            }
            values.push(
                self.key_value(name, value)
                    .map_err(|_| KeySource::StartKey.mismatch())?,
            );
        }
        // The table's own half of the token is read through the one walk every
        // `ExclusiveStartKey` is read through, over just the attributes it declares.
        let table = self.key_of_start_key(&self.key_attributes(key))?;
        Ok(IndexItemKey {
            index: values,
            table,
        })
    }

    /// The number of items stored right now.
    fn live_item_count(&self) -> i64 {
        i64::try_from(self.items.len()).unwrap_or(i64::MAX)
    }

    /// The stored size of every item right now.
    fn live_table_size_bytes(&self) -> i64 {
        self.items.values().map(item_size).sum()
    }

    /// The items one index projects, and the bytes they take there, right now. One rule
    /// for both index kinds, from the key schema and projection each of them carries, in
    /// a single pass over the items.
    fn live_index_figures(
        &self,
        key_schema: &[KeySchemaElement],
        projection: &Projection,
    ) -> (i64, i64) {
        self.items
            .values()
            .filter(|item| index_covers(key_schema, item))
            .fold((0i64, 0i64), |(count, bytes), item| {
                (
                    count.saturating_add(1),
                    bytes.saturating_add(index_projected_size(
                        key_schema,
                        projection,
                        item,
                        &self.key_schema,
                    )),
                )
            })
    }

    /// Materialise the metering refresh the clock has already made due for this table
    /// and every index on it.
    ///
    /// This is the ONLY thing that moves what `ItemCount` and
    /// `TableSizeBytes`/`IndexSizeBytes` report: "DynamoDB updates this value
    /// approximately every six hours. Recent changes might not be reflected in this
    /// value" (`TableDescription$ItemCount`). A refresh owed at this instant was owed
    /// whether or not anybody called, so a reporting operation may finish it; a write is
    /// never allowed to.
    pub fn refresh_due_counters(&mut self, now: SystemTime) {
        if !self.reported_counters.refresh_due(now) {
            return;
        }
        self.meter_counters(now);
    }

    /// Remove every item of this table whose TTL deletion the clock has already made
    /// due, returning the items removed so their tables' streams can carry the change.
    ///
    /// "If the epoch time value stored in the attribute is less than the current time,
    /// the item is marked as expired and subsequently deleted" (the `UpdateTimeToLive`
    /// doc): the deletion was owed at this instant whether or not anybody called, so a
    /// reporting operation may finish it. Nothing here is a filter — an item that is
    /// expired but whose deletion is not due yet is still STORED and still served,
    /// because "items that have expired and not been deleted will still show up in
    /// reads, queries, and scans" (the same doc).
    ///
    /// A table whose TTL is off, or whose last TTL change is still settling, has nothing
    /// to sweep, and a table that was swept within the last [`TTL_DELETION_DELAY`] is
    /// left alone, so the walk over this table's items runs at most once per span.
    pub fn apply_due_expirations(&mut self, now: SystemTime) -> Vec<Item> {
        let Some(attribute) = self.time_to_live.due_sweep_attribute(now) else {
            return Vec::new();
        };
        let Some(cutoff) = expiry_cutoff(now) else {
            return Vec::new();
        };
        self.time_to_live.mark_swept(now);
        let due: Vec<ItemKey> = self
            .items
            .iter()
            .filter(|(_, item)| {
                item.get(&attribute)
                    .is_some_and(|value| expiry_due(value, &cutoff))
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut removed = Vec::with_capacity(due.len());
        for key in due {
            if let Some(item) = self.items.remove(&key) {
                self.stored_bytes = self.stored_bytes.saturating_sub(item_size(&item));
                // A TTL deletion is an item-level change like any other, so the version it
                // removed is what a restore to a point before it hands back
                // ([`Self::record_item_version`]).
                self.record_item_version(now, &key, Some(&item));
                removed.push(item);
            }
        }
        removed
    }

    /// Keep the version of the item at `key` that a change landing at `now` replaced, for
    /// the table's continuous backups ([`PointInTimeRecovery::record_item_version`]).
    ///
    /// Called from every site that changes what a key holds — a write, a delete, a TTL
    /// expiration, a replicated change — so one table's history is the history of its
    /// items whichever call moved them. A table whose recovery is off keeps nothing, and a
    /// call that found the key empty and left it empty changed nothing, so neither copies
    /// the item: the version is borrowed here and cloned only where it is kept.
    pub fn record_item_version(&mut self, now: SystemTime, key: &ItemKey, before: Option<&Item>) {
        if !self.point_in_time_recovery.enabled()
            || (before.is_none() && !self.items.contains_key(key))
        {
            return;
        }
        self.point_in_time_recovery
            .record_item_version(now, key, before);
    }

    /// Meter this table's figures and every index's, at `now`, whatever the schedule.
    ///
    /// The walk [`Self::refresh_due_counters`] performs when the clock has made a refresh
    /// due, reached directly by the one caller whose table ALREADY HOLDS data when it is
    /// created — a table a restore wrote the backup's items into, whose figures at
    /// creation are those of that data rather than zeroes. Nothing else may call it: a
    /// write never moves what `ItemCount`/`TableSizeBytes` report.
    pub fn meter_counters(&mut self, now: SystemTime) {
        let item_count = self.live_item_count();
        let size_bytes = self.live_table_size_bytes();
        self.reported_counters.refresh(now, item_count, size_bytes);
        // Each index's figures are metered from the table's items on the same schedule,
        // computed first (they read `self`) and then recorded.
        let global: Vec<(i64, i64)> = self
            .global_secondary_indexes
            .iter()
            .map(|index| self.live_index_figures(&index.key_schema, &index.projection))
            .collect();
        let local: Vec<(i64, i64)> = self
            .local_secondary_indexes
            .iter()
            .map(|index| self.live_index_figures(&index.key_schema, &index.projection))
            .collect();
        // A vector index is metered from the same items, by its own coverage and
        // projection rules ([`VectorIndex::covers`]).
        let vector: Vec<(i64, i64)> = self
            .vector_indexes
            .iter()
            .map(|index| {
                self.items
                    .values()
                    .filter(|item| index.covers(item))
                    .fold((0i64, 0i64), |(count, bytes), item| {
                        (
                            count.saturating_add(1),
                            bytes.saturating_add(index.projected_size(item, &self.key_schema)),
                        )
                    })
            })
            .collect();
        for (index, (item_count, size_bytes)) in
            self.global_secondary_indexes.iter_mut().zip(global)
        {
            index.reported_counters.refresh(now, item_count, size_bytes);
        }
        for (index, (item_count, size_bytes)) in self.local_secondary_indexes.iter_mut().zip(local)
        {
            index.reported_counters.refresh(now, item_count, size_bytes);
        }
        for (index, (item_count, size_bytes)) in self.vector_indexes.iter_mut().zip(vector) {
            index.reported_counters.refresh(now, item_count, size_bytes);
        }
    }

    /// The index by that name, as `UpdateTable` and the index describes need it.
    pub fn index(&self, name: &str) -> Option<&GlobalSecondaryIndex> {
        self.global_secondary_indexes
            .iter()
            .find(|index| index.name == name)
    }

    /// The same, mutably, for the operations that move an index's throughput.
    pub fn index_mut(&mut self, name: &str) -> Option<&mut GlobalSecondaryIndex> {
        self.global_secondary_indexes
            .iter_mut()
            .find(|index| index.name == name)
    }

    /// The contributor insights setting the request addressed: this table's own, or the
    /// one of the global secondary index it named.
    ///
    /// An `IndexName` the table does not carry names no resource whose setting could be
    /// read, so it is the same miss as a table that is not there ("The operation tried
    /// to access a nonexistent table or index" — the `ResourceNotFoundException` doc).
    pub fn contributor_insights(
        &self,
        index_name: Option<&str>,
    ) -> Result<&ContributorInsights, RequestError> {
        match index_name {
            None => Ok(&self.contributor_insights),
            Some(name) => self
                .index(name)
                .map(|index| &index.contributor_insights)
                .ok_or_else(|| index_not_found(&self.name, name)),
        }
    }

    /// The same, mutably, for the operation that writes the setting.
    pub fn contributor_insights_mut(
        &mut self,
        index_name: Option<&str>,
    ) -> Result<&mut ContributorInsights, RequestError> {
        match index_name {
            None => Ok(&mut self.contributor_insights),
            Some(name) => {
                // The miss is reported over the table's stored name, so the borrow of
                // the index below cannot overlap reading it.
                let table = self.name.clone();
                self.index_mut(name)
                    .map(|index| &mut index.contributor_insights)
                    .ok_or_else(|| index_not_found(&table, name))
            }
        }
    }

    /// The destination the table's streaming state is about: its LIVE replication, to
    /// whichever stream it replicates to, or `None` while it replicates to none.
    ///
    /// A table replicates to AT MOST ONE Kinesis data stream, so this is what an enable's
    /// admission is decided on ([`kinesis_destination_not_enableable`]): the records left
    /// over from replication that failed or was stopped are the table's history, and an
    /// enable may start again while only those are held.
    pub fn live_kinesis_destination(&self, now: SystemTime) -> Option<&KinesisDestination> {
        self.kinesis_destinations
            .iter()
            .find(|destination| destination.replicating(now))
    }

    /// Every destination of this table an item change made at `now` is DELIVERED to
    /// ([`KinesisDestination::delivers`]) — the data plane's half of a streaming
    /// destination.
    pub fn delivering_kinesis_destinations(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &KinesisDestination> {
        self.kinesis_destinations
            .iter()
            .filter(move |destination| destination.delivers(now))
    }

    /// Whether a change made at `now` is delivered to any Kinesis data stream — what a
    /// write asks before it keeps the images a delivered record carries.
    pub fn delivers_to_kinesis(&self, now: SystemTime) -> bool {
        self.delivering_kinesis_destinations(now).next().is_some()
    }

    /// The destination the command addressed, mutably: this table's LIVE replication to
    /// the stream that `stream_arn` names, or `None` when the table replicates to no
    /// such stream — a stream it has never replicated to, and one it has stopped
    /// replicating to, alike ([`KinesisDestination::replicating`]).
    pub fn replicating_kinesis_destination_mut(
        &mut self,
        stream_arn: &str,
        now: SystemTime,
    ) -> Option<&mut KinesisDestination> {
        self.kinesis_destinations
            .iter_mut()
            .find(|destination| destination.stream_arn == stream_arn && destination.replicating(now))
    }

    /// The destination record this table holds for the stream that `stream_arn` names,
    /// mutably, WHATEVER status it is in — `None` only when the table has no record of
    /// that stream at all.
    ///
    /// This is the lookup a command that refuses a destination on its STATUS needs: a
    /// record it does not see cannot be told apart from one that was never registered.
    pub fn kinesis_destination_mut(&mut self, stream_arn: &str) -> Option<&mut KinesisDestination> {
        self.kinesis_destinations
            .iter_mut()
            .find(|destination| destination.stream_arn == stream_arn)
    }

    /// The destination the command addressed, mutably, only while it is `ACTIVE`
    /// ([`KinesisDestination::active`]) — the status a command that changes a
    /// destination requires of it.
    pub fn active_kinesis_destination_mut(
        &mut self,
        stream_arn: &str,
        now: SystemTime,
    ) -> Option<&mut KinesisDestination> {
        self.kinesis_destination_mut(stream_arn)
            .filter(|destination| destination.active(now))
    }

    /// Whether this table already holds as many destination records as it keeps and
    /// `stream_arn` is not one of them — the registration that would grow the list past
    /// [`MAX_KINESIS_DESTINATIONS_PER_TABLE`].
    pub fn kinesis_destination_slots_exhausted(&self, stream_arn: &str) -> bool {
        self.kinesis_destinations.len() >= MAX_KINESIS_DESTINATIONS_PER_TABLE
            && !self
                .kinesis_destinations
                .iter()
                .any(|destination| destination.stream_arn == stream_arn)
    }

    /// Register replication to `stream_arn`, as the command that starts it leaves the
    /// record: `ENABLING` for the span, and after it `ACTIVE` when the stream it names
    /// resolves and `ENABLE_FAILED` when it does not, under the precision the command named
    /// and under none where it named none.
    ///
    /// Whether the stream resolves is the KINESIS service's to answer
    /// ([`resolve_kinesis_stream`]), so the caller reads it there and passes the failure in
    /// — this record only remembers which of the two ends the enable is headed for, and why
    /// it could not be completed where it failed.
    ///
    /// A stream this table once replicated to is re-registered IN PLACE, so one stream
    /// has one record whatever number of times replication to it is started and stopped,
    /// and it keeps its place in the order the describing operation reports.
    pub fn register_kinesis_destination(
        &mut self,
        stream_arn: &str,
        precision: Option<&str>,
        failure: Option<&KinesisEnableFailure>,
        now: SystemTime,
    ) -> &KinesisDestination {
        let registered = match failure {
            None => KinesisDestination::enabling(stream_arn, precision, now),
            Some(failure) => {
                KinesisDestination::enable_failing(stream_arn, precision, failure, now)
            }
        };
        let index = match self
            .kinesis_destinations
            .iter()
            .position(|destination| destination.stream_arn == stream_arn)
        {
            Some(index) => {
                self.kinesis_destinations[index] = registered;
                index
            }
            None => {
                self.kinesis_destinations.push(registered);
                self.kinesis_destinations.len() - 1
            }
        };
        &self.kinesis_destinations[index]
    }

    /// Every Kinesis data stream destination this table carries, in registration order,
    /// as the describing operation reports them — the stopped ones included, which is
    /// what makes that report the table's replication history.
    pub fn describe_kinesis_destinations(
        &self,
        now: SystemTime,
    ) -> Vec<KinesisDataStreamDestination> {
        self.kinesis_destinations
            .iter()
            .map(|destination| destination.describe(now))
            .collect()
    }

    /// Every contributor insights summary this table reports: its own first, then one
    /// per global secondary index, in the order the indexes are stored —
    /// "ContributorInsightsSummary for a table and all its global secondary indexes"
    /// (the `ListContributorInsights` doc).
    ///
    /// Only a resource whose setting the listing reports ([`ContributorInsights::listed`])
    /// is in it, so a table whose insights are off has no summary and drops out once a
    /// `DISABLE` settles.
    ///
    /// A local secondary index has none: the feature is addressed through `IndexName`
    /// against the global indexes an `UpdateContributorInsights` can name.
    pub fn contributor_insights_summaries(&self, now: SystemTime) -> Vec<ContributorInsightsSummary> {
        ::std::iter::once(ContributorInsightsSummary {
            table_name: Some(self.name.clone()),
            index_name: None,
            contributor_insights_status: Some(self.contributor_insights.status(now).to_string()),
            contributor_insights_mode: self.contributor_insights.mode(),
        })
        .filter(|_| self.contributor_insights.listed(now))
        .chain(
            self.global_secondary_indexes
                .iter()
                .filter(|index| index.contributor_insights.listed(now))
                .map(|index| ContributorInsightsSummary {
                    table_name: Some(self.name.clone()),
                    index_name: Some(index.name.clone()),
                    contributor_insights_status: Some(
                        index.contributor_insights.status(now).to_string(),
                    ),
                    contributor_insights_mode: index.contributor_insights.mode(),
                }),
        )
        .collect()
    }

    /// Project one global secondary index into the description `DescribeTable` and the
    /// table-mutating operations report. `ItemCount` and `IndexSizeBytes` are the
    /// metered figures of the last refresh, never the live projection.
    fn describe_index(
        &self,
        index: &GlobalSecondaryIndex,
        now: SystemTime,
    ) -> GlobalSecondaryIndexDescription {
        let (number_of_decreases_today, last_increase_date_time, last_decrease_date_time) =
            index.throughput_history.describe(now);
        let capacity = reported_capacity(index.provisioned_throughput.as_ref());
        // An index keeps warm the capacity it is provisioned for — or, once that capacity
        // has been withdrawn, the pair it was keeping warm — by the same rule the table
        // follows; no request can name one per index (`CreateTable`/`UpdateTable` refuse
        // that member with the scaffold's 501), so none is ever in flight and the status
        // is `ACTIVE` — "The status can only be UPDATING or ACTIVE"
        // (`GlobalSecondaryIndexWarmThroughputDescription$Status`).
        let (warm_read_units, warm_write_units) = index.warm_throughput.in_force();
        GlobalSecondaryIndexDescription {
            index_name: Some(index.name.clone()),
            index_arn: Some(index.arn.clone()),
            key_schema: Some(index.key_schema.clone()),
            projection: Some(index.projection.clone()),
            index_status: Some(index.status(now).as_str().to_string()),
            // "For indexes that were created during a CreateTable operation, the
            // Backfilling attribute does not appear in the DescribeTable output" (the
            // `Backfilling` doc), and this emulator projects an index's items from the
            // table on every read, so no index is ever backfilling.
            backfilling: None,
            provisioned_throughput: Some(ProvisionedThroughputDescription {
                read_capacity_units: Some(capacity.read_capacity_units),
                write_capacity_units: Some(capacity.write_capacity_units),
                number_of_decreases_today,
                last_increase_date_time,
                last_decrease_date_time,
            }),
            item_count: Some(index.reported_counters.item_count),
            index_size_bytes: Some(index.reported_counters.size_bytes),
            warm_throughput: Some(GlobalSecondaryIndexWarmThroughputDescription {
                read_units_per_second: Some(warm_read_units),
                write_units_per_second: Some(warm_write_units),
                status: Some(IndexStatus::Active.as_str().to_string()),
            }),
            ..GlobalSecondaryIndexDescription::default()
        }
    }

    /// Project one local secondary index into the description `DescribeTable` and the
    /// table-mutating operations report. `ItemCount` and `IndexSizeBytes` are the metered
    /// figures of the last refresh, by the same rule the global indexes follow.
    fn describe_local_index(&self, index: &LocalSecondaryIndex) -> LocalSecondaryIndexDescription {
        LocalSecondaryIndexDescription {
            index_name: Some(index.name.clone()),
            index_arn: Some(index.arn.clone()),
            key_schema: Some(index.key_schema.clone()),
            projection: Some(index.projection.clone()),
            item_count: Some(index.reported_counters.item_count),
            index_size_bytes: Some(index.reported_counters.size_bytes),
        }
    }

    /// The capacity ONE write consumes on this table and on each secondary index the
    /// write affects, from the item that stood at the key `before` it and the item that
    /// stands there `after`.
    ///
    /// The table's own units are one per 1 KB of the item, weighing BOTH versions and
    /// charging for the larger: "PutItem — Writes a single item to a table. If an item
    /// with the same primary key exists in the table, the operation replaces the item.
    /// For calculating provisioned throughput consumption, the item size that matters is
    /// the larger of the two" ("Capacity unit consumption for write operations" in the
    /// Developer Guide). A delete weighs the removed item alone, since nothing stands at
    /// the key afterwards, and a delete that found nothing weighs neither — it still
    /// costs the table the one-unit minimum and touches no index.
    ///
    /// Each index is charged by [`index_write_units`], so an index the write leaves
    /// untouched gets no entry at all — neither one the item does not reach, nor one
    /// whose stored entry this write does not change.
    ///
    /// The item is weighed as this table meters a write of it
    /// ([`Self::metered_item_size`]), which counts a vector at the four bytes per
    /// dimension it is stored as.
    pub fn write_capacity(&self, before: Option<&Item>, after: Option<&Item>) -> WriteCapacity {
        let weighed = |item: Option<&Item>| item.map_or(0, |item| self.metered_item_size(item));
        let mut capacity = WriteCapacity {
            table: write_capacity_units(weighed(before).max(weighed(after))),
            ..WriteCapacity::default()
        };
        for index in &self.local_secondary_indexes {
            if let Some(units) = index_write_units(
                &index.key_schema,
                &index.projection,
                &self.key_schema,
                before,
                after,
            ) {
                capacity
                    .local_secondary_indexes
                    .insert(index.name.clone(), units);
            }
        }
        for index in &self.global_secondary_indexes {
            if let Some(units) = index_write_units(
                &index.key_schema,
                &index.projection,
                &self.key_schema,
                before,
                after,
            ) {
                capacity
                    .global_secondary_indexes
                    .insert(index.name.clone(), units);
            }
        }
        // A VECTOR index is metered in bytes rather than in capacity units
        // ("ConsumedCapacity … also includes a VectorIndexes field with
        // VectorWriteRequestBytes consumed for each affected vector index" —
        // `PutItem$ConsumedCapacity`), so each index the write reached carries its own
        // figure beside the capacity breakdown above.
        for index in &self.vector_indexes {
            let reached = [before, after]
                .into_iter()
                .flatten()
                .find(|item| index.covers(item));
            if let Some(item) = reached {
                let key_bytes = item_size(&self.key_attributes(item));
                capacity.vector_indexes.insert(
                    index.name.clone(),
                    // Exact: the store admits far fewer bytes than an `f64` carries
                    // whole numbers for, so the figure reported is the figure counted.
                    index.write_request_bytes(key_bytes) as f64,
                );
            }
        }
        capacity
    }

    /// Admit a write that consumes `units` write capacity units, reporting whether the
    /// capacity this table is provisioned for covered it.
    ///
    /// "Provisioned throughput is the maximum amount of capacity that an application can
    /// consume from a table or index" per second (DynamoDB Developer Guide, "Read/write
    /// capacity mode"), so a PROVISIONED table admits its provisioned write units each
    /// second plus whatever of them went unused inside [`BURST_CAPACITY_WINDOW`], and a
    /// write past that is throttled rather than applied. A table on-demand
    /// (`PAY_PER_REQUEST`) is provisioned for no capacity to exceed, so every write of
    /// one is admitted.
    ///
    /// Only writes are admitted from a budget: a read in this emulator leaves nothing
    /// behind it, so what a later caller is told never depends on how much has been
    /// read.
    pub fn admit_write(&mut self, units: f64, now: SystemTime) -> bool {
        let Some(provisioned) = self.provisioned_write_rate() else {
            // On-demand: there is no capacity to exceed, and the write still consumed the
            // units the response reports, so they are counted either way.
            self.pending_consumed
                .charge(Consumed::Write, units, now);
            return true;
        };
        let admitted = self.write_budget.take(provisioned, units, now);
        if admitted {
            // What a request consumed is what the service publishes as the table's own
            // throughput metric: counted here, where the capacity is taken, so every write
            // this store admits is in the series whatever operation made it.
            self.pending_consumed
                .charge(Consumed::Write, units, now);
        }
        admitted
    }

    /// Admit a read that consumes `units` read capacity units, reporting the throttle
    /// when the capacity this table is provisioned for did not cover it.
    ///
    /// The read side of [`Self::admit_write`], and for the same documented reason:
    /// "provisioned throughput is the maximum amount of capacity that an application can
    /// consume from a table or index" per second, in reads as in writes, so a
    /// PROVISIONED table serves its provisioned read units each second plus whatever of
    /// them went unused inside [`BURST_CAPACITY_WINDOW`], and a read past that is
    /// refused with `ProvisionedThroughputExceededException` rather than served. A table
    /// on-demand (`PAY_PER_REQUEST`) is provisioned for no capacity to exceed, so every
    /// read of one is served.
    ///
    /// What the read SEES is untouched by this: the items, and everything a later caller
    /// reads back, are exactly what they were. What it spends is the capacity the service
    /// itself bills a read for — the figure the same call reports as
    /// `ConsumedCapacity.ReadCapacityUnits` — which is why a read is held to it at all.
    ///
    /// A read the budget cannot cover takes nothing from it, so a throttled read is not
    /// charged for work it was refused.
    pub fn admit_read(&mut self, units: f64, now: SystemTime) -> Result<(), ReadThrottle> {
        self.admit_read_from(None, units, now)
    }

    /// [`Self::admit_read`] for a read SERVED FROM a secondary index: `global_index` names the
    /// GLOBAL secondary index the units were billed to, and is `None` for a read of the table
    /// itself or of a LOCAL secondary index.
    ///
    /// The capacity comes out of the same place either way — an index of a table in this store
    /// has no budget of its own — but WHICH series the units are counted into follows the line
    /// of `ConsumedCapacity` that itemised them
    /// ([`crate::state::consumed_capacity_metrics`]): a global index's units are the index's,
    /// while a local index draws on the table's throughput and is counted with it.
    pub fn admit_read_from(
        &mut self,
        global_index: Option<&str>,
        units: f64,
        now: SystemTime,
    ) -> Result<(), ReadThrottle> {
        let Some(provisioned) = self.provisioned_read_rate() else {
            // On-demand: there is no capacity to exceed, and the read still consumed the
            // units the response reports, so they are counted either way.
            self.charge_read(global_index, units, now);
            return Ok(());
        };
        if self.read_budget.take(provisioned, units, now) {
            // See [`Self::admit_write`]: the units a served read consumed are the table's own
            // throughput metric, counted where the capacity is taken.
            self.charge_read(global_index, units, now);
            Ok(())
        } else {
            Err(ReadThrottle::TableProvisioned)
        }
    }

    /// Count the units a served read consumed into the series that read billed them to.
    fn charge_read(&mut self, global_index: Option<&str>, units: f64, now: SystemTime) {
        match global_index {
            Some(index) => self
                .pending_consumed
                .charge_global_index(Consumed::Read, index, units, now),
            None => self.pending_consumed.charge(Consumed::Read, units, now),
        }
    }

    /// Whether the read capacity this table is provisioned for would cover `units` at
    /// `now`, WITHOUT taking them.
    ///
    /// What lets a read that spans SEVERAL tables be admitted all-or-nothing — a
    /// transactional get is one read, and "the entire transaction is rejected" when any
    /// part of it cannot be served, so no table is debited unless every one of them has
    /// the capacity ([`Self::admit_read`] is what takes it once they all do).
    pub fn read_admits(&self, units: f64, now: SystemTime) -> Result<(), ReadThrottle> {
        let Some(provisioned) = self.provisioned_read_rate() else {
            return Ok(());
        };
        if self
            .read_budget
            .covers_within(provisioned, BURST_CAPACITY_WINDOW, units, now)
        {
            Ok(())
        } else {
            Err(ReadThrottle::TableProvisioned)
        }
    }

    /// The read units per second this table is PROVISIONED for, or `None` for a table
    /// on-demand (`PAY_PER_REQUEST`), which is provisioned for no capacity to exceed.
    fn provisioned_read_rate(&self) -> Option<i64> {
        match (
            self.billing_mode.as_str(),
            self.provisioned_throughput.as_ref(),
        ) {
            ("PROVISIONED", Some(throughput)) if throughput.read_capacity_units > 0 => {
                Some(throughput.read_capacity_units)
            }
            _ => None,
        }
    }

    /// Admit one ITEM write — a put, an update or a delete of the item whose partition
    /// key value is `partition` — against BOTH ceilings a write is held to, reporting
    /// which one refused it.
    ///
    /// Beneath the table's own provisioned capacity sits the storage layer's
    /// per-partition-key ceiling ([`PARTITION_WRITE_UNITS_PER_SECOND`]): every item that
    /// shares a partition key value lives in one partition, so a stream of writes aimed
    /// at one key is bounded by that partition whatever the table is provisioned for,
    /// while the same rate spread over many keys is not. The table ceiling is checked
    /// first, so a table that is out of capacity reports its own provisioning rather
    /// than blaming the key.
    ///
    /// Neither budget is debited unless BOTH cover the write: a throttled write does no
    /// work, so it consumes no capacity anywhere.
    pub fn admit_item_write(
        &mut self,
        partition: &AttributeValueKey,
        capacity: &WriteCapacity,
        now: SystemTime,
    ) -> Result<(), WriteThrottle> {
        let total = capacity.total();
        let provisioned = self.provisioned_write_rate();
        if let Some(provisioned) = provisioned {
            if !self
                .write_budget
                .covers_within(provisioned, BURST_CAPACITY_WINDOW, total, now)
            {
                return Err(WriteThrottle::TableProvisioned);
            }
        }
        // The partition holds the TABLE's items: an index entry this write fans out to
        // is a write to that index's own partitions, so the key's ceiling is measured
        // against the units the table itself took.
        if !self.key_write_budgets.covers(partition, capacity.table, now) {
            return Err(WriteThrottle::KeyRange);
        }
        if let Some(provisioned) = provisioned {
            self.write_budget.take(provisioned, total, now);
        }
        self.key_write_budgets
            .take(partition, capacity.table, now);
        // What a request consumed is what this service publishes as the table's throughput
        // metrics ([`crate::state::consumed_capacity_metrics`]): counted here, where the
        // capacity is taken, so every item write this store admits is in the series whichever
        // operation made it — the table's own units in the table's series and each global
        // secondary index's in that index's, exactly as this write itemises them.
        self.pending_consumed.charge_write(capacity, now);
        Ok(())
    }

    /// Whether the ceilings a write is admitted from could EVER cover this write — that
    /// is, whether waiting for capacity to refill could admit it at all.
    ///
    /// A budget never holds more than its whole burst window's worth of credit
    /// ([`burst_ceiling`]), so a write that asks for more than that is not waiting for
    /// capacity: no later attempt at it is admitted either, whatever the caller does.
    /// That is what tells a request worth handing back for a retry from one a retry
    /// could never get through.
    pub fn can_ever_admit_item_write(&self, capacity: &WriteCapacity) -> bool {
        if let Some(provisioned) = self.provisioned_write_rate() {
            if capacity.total() > burst_ceiling(provisioned, BURST_CAPACITY_WINDOW) {
                return false;
            }
        }
        capacity.table <= burst_ceiling(PARTITION_WRITE_UNITS_PER_SECOND, PARTITION_BURST_WINDOW)
    }

    /// Admit one item write that must make PROGRESS: the same two ceilings as
    /// [`Self::admit_item_write`], except that a write they are merely SHORT of capacity
    /// for may be served by BORROWING against their own refill, which leaves them in
    /// deficit by the shortfall.
    ///
    /// What this serves is the batch write surface, whose residue a caller is documented
    /// to resend ("you can provide this value directly to a subsequent BatchWriteItem
    /// operation"): the capacity a table has left decides HOW MANY of a batch's requests
    /// land, not WHETHER the call got anything done, so a write the budgets are merely
    /// short for can be served from borrowed capacity rather than handed back — which is
    /// what keeps that retry loop converging.
    ///
    /// WHICH ceiling may lend is `borrowing`, and the two differ because the two ceilings
    /// are different promises. The capacity a table is PROVISIONED for is the rate its
    /// caller asked for and is billed at, so only the request that makes the call make
    /// progress at all is served ahead of it, and only by one partition-second of it
    /// ([`progress_borrowing_allowance`]). The per-partition-key ceiling is the storage
    /// layer's own, and it lends every request of a call what ONE call can ask of a
    /// partition ([`batch_partition_borrowing_allowance`]), so a batch of writes aimed at
    /// one partition key lands whole and hands nothing back, as it does on the real
    /// service.
    ///
    /// The units borrowed are charged in full and repay at the rate in force, so the
    /// SUSTAINED rate either ceiling admits is exactly the rate it is provisioned for;
    /// and the borrowing is bounded ([`progress_borrowing_allowance`]), so a caller that
    /// keeps writing past either ceiling runs the allowance out and is refused from then
    /// on. Two writes are refused here as they are there: one past
    /// what the ceilings can ever hold ([`Self::can_ever_admit_item_write`]), because no
    /// later attempt at it would be admitted either, and one whose shortfall is deeper
    /// than the allowance.
    pub fn admit_item_write_with_progress(
        &mut self,
        partition: &AttributeValueKey,
        capacity: &WriteCapacity,
        now: SystemTime,
        borrowing: Borrowing,
    ) -> Result<(), WriteThrottle> {
        let Err(throttle) = self.admit_item_write(partition, capacity, now) else {
            return Ok(());
        };
        if !self.can_ever_admit_item_write(capacity) {
            return Err(throttle);
        }
        // Neither budget is borrowed from unless BOTH have the room to lend: a write
        // that is refused did no work, so it owes no capacity anywhere.
        let total = capacity.total();
        let provisioned = self.provisioned_write_rate();
        if let Some(provisioned) = provisioned {
            let covered = match borrowing {
                Borrowing::BothCeilings => self.write_budget.covers_borrowing(
                    provisioned,
                    BURST_CAPACITY_WINDOW,
                    total,
                    now,
                    progress_borrowing_allowance(),
                ),
                Borrowing::PartitionCeiling => self.write_budget.covers_within(
                    provisioned,
                    BURST_CAPACITY_WINDOW,
                    total,
                    now,
                ),
            };
            if !covered {
                return Err(WriteThrottle::TableProvisioned);
            }
        }
        if !self.key_write_budgets.covers_borrowing(
            partition,
            capacity.table,
            now,
            batch_partition_borrowing_allowance(),
        ) {
            return Err(WriteThrottle::KeyRange);
        }
        if let Some(provisioned) = provisioned {
            self.write_budget
                .take_borrowing(provisioned, BURST_CAPACITY_WINDOW, total, now);
        }
        self.key_write_budgets
            .take_borrowing(partition, capacity.table, now);
        // See [`Self::admit_item_write`]: a write served by borrowing consumed its units
        // just the same.
        self.pending_consumed.charge_write(capacity, now);
        Ok(())
    }

    /// The write units per second this table is PROVISIONED for, or `None` for a table
    /// on-demand (`PAY_PER_REQUEST`), which is provisioned for no capacity to exceed.
    fn provisioned_write_rate(&self) -> Option<i64> {
        match (
            self.billing_mode.as_str(),
            self.provisioned_throughput.as_ref(),
        ) {
            ("PROVISIONED", Some(throughput)) if throughput.write_capacity_units > 0 => {
                Some(throughput.write_capacity_units)
            }
            _ => None,
        }
    }

    /// The `ItemCollectionKey` of an item: "The partition key value of the item
    /// collection. This value is the same as the partition key value of the item"
    /// (`ItemCollectionMetrics$ItemCollectionKey`) — so it is the item's own partition
    /// key attribute, name and value, and nothing else.
    fn item_collection_key(&self, item: &Item) -> Item {
        item.get_key_value(self.partition_key_name())
            .map(|(name, value)| (name.clone(), value.clone()))
            .into_iter()
            .collect()
    }

    /// The bytes one item collection holds: "the size of all the items in the table,
    /// plus the size of all attributes projected into all of the local secondary indexes
    /// on that table" (`ItemCollectionMetrics$SizeEstimateRangeGB`), summed over the
    /// items that share `partition` — which is what an item collection is.
    ///
    /// One partition's items are contiguous in the store, so this is a range scan over
    /// that collection rather than a walk over the table.
    fn item_collection_bytes(&self, partition: &AttributeValueKey) -> i64 {
        let first = ItemKey {
            partition: partition.clone(),
            sort: None,
        };
        sum_of(
            self.items
                .range(first..)
                .take_while(|(key, _)| &key.partition == partition)
                .map(|(_, item)| {
                    sum_of(
                        self.local_secondary_indexes
                            .iter()
                            .filter(|index| index.covers(item))
                            .map(|index| index.projected_size(item, &self.key_schema)),
                    )
                    .saturating_add(item_size(item))
                }),
        )
    }

    /// The `ItemCollectionMetrics` for the collection the key of `item` belongs to, or
    /// `None` when this table has no local secondary index: "If the table does not have
    /// any local secondary indexes, this information is not returned in the response"
    /// (`ItemCollectionMetrics`).
    ///
    /// `item` may be a whole item or just a `Key` map — only its key attributes are
    /// read — so one rule serves every write operation.
    pub fn item_collection_metrics(&self, item: &Item) -> Option<ItemCollectionMetrics> {
        if self.local_secondary_indexes.is_empty() {
            return None;
        }
        let key = self.key_of_item(item).ok()?;
        Some(ItemCollectionMetrics {
            item_collection_key: Some(self.item_collection_key(item)),
            size_estimate_range_gb: Some(size_estimate_range_gb(
                self.item_collection_bytes(&key.partition),
            )),
        })
    }

    /// Project this table into the `TableDescription` every table operation returns.
    ///
    /// Every member here comes from what a previous request wrote: the id minted at
    /// create, the metered counters of the last refresh, the billing mode and the
    /// instant `PAY_PER_REQUEST` was last set, the throughput and its history, and the
    /// warm throughput in force. `AttributeDefinitions` is reported in the order
    /// When `PAY_PER_REQUEST` was LAST SET as this table's read/write mode:
    /// "Represents the time when `PAY_PER_REQUEST` was last set as the read/write
    /// capacity mode" (`BillingModeSummary$LastUpdateToPayPerRequestDateTime`).
    ///
    /// The instant of the CALL that set the mode — the create that created the table
    /// on-demand, which is that table's own creation instant, or the `UpdateTable` that
    /// switched it — and `None` for a table that has never been on-demand. Every summary
    /// member that reports the switch reads it from here, so they all report the same
    /// instant, and a description taken while the switch is still settling reports the
    /// instant the switch was asked for rather than nothing.
    fn pay_per_request_set_at(&self) -> Option<::aws_smithy_types::DateTime> {
        self.pay_per_request_at.map(super::reported_date)
    }

    /// The `BillingModeSummary` a description carries, or `None` for a table whose
    /// read/write mode has never been on-demand.
    ///
    /// "You may need to switch to on-demand mode at least once in order to return a
    /// `BillingModeSummary` response" (that shape's own note): the summary records the
    /// `PAY_PER_REQUEST` switch, so a table that has only ever been `PROVISIONED` has
    /// nothing to report under it and the member is absent. The mode of EVERY table is
    /// still reported, through `TableThroughputModeSummary`, which carries no such note.
    ///
    /// A table that HAS been on-demand carries the summary from then on: the mode in
    /// force — which may be `PROVISIONED` again, for a table that has switched back —
    /// beside the instant `PAY_PER_REQUEST` was last set.
    fn billing_mode_summary(&self) -> Option<BillingModeSummary> {
        let last_set = self.pay_per_request_set_at()?;
        Some(BillingModeSummary {
            billing_mode: Some(self.billing_mode.clone()),
            last_update_to_pay_per_request_date_time: Some(last_set),
        })
    }

    /// Project this table into the `TableDescription` every table operation returns.
    ///
    /// Every member here comes from what a previous request wrote: the id minted at
    /// create, the metered counters of the last refresh, the billing mode and the
    /// instant `PAY_PER_REQUEST` was last set, the throughput and its history, and the
    /// warm throughput in force. `AttributeDefinitions` is reported in the order
    /// [`described_definitions`] gives it, which is by attribute name.
    pub fn describe(&self, now: SystemTime) -> TableDescription {
        let (number_of_decreases_today, last_increase_date_time, last_decrease_date_time) =
            self.throughput_history.describe(now);
        let capacity = reported_capacity(self.provisioned_throughput.as_ref());
        let (warm_read_units, warm_write_units) = self.warm_throughput.in_force();
        TableDescription {
            table_name: Some(self.name.clone()),
            table_arn: Some(self.arn.clone()),
            table_id: Some(self.id.clone()),
            key_schema: Some(self.key_schema.clone()),
            attribute_definitions: Some(described_definitions(&self.attribute_definitions)),
            table_status: Some(self.status(now).as_str().to_string()),
            creation_date_time: Some(super::reported_date(self.creation_date_time)),
            item_count: Some(self.reported_counters.item_count),
            table_size_bytes: Some(self.reported_counters.size_bytes),
            billing_mode_summary: self.billing_mode_summary(),
            // The SAME read/write mode, reported through the newer summary member every
            // recorded table description carries beside `BillingModeSummary`: one stored
            // mode and one stored switch instant feed both, so the two can never disagree
            // about which mode a table is in.
            table_throughput_mode_summary: Some(TableThroughputModeSummary {
                table_throughput_mode: Some(self.billing_mode.clone()),
                last_update_to_pay_per_request_date_time: self.pay_per_request_set_at(),
            }),
            provisioned_throughput: Some(ProvisionedThroughputDescription {
                // A PAY_PER_REQUEST table reports zeroes: on-demand mode does not use
                // provisioned throughput.
                read_capacity_units: Some(capacity.read_capacity_units),
                write_capacity_units: Some(capacity.write_capacity_units),
                number_of_decreases_today,
                last_increase_date_time,
                last_decrease_date_time,
            }),
            // A table with no secondary index reports no list at all, which is what
            // DescribeTable does for a table that has none.
            global_secondary_indexes: (!self.global_secondary_indexes.is_empty()).then(|| {
                self.global_secondary_indexes
                    .iter()
                    .map(|index| self.describe_index(index, now))
                    .collect()
            }),
            local_secondary_indexes: (!self.local_secondary_indexes.is_empty()).then(|| {
                self.local_secondary_indexes
                    .iter()
                    .map(|index| self.describe_local_index(index))
                    .collect()
            }),
            // "The vector indexes, if any, on the table" (`TableDescription$VectorIndexes`):
            // reported only for a table that carries one, like both secondary index lists.
            vector_indexes: (!self.vector_indexes.is_empty()).then(|| {
                self.vector_indexes
                    .iter()
                    .map(|index| index.describe(now))
                    .collect()
            }),
            table_class_summary: self.table_class.as_ref().map(|class| TableClassSummary {
                table_class: Some(class.clone()),
                // "The date and time at which the table class was last updated"
                // (`TableClassSummary$LastUpdateDateTime`): the instant of the
                // `UpdateTable` that last set the class, and absent on a table whose
                // class has only ever been the one its create named — that class was
                // never updated.
                last_update_date_time: self
                    .table_class_updated_at
                    .map(super::reported_date),
            }),
            // Every table has a warm throughput, so every description carries one: the
            // pair in force — the one a request named, or the capacity the table keeps
            // warm by being provisioned for it — and the status of that pair rather than
            // of the table.
            warm_throughput: Some(TableWarmThroughputDescription {
                read_units_per_second: Some(warm_read_units),
                write_units_per_second: Some(warm_write_units),
                status: Some(self.warm_throughput.status(now).as_str().to_string()),
            }),
            deletion_protection_enabled: Some(self.deletion_protection_enabled),
            // "The current DynamoDB Streams configuration for the table": reported
            // only for a table whose stream is ON ([`TableStream::specification`]),
            // which is what DescribeTable does both for a table that never enabled
            // one and for one whose stream has been turned off. The table keeps
            // naming its LATEST stream either way.
            stream_specification: self.stream.as_ref().and_then(TableStream::specification),
            latest_stream_arn: self.stream.as_ref().map(|stream| stream.arn.clone()),
            latest_stream_label: self.stream.as_ref().map(|stream| stream.label.clone()),
            // "Contains details for the restore": reported only for a table a restore
            // created, which is what DescribeTable does for a table `CreateTable`
            // created. `RestoreInProgress` is the table's own transient status — the
            // restore is what is creating it — so the one stored span answers both
            // members and they cannot disagree.
            restore_summary: self.restore.as_ref().map(|restore| RestoreSummary {
                source_backup_arn: restore.source_backup_arn.clone(),
                source_table_arn: restore.source_table_arn.clone(),
                restore_date_time: super::reported_date(restore.restore_date_time),
                restore_in_progress: self.status(now) == TableStatus::Creating,
            }),
            // "Represents replicas of the table", and the version of global tables they
            // are replicated under: reported only for a table that HAS replicas, which is
            // what a standalone regional table is not ([`Table::described_replicas`]).
            replicas: self.described_replicas(now),
            global_table_version: self.global_table_version(),
            // "The description of the server-side encryption status on the specified
            // table": reported only for a table encrypted with a KMS key, which is what
            // the Amazon Web Services OWNED key a table is encrypted with by default is
            // not ([`TableEncryption`]).
            sse_description: self
                .encryption
                .as_ref()
                .map(|encryption| encryption.describe(now)),
            // "Sets the maximum number of read and write units for the specified
            // on-demand table" (the `OnDemandThroughput` doc): the pair belongs to the
            // on-demand capacity model, so it is reported for a table in that mode — each
            // dimension either capped or reported as the -1 that means "no maximum"
            // ([`OnDemandCeilings::describe`]) — and not for a provisioned one, whose
            // capacity is its `ProvisionedThroughput`.
            on_demand_throughput: (self.billing_mode == "PAY_PER_REQUEST")
                .then(|| self.on_demand_throughput.describe()),
            ..TableDescription::default()
        }
    }

    /// Snapshot this table as a backup taken from it records its source
    /// ([`SourceTableSnapshot`]).
    ///
    /// The figures are counted HERE, at the instant the backup is requested, rather than
    /// taken from the table's periodically metered counters: a backup catalogues the data
    /// committed up to its own time marker (the `CreateBackup` doc), so what it reports
    /// is what the table held then — and the table itself may be changed or gone by the
    /// time the backup is read.
    pub fn backup_snapshot(&self) -> SourceTableSnapshot {
        SourceTableSnapshot {
            key_schema: self.key_schema.clone(),
            attribute_definitions: self.attribute_definitions.clone(),
            table_creation_date_time: self.creation_date_time,
            billing_mode: self.billing_mode.clone(),
            provisioned_throughput: reported_capacity(self.provisioned_throughput.as_ref()),
            item_count: i64::try_from(self.items.len()).unwrap_or(i64::MAX),
            table_size_bytes: self.stored_bytes,
            global_secondary_indexes: self
                .global_secondary_indexes
                .iter()
                .map(|index| GlobalSecondaryIndexInfo {
                    index_name: Some(index.name.clone()),
                    key_schema: Some(index.key_schema.clone()),
                    projection: Some(index.projection.clone()),
                    provisioned_throughput: index.provisioned_throughput.clone(),
                    on_demand_throughput: None,
                })
                .collect(),
            local_secondary_indexes: self
                .local_secondary_indexes
                .iter()
                .map(|index| LocalSecondaryIndexInfo {
                    index_name: Some(index.name.clone()),
                    key_schema: Some(index.key_schema.clone()),
                    projection: Some(index.projection.clone()),
                })
                .collect(),
            // The vector indexes the table carried, recorded with the backup so a restore
            // from it rebuilds them and `DescribeBackup` reports them.
            vector_indexes: self
                .vector_indexes
                .iter()
                .map(|index| VectorIndexInfo {
                    index_name: Some(index.name.clone()),
                    vector_attribute: Some(index.vector_attribute.clone()),
                    search_schema: (!index.search_schema.is_empty())
                        .then(|| index.search_schema.clone()),
                    projection: Some(index.projection.clone()),
                    dimensions: Some(index.dimensions),
                    distance_function: Some(index.distance_function.clone()),
                })
                .collect(),
            // The stream settings the table had when the backup was taken, by the same
            // rule its own description reports them: a stream that was OFF then is no
            // setting to carry, so the member is absent.
            stream_specification: self.stream.as_ref().and_then(TableStream::specification),
            // The TTL SETTING in force at this instant, copied as the setting it is
            // ([`SourceTableSnapshot::time_to_live`]): the status it is reported in is
            // derived when the backup is read, by the same rule the table's own TTL reader
            // follows, so a change the clock had not settled yet is not frozen half-done.
            time_to_live: self.time_to_live.clone(),
        }
    }
}

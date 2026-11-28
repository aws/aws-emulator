// SPDX-License-Identifier: CC0-1.0

//! What a read of a table — or of one of its secondary indexes — evaluates and reports.
//!
//! `Query` and `Scan` differ in WHICH entries they walk: a query reads one partition of
//! the schema it was pointed at, a scan reads every entry of it. Everything else about
//! being pointed at an index is the same for both, and is decided here: which index an
//! `IndexName` resolves to and the two refusals that resolution carries, what each
//! reported item is narrowed to ([`Selection`]), whether a read has to go on to the
//! parent table for an attribute its index does not project, and what one row of a page
//! was SEEN as ([`Read`]).
//!
//! A file of its own beside the item store rather than inside it: this is about what
//! one REQUEST reads and reports, not about what the store holds. It is re-exported up
//! to `crate::state`, so every caller names it `crate::state::…`.

use std::collections::{BTreeSet, HashMap};

use super::projection::{project_paths, projected_paths, projection_expression_paths};
use crate::ops::types::{ConsumedCapacity, KeySchemaElement};
use crate::state::{
    consumed_capacity, consumed_index_capacity, enum_value_requirement, expression_conflict,
    invalid_member, unknown_index_named, AttributeValueKey, DocumentPath, IndexItemKey, Item,
    ItemKey, KindReport, RequestError, SecondaryIndex, Substitutions, Table,
};

/// How a strongly consistent read of a GLOBAL secondary index is refused: "The
/// ConsistentRead parameter is not supported on global secondary indexes. If you query
/// a global secondary index with ConsistentRead set to true, you will receive a
/// ValidationException" (`Query$ConsistentRead`, and `Scan$ConsistentRead` states the
/// same rule). The index is not named: the rule holds for every global secondary index,
/// so the refusal reports nothing of the table.
pub const CONSISTENT_READ_ON_GLOBAL_INDEX_MESSAGE: &str =
    "Consistent reads are not supported on global secondary indexes";

/// How a read of every item attribute FROM a global secondary index that projects only
/// some of them is refused: "If you query or scan a global secondary index, you can only
/// request attributes that are projected into the index. Global secondary index queries
/// cannot fetch attributes from the parent table" (`Query$Select`), so the request names
/// an index whose projection cannot answer it.
pub fn all_attributes_unprojected(index_name: &str) -> String {
    format!(
        "One or more parameter values were invalid: Select type ALL_ATTRIBUTES is not supported \
         for global secondary index {index_name} because its projection type is not ALL"
    )
}

/// How a read of a table's VECTOR index is refused: a vector index is searched rather than
/// queried or scanned — `SearchVectors` "performs a vector similarity search on a vector
/// index" and takes its own `IndexName` — so an index of that kind answers neither of the
/// two readers, whichever one named it. The sentence names the OPERATION that cannot read
/// it and nothing of the index, because the refusal is about the kind of index the name
/// resolved to.
pub fn unsupported_index_type(operation: &str) -> String {
    format!("{operation} operation not supported on this index type.")
}

/// The secondary index a read is served from, or `None` when it reads the table itself.
///
/// Three facts of the request are settled here, in the order a reader meets them:
///
/// * "The name of a secondary index to [read]. This index can be any local secondary
///   index or global secondary index" (`Query$IndexName`): a name this table declares no
///   index for is the request's own mistake, stated over the name it sent
///   ([`unknown_index_named`]) — while a name the table does carry an index for, of a kind
///   `operation` cannot read, is refused as the index type it is
///   ([`unsupported_index_type`]) rather than reported absent;
/// * a strongly consistent read of a global secondary index is refused
///   ([`CONSISTENT_READ_ON_GLOBAL_INDEX_MESSAGE`]);
/// * a global secondary index whose BUILD has not finished at `now` is no source a read can
///   be served from ([`Table::global_index_is_building`]);
/// * a read of every item attribute can only be served from a global index that projects
///   them all ([`all_attributes_unprojected`]).
///
/// All three are decided before anything is read, so the caller is told to fix the
/// request rather than to retry it.
pub fn resolve_index<'t>(
    table: &'t Table,
    operation: &str,
    index_name: Option<&str>,
    consistent_read: bool,
    selection: &Selection,
    now: ::std::time::SystemTime,
) -> Result<Option<SecondaryIndex<'t>>, RequestError> {
    let Some(index_name) = index_name else {
        return Ok(None);
    };
    let index = table.secondary_index(index_name).ok_or_else(|| {
        // An index of this name the table DOES carry, that this reader cannot be served
        // from, is reported as the kind of index it is: the table has it, so saying it is
        // absent would send the caller looking for a resource that is there.
        if table
            .vector_indexes
            .iter()
            .any(|vector| vector.name == index_name)
        {
            RequestError::Validation(unsupported_index_type(operation))
        } else {
            unknown_index_named(index_name)
        }
    })?;
    // A GLOBAL index whose build has not finished holds no complete copy of the table's
    // items yet, so it is no source a read can be served from: a read naming one is
    // answered exactly as a read naming an index no table of this account declares is
    // ([`unknown_index_named`]), rather than served a page out of a half-built index that
    // the caller would read as the whole answer. The index is still THERE — its description
    // reports the status it is in — which is what tells a client to wait.
    if table.global_index_is_building(index_name, now) {
        return Err(unknown_index_named(index_name));
    }
    if consistent_read && index.global {
        return Err(RequestError::Validation(
            CONSISTENT_READ_ON_GLOBAL_INDEX_MESSAGE.to_string(),
        ));
    }
    if index.global && matches!(selection, Selection::AllAttributes) && !index.projects_all() {
        return Err(RequestError::Validation(all_attributes_unprojected(
            index.name,
        )));
    }
    Ok(Some(index))
}

/// What a read reports for each item its page selected.
#[derive(Debug)]
pub enum Selection {
    /// "ALL_ATTRIBUTES - Returns all of the item attributes from the specified table or
    /// index" (`Query$Select`) — the default when a table is read.
    AllAttributes,
    /// "ALL_PROJECTED_ATTRIBUTES - Allowed only when querying an index. Retrieves all
    /// attributes that have been projected into the index" — the default when an index
    /// is read.
    AllProjected,
    /// "SPECIFIC_ATTRIBUTES - Returns only the attributes listed in
    /// ProjectionExpression", which is also what naming a projection and no `Select` at
    /// all asks for. The projection is a list of document paths, so an entry may reach
    /// inside an attribute ([`project_paths`]).
    Specific(Vec<DocumentPath>),
    /// "COUNT - Returns the number of matching items, rather than the matching items
    /// themselves."
    Count,
}

impl Selection {
    /// The document paths this read REPORTS by, or none at all for a read that reports
    /// whole items or only a count.
    ///
    /// What they are for: the paths of a projection are held to the addressed table's key
    /// schema once it is resolved ([`check_scalar_key_paths`]), which is the one path rule
    /// that cannot be settled where the member is read.
    pub fn projected_paths(&self) -> &[DocumentPath] {
        match self {
            Self::Specific(paths) => paths,
            Self::AllAttributes | Self::AllProjected | Self::Count => &[],
        }
    }
}

/// How a refusal of the `Select`/projection pair names what was asked for: `COUNT`
/// "returns the number of matching items, rather than the matching items themselves"
/// (`Query$Select`), which AWS's sentence words as the count itself; every other value
/// is named as the request spelled it.
fn select_target(select: &str) -> &str {
    if select == "COUNT" {
        "only the Count"
    } else {
        select
    }
}

/// What each reported item carries, from the request's `Select` and the paths its
/// projection named.
///
/// "If neither Select nor ProjectionExpression are specified, DynamoDB defaults to
/// ALL_ATTRIBUTES when accessing a table, and ALL_PROJECTED_ATTRIBUTES when accessing an
/// index. You cannot use both Select and ProjectionExpression together in a single
/// request, unless the value for Select is SPECIFIC_ATTRIBUTES" (`Query$Select`, which
/// `Scan$Select` repeats), which is the whole of the rule below.
///
/// Each refusal is a violation of a rule over the request's own members, so it is stated
/// as one clause INSIDE the `<N> validation error(s) detected: ` envelope, naming the
/// member the request carried its projection in ([`stated_projection_member`]) and the
/// `Select` it stated. The envelope is part of the refusal recorded for these three
/// clauses — a reader tempted to state them bare should check that recording first.
pub fn resolve_selection(
    select: Option<&str>,
    projection_member: Option<&'static str>,
    requested: Option<Vec<DocumentPath>>,
    on_index: bool,
) -> Result<Selection, RequestError> {
    if let Some(value) = select {
        if !crate::ops::types::SELECT_VALUES.contains(&value) {
            return Err(invalid_member(
                "select",
                Some(value),
                &enum_value_requirement(crate::ops::types::SELECT_VALUES),
            ));
        }
    }
    let member = projection_member.unwrap_or(PROJECTION_MEMBER);
    match select {
        None => Ok(match requested {
            Some(paths) => Selection::Specific(paths),
            None if on_index => Selection::AllProjected,
            None => Selection::AllAttributes,
        }),
        // "SPECIFIC_ATTRIBUTES - Returns only the attributes listed in
        // ProjectionExpression": there are none to return when the request listed none,
        // and the refusal names BOTH members a projection can be listed in, because the
        // request listed it in neither.
        Some("SPECIFIC_ATTRIBUTES") => requested.map(Selection::Specific).ok_or_else(|| {
            super::refused(format!(
                "Must specify the {LEGACY_PROJECTION_MEMBER} or {PROJECTION_MEMBER} when \
                 choosing to get SPECIFIC_ATTRIBUTES"
            ))
        }),
        // "If you use the ProjectionExpression parameter, then the value for Select can
        // only be SPECIFIC_ATTRIBUTES. Any other value for Select will return an error."
        Some(other) if requested.is_some() => Err(super::refused(format!(
            "Cannot specify the {member} when choosing to get {}",
            select_target(other)
        ))),
        Some("ALL_ATTRIBUTES") => Ok(Selection::AllAttributes),
        Some("COUNT") => Ok(Selection::Count),
        Some("ALL_PROJECTED_ATTRIBUTES") if on_index => Ok(Selection::AllProjected),
        // "ALL_PROJECTED_ATTRIBUTES - Allowed only when querying an index": the read has
        // no index projection to report when it was pointed at the table itself. The
        // sentence states that ONE rule, with the verb the rule itself is written in —
        // the refusal is the same sentence whichever read made the request, and nothing
        // in it is taken from the operation the caller called.
        Some(other) => Err(super::refused(format!(
            "{other} can be used only when Querying using an IndexName"
        ))),
    }
}

/// The member a request stated its projection through, or `None` when it stated none:
/// which of the two a refusal names follows the request, not the reader.
pub fn stated_projection_member(
    projection_expression: Option<&str>,
    attributes_to_get: Option<&[String]>,
) -> Option<&'static str> {
    if projection_expression.is_some() {
        return Some(PROJECTION_MEMBER);
    }
    attributes_to_get
        .filter(|names| !names.is_empty())
        .map(|_| LEGACY_PROJECTION_MEMBER)
}

/// The member a request states its projection through today: "ProjectionExpression - A
/// string that identifies one or more attributes to retrieve from the table"
/// (`Query$ProjectionExpression`).
const PROJECTION_MEMBER: &str = "ProjectionExpression";

/// The document paths the request's PROJECTION names, or `None` when it names none at
/// all — in which case "all attributes will be returned" (`ProjectionExpression`).
///
/// The two members are the same ask one generation apart: `AttributesToGet` is "a legacy
/// parameter. Use ProjectionExpression instead" (that member's doc), so a request that
/// states the projection both ways is refused the way the service refuses a request that
/// states one thing twice, and either member is read into the one path list the reader
/// projects by — a legacy name addressing the whole attribute it names.
///
/// Either way the list is the one [`projected_paths`] settles: held to the ceiling that
/// module names — which is what bounds the legacy member, since it carries no modelled
/// length — refused when two paths overlap, and in the order a report applies it. So what
/// [`reported_items`] does per item it answers is a function of this service's constants
/// rather than of the request body.
pub fn requested_projection(
    member: &'static str,
    projection_expression: Option<&str>,
    attributes_to_get: Option<&[String]>,
    names: Option<&HashMap<String, String>>,
) -> Result<Option<Vec<DocumentPath>>, RequestError> {
    // A key of the substitution table that is no substitution TOKEN defines nothing an
    // expression could dereference, so that defect is settled before the projection that
    // would dereference one is read ([`Substitutions::check_keys_are_tokens`]).
    Substitutions::new(names, None).check_keys_are_tokens(&[member])?;
    let legacy = attributes_to_get.filter(|names| !names.is_empty());
    // Whichever member the request stated its projection through is the member its
    // refusals name: a caller that sent no `ProjectionExpression` at all is told about
    // the list it did send.
    let (stated, paths) = match (projection_expression, legacy) {
        (Some(_), Some(_)) => return Err(expression_conflict(LEGACY_PROJECTION_MEMBER, member)),
        (Some(expression), None) => (
            member,
            projection_expression_paths(member, expression, Substitutions::new(names, None))?,
        ),
        (None, Some(names)) => {
            // The member is "an array of attributes to retrieve", a list of attribute
            // NAMES: a name it lists twice is a duplicate value in that list, refused
            // over the repeated name rather than read as two paths addressing one
            // location ([`duplicate_attribute_name`]).
            let mut stated: BTreeSet<&str> = BTreeSet::new();
            for name in names {
                if !stated.insert(name.as_str()) {
                    return Err(duplicate_attribute_name(name));
                }
            }
            (
                LEGACY_PROJECTION_MEMBER,
                names
                    .iter()
                    .map(|name| DocumentPath::attribute_named(name))
                    .collect(),
            )
        }
        (None, None) => return Ok(None),
    };
    projected_paths(stated, paths).map(Some)
}

/// How a legacy `AttributesToGet` that lists one attribute name twice is refused.
///
/// That member is a list of attribute NAMES rather than of document paths, so listing
/// one name twice is a duplicate VALUE in the list — stated as the parameter-value
/// sentence over the repeated name, and not as the overlapping-paths refusal a
/// `ProjectionExpression` naming one location twice earns.
fn duplicate_attribute_name(name: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Duplicate value in attribute name: {name}"
    ))
}

/// The member a request states its projection through one generation back:
/// "AttributesToGet - This is a legacy parameter. Use ProjectionExpression instead"
/// (that member's doc). Named here because it is what the refusals of a request that
/// used it have to say.
const LEGACY_PROJECTION_MEMBER: &str = "AttributesToGet";

/// Which of `names` an index does NOT project, in name order — the ONE projection test
/// every reader asks, over whatever set of attribute names it needs.
///
/// An index holds "attributes that are copied (projected) from the table into the index"
/// (`GlobalSecondaryIndex$Projection`), and which names those are is
/// [`SecondaryIndex::projects`]'s answer: this is that answer over a SET of names, so
/// "can this index serve these attributes" has one implementation — for the members a
/// classic read states its projection and filter in, and for the paths a statement
/// spells alike.
pub fn unprojected_of(
    index: &SecondaryIndex<'_>,
    table_key_schema: &[KeySchemaElement],
    names: &BTreeSet<String>,
) -> Vec<String> {
    if index.projects_all() {
        return Vec::new();
    }
    names
        .iter()
        .filter(|name| !index.projects(table_key_schema, name))
        .cloned()
        .collect()
}

/// Whether a read needs an attribute its index does not project, which is what sends it
/// on to the parent table.
///
/// What it needs is what it REPORTS plus what its FILTER reads: a filter is judged
/// against the item, so an attribute only the filter names is still read even though no
/// response carries it. A projection that reaches INSIDE an attribute needs that
/// attribute, which is the top-level name the path starts at.
pub fn needs_unprojected(
    index: &SecondaryIndex<'_>,
    table_key_schema: &[KeySchemaElement],
    selection: &Selection,
    filter_attributes: &BTreeSet<String>,
) -> bool {
    if index.projects_all() {
        return false;
    }
    if matches!(selection, Selection::AllAttributes) {
        return true;
    }
    let mut needed = filter_attributes.clone();
    if let Selection::Specific(paths) = selection {
        needed.extend(paths.iter().map(|path| path.attribute().to_string()));
    }
    !unprojected_of(index, table_key_schema, &needed).is_empty()
}

/// The attributes one read's PROJECTION names that a GLOBAL secondary index does not
/// project — [`unprojected_projection_refusal`] reports them, and an empty list is a
/// projection the index can answer on its own.
///
/// "If you query or scan a global secondary index, you can only request attributes that
/// are projected into the index. Global secondary index queries cannot fetch attributes
/// from the parent table" (`Query$Select`, which `Scan$Select` repeats): what a read
/// REQUESTS is what it reports, so a projection naming an attribute the index does not
/// hold cannot be served from the index at all, and a path that reaches INSIDE an
/// attribute requests that attribute — the top-level name the path starts at.
///
/// A read of EVERY attribute names no attribute in particular — it is refused where the
/// index is resolved ([`all_attributes_unprojected`]) — and a LOCAL secondary index
/// fetches what it does not project from the parent table instead
/// ([`needs_unprojected`]), so neither contributes here.
pub fn unprojected_projection_attributes(
    index: &SecondaryIndex<'_>,
    table_key_schema: &[KeySchemaElement],
    selection: &Selection,
) -> Vec<String> {
    let Selection::Specific(paths) = selection else {
        return Vec::new();
    };
    if !index.global {
        return Vec::new();
    }
    let named: BTreeSet<String> = paths
        .iter()
        .map(|path| path.attribute().to_string())
        .collect();
    unprojected_of(index, table_key_schema, &named)
}

/// How a projection a global secondary index cannot answer is refused: the index the
/// request named and the attributes it does not project, in name order.
pub fn unprojected_projection_refusal(index_name: &str, attributes: &[String]) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Global secondary index {index_name} does \
         not project [{}]",
        attributes.join(", ")
    ))
}

/// The attributes one QUERY's filter names that the secondary index it reads does not
/// project, refused through the sentence this service states that refusal with
/// ([`super::unprojected_filter_refusal`]).
///
/// A filter is judged against what the read SEES, and a query pointed at a secondary
/// index sees that index's entries: a predicate on an attribute the index does not hold
/// could never be decided from them, so the query is refused rather than answered with
/// the page that predicate would match. The parent table is no answer to it — a global
/// index read "cannot fetch attributes from the parent table" (`Query$Select`) at all,
/// and the fetch a local index does is for the attributes a read REPORTS, not for the
/// ones its filter reads. A SCAN of the same index is not refused: it evaluates the
/// predicate against the entry, which simply matches nothing.
pub fn unprojected_read_filter_attributes(
    index: &SecondaryIndex<'_>,
    table_key_schema: &[KeySchemaElement],
    filter_attributes: &BTreeSet<String>,
) -> Vec<String> {
    unprojected_of(index, table_key_schema, filter_attributes)
}

/// One row a read evaluated: the stored item, and the entry the index it was read
/// through holds for it (`None` on a base-table read).
#[derive(Debug)]
pub struct Read<'a> {
    item: &'a Item,
    entry: Option<Item>,
    /// Whether this read went on to the parent table for the attributes its index does
    /// not project, which only a local secondary index can do.
    fetched: bool,
}

impl<'a> Read<'a> {
    /// The row `item` is read as through `index` (`None` for the table itself), having
    /// gone on to the parent table when `fetched`.
    pub fn new(
        item: &'a Item,
        index: Option<&SecondaryIndex<'_>>,
        table_key_schema: &[KeySchemaElement],
        fetched: bool,
    ) -> Self {
        Self {
            item,
            entry: index.map(|index| index.entry(item, table_key_schema)),
            fetched,
        }
    }

    /// The STORED item this row projects, which is what a `LastEvaluatedKey` is taken
    /// from.
    pub fn item(&self) -> &'a Item {
        self.item
    }

    /// The attributes this read SAW — what its filter is judged against, what it is
    /// charged for, and what `ALL_ATTRIBUTES` reports: the index's entry, or the whole
    /// stored item wherever the table itself was read (a base-table read, or an index
    /// read that fetched).
    pub fn seen(&self) -> &Item {
        match &self.entry {
            Some(entry) if !self.fetched => entry,
            _ => self.item,
        }
    }

    /// The attributes the index HOLDS for this row, which is what
    /// `ALL_PROJECTED_ATTRIBUTES` reports; on a base-table read the item is its own
    /// projection.
    pub fn projected(&self) -> &Item {
        self.entry.as_ref().unwrap_or(self.item)
    }
}

/// Where one row sits in the order a read walks, which is what a `LastEvaluatedKey`
/// reports and an `ExclusiveStartKey` resumes from.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadPosition<'a> {
    /// A base-table read walks the table's own primary-key order.
    Table(&'a ItemKey),
    /// An index read walks the order that index holds its entries in.
    Index(IndexItemKey),
}

/// The place a page resumes from, read out of `ExclusiveStartKey` through the schema the
/// read walks ([`resume_from`]).
#[derive(Debug)]
pub enum ReadResume {
    Table(ItemKey),
    Index(IndexItemKey),
}

impl ReadResume {
    /// The PARTITION-key component of the place this token resumes from, in the key space
    /// being walked — the same component a row of that walk sits at
    /// ([`ReadPosition::partition`]), which is what decides which SEGMENT of a parallel
    /// walk a place belongs to.
    pub fn partition(&self, index: Option<&SecondaryIndex<'_>>) -> &[AttributeValueKey] {
        match self {
            Self::Table(key) => std::slice::from_ref(&key.partition),
            Self::Index(key) => index_partition(key, index),
        }
    }
}

/// The leading `HASH` components of one index key.
///
/// An index may declare several partition-key elements
/// ([`crate::state::MAX_INDEX_PARTITION_KEYS`]) and an index key lays its `HASH` values
/// first ([`Table::index_item_key`]), so the partition of a place in an index walk is
/// that leading run. Stated once, for the two places that hold one: a row the walk
/// reached ([`ReadPosition::partition`]) and the place a token resumes from
/// ([`ReadResume::partition`]).
fn index_partition<'k>(
    key: &'k IndexItemKey,
    index: Option<&SecondaryIndex<'_>>,
) -> &'k [AttributeValueKey] {
    let leading = index
        .map(|index| {
            index
                .key_schema
                .iter()
                .filter(|element| element.key_type == "HASH")
                .count()
        })
        .unwrap_or(1)
        .clamp(1, key.index.len().max(1));
    key.index.get(..leading).unwrap_or(&key.index)
}

impl ReadPosition<'_> {
    /// The PARTITION-key component of this row's place in the key space being walked:
    /// the table's partition key value, or the `HASH` values of the index's key when the
    /// walk is `index`'s.
    ///
    /// A list rather than a value because an index may declare several partition-key
    /// elements ([`crate::state::MAX_INDEX_PARTITION_KEYS`]), and an index key lays its
    /// `HASH` values first ([`Table::index_item_key`]). Two rows that agree here are in
    /// the same partition of the key space, which is what a parallel `Scan` divides into
    /// segments.
    pub fn partition(&self, index: Option<&SecondaryIndex<'_>>) -> &[AttributeValueKey] {
        match self {
            Self::Table(key) => std::slice::from_ref(&key.partition),
            Self::Index(key) => index_partition(key, index),
        }
    }

    /// Whether this row comes strictly PAST the place a previous page stopped at, in the
    /// direction this read walks: "Use this value to start a new operation, EXCLUDING
    /// this value in the new request" (`LastEvaluatedKey`), which for a descending read
    /// is everything BELOW that place.
    pub fn past(&self, resumed: &ReadResume, forward: bool) -> bool {
        match (self, resumed) {
            (Self::Table(key), ReadResume::Table(from)) => {
                if forward {
                    *key > from
                } else {
                    *key < from
                }
            }
            (Self::Index(key), ReadResume::Index(from)) => {
                if forward {
                    key > from
                } else {
                    key < from
                }
            }
            // A resume token is read through the same schema the read walks, so the two
            // are always the same kind; a row is never skipped on a mismatch.
            _ => true,
        }
    }
}

/// Which sentence a paged read states for an EMPTY `S`/`B` element of the
/// `ExclusiveStartKey` it was handed.
///
/// The non-emptiness a key value must have is one rule ("Attribute values of type String
/// and Binary must have a length greater than zero if the attribute is used as a key
/// attribute for a table or index"), and DynamoDB states it in two wordings: the plain
/// sentence naming the key attribute, and the one that states a value is not supported for
/// a SECONDARY INDEX key and names the index beside the element. Which of the two a
/// resume token earns follows the READ the token was handed to, so each reader names its
/// own ([`resume_from`]) rather than the rule guessing from the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyStartKeyElement<'i> {
    /// The plain sentence, naming the key attribute alone: what `Query` states, whichever
    /// schema it pages.
    NamesTheKey,
    /// The secondary-index sentence, over the index named here: what `Scan` states, which
    /// validates its token against a secondary index of the table and reports that index
    /// beside the element — for EVERY element of the token, the table key attributes that
    /// index does not declare included.
    NamesTheIndexKey(&'i str),
}

/// Which sentence a paged read states for an element of the `ExclusiveStartKey` it was
/// handed that carries another type than the table's `AttributeDefinitions` declare for that
/// key attribute, where the read pages a secondary INDEX.
///
/// Like the empty element above, this is one rule stated in two wordings, and which of them
/// a token earns follows the READ it was handed to rather than the token: a table WALK
/// DETAILS what it found — the index being walked beside the attribute and the two types —
/// while a QUERY states the one fixed invalid-token sentence for every token it cannot place
/// in the schema it pages, that element included. So each reader names its own
/// ([`resume_from`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MistypedStartKeyElement {
    /// The fixed invalid-token sentence ([`crate::state::START_KEY_INVALID_MESSAGE`]): what
    /// `Query` states, whichever schema it pages.
    InvalidToken,
    /// The sentence that details the element: what `Scan` states.
    NamesTheMismatch,
}

/// The place an `ExclusiveStartKey` resumes a read from, read back through the schema
/// that read walks.
///
/// "The data type for ExclusiveStartKey must be String, Number or Binary. No set data
/// types are allowed" — and for an index read the token is the index's key together with
/// the table's, because that pair is what addresses one entry of an index.
///
/// `empty_element` and `mistyped` are the two wordings the READERS differ in, each named by
/// the reader that was handed the token ([`EmptyStartKeyElement`],
/// [`MistypedStartKeyElement`]).
pub fn resume_from(
    table: &Table,
    index: Option<&SecondaryIndex<'_>>,
    start: Option<&Item>,
    empty_element: EmptyStartKeyElement<'_>,
    mistyped: MistypedStartKeyElement,
) -> Result<Option<ReadResume>, RequestError> {
    match (start, index) {
        (None, _) => Ok(None),
        (Some(start), None) => Ok(Some(ReadResume::Table(
            table.key_of_start_key(start, empty_element)?,
        ))),
        (Some(start), Some(index)) => Ok(Some(ReadResume::Index(
            table.index_key_of_start_key(index, start, empty_element, mistyped)?,
        ))),
    }
}

/// The key attributes a page reports as its `LastEvaluatedKey`: the table's primary key,
/// or an index entry's own key beside it.
pub fn evaluated_key(table: &Table, index: Option<&SecondaryIndex<'_>>, item: &Item) -> Item {
    match index {
        Some(index) => table.index_key_attributes(index, item),
        None => table.key_attributes(item),
    }
}

/// Whether a page reports the key it STOPPED AT, where `limit` is the page size the
/// request asked for, `evaluated` is how many rows the page took and `more` says whether
/// the walk left a row unevaluated.
///
/// A paginated read stops for one of two reasons: rows were left to evaluate, or it
/// reached the `Limit` the caller asked for. It reports where it stopped in BOTH cases —
/// "if LastEvaluatedKey is not empty, it does not necessarily mean that there is more
/// data in the result set. The only way to know when you have reached the end of the
/// result set is when LastEvaluatedKey is empty" (`Scan$LastEvaluatedKey`) — so a page
/// whose limit was reached by the last row the walk held still carries a key, and the
/// client learns the read is done from the page it asks for next. Only a read that ran
/// out of rows with the caller's limit unreached is the last page and reports none,
/// which is also every read that named no limit at all.
pub fn reports_stop_key(limit: Option<i32>, evaluated: usize, more: bool) -> bool {
    more || limit.is_some_and(|limit| usize::try_from(limit).is_ok_and(|limit| evaluated >= limit))
}

/// The `ConsumedCapacity` a read reports, billed to the index it was served from when it
/// named one ([`consumed_index_capacity`]) and to the table otherwise
/// ([`consumed_capacity`]).
///
/// `units` is what reading the SOURCE cost, rounded the way the reader charges its own
/// rows — a `Scan` is charged "a read unit for every item it examined" and a `Query` once
/// over the page it read — and `fetched` is what the read spent going on to the parent
/// table for attributes the index it read does not project, which is the table's own arm
/// of an index read ([`fetch_capacity_units`]); a base-table read spends everything on the
/// table and so has none.
pub fn consumed_read_capacity(
    requested: Option<&str>,
    table_name: &str,
    index: Option<&SecondaryIndex<'_>>,
    units: f64,
    fetched: f64,
) -> Option<ConsumedCapacity> {
    match index {
        Some(index) => consumed_index_capacity(requested, table_name, index, units, fetched),
        None => consumed_capacity(requested, table_name, units, KindReport::Itemised),
    }
}

/// The `Items` a response carries for the rows its page matched, or `None` for a
/// `COUNT`, which "returns the number of matching items, rather than the matching items
/// themselves".
///
/// The projection is applied AFTER the filter, so an attribute the response does not
/// carry still narrows the read.
pub fn reported_items(selection: &Selection, matched: &[&Read<'_>]) -> Option<Vec<Item>> {
    match selection {
        Selection::Count => None,
        Selection::AllAttributes => Some(matched.iter().map(|read| read.seen().clone()).collect()),
        Selection::AllProjected => {
            Some(matched.iter().map(|read| read.projected().clone()).collect())
        }
        Selection::Specific(paths) => Some(
            matched
                .iter()
                .map(|read| project_paths(read.seen(), paths))
                .collect(),
        ),
    }
}

// SPDX-License-Identifier: CC0-1.0

//! Indexes, split out of [`crate::state`] unchanged: an index of a domain, the schema
//! and settings it carries, and the packages its analysis chain references.

use super::*;

/// `IndexStatus`'s `CREATED` value — the status `CreateIndex` reports for the index
/// it minted. `IndexStatus` declares exactly `CREATED | UPDATED | DELETED`
/// ([`crate::ops::types::INDEX_STATUS_VALUES`]): a value per index call, so what the
/// member carries is which call was served, not a lifecycle state the resource moves
/// through.
pub const INDEX_STATUS_CREATED: &str = "CREATED";

/// How many indexes one domain may hold before a create is refused.
///
/// Like [`DOMAIN_QUOTA`] this is the emulator's OWN bound on the store — the model
/// publishes none — and it is what stops a caller looping with fresh index names
/// from growing this process's state without end.
pub const INDEX_QUOTA: usize = 100;

/// The reserved name a request addresses EVERY index of the cluster by, which is what a
/// read naming it resolves to ([`Domain::resolved_indexes`]).
pub const ALL_INDEXES: &str = "_all";

/// The character a RESERVED top-level path segment of the engine's REST surface begins
/// with — `_bulk`, `_search`, `_cat`, … — which no index name may begin with
/// ([`INDEX_NAME_FORBIDDEN_FIRST`]).
///
/// What keeps a reserved endpoint from being read as an index name: a first segment
/// carrying it names an API, so a request on one the surface does not serve is answered as
/// an unserved path rather than as a request about an index nothing could hold.
pub const RESERVED_SEGMENT_PREFIX: char = '_';

/// `IndexName`'s `@pattern`, as the model declares it
/// (`smithy_query(query="shape",shape="IndexName")`) — the expression a refusal of a
/// name outside it quotes, so the clause names the rule the model carries rather than a
/// sentence written here.
pub const INDEX_NAME_PATTERN: &str = r#"^(?!\.\.?$)[^_ ,:"+/*\\|?#><A-Z-][^ ,:"+/*\\|?#><A-Z]*$"#;

/// The characters `IndexName`'s pattern forbids ANYWHERE in the name, from its
/// trailing character class `[^ ,:"+/*\\|?#><A-Z]` (the upper-case letters that
/// class also excludes are tested as a range rather than listed here).
pub(super) const INDEX_NAME_FORBIDDEN: &str = " ,:\"+/*\\|?#><";

/// The characters that class forbids only as the FIRST character, from
/// `IndexName`'s leading class `[^_ ,:"+/*\\|?#><A-Z-]`: an index name may carry an
/// underscore or a hyphen, just not lead with one.
pub(super) const INDEX_NAME_FORBIDDEN_FIRST: [char; 2] = ['_', '-'];

/// Whether `value` is an `IndexName`: pattern
/// `^(?!\.\.?$)[^_ ,:"+/*\\|?#><A-Z-][^ ,:"+/*\\|?#><A-Z]*$` with
/// `@length(min: 1, max: 255)`.
///
/// The request boundary enforces the length bound but not this pattern — its
/// negative lookahead is not a construct the boundary's regex engine compiles — so
/// an operation that takes an `IndexName` checks the shape itself rather than
/// storing a name no index could have. The same reasoning as [`is_guid`] and
/// [`is_arn`], and a SHAPE test only: whether the name resolves to a stored index is
/// the lookup's job.
pub fn is_index_name(value: &str) -> bool {
    // The lookahead: `.` and `..` are the two names the pattern refuses outright
    // (they are the relative path segments of the index's own on-disk directory).
    if value == "." || value == ".." {
        return false;
    }
    let forbidden =
        |ch: char| INDEX_NAME_FORBIDDEN.contains(ch) || ch.is_ascii_uppercase();
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        // The length bound already refuses the empty name; a handler that is handed
        // one anyway refuses it here rather than storing it.
        return false;
    };
    !forbidden(first) && !INDEX_NAME_FORBIDDEN_FIRST.contains(&first) && !characters.any(forbidden)
}

/// `IndexStatus`'s `UPDATED` value — the status `UpdateIndex` reports for the index
/// whose schema it rewrote, the sibling of [`INDEX_STATUS_CREATED`].
pub const INDEX_STATUS_UPDATED: &str = "UPDATED";

/// `IndexStatus`'s `DELETED` value — the status `DeleteIndex` reports for the index
/// it removed from its domain, the third sibling of [`INDEX_STATUS_CREATED`].
pub const INDEX_STATUS_DELETED: &str = "DELETED";

/// One OpenSearch index of a domain, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Index {
    /// `IndexName`. Unique within the domain, so the name IS the key the domain's
    /// map holds it under.
    pub index_name: String,
    /// The `IndexSchema` document this index carries, stored VERBATIM as the
    /// modelled document rather than as a parsed mirror of it: `GetIndex` answers
    /// with "the JSON schema of the index", which is this. The create persists the
    /// document it carried and `UpdateIndex` merges each later change onto it
    /// ([`merged_index_schema`]).
    pub schema: ::aws_smithy_types::Document,
    /// The documents this index holds, keyed by their `_id` — unique within the index,
    /// so the id IS the key. Written by [`ingest::index_document`], the one data-plane
    /// write into an index, and bounded by [`ingest::INDEX_DOCUMENT_QUOTA`]; deleting
    /// the index or its domain releases them.
    pub documents: BTreeMap<String, ingest::IndexedDocument>,
    /// The aliases that point at this index, keyed by the alias name — an alias points at
    /// one index at most once, so the alias's name IS the key. Written by the alias
    /// actions API and reported beside the index's mappings and settings; deleting the
    /// index releases its aliases with it.
    pub aliases: BTreeMap<String, IndexAlias>,
    /// The `_seq_no` the NEXT write into this index takes.
    ///
    /// "The sequence number assigned to the document for the indexing operation":
    /// every write to an index — an index, an update or a delete — takes the next number
    /// of the shard it lands on, counted from 0, and a reader feeds the pair back as
    /// `if_seq_no`/`if_primary_term` to write only if the document has not moved since.
    /// Counted per index, which is the one shard this emulator keeps a document on.
    pub next_seq_no: u64,
}

impl Index {
    /// The `_seq_no` this write takes, moving the index's counter on: every write to an
    /// index takes the next number, so two writes never report one sequence number and a
    /// conditional write can tell the document it read from the one it would replace.
    pub fn take_seq_no(&mut self) -> u64 {
        let taken = self.next_seq_no;
        self.next_seq_no = self.next_seq_no.saturating_add(1);
        taken
    }

    /// The `_seq_no` and `_primary_term` of the document `id`, or `None` where the index
    /// holds no such document — the pair a conditional write is checked against.
    #[must_use]
    pub fn document_revision(&self, id: &str) -> Option<(u64, u64)> {
        self.documents
            .get(id)
            .map(|document| (document.seq_no, document.primary_term))
    }
}

/// One ALIAS of an index: "a virtual index name that can point to one or more indexes".
///
/// Held on the index the alias points AT, because that is what the alias is a name for:
/// the index's own record carries it, so deleting the index takes its aliases with it and
/// no second map can disagree about which indexes an alias names.
#[derive(Debug, Clone)]
pub struct IndexAlias {
    /// The alias's own name, which is how a request addresses it.
    pub alias: String,
    /// `is_write_index`: whether a write addressed to the alias lands in THIS index.
    /// `None` where the action named none, which is what makes a single-index alias
    /// writable and a multi-index one not.
    pub is_write_index: Option<bool>,
    /// The alias action's own members as they were written (`filter`, `routing`, …),
    /// which the alias API reports back.
    pub definition: ::serde_json::Value,
}

/// One COMPONENT TEMPLATE of a domain: "a building block for constructing index templates
/// that specifies index mappings, settings, and aliases".
///
/// A component template configures no index by itself; an index template REFERENCES it
/// through `composed_of`, and the index that template creates carries what the components
/// it names declare, with the template's own section merged on top
/// ([`Domain::templated_schema`]).
#[derive(Debug, Clone)]
pub struct ComponentTemplate {
    /// The component's name — unique on the domain, so the name IS the key.
    pub name: String,
    /// The index configuration it declares, in the same `{"mappings": …, "settings": …}`
    /// form a schema is stored in ([`Index::schema`]).
    pub schema: ::aws_smithy_types::Document,
    /// The component document as it was written, which the component template API reports
    /// back.
    pub definition: ::serde_json::Value,
}

/// One index TEMPLATE of a domain: the schema every index whose name matches one of its
/// patterns is CREATED with.
///
/// "An index template is a way to tell OpenSearch how to configure an index when it is
/// created": a template applies only to an index created AFTER it was written, and never
/// to one that already exists, which is why it is read exactly where an index is minted
/// without a schema of its own ([`Domain::templated_schema`]).
#[derive(Debug, Clone)]
pub struct IndexTemplate {
    /// The template's name — unique on the domain, so the name IS the key.
    pub name: String,
    /// `index_patterns`: the index names this template applies to, each of which may
    /// carry the `*` and `?` wildcards.
    pub index_patterns: Vec<String>,
    /// `priority`: of two templates matching one index name the HIGHER priority wins, and
    /// a template naming none has priority 0.
    pub priority: i64,
    /// `composed_of`: the component templates this one is built from, in the order it
    /// names them — "the component templates are merged in the order specified", so a
    /// later component's section wins over an earlier one's, and the template's own
    /// section wins over all of them ([`Domain::templated_schema`]).
    pub composed_of: Vec<String>,
    /// The schema an index this template applies to is created with, in the same
    /// `{"mappings": …, "settings": …}` form a schema is stored in
    /// ([`Index::schema`]), so a templated index and one a client created carry one shape.
    pub schema: ::aws_smithy_types::Document,
    /// The template document as it was written, which the template API reports back.
    pub definition: ::serde_json::Value,
}

/// The cluster-wide settings of a domain's cluster: the two scopes `_cluster/settings`
/// writes and reports, each a map from a setting's FLAT dotted name to its value.
///
/// Flat because that is the name a setting HAS — `action.auto_create_index` is one key, not
/// a path through three objects — so one spelling is stored however a client wrote it and
/// every reader of a setting looks it up by its name ([`Domain::cluster_setting`]).
///
/// "transient" settings are "lost when the cluster restarts" while persistent ones survive
/// it, and a transient value OVERRIDES the persistent one for the same setting, which is
/// what the lookup implements. A setting written with a null value is REMOVED from its
/// scope, which is how a client resets one.
#[derive(Debug, Clone, Default)]
pub struct ClusterSettings {
    /// The `persistent` scope.
    pub persistent: BTreeMap<String, String>,
    /// The `transient` scope.
    pub transient: BTreeMap<String, String>,
}

/// The two scopes a cluster setting is written in, in the order a lookup consults them:
/// a transient value overrides the persistent one for the same setting.
pub const CLUSTER_SETTING_SCOPES: [&str; 2] = [SETTINGS_TRANSIENT, SETTINGS_PERSISTENT];

/// The `transient` scope's own name on the wire.
pub const SETTINGS_TRANSIENT: &str = "transient";

/// The `persistent` scope's own name on the wire.
pub const SETTINGS_PERSISTENT: &str = "persistent";

/// The setting that decides whether writing a document to an index the cluster does not
/// hold CREATES that index: "automatically create an index if it doesn't already exist".
pub const AUTO_CREATE_INDEX_SETTING: &str = "action.auto_create_index";

/// How many settings one scope may hold before a write is refused — the same kind of bound
/// [`INDEX_QUOTA`] is, for the same reason: a caller looping with fresh names must not grow
/// this process's state without end.
pub const CLUSTER_SETTING_QUOTA: usize = 500;

/// How many ingest pipelines one domain may hold before a write is refused.
pub const INGEST_PIPELINE_QUOTA: usize = 100;

/// How many index-management policies one domain may hold before a write is refused.
pub const ISM_POLICY_QUOTA: usize = 100;

/// One INGEST PIPELINE of a domain: "a series of processors that are to be executed in the
/// same order as they are declared", applied to a document on its way into an index.
#[derive(Debug, Clone)]
pub struct IngestPipeline {
    /// The pipeline's id — unique on the cluster, so the id IS the key.
    pub id: String,
    /// The pipeline document as it was written (`{"description": …, "processors": […]}`),
    /// which the pipeline API reports back and which the write path reads the processors to
    /// run from. Stored as the ONE copy both read, so a pipeline cannot run one set of
    /// processors and report another.
    pub definition: ::serde_json::Value,
}

/// One index-management POLICY of a domain, as the index-management plugin's policy API
/// holds it.
///
/// The policy DOCUMENT is stored as it was written, under the `policy` member the API takes
/// and reports it in; the revision counters beside it are the ones every write of a
/// document to that plugin's own index moves, and are what a concurrent update is checked
/// against.
#[derive(Debug, Clone)]
pub struct IsmPolicy {
    /// The policy's id — unique on the cluster, so the id IS the key.
    pub policy_id: String,
    /// The `policy` object as it was written.
    pub definition: ::serde_json::Value,
    /// `_version`: 1 when the policy was created, bumped by each update of it.
    pub version: i64,
    /// `_seq_no`: the sequence number of the write that last changed the policy, counted
    /// from 0 over the writes to the plugin's own configuration index.
    pub seq_no: i64,
    /// `_primary_term`: the term of the shard that took that write. One cluster run takes
    /// every write in one term, so this stays at its initial value.
    pub primary_term: i64,
    /// When the policy was last written — what `last_updated_time` reports.
    pub updated_at: SystemTime,
}

/// Whether an implicit index creation is permitted, and why not when it is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoCreate {
    /// The setting permits creating this index.
    Allowed,
    /// It does not: the sentence the engine refuses the write with.
    Refused(String),
}

/// Whether `action.auto_create_index` as the cluster holds it lets a write CREATE the index
/// `index_name` names.
///
/// The setting's value is either a boolean — `true` permits every index and `false` none —
/// or "a comma-separated list of patterns you want to allow, or prefix each pattern with +
/// or - to indicate whether it should be allowed or blocked": the FIRST pattern the name
/// matches decides, a bare pattern being an allow, and a name no pattern matches is
/// blocked. A cluster holding no value for the setting permits every index, which is its
/// documented default.
///
/// The refusal sentences are the engine's own for the two cases it distinguishes — the
/// setting switched off, and a name its patterns do not match — each naming the index asked
/// for and, for the pattern case, the value that refused it.
pub fn auto_create_index(settings: Option<&str>, index_name: &str) -> AutoCreate {
    let Some(value) = settings else {
        return AutoCreate::Allowed;
    };
    let value = value.trim();
    match value {
        "true" => return AutoCreate::Allowed,
        "false" => {
            return AutoCreate::Refused(format!(
                "no such index [{index_name}] and [{AUTO_CREATE_INDEX_SETTING}] is [false]"
            ))
        }
        _ => {}
    }
    for pattern in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (allowed, pattern) = match pattern.strip_prefix('-') {
            Some(rest) => (false, rest),
            None => (true, pattern.strip_prefix('+').unwrap_or(pattern)),
        };
        if ::service_common::iam_authorization::wildcard_match(pattern, index_name, true) {
            return if allowed {
                AutoCreate::Allowed
            } else {
                AutoCreate::Refused(format!(
                    "no such index [{index_name}] and [{AUTO_CREATE_INDEX_SETTING}] contains \
                     [-{pattern}] which forbids automatic creation of the index"
                ))
            };
        }
    }
    AutoCreate::Refused(format!(
        "no such index [{index_name}] and [{AUTO_CREATE_INDEX_SETTING}] ([{value}]) doesn't \
         match"
    ))
}

/// The `IndexSchema` section an index's field mappings live in: `UpdateIndex`
/// carries "any changes to mappings, settings, and semantic enrichment
/// configuration", and it is the mapped fields under this one whose types are
/// settled once written.
pub const SCHEMA_MAPPINGS: &str = "mappings";

/// The member a mapped field names its field type in.
pub(super) const MAPPING_TYPE: &str = "type";

/// The members a mapping document nests further field mappings under: an object
/// field's own sub-properties, and a field's additional multi-fields. Neither names
/// a field itself, so a field's path through them is the path of its ancestors and
/// its own name only.
pub(super) const MAPPING_CONTAINERS: [&str; 2] = ["properties", "fields"];

/// How many values ONE index's schema may hold once an update has been merged onto
/// it, before that update is refused.
///
/// Like [`INDEX_QUOTA`] this is the emulator's OWN bound on the store — the model
/// publishes none. A create persists ONE document, so the request boundary bounds
/// what it stores; an update MERGES onto the document the index already carries, so
/// without this a caller looping with fresh members would grow this process's state
/// without end, one bounded request at a time.
pub const INDEX_SCHEMA_QUOTA: usize = 10_000;

/// How many values `schema` holds: the document itself, every member of every named
/// set in it and every element of every list in it.
///
/// What [`INDEX_SCHEMA_QUOTA`] bounds. Iterative rather than recursive, so a deeply
/// nested document costs no stack, and each pending entry is one borrow, so it costs
/// no heap beyond that either.
pub fn index_schema_values(schema: &::aws_smithy_types::Document) -> usize {
    use ::aws_smithy_types::Document;

    let mut values = 0_usize;
    let mut pending = vec![schema];
    while let Some(node) = pending.pop() {
        values = values.saturating_add(1);
        match node {
            Document::Object(members) => pending.extend(members.values()),
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }
    values
}

/// How deeply ONE index's schema may nest, before the request carrying it is refused.
///
/// Like [`INDEX_SCHEMA_QUOTA`] this is the emulator's OWN bound — the model publishes
/// none — and it is what keeps the cost of accepting a document tied to its SIZE: the
/// walks over a schema ([`merged_index_schema`], [`mapping_type_conflict`]) address a
/// member by its path, and this service's own bound on a path's length is what stops a
/// small request naming paths that cost more than the bytes it carried. It sits below
/// the depth the request boundary's own deserializer admits, so this is the bound a
/// caller meets, and comfortably above the nesting a mapping document needs (a search
/// engine's own limit on the object fields one field may nest under is an order of
/// magnitude smaller), so no schema a caller would write reaches it.
pub const INDEX_SCHEMA_MAX_DEPTH: usize = 100;

/// How deeply `schema` nests: 1 for a document carrying no set and no list, one more
/// for each level of nesting under it.
///
/// What [`INDEX_SCHEMA_MAX_DEPTH`] bounds. Iterative rather than recursive, so
/// measuring even a document past the bound costs no stack, and each pending entry is
/// one borrow and one counter.
pub fn index_schema_depth(schema: &::aws_smithy_types::Document) -> usize {
    use ::aws_smithy_types::Document;

    let mut deepest = 0_usize;
    let mut pending = vec![(1_usize, schema)];
    while let Some((depth, node)) = pending.pop() {
        deepest = deepest.max(depth);
        match node {
            Document::Object(members) => {
                pending.extend(members.values().map(|child| (depth.saturating_add(1), child)));
            }
            Document::Array(elements) => {
                pending.extend(elements.iter().map(|child| (depth.saturating_add(1), child)));
            }
            _ => {}
        }
    }
    deepest
}

/// The `ValidationException` payload for an `IndexSchema` nesting deeper than
/// [`INDEX_SCHEMA_MAX_DEPTH`]. It reports both numbers, the same way
/// [`index_schema_over_quota`] does, so a caller knows how much nesting to drop.
pub fn index_schema_too_deep(depth: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "IndexSchema nests {depth} levels deep, more than the maximum of \
             {INDEX_SCHEMA_MAX_DEPTH} for one index."
        )),
    }
}

/// The `ValidationException` payload for an update whose merged schema would hold
/// more values than [`INDEX_SCHEMA_QUOTA`]. It reports both numbers, so a caller
/// knows how much of the update to drop.
pub fn index_schema_over_quota(values: usize) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "IndexSchema would hold {values} values, more than the maximum of \
             {INDEX_SCHEMA_QUOTA} for one index."
        )),
    }
}

/// A field whose mapped type an update would change: the field's dotted path within
/// the index's mappings, the type the index maps it with, and the type the update
/// names for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingTypeConflict {
    /// The field's path under `mappings`, the way a mapping document addresses a
    /// field of an object field (`author.name`).
    pub field: String,
    /// The type the stored schema maps the field with.
    pub from: String,
    /// The type the update names for it.
    pub to: String,
}

/// The mapped field whose type `update` would change against `stored`, or `None`
/// when the update changes no mapped field's type.
///
/// A field's type is settled by the write that first maps it: a later mapping of
/// the same field may add to it but not re-type it. Walking BOTH documents is what
/// makes that a property of the stored schema rather than of one request — a field
/// the stored schema does not map yet has no type to conflict with, and a field the
/// update does not mention is not touched.
///
/// Iterative rather than recursive, so a deeply nested schema costs no stack, and the
/// field path is held as a parent chain rather than on each pending entry, so a
/// pending entry costs one borrow and the whole walk one name per mapping node —
/// never a copy of the path per child. Only the SMALLEST conflict is kept, so the
/// walk's cost does not grow with how many fields conflict. The conflict it reports is
/// the least field path, so the same pair of documents always reports the same one. A
/// field literally named `properties` or `fields` is addressed by its parent's path,
/// since those two names are how a mapping document nests rather than field names.
pub fn mapping_type_conflict(
    stored: &::aws_smithy_types::Document,
    update: &::aws_smithy_types::Document,
) -> Option<MappingTypeConflict> {
    use ::aws_smithy_types::Document;

    let (Document::Object(stored_root), Document::Object(update_root)) = (stored, update) else {
        return None;
    };
    let (Some(Document::Object(stored_mappings)), Some(Document::Object(update_mappings))) = (
        stored_root.get(SCHEMA_MAPPINGS),
        update_root.get(SCHEMA_MAPPINGS),
    ) else {
        return None;
    };

    // The field names walked so far, each entry naming its parent entry: the path of
    // a node is read off this chain ([`mapping_field_path`]) only where a conflict has
    // to be reported, so no pending entry carries a path of its own.
    let mut names: Vec<(&str, Option<usize>)> = Vec::new();
    let mut least: Option<MappingTypeConflict> = None;
    let mut pending = vec![(None, stored_mappings, update_mappings)];
    while let Some((at, stored_node, update_node)) = pending.pop() {
        if at.is_some() {
            if let (Some(from), Some(to)) =
                (stored_node.get(MAPPING_TYPE), update_node.get(MAPPING_TYPE))
            {
                // Compared as documents rather than as type NAMES, so a mapping
                // carrying something other than a name where its type belongs is
                // still settled by the write that first made it rather than
                // rewritten by the next one.
                if from != to {
                    let conflict = MappingTypeConflict {
                        field: mapping_field_path(&names, at),
                        from: mapping_type_label(from),
                        to: mapping_type_label(to),
                    };
                    if least
                        .as_ref()
                        .is_none_or(|best| conflict.field < best.field)
                    {
                        least = Some(conflict);
                    }
                }
            }
        }
        // Descend only where BOTH documents carry a mapping node, which is where a
        // type the stored schema already settled can be contradicted.
        for (name, update_child) in update_node {
            let Document::Object(update_child) = update_child else {
                continue;
            };
            let Some(Document::Object(stored_child)) = stored_node.get(name) else {
                continue;
            };
            let child = if MAPPING_CONTAINERS.contains(&name.as_str()) {
                at
            } else {
                names.push((name.as_str(), at));
                Some(names.len() - 1)
            };
            pending.push((child, stored_child, update_child));
        }
    }
    least
}

/// The dotted path of the mapping node `at` names, read off the parent chain
/// [`mapping_type_conflict`] records its walk in — the way a mapping document
/// addresses a field of an object field (`author.name`).
pub(super) fn mapping_field_path(names: &[(&str, Option<usize>)], at: Option<usize>) -> String {
    let mut path: Vec<&str> = Vec::new();
    let mut cursor = at;
    while let Some(index) = cursor {
        let (name, parent) = names[index];
        path.push(name);
        cursor = parent;
    }
    path.reverse();
    path.join(".")
}

/// How a field's `type` member reads in a refusal: the type name the mapping names,
/// or the value standing where a type name belongs.
pub(super) fn mapping_type_label(value: &::aws_smithy_types::Document) -> String {
    use ::aws_smithy_types::{Document, Number};

    match value {
        Document::String(name) => name.clone(),
        Document::Bool(flag) => flag.to_string(),
        Document::Null => "null".to_string(),
        Document::Number(Number::PosInt(unsigned)) => unsigned.to_string(),
        Document::Number(Number::NegInt(signed)) => signed.to_string(),
        Document::Number(Number::Float(float)) => float.to_string(),
        Document::Array(_) => "a list".to_string(),
        Document::Object(_) => "a set of members".to_string(),
    }
}

/// `stored` with `update` merged onto it — the schema an accepted update leaves the
/// index carrying.
///
/// An update carries "any CHANGES to mappings, settings, and semantic enrichment
/// configuration", so every member it names is written and every member of the
/// stored schema it does not name stays. Only a member whose value is a named set
/// of further members is descended into: a scalar, a null and an array are single
/// values, so one of those replaces whatever the stored schema held there, and an
/// object that names no member changes nothing.
///
/// Iterative rather than recursive, so a deeply nested document costs no stack, and
/// the path of a member is held as a parent chain rather than on each pending entry,
/// so a pending entry costs one borrow and the whole walk one name per member — never
/// a copy of the path per child. With the depth of an accepted document bounded
/// ([`INDEX_SCHEMA_MAX_DEPTH`]) and its value count bounded
/// ([`INDEX_SCHEMA_QUOTA`]), the heap this needs is tied to the size of the request
/// that carried it.
pub fn merged_index_schema(
    stored: &::aws_smithy_types::Document,
    update: &::aws_smithy_types::Document,
) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::Document;

    if !matches!(stored, Document::Object(_)) || !matches!(update, Document::Object(_)) {
        return update.clone();
    }
    let mut merged = stored.clone();
    // The member names walked so far, each entry naming its parent entry: a value's
    // path is read off this chain where the value is written.
    let mut names: Vec<(&str, Option<usize>)> = Vec::new();
    let mut pending: Vec<(Option<usize>, &Document)> = vec![(None, update)];
    while let Some((at, node)) = pending.pop() {
        match node {
            // A named set of members names no value of its own: it is the way to the
            // members under it, so one that names none changes nothing.
            Document::Object(members) => {
                for (name, child) in members {
                    names.push((name.as_str(), at));
                    pending.push((Some(names.len() - 1), child));
                }
            }
            value => {
                let path = member_path(&names, at);
                write_member(&mut merged, &path, value.clone());
            }
        }
    }
    merged
}

/// The path of the member `at` names, root first, read off the parent chain
/// [`merged_index_schema`] records its walk in.
pub(super) fn member_path<'a>(names: &[(&'a str, Option<usize>)], at: Option<usize>) -> Vec<&'a str> {
    let mut path: Vec<&str> = Vec::new();
    let mut cursor = at;
    while let Some(index) = cursor {
        let (name, parent) = names[index];
        path.push(name);
        cursor = parent;
    }
    path.reverse();
    path
}

/// Write `value` at `path` in `document`, as [`merged_index_schema`]'s one writing
/// step. Each node above it that is not a named set of members becomes one, so a
/// nested member is written without dropping the members beside it.
pub(super) fn write_member(
    document: &mut ::aws_smithy_types::Document,
    path: &[&str],
    value: ::aws_smithy_types::Document,
) {
    use ::aws_smithy_types::Document;

    // The root itself is not a member, so a document that names nothing writes
    // nothing.
    let Some((name, ancestors)) = path.split_last() else {
        return;
    };
    let mut node = document;
    for ancestor in ancestors {
        if !matches!(node, Document::Object(_)) {
            *node = Document::Object(::std::collections::HashMap::new());
        }
        let Document::Object(members) = node else {
            return;
        };
        node = members
            .entry((*ancestor).to_string())
            .or_insert_with(|| Document::Object(::std::collections::HashMap::new()));
    }
    if !matches!(node, Document::Object(_)) {
        *node = Document::Object(::std::collections::HashMap::new());
    }
    if let Document::Object(members) = node {
        members.insert((*name).to_string(), value);
    }
}

/// Remove the member at `path` from `document`, as [`with_index_settings`]'s one
/// erasing step — the sibling of [`write_member`].
///
/// A setting written with no value at all is one the index no longer carries, so it is
/// taken OUT of the schema rather than held as a value that is nothing: what the index
/// then reports for it is what the cluster's own default gives it. A path no member of
/// the document stands at removes nothing.
pub(super) fn remove_member(document: &mut ::aws_smithy_types::Document, path: &[&str]) {
    use ::aws_smithy_types::Document;

    // The root itself is not a member, so a document that names nothing removes nothing.
    let Some((name, ancestors)) = path.split_last() else {
        return;
    };
    let mut node = document;
    for ancestor in ancestors {
        let Document::Object(members) = node else {
            return;
        };
        let Some(child) = members.get_mut(*ancestor) else {
            return;
        };
        node = child;
    }
    if let Document::Object(members) = node {
        members.remove(*name);
    }
}

/// The member an index's schema carries its settings in: "mappings, settings, and
/// semantic enrichment configuration" are the sections of an `IndexSchema`, and this is
/// the second of them.
pub const SCHEMA_SETTINGS: &str = "settings";

/// The section INSIDE those settings every index setting has its canonical name under:
/// an index setting is named `index.<setting>`, so `index` is the section the whole of
/// an index's settings live in once they are stored.
pub const SETTINGS_INDEX_SECTION: &str = "index";

/// The settings `schema` carries, as ONE section: every setting under the name it has
/// below [`SETTINGS_INDEX_SECTION`], whichever of the SPELLINGS a caller wrote it in.
///
/// A client writes an index's settings either directly under [`SCHEMA_SETTINGS`]
/// (`{"settings": {"number_of_shards": 2}}`) or under the `index` section inside them
/// (`{"settings": {"index": {"number_of_shards": 2}}}`), and both name the same setting:
/// `index.number_of_shards`. So the two spellings are read as one section here — the one
/// every reader of an index's settings sees — with the explicit spelling standing where
/// both carry a name, and the merge is [`merged_index_schema`]'s so a nested section
/// (`analysis`) written in both spellings keeps the members of each.
///
/// A name is read in either FORM as well: "settings in a flattened or expanded format"
/// name the same setting, so a flattened name — `index.search.slowlog.threshold.query.warn`
/// or `search.slowlog.threshold.query.warn` — is read as the sections it names
/// ([`expanded_settings`]) rather than as one member whose name happens to carry dots.
/// Without that, a setting an index was CREATED with in the flattened form would be stored
/// where no reader of that setting looks, and the index would behave as though it had never
/// been given it.
pub fn index_settings(
    schema: &::aws_smithy_types::Document,
) -> ::serde_json::Map<String, ::serde_json::Value> {
    use ::serde_json::{Map, Value};

    let Some(Value::Object(mut written)) = json_from_document(schema).get(SCHEMA_SETTINGS).cloned()
    else {
        return Map::new();
    };
    let Some(nested) = written.remove(SETTINGS_INDEX_SECTION) else {
        return expanded_settings(written);
    };
    let merged = merged_index_schema(
        &document_from_json(&Value::Object(expanded_settings(written))),
        &document_from_json(&match nested {
            Value::Object(members) => Value::Object(expanded_settings(members)),
            other => other,
        }),
    );
    match json_from_document(&merged) {
        Value::Object(members) => members,
        _ => Map::new(),
    }
}

/// `settings` with every FLATTENED name expanded into the sections that name names: the
/// `index.` prefix an index setting's full name carries dropped ([`index_setting_name`]),
/// and what is left split on the `.` that separates one section from the next — which is
/// the engine's own reading of a setting name, the reading its `flat_settings` form writes
/// one in.
///
/// Applied all the way down, because either spelling may carry the other inside it
/// (`{"index": {"search.slowlog.threshold.query.warn": "0ms"}}`), and two names that expand
/// into one section keep the members of both rather than one replacing the other.
fn expanded_settings(
    settings: ::serde_json::Map<String, ::serde_json::Value>,
) -> ::serde_json::Map<String, ::serde_json::Value> {
    use ::serde_json::{Map, Value};

    let mut expanded = Map::new();
    for (name, value) in settings {
        let value = match value {
            Value::Object(members) => Value::Object(expanded_settings(members)),
            other => other,
        };
        let path: Vec<&str> = index_setting_name(&name)
            .split('.')
            .filter(|section| !section.is_empty())
            .collect();
        written_setting(&mut expanded, &path, value);
    }
    expanded
}

/// Write `value` at `path` in `settings`, as [`expanded_settings`]'s one step: the sections
/// above it are created where they are not there, and a section both the existing value and
/// the new one are a set of members of keeps the members of each.
fn written_setting(
    settings: &mut ::serde_json::Map<String, ::serde_json::Value>,
    path: &[&str],
    value: ::serde_json::Value,
) {
    use ::serde_json::{Map, Value};

    let Some((name, ancestors)) = path.split_last() else {
        return;
    };
    let mut node = settings;
    for ancestor in ancestors {
        let entry = node
            .entry((*ancestor).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        let Value::Object(members) = entry else {
            return;
        };
        node = members;
    }
    match (node.get_mut(*name), value) {
        // Two names that name the same section — one of them written flattened — are the
        // same section, so the members of both stand.
        (Some(Value::Object(held)), Value::Object(members)) => {
            for (name, value) in members {
                written_setting(held, &[name.as_str()], value);
            }
        }
        (_, value) => {
            node.insert((*name).to_string(), value);
        }
    }
}

/// `schema` with the settings `updates` names written into it: the schema an accepted
/// index-settings update leaves the index carrying.
///
/// Each update names ONE setting by the dotted name it has below
/// [`SETTINGS_INDEX_SECTION`] and carries the value it is written with, or no value at
/// all for a setting the update takes out ([`remove_member`]). The settings the schema
/// already held come along as the one section [`index_settings`] reads them as, so an
/// update cannot leave the same setting stored under two spellings; every other section
/// of the schema — the mappings above all — is left exactly as it was.
pub fn with_index_settings(
    schema: &::aws_smithy_types::Document,
    updates: &[(String, Option<::serde_json::Value>)],
) -> ::aws_smithy_types::Document {
    use ::serde_json::Value;

    let mut settings = document_from_json(&Value::Object(index_settings(schema)));
    for (name, value) in updates {
        let path: Vec<&str> = name.split('.').filter(|part| !part.is_empty()).collect();
        match value {
            Some(value) => write_member(&mut settings, &path, document_from_json(value)),
            None => remove_member(&mut settings, &path),
        }
    }
    let mut updated = schema.clone();
    write_member(
        &mut updated,
        &[SCHEMA_SETTINGS, SETTINGS_INDEX_SECTION],
        settings,
    );
    // The canonical section now carries every setting the index has, including the ones
    // the schema held in the direct spelling, so those are dropped rather than left
    // beside it as a second copy of the same setting.
    if let ::aws_smithy_types::Document::Object(members) = &mut updated {
        if let Some(::aws_smithy_types::Document::Object(section)) =
            members.get_mut(SCHEMA_SETTINGS)
        {
            section.retain(|name, _| name == SETTINGS_INDEX_SECTION);
        }
    }
    updated
}

/// The name the setting `written` names has below [`SETTINGS_INDEX_SECTION`]: an index
/// setting may be written with its `index.` prefix or without it, and both name the same
/// setting, so the prefix is dropped where a request carried it.
pub fn index_setting_name(written: &str) -> &str {
    written
        .strip_prefix(SETTINGS_INDEX_SECTION)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(written)
}

/// The `IndexSchema` section an index's analysis settings live in: the tokenizers,
/// filters and analyzers the domain's cluster builds the index's text pipeline from.
///
/// Looked for wherever it appears rather than at one fixed depth, because the same
/// settings reach the cluster both directly under the index's settings and under the
/// `index` section inside them.
pub(super) const SCHEMA_ANALYSIS: &str = "analysis";

/// The suffix of an analysis setting that names its content BY PATH rather than inline
/// (`synonyms_path`, `stopwords_path`, `rules_path`, …): the form every such file
/// reference takes in the engine's own settings, which is why the suffix rather than a
/// hand-written list of setting names is what identifies one.
pub(super) const PATH_SETTING_SUFFIX: &str = "_path";

/// One file an index's analysis settings resolve by path on the cluster nodes, as the
/// index names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageReference {
    /// The analysis setting that carried the path (`synonyms_path`) — what a refusal
    /// names, because it is the setting whose file the cluster could not read.
    pub setting: String,
    /// The path the setting carries, relative to the cluster nodes' package directory
    /// — the same form as [`package_reference_path`].
    pub path: String,
    /// The `PackageID` the path names under the analyzers directory: the package that
    /// has to be in place on the domain for the file to be there to read.
    pub package_id: String,
}

impl PackageReference {
    /// Whether this reference resolves content the package `package_id` names provides:
    /// the package's own directory, or a file inside it.
    pub fn names_package(&self, package_id: &str) -> bool {
        self.package_id == package_id
    }
}

/// Every sub-document of `schema` that a member named `name` carries, wherever in the
/// schema that member appears.
///
/// ONE statement of the walk every reading of a named schema section makes — the
/// analysis settings ([`package_references`]) and the field mappings
/// ([`crate::state::mapping_analyzer_references`], [`crate::state::configured_analyzers`])
/// — so where a section is looked for is decided in one place. `descend` says whether a
/// section nested INSIDE a matched one is reported as a section of its own: a reading that
/// then walks the whole matched subtree needs it false, one that reads only each section's
/// own members needs it true. Iterative rather than recursive, so a deeply nested document
/// costs no stack, and each pending entry is one borrow.
pub(super) fn sections_named<'a>(
    schema: &'a ::aws_smithy_types::Document,
    name: &str,
    descend: bool,
) -> Vec<&'a ::aws_smithy_types::Document> {
    use ::aws_smithy_types::Document;

    let mut found: Vec<&Document> = Vec::new();
    let mut pending = vec![schema];
    while let Some(node) = pending.pop() {
        match node {
            Document::Object(members) => {
                for (member, child) in members {
                    if member == name {
                        found.push(child);
                        if !descend {
                            continue;
                        }
                    }
                    pending.push(child);
                }
            }
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }
    found
}

/// Every package file `schema`'s analysis settings name by path.
///
/// An index's analysis settings reference a custom dictionary package's content BY the
/// relative path the package is mounted at ([`package_reference_path`]), so this walks
/// the schema's analysis sections and reads out each path setting that names one.
/// Ordered by setting name, so the same document always reports the same first
/// reference, and iterative rather than recursive, so a deeply nested document costs no
/// stack.
pub fn package_references(schema: &::aws_smithy_types::Document) -> Vec<PackageReference> {
    use ::aws_smithy_types::Document;

    // The analysis sections first: a path setting outside one names no analyzer's
    // content, so only what is under them is read out. Each matched section is walked
    // whole below, so one nested inside another is not reported separately.
    let analysis = sections_named(schema, SCHEMA_ANALYSIS, false);

    let mut references: Vec<PackageReference> = Vec::new();
    let mut pending = analysis;
    while let Some(node) = pending.pop() {
        match node {
            Document::Object(members) => {
                for (name, child) in members {
                    match child {
                        Document::String(value) if name.ends_with(PATH_SETTING_SUFFIX) => {
                            if let Some(package_id) = referenced_package(value) {
                                references.push(PackageReference {
                                    setting: name.clone(),
                                    path: value.clone(),
                                    package_id,
                                });
                            }
                        }
                        other => pending.push(other),
                    }
                }
            }
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }
    references.sort_by(|left, right| left.setting.cmp(&right.setting));
    references
}

/// The `PackageID` `path` names under the analyzers directory, or `None` where the path
/// names no package's content at all.
///
/// The documented form of such a reference is the analyzers directory followed by the
/// package's id ([`package_reference_path`]), optionally followed by a file inside it,
/// so the id is the segment after the directory.
pub(super) fn referenced_package(path: &str) -> Option<String> {
    let rest = path.strip_prefix(ANALYZERS_DIRECTORY)?.strip_prefix('/')?;
    let package_id = rest.split('/').next()?;
    (!package_id.is_empty()).then(|| package_id.to_string())
}

/// The `ValidationException` payload for an index whose analysis settings name a file
/// the cluster cannot read, because the domain holds no association in place with the
/// package that provides it.
///
/// The cluster builds the index's analysis pipeline by READING each file its settings
/// name, so the refusal is the failure to read that one setting's file, reported the way
/// the engine reports it: the setting it was reading, followed by the suffix the engine
/// appends to whatever setting it was handed, and what went wrong. The managed layer
/// hands it the member key it found in the schema, so a member a request carries as
/// `<name>_path` is reported as `<name>_path_path` — the engine's own formatting applied
/// to the request's own member name. The only observed sentence of this class is for a
/// synonym filter's path setting; whether the managed layer hands the engine the member
/// key for EVERY path setting is not established, so the wording for the other path
/// settings follows from this one derivation rather than from an observation of each.
pub fn package_file_not_readable(setting: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "IOException while reading {setting}{PATH_SETTING_SUFFIX}: file not readable"
        )),
    }
}

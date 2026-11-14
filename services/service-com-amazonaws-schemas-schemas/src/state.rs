// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-schemas-schemas service.
//!
//! This is the service's single store: the registry → schema → schema-version tree
//! every handler reads and writes. It also holds the few helpers more than one
//! handler needs (ARN synthesis, the request's region, tag conversion, listing
//! pagination and the error payloads), because `src/lib.rs` and `src/ops/mod.rs` are
//! generated and cannot declare another module.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BadRequestException, ConflictException, ForbiddenException, InternalServerErrorException,
    NotFoundException, PreconditionFailedException, TYPE_VALUES,
};

/// Per-service state for com-amazonaws-schemas-schemas. A handler reaches it through [`service_state`].
///
/// `BTreeMap` rather than `HashMap`: every listing operation this service models is
/// `@paginated` with an opaque `NextToken`, and a token can only resume a page
/// deterministically if the underlying order is stable. `BTreeMap`'s key order is.
#[derive(Debug)]
pub struct ComAmazonawsSchemasSchemasState {
    /// Registries by `RegistryName`, of both scopes: the ones callers create and the
    /// service-owned catalogue [`aws_managed_schemas`] seeds.
    pub registries: BTreeMap<String, Registry>,
    /// Source of resource-policy revision ids, so every revision this process mints
    /// is distinct. See [`ComAmazonawsSchemasSchemasState::mint_policy_revision_id`].
    pub next_policy_revision_seq: u64,
    /// Discoverers by `DiscovererId`, as `CreateDiscoverer` derived it from the source
    /// they subscribe to ([`discoverer_id_for_source`]). A discoverer belongs to no
    /// registry — it is keyed by that id, which is what every other discoverer
    /// operation names it by.
    pub discoverers: BTreeMap<String, Discoverer>,
}

impl ComAmazonawsSchemasSchemasState {
    /// Mint the next resource-policy revision id.
    ///
    /// A revision id identifies ONE revision of a policy: it must differ from the
    /// revision it replaces (otherwise a stale-revision precondition could not be
    /// detected) and must stay put until the next write. It is drawn from a
    /// store-wide counter rather than at random, so a process's ids are
    /// deterministic and no two policies can ever share one, and rendered in the
    /// 8-4-4-4-12 shape AWS opaque ids use (version nibble 4, variant nibble 8, as
    /// RFC 4122 lays them out) so a client sees the form it expects.
    pub fn mint_policy_revision_id(&mut self) -> String {
        self.next_policy_revision_seq = self.next_policy_revision_seq.saturating_add(1);
        let seq = self.next_policy_revision_seq;
        format!("{:08x}-0000-4000-8000-{seq:012x}", seq & 0xffff_ffff)
    }

    /// The tags this store holds for the resource a `ResourceArn` names, as the
    /// resource reports its ARN to a request signed for `region`.
    ///
    /// A resource is identified by the ARN this service ISSUES for it, not by its name
    /// alone: a caller's registry, schema or discoverer by the ARN its create minted,
    /// and a service-owned registry by the ARN it reports to the asking region
    /// ([`Registry::arn_in`]). So an ARN this service never issued — another account's,
    /// another region's, or one of a service-owned SCHEMA, which carries no ARN at all
    /// ([`Registry::schema_arn_in`]) — names no resource here.
    ///
    /// `Err` keeps the two refusals apart: a value that is no resource ARN of this
    /// service, and one that is but names nothing this store holds.
    pub fn tags_for_arn(
        &self,
        arn: &str,
        region: &str,
    ) -> Result<&BTreeMap<String, String>, ResourceArnError> {
        let held = match parse_resource_arn(arn).ok_or(ResourceArnError::NotAResourceArn)? {
            TaggedResource::Registry { registry } => self
                .registries
                .get(&registry)
                .filter(|held| held.arn_in(region) == arn)
                .map(|held| &held.tags),
            TaggedResource::Schema { registry, schema } => {
                self.registries.get(&registry).and_then(|held| {
                    let schema = held.schemas.get(&schema)?;
                    (held.schema_arn_in(region, schema).as_deref() == Some(arn))
                        .then_some(&schema.tags)
                })
            }
            TaggedResource::Discoverer { discoverer_id } => self
                .discoverers
                .get(&discoverer_id)
                .filter(|held| held.arn == arn)
                .map(|held| &held.tags),
        };
        held.ok_or(ResourceArnError::NoSuchResource)
    }

    /// The tag set a caller's WRITE reaches through a `ResourceArn`, as the resource
    /// reports its ARN to a request signed for `region`.
    ///
    /// Which resource an ARN names is the READ's answer — this resolves through
    /// [`Self::tags_for_arn`], so a write cannot reach a resource a read of the same
    /// ARN would not, and the two can never disagree about which ARN names what.
    ///
    /// What a write adds is the one refusal a read does not have: what is in a
    /// service-owned registry is AWS's, so a caller may read it but not write to it —
    /// the same refusal every other write to one gets. A service-owned SCHEMA needs
    /// no case here: it carries no ARN at all ([`Registry::schema_arn_in`]), so no
    /// ARN names one and the resolve above already answers that nothing is there.
    pub fn writable_tags_for_arn(
        &mut self,
        arn: &str,
        region: &str,
    ) -> Result<&mut BTreeMap<String, String>, ResourceArnWriteError> {
        // Resolve as a read does, and refuse what a read refuses: this is what makes
        // the write reach the same tag set a later read of the same ARN reports.
        self.tags_for_arn(arn, region)?;
        let resource = parse_resource_arn(arn).ok_or(ResourceArnWriteError::NotAResourceArn)?;
        // The registry whose scope says whether AWS or the caller owns what the ARN
        // names: the registry itself, or the registry a schema belongs to.
        let owning_registry = match &resource {
            TaggedResource::Registry { registry } | TaggedResource::Schema { registry, .. } => {
                Some(registry.clone())
            }
            TaggedResource::Discoverer { .. } => None,
        };
        if let Some(registry) = owning_registry {
            if self.registry_is_service_owned(&registry) {
                return Err(ResourceArnWriteError::ServiceOwned(registry));
            }
        }
        // The resolve above already established that this store holds the resource
        // this ARN names, so these lookups answer; they are fallible only because a
        // map lookup is.
        let held = match resource {
            TaggedResource::Registry { registry } => self
                .registries
                .get_mut(&registry)
                .map(|held| &mut held.tags),
            TaggedResource::Schema { registry, schema } => self
                .registries
                .get_mut(&registry)
                .and_then(|held| held.schemas.get_mut(&schema))
                .map(|held| &mut held.tags),
            TaggedResource::Discoverer { discoverer_id } => self
                .discoverers
                .get_mut(&discoverer_id)
                .map(|held| &mut held.tags),
        };
        held.ok_or(ResourceArnWriteError::NoSuchResource)
    }

    /// Whether the registry of this name is one whose contents AWS owns.
    ///
    /// The one place the scope of a stored registry is read to decide ownership, so
    /// every write that asks the question gets the same answer. A name this store
    /// holds no registry for is not owned by AWS — it names no registry at all, which
    /// is a different refusal ([`Self::writable_registry`]).
    fn registry_is_service_owned(&self, name: &str) -> bool {
        self.registries
            .get(name)
            .is_some_and(|held| held.scope.is_managed())
    }

    /// The registry a caller's WRITE names, resolved for mutation, or why the write
    /// reaches none.
    ///
    /// Every write this service serves that changes what a registry HOLDS — the
    /// registry itself, a schema or schema version in it, its tags or its
    /// resource-based policy — takes the same two steps before it changes anything: the
    /// store must hold a registry of that name, and what is in a service-owned registry
    /// is AWS's, so a caller may read it but not write to it. Resolving and refusing
    /// together is what keeps the second step from being forgotten by a handler that
    /// remembers the first.
    ///
    /// A call that changes nothing the registry holds does not resolve through here, and
    /// `PutCodeBinding` is the one such call this service serves: the code it starts
    /// generating is derived FROM a schema version it only reads, and the binding is the
    /// asking caller's own artifact rather than part of the registry's content. That is
    /// why code generation is served for the schemas of a service-owned registry —
    /// generating bindings for the published AWS event schemas is what the code-binding
    /// operations are for — and why that put resolves through
    /// [`Self::schema_at_version_mut`], deliberately and not by omission.
    ///
    /// The refusals are RETURNED rather than worded here, because each operation
    /// answers them through its own modelled error enum; the sentences are the
    /// service's constants ([`registry_not_found`] and
    /// [`managed_registry_is_read_only`]), so every call site words them alike. This is
    /// the registry-by-name counterpart of [`Self::writable_tags_for_arn`], which
    /// resolves the same write permission through a `ResourceArn`.
    pub fn writable_registry(&mut self, name: &str) -> Result<&mut Registry, RegistryWriteError> {
        if !self.registries.contains_key(name) {
            return Err(RegistryWriteError::NoSuchRegistry(name.to_string()));
        }
        if self.registry_is_service_owned(name) {
            return Err(RegistryWriteError::ServiceOwned(name.to_string()));
        }
        Ok(self
            .registries
            .get_mut(name)
            .expect("the registry resolved just above is still present"))
    }

    /// The registry a caller's READ names, or why the read reaches none.
    ///
    /// The first step of the resolution every read that names a resource INSIDE a
    /// registry makes ([`Self::readable_schema_version`]), and the whole of it for a
    /// read of the registry itself: the store must hold a registry of that name.
    /// Unlike [`Self::writable_registry`] it asks nothing about who OWNS the registry —
    /// what AWS owns, a caller may read.
    pub fn readable_registry(&self, name: &str) -> Result<&Registry, ReadResolutionError> {
        self.registries
            .get(name)
            .ok_or_else(|| ReadResolutionError::NoSuchRegistry(name.to_string()))
    }

    /// The schema a caller's read names, together with the registry holding it, or
    /// which step of the resolution reaches nothing.
    ///
    /// The first two steps of [`Self::readable_schema_version`], for a read that names a
    /// schema but no version of it.
    pub fn readable_schema(
        &self,
        registry_name: &str,
        schema_name: &str,
    ) -> Result<(&Registry, &Schema), ReadResolutionError> {
        let registry = self.readable_registry(registry_name)?;
        let schema = registry.schema(schema_name)?;
        Ok((registry, schema))
    }

    /// The schema VERSION a caller's read names, or which step of the resolution
    /// reaches nothing.
    ///
    /// Every read this service serves that names a schema version — the stored
    /// definition, its export, a code binding generated from it — takes the same three
    /// steps before it answers anything: the store must hold a registry of that name,
    /// that registry must hold a schema of that name, and the schema must hold the
    /// version the request's `SchemaVersion` names, or its latest stored version when
    /// the request names none ([`Schema::version_for`]). Resolving the three together,
    /// in that order, is what keeps a handler from answering one step's refusal for
    /// another step's failure.
    ///
    /// `SchemaVersion` is an OPTIONAL member that limits the read to one version, so a
    /// request that sends it with no characters in it limits the read to no version at
    /// all and is served as the request that omits it ([`specified`]): the latest
    /// stored version answers, and a refusal this resolution does reach names no
    /// version either.
    ///
    /// The refusals are RETURNED rather than worded here, because each operation answers
    /// them through its own modelled error enum; the sentences are the service's
    /// constants ([`ReadResolutionError::not_found`]), so every call site words them
    /// alike. This is the read counterpart of [`Self::writable_registry`].
    pub fn readable_schema_version(
        &self,
        registry_name: &str,
        schema_name: &str,
        requested: Option<&str>,
    ) -> Result<ResolvedSchemaVersion<'_>, ReadResolutionError> {
        let requested = specified(requested);
        let (registry, schema) = self.readable_schema(registry_name, schema_name)?;
        let version = schema.version_for(requested).ok_or_else(|| {
            ReadResolutionError::NoSuchSchemaVersion {
                schema: schema_name.to_string(),
                requested: requested.map(str::to_string),
            }
        })?;
        Ok(ResolvedSchemaVersion {
            registry,
            schema,
            version,
        })
    }

    /// The same three-step resolution, for a call that then CHANGES something attached
    /// to the resolved version (a code binding generated from it): the schema for
    /// mutation, and the version number the request resolved to.
    ///
    /// The NUMBER is handed back rather than a borrow of the version, because the schema
    /// is borrowed mutably; it is resolved before anything is written, so a version
    /// added later cannot re-point what this call attaches.
    ///
    /// The second lookup below is how a shared resolution is reused under a mutable
    /// borrow, not an assumption about the store: the caller holds the one state lock
    /// across both, so nothing can have removed what the first lookup resolved — the
    /// same reason [`Self::writable_registry`] re-fetches what it has just resolved.
    ///
    /// It asks nothing about who OWNS the registry, and that is deliberate: the only
    /// thing a caller attaches through this resolution is a code binding, which is code
    /// generated from the resolved version for the caller who asked and leaves the
    /// schema, its versions, its metadata and the registry exactly as they were. A
    /// caller may generate bindings for every schema it may READ, the published AWS
    /// event schemas included, so a write that CHANGES a registry's content resolves
    /// through [`Self::writable_registry`] and this one must not.
    pub fn schema_at_version_mut(
        &mut self,
        registry_name: &str,
        schema_name: &str,
        requested: Option<&str>,
    ) -> Result<(&mut Schema, u64), ReadResolutionError> {
        let version = self
            .readable_schema_version(registry_name, schema_name, requested)?
            .version
            .version;
        let schema = self
            .registries
            .get_mut(registry_name)
            .and_then(|registry| registry.schemas.get_mut(schema_name))
            .expect("the schema resolved just above is still present");
        Ok((schema, version))
    }
}

/// A fresh store already holds the service-owned registries: they exist in every
/// account before any call is made, so they are part of what "empty" means here.
/// Only [`RegistryScope::Local`] registries are a caller's.
impl Default for ComAmazonawsSchemasSchemasState {
    fn default() -> Self {
        Self {
            registries: aws_managed_registries(),
            next_policy_revision_seq: 0,
            // No discoverer exists until a caller creates one: AWS seeds none.
            discoverers: BTreeMap::new(),
        }
    }
}

/// Who owns a registry — what `ListRegistries`' `Scope` selects between.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RegistryScope {
    /// A caller's own registry, created through `CreateRegistry`.
    #[default]
    Local,
    /// A service-owned registry, present before any call: AWS owns its contents, so a
    /// caller may read it but not write to it.
    Aws,
}

impl RegistryScope {
    /// Whether AWS, rather than the caller, owns what is in this registry.
    pub fn is_managed(self) -> bool {
        matches!(self, Self::Aws)
    }
}

/// A schema registry, as `CreateRegistry` persisted it or as the service-owned
/// catalogue seeded it.
#[derive(Debug, Clone)]
pub struct Registry {
    pub name: String,
    /// `arn:aws:schemas:<region>:<account>:registry/<name>` as minted at create time,
    /// for a registry a caller created. Read it through [`Registry::arn_in`], which is
    /// what a service-owned registry — created in no single request, so signed for no
    /// single region — needs.
    pub arn: String,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// Whether AWS or the caller owns this registry.
    pub scope: RegistryScope,
    /// Schemas by `SchemaName`.
    pub schemas: BTreeMap<String, Schema>,
    /// The resource-based policy attached to this registry, as `PutResourcePolicy`
    /// last wrote it; `None` for a registry no policy is attached to. It lives on
    /// the registry because the policy is a property OF the registry: deleting the
    /// registry takes its policy with it.
    pub policy: Option<ResourcePolicy>,
}

/// A registry's resource-based policy and the revision it is at.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// The policy document, as [`canonical_json`] encodes what the caller sent.
    pub document: String,
    /// The revision this policy is at, minted by
    /// [`ComAmazonawsSchemasSchemasState::mint_policy_revision_id`] on every write.
    pub revision_id: String,
}

/// Whether a discoverer is discovering schemas from its source, or is paused.
///
/// The two states `DiscovererState` models ([`crate::ops::types::DISCOVERER_STATE_VALUES`]);
/// stored as this enum rather than as the wire string so no other value can ever be
/// persisted into the member. There is no transient state between the two: AWS answers a
/// discoverer `STARTED` or `STOPPED` and nothing in between, so a start or a stop is
/// complete when it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscovererState {
    /// Discovering schemas from the source — the state a created discoverer is in.
    Started,
    /// Paused by `StopDiscoverer`: the discoverer still exists and still names its
    /// source, and discovers nothing until it is started again.
    Stopped,
}

impl DiscovererState {
    /// This state as `DiscovererState` renders it on the wire — one of
    /// [`crate::ops::types::DISCOVERER_STATE_VALUES`].
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Started => "STARTED",
            Self::Stopped => "STOPPED",
        }
    }
}

/// A discoverer, as `CreateDiscoverer` persisted it and as every later discoverer
/// operation reads and writes it.
#[derive(Debug, Clone)]
pub struct Discoverer {
    /// The id the service gave this discoverer, derived from the source it subscribes
    /// to ([`discoverer_id_for_source`]); the key it is stored under and the name every
    /// id-addressed discoverer operation reaches it by.
    pub id: String,
    /// `arn:aws:schemas:<region>:<account>:discoverer/<id>`, minted at create time.
    pub arn: String,
    /// The event source this discoverer discovers schemas from, as the request named it.
    pub source_arn: String,
    pub description: Option<String>,
    /// Whether the discoverer is discovering or paused — what `StartDiscoverer` and
    /// `StopDiscoverer` move.
    pub state: DiscovererState,
    /// Whether schemas of events another account sent to the source are discovered too.
    pub cross_account: bool,
    pub tags: BTreeMap<String, String>,
}

/// The bytes a policy document is stored and reported as: the document re-encoded as
/// JSON, with its insignificant whitespace gone and its member order kept.
///
/// A resource policy travels as an opaque JSON DOCUMENT (`Policy` is modelled
/// `@mediaType("application/json")`), and the service that takes one parses it and
/// writes it back out: what a later read reports is the document's JSON content, not
/// the byte-for-byte text the caller's renderer happened to produce. So a document
/// indented or spaced by one caller reads back the same as the same document sent
/// compact by another, and a caller that hashes or diffs a policy round-trip — the
/// standard way to detect drift — sees no change.
///
/// Encoded by streaming the reader straight into the writer, so the members keep the
/// order the caller wrote them in; an intermediate [`serde_json::Value`] would sort
/// them, which would reorder a document rather than re-encode it.
///
/// What a re-encode does NOT keep is the LITERAL a number was written as: a number
/// reaches the writer as the value the reader read, so `1e2` comes back `100.0` and a
/// literal with more digits than a double holds comes back rounded to one. The VALUE a
/// policy denotes is kept; its spelling is not. Keeping the spelling too would mean
/// turning on `serde_json`'s arbitrary-precision mode, which is a feature of the crate
/// as a whole and would change how every other service in this workspace reads and
/// writes a JSON number — a far larger change than a policy's digits are worth, and one
/// no recording asks for (a resource-based policy is statements of strings; the fixed
/// point `tests/all/resource_policy.rs` pins holds for every form a policy actually
/// carries).
///
/// `Err` carries what made the value unreadable, for a value that is not JSON at
/// all — the service's own diagnostic for an unreadable body, as
/// [`json_parse_detail`] builds it.
pub fn canonical_json(document: &str) -> Result<String, String> {
    let mut encoded: Vec<u8> = Vec::with_capacity(document.len());
    let mut reader = ::serde_json::Deserializer::from_str(document);
    {
        let mut writer = ::serde_json::Serializer::new(&mut encoded);
        ::serde_transcode::transcode(&mut reader, &mut writer)
            .map_err(|error| json_parse_detail(document, &error))?;
    }
    // One document, not a stream of them: anything after the first value makes the
    // value unreadable rather than being silently dropped.
    reader
        .end()
        .map_err(|error| json_parse_detail(document, &error))?;
    String::from_utf8(encoded).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// A document whose member order is part of the document
//
// A schema document is an opaque TEXT this service stores and serves: `Content` is
// modelled `__string`, so a read answers the document's own bytes — its members in the
// order the published document writes them. A client caches the document it fetched and
// diffs or hashes it against the next read, and a code generator renders the schemas in
// the order it reads them, so a document handed back with its members rearranged is a
// different document even though it denotes the same schema.
//
// [`serde_json::Map`] cannot state that order: it is a SORTED map or an
// insertion-ordered one depending on whether anything in the BUILD asks for
// `serde_json`'s `preserve_order` feature — a workspace-wide switch, so a document
// composed through [`serde_json::Value`] does not even render the same way in two builds
// of this emulator. Hence this node: its object members keep the order they were written
// or read in, whatever else is in the build, and are serialized in it. `canonical_json`
// above is the same decision for a document this service only passes through, and
// `GetDiscoveredSchema` writes the document it infers through this node too.
// ---------------------------------------------------------------------------

/// One JSON value of a document whose object member ORDER is part of the document.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonNode {
    /// JSON `null`.
    Null,
    /// `true` or `false`.
    Flag(bool),
    /// A JSON number, as the value it denotes.
    Number(::serde_json::Number),
    /// A JSON string.
    Text(String),
    /// A JSON array, in its own order.
    List(Vec<JsonNode>),
    /// A JSON object, its members in the order they were written or read in.
    Object(Vec<(String, JsonNode)>),
}

impl JsonNode {
    /// A JSON string node.
    pub fn text(value: &str) -> Self {
        Self::Text(value.to_string())
    }

    /// This node as the JSON text that goes on the wire.
    ///
    /// Infallible in practice: the only failures `serde_json` reports here are a
    /// non-string map key and a writer error, and this node's keys are `String` and the
    /// writer is an in-memory buffer.
    pub fn to_json(&self) -> String {
        ::serde_json::to_string(self).expect("a JsonNode serializes into a String buffer")
    }

    /// `document` read as this node: every object's members keep the order the document
    /// writes them in. `None` where `document` is not one JSON value.
    pub fn parse(document: &str) -> Option<Self> {
        ::serde_json::from_str(document).ok()
    }

    /// The same value as `value`, each object's members in the order
    /// [`serde_json::Value`] happens to hold them in. For a scalar there is nothing to
    /// order; for an object or an array, order the members deliberately instead of
    /// relying on this.
    pub fn from_value(value: &::serde_json::Value) -> Self {
        match value {
            ::serde_json::Value::Null => Self::Null,
            ::serde_json::Value::Bool(flag) => Self::Flag(*flag),
            ::serde_json::Value::Number(number) => Self::Number(number.clone()),
            ::serde_json::Value::String(text) => Self::Text(text.clone()),
            ::serde_json::Value::Array(items) => {
                Self::List(items.iter().map(Self::from_value).collect())
            }
            ::serde_json::Value::Object(members) => Self::Object(
                members
                    .iter()
                    .map(|(name, member)| (name.clone(), Self::from_value(member)))
                    .collect(),
            ),
        }
    }

    /// The member named `name`, for a node that is an object.
    pub fn member(&self, name: &str) -> Option<&Self> {
        match self {
            Self::Object(members) => members
                .iter()
                .find(|(written, _)| written == name)
                .map(|(_, member)| member),
            _ => None,
        }
    }

    /// Write `name` as this object's LAST member, replacing any member it already has
    /// under that name — a JSON object carries one member per name. Does nothing to a
    /// node that is not an object.
    pub fn set_member(&mut self, name: &str, value: Self) {
        if let Self::Object(members) = self {
            members.retain(|(written, _)| written != name);
            members.push((name.to_string(), value));
        }
    }
}

impl ::serde::Serialize for JsonNode {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use ::serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Flag(flag) => serializer.serialize_bool(*flag),
            Self::Number(number) => number.serialize(serializer),
            Self::Text(text) => serializer.serialize_str(text),
            Self::List(items) => {
                let mut sequence = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    sequence.serialize_element(item)?;
                }
                sequence.end()
            }
            Self::Object(members) => {
                let mut map = serializer.serialize_map(Some(members.len()))?;
                for (name, value) in members {
                    map.serialize_entry(name, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> ::serde::Deserialize<'de> for JsonNode {
    fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonNodeVisitor)
    }
}

/// Reads one JSON value into a [`JsonNode`], an object's members in document order:
/// `serde_json`'s reader hands them over in that order, and this keeps them in it
/// rather than inserting them into a sorted map.
struct JsonNodeVisitor;

impl<'de> ::serde::de::Visitor<'de> for JsonNodeVisitor {
    type Value = JsonNode;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E: ::serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(JsonNode::Null)
    }

    fn visit_none<E: ::serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(JsonNode::Null)
    }

    fn visit_bool<E: ::serde::de::Error>(self, flag: bool) -> Result<Self::Value, E> {
        Ok(JsonNode::Flag(flag))
    }

    fn visit_i64<E: ::serde::de::Error>(self, number: i64) -> Result<Self::Value, E> {
        Ok(JsonNode::Number(number.into()))
    }

    fn visit_u64<E: ::serde::de::Error>(self, number: u64) -> Result<Self::Value, E> {
        Ok(JsonNode::Number(number.into()))
    }

    fn visit_f64<E: ::serde::de::Error>(self, number: f64) -> Result<Self::Value, E> {
        ::serde_json::Number::from_f64(number)
            .map(JsonNode::Number)
            .ok_or_else(|| E::custom(format!("{number} is not a JSON number")))
    }

    fn visit_str<E: ::serde::de::Error>(self, text: &str) -> Result<Self::Value, E> {
        Ok(JsonNode::text(text))
    }

    fn visit_seq<A: ::serde::de::SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = sequence.next_element()? {
            items.push(item);
        }
        Ok(JsonNode::List(items))
    }

    fn visit_map<A: ::serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> Result<Self::Value, A::Error> {
        let mut node = JsonNode::Object(Vec::new());
        while let Some((name, value)) = map.next_entry::<String, JsonNode>()? {
            // One member per name, the last spelling of it winning — what a sorted map
            // would do too, and the only part of this a document cannot state twice.
            node.set_member(&name, value);
        }
        Ok(node)
    }
}

// ---------------------------------------------------------------------------
// Where a JSON body stops being JSON
//
// Every refusal this service gives for a body it could not read — `CreateSchema`'s
// and `UpdateSchema`'s `Content`, `PutResourcePolicy`'s `Policy` — says WHERE the
// body stopped being JSON, and says it the way the service does: by naming the
// OFFENDING CHARACTER, what was being read when the parser reached it, and that
// character's BYTE OFFSET into the body the caller sent
// (`invalid character 'o' in literal null (expecting 'u') at offset 2`).
//
// The OFFSET is the count of bytes the parser had consumed when it reached the
// offending character — so the 1-based byte position of that character, which is what
// the recorded refusals report: the instance above names the second byte of its body
// as `offset 2`.
//
// The offset is the only part of such a refusal that tells a client WHERE in its own
// document the defect is, and a byte position in the submitted bytes stays meaningful
// whatever shape the document has — one line or a thousand. `serde_json`, the parser
// this emulator reads JSON with, reports a LINE and COLUMN in its own vocabulary
// instead (`expected ident at line 1 column 2`): a different coordinate system,
// naming a different place in any multi-line document, in words no client of this API
// is written against. So `serde_json` stays what decides whether a body is VALID
// JSON, and the position and wording a refusal carries are produced here.
//
// The scan below walks the bytes with an explicit stack rather than by recursion: a
// body is bounded (`Content` is modelled at most 100000 bytes) but its NESTING is
// not, and a hundred thousand open brackets must not be able to exhaust the stack.
// ---------------------------------------------------------------------------

/// Where a JSON document stops being JSON, as a refusal reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonFault {
    /// The offset the refusal reports: the number of bytes read when the offending
    /// character was reached, which is that character's 1-based byte position in the
    /// document. `None` where the document simply ran out: then there is no character
    /// to point a client at.
    pub offset: Option<usize>,
    /// What was wrong there: the offending character and what was being read.
    pub detail: String,
}

impl ::std::fmt::Display for JsonFault {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self.offset {
            Some(offset) => write!(f, "{} at offset {offset}", self.detail),
            None => write!(f, "{}", self.detail),
        }
    }
}

/// What this service says about a body `serde_json` refused: the offending character
/// and its byte offset, as [`json_fault`] finds them.
///
/// `serde_json` decides VALIDITY; this decides the wording. Where the scan finds no
/// fault at all the document is one well-formed JSON value that `serde_json` still
/// refused — it holds itself to a nesting limit this scan does not — so the position
/// `serde_json` names is converted to a byte offset in the same coordinates and
/// reported without its line/column vocabulary.
pub fn json_parse_detail(document: &str, error: &::serde_json::Error) -> String {
    match json_fault(document) {
        Some(fault) => fault.to_string(),
        None => format!(
            "invalid JSON at offset {}",
            byte_offset_of(document, error.line(), error.column()).saturating_add(1)
        ),
    }
}

/// The 0-based byte index a `serde_json` position names: its `line` is counted from 1
/// and its `column` is the 1-based position within that line.
fn byte_offset_of(document: &str, line: usize, column: usize) -> usize {
    let mut offset = 0usize;
    for (number, text) in document.split_inclusive('\n').enumerate() {
        if number + 1 == line {
            return (offset + column.saturating_sub(1)).min(document.len());
        }
        offset += text.len();
    }
    document.len()
}

/// Scan `document` as ONE JSON value and report where it stops being JSON, or `None`
/// where it is a well-formed JSON document with nothing after it.
///
/// The grammar is RFC 8259's: one value, with insignificant whitespace around it and
/// nothing following it. What this reports about a document it refuses is the
/// offending character, what was being read, and the character's byte offset.
pub fn json_fault(document: &str) -> Option<JsonFault> {
    JsonScan {
        bytes: document.as_bytes(),
        at: 0,
    }
    .fault()
}

/// Which container the scan is inside, so the character that may follow a value is
/// known: a `,` and the container's own closing bracket, and after a `,` in an object
/// another member.
#[derive(Debug, Clone, Copy)]
enum JsonFrame {
    Array,
    Object,
}

/// The scan: a cursor over the document's bytes.
///
/// Bytes, not characters: a refusal names the offending BYTE's offset, and a
/// multi-byte character that cannot start a JSON value is reported by its leading
/// byte the same way a parser reading octets reports it.
struct JsonScan<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl JsonScan<'_> {
    /// The byte at the cursor, without consuming it.
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// Consume the byte at the cursor.
    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek();
        if byte.is_some() {
            self.at += 1;
        }
        byte
    }

    /// Skip the whitespace JSON allows between tokens: space, tab, newline, carriage
    /// return (RFC 8259, section 2).
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// A document that ran out before its value was complete. There is no offending
    /// character, so no offset is reported.
    fn truncated() -> JsonFault {
        JsonFault {
            offset: None,
            detail: "unexpected end of JSON input".to_string(),
        }
    }

    /// The character at byte index `index` cannot appear where the scan is: `reading`
    /// says what was being read there.
    ///
    /// The offset the refusal reports is the number of bytes read to reach that
    /// character — `index + 1` — which is the coordinate the service's refusals are
    /// recorded in.
    fn invalid(index: usize, byte: u8, reading: &str) -> JsonFault {
        JsonFault {
            offset: Some(index.saturating_add(1)),
            detail: format!("invalid character {} {reading}", quoted_byte(byte)),
        }
    }

    /// Walk the whole document; `None` when it is one well-formed JSON value.
    fn fault(&mut self) -> Option<JsonFault> {
        let mut stack: Vec<JsonFrame> = Vec::new();
        'value: loop {
            // A value is expected here: the start of the document, an element after a
            // `,` in an array, or a member's value after a `:` in an object.
            self.skip_whitespace();
            let offset = self.at;
            // Nothing where a value must be: the document ran out (or holds only
            // whitespace), which is not a JSON document.
            let Some(byte) = self.peek() else {
                return Some(Self::truncated());
            };
            match byte {
                b'{' => {
                    self.at += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(b'}') {
                        // The empty object: complete, with no member to read.
                        self.at += 1;
                    } else {
                        stack.push(JsonFrame::Object);
                        if let Some(fault) = self.member_name() {
                            return Some(fault);
                        }
                        continue 'value;
                    }
                }
                b'[' => {
                    self.at += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(b']') {
                        self.at += 1;
                    } else {
                        stack.push(JsonFrame::Array);
                        continue 'value;
                    }
                }
                b'"' => {
                    if let Some(fault) = self.string() {
                        return Some(fault);
                    }
                }
                b't' => {
                    if let Some(fault) = self.literal("true") {
                        return Some(fault);
                    }
                }
                b'f' => {
                    if let Some(fault) = self.literal("false") {
                        return Some(fault);
                    }
                }
                b'n' => {
                    if let Some(fault) = self.literal("null") {
                        return Some(fault);
                    }
                }
                b'-' | b'0'..=b'9' => {
                    if let Some(fault) = self.number() {
                        return Some(fault);
                    }
                }
                other => {
                    return Some(Self::invalid(
                        offset,
                        other,
                        "looking for beginning of value",
                    ))
                }
            }

            // One value is complete. What may follow it depends on what encloses it,
            // and a closing bracket completes the enclosing value in turn.
            loop {
                match stack.last().copied() {
                    None => {
                        // The document's one value is complete: only whitespace may
                        // follow it.
                        self.skip_whitespace();
                        let offset = self.at;
                        return self
                            .peek()
                            .map(|byte| Self::invalid(offset, byte, "after top-level value"));
                    }
                    Some(JsonFrame::Array) => {
                        self.skip_whitespace();
                        let offset = self.at;
                        match self.bump() {
                            None => return Some(Self::truncated()),
                            Some(b',') => continue 'value,
                            Some(b']') => {
                                stack.pop();
                            }
                            Some(byte) => {
                                return Some(Self::invalid(offset, byte, "after array element"))
                            }
                        }
                    }
                    Some(JsonFrame::Object) => {
                        self.skip_whitespace();
                        let offset = self.at;
                        match self.bump() {
                            None => return Some(Self::truncated()),
                            Some(b',') => {
                                if let Some(fault) = self.member_name() {
                                    return Some(fault);
                                }
                                continue 'value;
                            }
                            Some(b'}') => {
                                stack.pop();
                            }
                            Some(byte) => {
                                return Some(Self::invalid(
                                    offset,
                                    byte,
                                    "after object key:value pair",
                                ))
                            }
                        }
                    }
                }
            }
        }
    }

    /// An object member's name and the `:` that separates it from its value.
    fn member_name(&mut self) -> Option<JsonFault> {
        self.skip_whitespace();
        let offset = self.at;
        match self.peek() {
            None => return Some(Self::truncated()),
            Some(b'"') => {
                if let Some(fault) = self.string() {
                    return Some(fault);
                }
            }
            Some(byte) => {
                return Some(Self::invalid(
                    offset,
                    byte,
                    "looking for beginning of object key string",
                ))
            }
        }
        self.skip_whitespace();
        let offset = self.at;
        match self.bump() {
            None => Some(Self::truncated()),
            Some(b':') => None,
            Some(byte) => Some(Self::invalid(offset, byte, "after object key")),
        }
    }

    /// A string, from the opening quote at the cursor to its closing quote, including
    /// the escapes RFC 8259 section 7 allows.
    fn string(&mut self) -> Option<JsonFault> {
        // The opening quote is what selected this scan.
        self.at += 1;
        loop {
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(b'"') => return None,
                Some(b'\\') => {
                    let offset = self.at;
                    match self.bump() {
                        None => return Some(Self::truncated()),
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {}
                        Some(b'u') => {
                            // `\uXXXX`: exactly four hexadecimal digits.
                            for _ in 0..4 {
                                let offset = self.at;
                                match self.bump() {
                                    None => return Some(Self::truncated()),
                                    Some(byte) if byte.is_ascii_hexdigit() => {}
                                    Some(byte) => {
                                        return Some(Self::invalid(
                                            offset,
                                            byte,
                                            "in \\u hexadecimal character escape",
                                        ))
                                    }
                                }
                            }
                        }
                        Some(byte) => {
                            return Some(Self::invalid(offset, byte, "in string escape code"))
                        }
                    }
                }
                // A control character has to be escaped to appear in a string.
                Some(byte) if byte < 0x20 => {
                    return Some(Self::invalid(offset, byte, "in string literal"))
                }
                Some(_) => {}
            }
        }
    }

    /// One of the three literals, whose first byte the cursor is on: the rest of it
    /// must follow exactly.
    fn literal(&mut self, literal: &str) -> Option<JsonFault> {
        self.at += 1;
        for expected in &literal.as_bytes()[1..] {
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(byte) if byte == *expected => {}
                Some(byte) => {
                    return Some(Self::invalid(
                        offset,
                        byte,
                        &format!("in literal {literal} (expecting '{}')", *expected as char),
                    ))
                }
            }
        }
        None
    }

    /// A number: an optional `-`, an integer part, an optional fraction and an
    /// optional exponent (RFC 8259, section 6).
    ///
    /// A number ENDS at the first byte that cannot continue it, which is not an error
    /// here: whether that byte may follow a value is decided by what encloses the
    /// number, so `01` at the top level is reported as a character after a complete
    /// value rather than as a bad number.
    fn number(&mut self) -> Option<JsonFault> {
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        let offset = self.at;
        match self.bump() {
            None => return Some(Self::truncated()),
            // A leading zero is the whole integer part: `01` is not a JSON number.
            Some(b'0') => {}
            Some(byte) if byte.is_ascii_digit() => self.skip_digits(),
            Some(byte) => return Some(Self::invalid(offset, byte, "in numeric literal")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(byte) if byte.is_ascii_digit() => self.skip_digits(),
                Some(byte) => {
                    return Some(Self::invalid(
                        offset,
                        byte,
                        "after decimal point in numeric literal",
                    ))
                }
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(byte) if byte.is_ascii_digit() => self.skip_digits(),
                Some(byte) => {
                    return Some(Self::invalid(
                        offset,
                        byte,
                        "in exponent of numeric literal",
                    ))
                }
            }
        }
        None
    }

    /// Consume the digits that continue a number.
    fn skip_digits(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
    }
}

/// One byte as a refusal names it: printable ASCII as itself, and anything else by
/// the escape that stands for it, so a refusal stays one line of text whatever byte
/// it is about.
fn quoted_byte(byte: u8) -> String {
    let escaped = match byte {
        b'\'' => "\\'",
        b'\\' => "\\\\",
        0x07 => "\\a",
        0x08 => "\\b",
        0x09 => "\\t",
        0x0a => "\\n",
        0x0b => "\\v",
        0x0c => "\\f",
        0x0d => "\\r",
        0x20..=0x7e => return format!("'{}'", byte as char),
        _ => return format!("'\\x{byte:02x}'"),
    };
    format!("'{escaped}'")
}

impl Registry {
    /// The ARN this registry reports to a request signed for `region`.
    ///
    /// A caller's registry reports the ARN its `CreateRegistry` call minted. A
    /// service-owned one was created by no request, so no request's region is stored
    /// for it: it is present in every region, and reports the region of the request
    /// asking.
    pub fn arn_in(&self, region: &str) -> String {
        match self.scope {
            RegistryScope::Local => self.arn.clone(),
            RegistryScope::Aws => registry_arn(region, &self.name),
        }
    }

    /// The ARN one of this registry's schemas reports, or `None` for a schema that has
    /// none.
    ///
    /// A schema a caller created IS a customer resource: it was minted by a request, it
    /// can be tagged and named in a policy, and it reports the ARN that create stored.
    /// A schema of a service-owned registry is not — AWS publishes it, no account owns
    /// it, and no `TagResource` or policy call accepts an identifier for it — so it
    /// carries no ARN at all. Reporting one would hand a client a resource identifier
    /// the service never issued.
    ///
    /// The `region` argument is kept so a future service-owned resource that DOES carry
    /// a regional ARN can be rendered here beside a caller's; nothing needs it today.
    pub fn schema_arn_in(&self, _region: &str, schema: &Schema) -> Option<String> {
        match self.scope {
            RegistryScope::Local => Some(schema.arn.clone()),
            RegistryScope::Aws => None,
        }
    }

    /// The `SchemaArn` a single-schema response reports: the schema's ARN, or the empty
    /// string for a schema that has none ([`Registry::schema_arn_in`]).
    ///
    /// `DescribeSchema` and `ExportSchema` answer ABOUT one schema and always carry the
    /// member; a schema with no ARN reports it empty. A listing's SUMMARY leaves the
    /// member out instead, because a summary carries only the members a schema has.
    pub fn reported_schema_arn_in(&self, region: &str, schema: &Schema) -> String {
        self.schema_arn_in(region, schema).unwrap_or_default()
    }

    /// The schema this registry holds under `name`, or the refusal for a name it holds
    /// none for.
    ///
    /// The schema step of the one read resolution
    /// ([`ComAmazonawsSchemasSchemasState::readable_schema_version`]), so a handler that
    /// has already resolved the registry takes the same step with the same refusal.
    pub fn schema(&self, name: &str) -> Result<&Schema, ReadResolutionError> {
        self.schemas
            .get(name)
            .ok_or_else(|| ReadResolutionError::NoSuchSchema(name.to_string()))
    }

    /// The same step, for a caller that then changes the schema it resolved: a write to
    /// a schema reaches it by the same name and is refused the same way.
    pub fn schema_mut(&mut self, name: &str) -> Result<&mut Schema, ReadResolutionError> {
        self.schemas
            .get_mut(name)
            .ok_or_else(|| ReadResolutionError::NoSuchSchema(name.to_string()))
    }
}

/// A schema and all of its versions.
#[derive(Debug, Clone)]
pub struct Schema {
    pub name: String,
    /// `arn:aws:schemas:<region>:<account>:schema/<registry>/<name>`.
    pub arn: String,
    pub description: Option<String>,
    /// The type of the definition LAST WRITTEN to this schema, one of
    /// [`crate::ops::types::TYPE_VALUES`]: [`Schema::add_version`] is the only writer
    /// of it, so a delete of that definition leaves it naming content the store no
    /// longer holds. What the schema's type IS at any instant is
    /// [`Schema::current_type`], which reads it from the definition the schema still
    /// holds and falls back here only for a schema that holds none.
    pub r#type: String,
    pub tags: BTreeMap<String, String>,
    /// The highest version number ever minted for this schema; rendered as the
    /// decimal `SchemaVersion`. It only ever rises, so a number a delete freed is
    /// never handed out again — which is why the version a read of "the latest"
    /// selects comes from [`Schema::latest_stored_version`], the highest version the
    /// schema still HOLDS, and not from here.
    pub latest_version: u64,
    /// When the schema itself was last written: created, a definition minted or
    /// removed, or its own metadata replaced. [`Schema::modified_at`] is the one place
    /// this is written after creation.
    pub last_modified: SystemTime,
    /// Versions by number. Numeric keys, so the map's order is the version order —
    /// a string key would sort `"10"` before `"2"`.
    pub versions: BTreeMap<u64, SchemaVersion>,
    /// The version each `ClientTokenId` already minted, so a RETRIED `UpdateSchema`
    /// answers the version its first attempt wrote instead of writing a second copy
    /// of the same definition — what the member's `@idempotencyToken` trait is for.
    ///
    /// Bounded by the version map: only a call that mints a version records a token,
    /// so there is at most one entry per version the schema HOLDS — an entry is
    /// removed with the version it names ([`Schema::remove_version`]), so freeing a
    /// version number frees its token entry too and this map never outgrows the
    /// versions it records mints into.
    pub minted_by_token: BTreeMap<String, u64>,
    /// The code bindings `PutCodeBinding` has started for this schema, keyed by the
    /// schema version they were generated from and the language they were generated
    /// for: a binding is identified by that pair, so one schema version can carry a
    /// binding per language and one language a binding per version.
    pub code_bindings: BTreeMap<(u64, String), CodeBinding>,
    /// The event this schema is PUBLISHED for, where the service publishes it: the
    /// identity the registry indexes a catalogue entry under, held beside the document
    /// because a published document's text does not state it (see [`PublishedEvent`]).
    /// `None` for a schema a caller created — its event is whatever its own document
    /// declares.
    pub published_event: Option<PublishedEvent>,
}

/// One version of a schema: the content a caller sent for it.
///
/// The `content`, the `type` and the `created_date` are written once and never
/// rewritten — a reader of version N gets the bytes version N was minted with.
///
/// A version holds NO description: `Description` is metadata of the SCHEMA, not of a
/// definition ([`Schema::description`]), so an `UpdateSchema` that changes it changes
/// what every read of the schema reports, whichever version the read selects. Keeping
/// a second copy per version is what let a read of one version report a description a
/// later update had already replaced.
#[derive(Debug, Clone)]
pub struct SchemaVersion {
    pub version: u64,
    /// Returned verbatim by `DescribeSchema`.
    pub content: String,
    pub r#type: String,
    pub created_date: SystemTime,
}

/// What the one read resolution answers: the registry a read named, the schema it
/// holds, and the version of it the request selected.
///
/// All three are handed back together because a read that names a version answers from
/// all three — the version's content, the schema's metadata, and the ARN the registry
/// reports for the schema — so resolving them once is what keeps a response from being
/// assembled out of two separate lookups.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedSchemaVersion<'a> {
    pub registry: &'a Registry,
    pub schema: &'a Schema,
    pub version: &'a SchemaVersion,
}

/// One code binding: the code generation `PutCodeBinding` started for one schema
/// version in one language.
///
/// It holds no generated source — this slice models the binding's LIFECYCLE only, as
/// `PutCodeBinding` and `DescribeCodeBinding` report it. The source itself is COMPUTED
/// from the schema version the binding is attached to, whose document never changes
/// once written, by the generator in `crate::ops::get_code_binding_source`: storing a
/// second copy of it here would be a copy that could disagree with the document it was
/// generated from.
#[derive(Debug, Clone)]
pub struct CodeBinding {
    /// The language generation was asked for: one of [`CODE_BINDING_LANGUAGES`],
    /// which is what [`code_binding_language`] admits.
    ///
    /// The model types `Language` as an unconstrained `__string`, but the service
    /// does constrain it — it refuses anything outside the pattern quoted on
    /// [`CODE_BINDING_LANGUAGES`] — so only a language this service generates for
    /// ever reaches the store.
    pub language: String,
    /// The schema version the binding was generated from, resolved at put time: an
    /// absent request `SchemaVersion` means the then-latest version, and the binding
    /// stays attached to that number afterwards.
    pub schema_version: u64,
    /// When `PutCodeBinding` started this generation.
    pub created_date: SystemTime,
    /// The instant generation is due to finish; [`CodeBinding::status_at`] reports
    /// `CREATE_IN_PROGRESS` before it and `CREATE_COMPLETE` from it on.
    ///
    /// Stored as an instant rather than waited for: the transition is the clock's, so
    /// every later read derives it and no request sleeps.
    pub generation_due: SystemTime,
}

/// The `CodeGenerationStatus` of a generation that has not finished yet. One of
/// [`crate::ops::types::CODE_GENERATION_STATUS_VALUES`].
pub const CODE_GENERATION_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The `CodeGenerationStatus` of a generation that has finished. One of
/// [`crate::ops::types::CODE_GENERATION_STATUS_VALUES`].
pub const CODE_GENERATION_COMPLETE: &str = "CREATE_COMPLETE";

/// How long after `PutCodeBinding` a code binding's generation is due to complete.
///
/// AWS publishes no duration for code generation, so this span is the emulator's own
/// choice. It is deliberately NOT zero: a caller that describes the binding straight
/// after starting it must still observe the transient `CREATE_IN_PROGRESS` status that
/// the real service reports, and the 202 `PutCodeBinding` answers with says the work
/// is accepted, not done. It is short enough that a caller polling the binding reaches
/// `CREATE_COMPLETE` in well under a second.
pub const CODE_GENERATION_DURATION: Duration = Duration::from_millis(400);

/// What both `PutCodeBinding` and `DescribeCodeBinding` report about one binding.
///
/// The two operations answer the same four members from the same stored binding, so
/// they render it through one function ([`CodeBinding::report_at`]) and cannot drift.
#[derive(Debug, Clone)]
pub struct CodeBindingReport {
    pub creation_date: SystemTime,
    pub last_modified: SystemTime,
    /// The version number in decimal, as `SchemaVersion` is rendered everywhere else.
    pub schema_version: String,
    pub status: &'static str,
}

impl CodeBinding {
    /// The status of this generation as of `now`: in progress until the instant it is
    /// due, complete from then on. Derived, so a read never has to write.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.generation_due {
            CODE_GENERATION_IN_PROGRESS
        } else {
            CODE_GENERATION_COMPLETE
        }
    }

    /// When this binding last changed, as of `now`: it was created, and the only later
    /// change is the completion the clock makes due.
    pub fn last_modified_at(&self, now: SystemTime) -> SystemTime {
        if now < self.generation_due {
            self.created_date
        } else {
            self.generation_due
        }
    }

    /// This binding as the two code-binding operations report it.
    pub fn report_at(&self, now: SystemTime) -> CodeBindingReport {
        CodeBindingReport {
            creation_date: self.created_date,
            last_modified: self.last_modified_at(now),
            schema_version: self.schema_version.to_string(),
            status: self.status_at(now),
        }
    }
}

/// The value an OPTIONAL request member specifies, if it specifies anything at all.
///
/// A member present with no characters in it specifies nothing, exactly as the member
/// left out specifies nothing: a value is what a member says, and a value with nothing
/// in it says nothing. The models of these members admit the empty string
/// (`@length(min: 0)`, or no length constraint at all), and a present-but-empty query
/// parameter reaches a handler as `Some("")` rather than `None`, so a request may
/// legitimately carry one and the reading has to be decided here rather than refused
/// at the boundary.
///
/// What "nothing specified" then MEANS is each member's own documented default and not
/// something decided here. For the members whose value SELECTS what the answer is drawn
/// from, nothing specified selects nothing, so nothing is narrowed: an empty
/// `SchemaVersion` limits a read to no version and the latest stored version answers
/// ([`ComAmazonawsSchemasSchemasState::readable_schema_version`]), as an empty
/// `NextToken` names no position and the first page answers ([`page_by_key`]) and an
/// empty `Keywords` or name prefix narrows no listing. For the member that NAMES the
/// call carrying it, nothing specified names no call ([`idempotency_token`]).
///
/// A REQUIRED member never comes here: a value it must carry is read as given, which is
/// why an empty `SchemaVersion` in the delete's URI label still names no version
/// ([`Schema::version_number`]).
pub fn specified(sent: Option<&str>) -> Option<&str> {
    sent.filter(|value| !value.is_empty())
}

/// The call a request's idempotency-token member NAMES, if it names one at all.
///
/// A member the request left out and one it sent with nothing in it are the same
/// thing ([`specified`]): the token is the identity of the call carrying it, and a
/// value with no characters in it is no identity, so such a request is served exactly
/// as the one that omits the member — it resolves to no earlier call, and the call it
/// makes is recorded under no token. Were the empty value read as an identity, two
/// unrelated calls that both left the member empty would be retries of each other and
/// the second one's writes would be dropped with a success status.
pub fn idempotency_token(sent: Option<&str>) -> Option<&str> {
    specified(sent)
}

impl Schema {
    /// The version a request asked for: `SchemaVersion` when it specifies one, else the
    /// latest.
    ///
    /// `None` means the request named a version this schema does not hold — including
    /// a value that is not a decimal version number at all, which no stored version
    /// can match. The raw label is read as given, by [`Schema::version_number`]: only
    /// a version's own decimal rendering names it, so a spelling that merely parses
    /// as that number (surrounding space, a sign, zero padding) names nothing. Whether
    /// an optional member SPECIFIES a label at all is settled before this, by
    /// [`specified`], so a request that sent it with nothing in it arrives here as
    /// `None` and is answered the latest.
    pub fn version_for(&self, requested: Option<&str>) -> Option<&SchemaVersion> {
        match requested {
            // The newest version the schema still HOLDS: a deleted version is gone
            // from every later read, so after one is deleted the latest is the
            // highest-numbered of those that remain. `None` here means the schema
            // holds no version at all.
            None => self.versions.values().next_back(),
            Some(raw) => self.versions.get(&Self::version_number(raw)?),
        }
    }

    /// The version number a `SchemaVersion` label names, if it names one at all.
    ///
    /// The ONE definition of how a request's version label is read, so every reader
    /// and the delete resolve it the same way: a label names a version only when it is
    /// exactly how this service RENDERS that version — the decimal digits of the
    /// number and nothing else — so any other spelling (surrounding space, a sign,
    /// zero padding, a decimal point, a word, a number too large to be a version)
    /// names no version at all. A label with nothing in it is no rendering of any
    /// number and so names nothing either — which is what the delete's REQUIRED URI
    /// label does with it; an OPTIONAL member sent empty specifies no label at all and
    /// never reaches here ([`specified`]).
    fn version_number(requested: &str) -> Option<u64> {
        // Screen the spelling before handing the digits to `u64: FromStr`, which on its
        // own accepts a leading `+` and so would let a signed label name a version.
        if requested.is_empty() || !requested.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        // Zero padding is the only way an all-digit label can still differ from the
        // rendering, which is why the rendering itself need not be built to compare.
        if requested.len() > 1 && requested.starts_with('0') {
            return None;
        }
        // A value wider than the counter is no version the store can ever hold.
        requested.parse::<u64>().ok()
    }

    /// The highest version number this schema still HOLDS, if it holds any.
    ///
    /// Distinct from [`Schema::latest_version`], which counts every version ever
    /// minted: once a version is deleted the store no longer holds it, so what a read
    /// of "the latest version" answers is the newest one that survives.
    pub fn latest_stored_version(&self) -> Option<u64> {
        self.versions.keys().next_back().copied()
    }

    /// The type this schema's definition CURRENTLY carries: the type of the newest
    /// version the schema still holds, and only when it holds none the type the last
    /// definition written to it carried ([`Schema::r#type`]).
    ///
    /// `Type` names the type of the schema CONTENT, so what the schema's type is at any
    /// instant is a property of the definition a version-less read resolves to — which
    /// is why the version is taken from [`Schema::version_for`] with no version
    /// requested, the one resolution every read of "the latest" makes, rather than
    /// selected again here. [`Schema::r#type`] alone is the type of the definition last
    /// written, which a [`Schema::remove_version`] of that definition leaves naming
    /// content the store no longer holds; it answers only for a schema with no
    /// definition left to read a type from, which still carries the type its
    /// definitions were written with.
    pub fn current_type(&self) -> &str {
        self.version_for(None)
            .map_or(self.r#type.as_str(), |version| version.r#type.as_str())
    }

    /// Remove the version numbered `version`, and with it everything derived from
    /// that version alone; answer whether the schema held it.
    ///
    /// A code binding is generated FROM one version's content, so a binding of a
    /// version the store no longer holds describes nothing and goes with it, and so
    /// does the idempotency-token entry that names it: a token whose version is gone
    /// names nothing a retry could answer with, which is also what keeps that map no
    /// larger than the version set it records mints into. Every other version and
    /// [`Schema::latest_version`] are left exactly as they were: deleting a definition
    /// is not deleting the schema, and a freed number is never minted again.
    ///
    /// Taking a definition away is a write TO the schema — what a version-less read
    /// resolves to and how many versions the schema holds both change — so it moves
    /// [`Schema::last_modified`] through [`Schema::modified_at`]; a call that removes
    /// nothing writes nothing and moves it not at all.
    pub fn remove_version(&mut self, version: u64, now: SystemTime) -> bool {
        if self.versions.remove(&version).is_none() {
            return false;
        }
        self.code_bindings
            .retain(|(bound_version, _), _| *bound_version != version);
        self.minted_by_token.retain(|_, minted| *minted != version);
        self.modified_at(now);
        true
    }

    /// Whether `requested` NAMES a version this schema still holds, resolving the label
    /// exactly as [`Schema::version_for`] resolves it for a read — so a value that is no
    /// rendering of a version number names none.
    ///
    /// What a delete asks before it removes anything: a label naming no stored version
    /// is refused as not found, and only a label that does name one can be the schema's
    /// last definition.
    pub fn holds_version_named(&self, requested: &str) -> bool {
        Self::version_number(requested).is_some_and(|version| self.versions.contains_key(&version))
    }

    /// How many definitions this schema still holds.
    ///
    /// A schema holds at least one at every instant a request can observe it: the delete
    /// of a schema's last version is refused ([`cannot_delete_every_version`]), so only
    /// deleting the schema itself takes its final definition.
    pub fn stored_version_count(&self) -> usize {
        self.versions.len()
    }

    /// Remove the version a request's `SchemaVersion` label NAMES, resolving the
    /// label exactly as [`Schema::version_for`] resolves it for a read; answer
    /// whether the schema held it.
    pub fn remove_version_named(&mut self, requested: &str, now: SystemTime) -> bool {
        Self::version_number(requested).is_some_and(|version| self.remove_version(version, now))
    }

    /// Replace the schema's description and record the write.
    ///
    /// The description is metadata OF THE SCHEMA rather than of any one definition, so
    /// replacing it is a change to the schema whichever version a later read selects —
    /// which is why it goes through the same recorder a definition write does.
    pub fn set_description(&mut self, description: String, now: SystemTime) {
        self.description = Some(description);
        self.modified_at(now);
    }

    /// Record that the schema itself was written at `now`.
    ///
    /// The ONE writer of [`Schema::last_modified`], so every path that changes the
    /// schema reports the member the same way: the member is the instant the schema was
    /// last modified, and what modifies a schema is a definition minted, a definition
    /// removed, or its own metadata replaced — not one of those three in particular. A
    /// read never calls this, so reading a schema cannot move the member.
    fn modified_at(&mut self, now: SystemTime) {
        self.last_modified = now;
    }

    /// The version the call named by `token` already minted for this schema, if it
    /// names one the schema still HOLDS.
    ///
    /// The ONE reader of [`Schema::minted_by_token`], paired with
    /// [`Schema::record_minted_token`], so the token a retry presents is resolved
    /// exactly as the token a mint recorded was read: through
    /// [`idempotency_token`], which is where "this value names no call" is decided. A
    /// token whose version has since been deleted names nothing a retry could be
    /// answered with.
    pub fn minted_for_token(&self, token: Option<&str>) -> Option<u64> {
        let token = idempotency_token(token)?;
        self.minted_by_token
            .get(token)
            .copied()
            .filter(|version| self.versions.contains_key(version))
    }

    /// Record that the call named by `token` minted `version`, so a retry of that call
    /// is answered with it instead of writing the same definition twice.
    ///
    /// The ONE writer of [`Schema::minted_by_token`]: a request whose token names no
    /// call ([`idempotency_token`]) records nothing, so no later request can resolve
    /// to this one by leaving the member as empty as this one left it.
    pub fn record_minted_token(&mut self, token: Option<&str>, version: u64) {
        if let Some(token) = idempotency_token(token) {
            self.minted_by_token.insert(token.to_string(), version);
        }
    }

    /// Store `content` as this schema's NEXT version and answer the version stored.
    ///
    /// The number is taken from `latest_version`, which only ever rises — so it is
    /// unique over the life of the schema even when a `DeleteSchemaVersion` freed an
    /// earlier number, and two writers cannot be handed the same one, because the
    /// whole mint-and-insert happens under the one state lock the caller already
    /// holds. An existing version is never rewritten.
    /// The schema's `description` is NOT touched here: it is metadata of the schema
    /// rather than of a definition, so only a request that sends `Description`
    /// changes it.
    pub fn add_version(
        &mut self,
        content: String,
        r#type: String,
        now: SystemTime,
    ) -> &SchemaVersion {
        let version = self.latest_version.saturating_add(1);
        self.latest_version = version;
        self.modified_at(now);
        self.r#type = r#type.clone();
        self.versions.insert(
            version,
            SchemaVersion {
                version,
                content,
                r#type,
                created_date: now,
            },
        );
        self.versions
            .get(&version)
            .expect("the version inserted above is present")
    }
}

/// The SERVICE-SPECIFIC event types the service-owned catalogue holds a schema for:
/// the event types a publishing source publishes of its own, beside the
/// CloudTrail-delivered family [`CLOUD_TRAIL_EVENT_FAMILY`] carries for the sources in
/// [`AWS_EVENT_SOURCES`].
///
/// This table is PER SOURCE, not a template over sources: which event types a source
/// publishes is published data, so a row exists for an event type AWS publishes and a
/// source carries exactly the rows written for it. A source may publish its own event
/// types and no CloudTrail-delivered schema at all, in which case it appears here and
/// not in [`AWS_EVENT_SOURCES`].
///
/// Where AWS has published ONE event type under more than one `detail-type` spelling —
/// a legacy name and a current one, differing in a letter's case — the catalogue holds
/// the entry each spelling yields, because each is a name a client resolves by, so both
/// are written out as rows of their own.
///
/// Source: the `detail` of each event as AWS documents it (the EventBridge event
/// reference for the publishing service); the `detail_type` is the `detail-type`
/// EventBridge delivers it under and the `event_name` is the name the catalogue
/// publishes it as, which AWS derives from that detail-type by taking the spaces out.
/// A row AWS documents the identity but not the `detail` of is written with
/// [`ManagedEventType::identified`].
///
/// This is seeded content, not a response: `ListRegistries`, `ListSchemas` and
/// `DescribeSchema` report it from the store like any other registry, and
/// `ExportSchema` converts it. A caller cannot write to it — see
/// [`RegistryScope::Aws`]. [`aws_managed_schemas`] is the whole catalogue, and is
/// what seeds the store.
pub const AWS_SERVICE_EVENT_TYPES: &[ManagedEventType] = &[
    ManagedEventType {
        source: "aws.autoscaling",
        detail_type: "EC2 Instance Launch Successful",
        event_name: "EC2InstanceLaunchSuccessful",
        components: r##"{"EC2InstanceLaunchSuccessful":{"type":"object","required":["AutoScalingGroupName","EC2InstanceId","RequestId"],"properties":{"AutoScalingGroupName":{"type":"string"},"EC2InstanceId":{"type":"string"},"RequestId":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.ec2",
        detail_type: "EC2 Instance State-change Notification",
        event_name: "EC2InstanceStateChangeNotification",
        components: r##"{"EC2InstanceStateChangeNotification":{"type":"object","required":["instance-id","state"],"properties":{"instance-id":{"type":"string"},"state":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.s3",
        detail_type: "Object Created",
        event_name: "ObjectCreated",
        components: r##"{"Bucket":{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}},"Object":{"type":"object","required":["key","size"],"properties":{"key":{"type":"string"},"size":{"type":"integer","format":"int64"},"etag":{"type":"string"}}},"ObjectCreated":{"type":"object","required":["bucket","object","reason","version"],"properties":{"bucket":{"$ref":"#/components/schemas/Bucket"},"object":{"$ref":"#/components/schemas/Object"},"reason":{"type":"string"},"version":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.batch",
        detail_type: "Batch Job State Change",
        event_name: "BatchJobStateChange",
        components: r##"{"BatchJobStateChange":{"type":"object","required":["jobArn","jobDefinition","jobId","jobName","jobQueue","status"],"properties":{"createdAt":{"type":"integer","format":"int64"},"jobArn":{"type":"string"},"jobDefinition":{"type":"string"},"jobId":{"type":"string"},"jobName":{"type":"string"},"jobQueue":{"type":"string"},"startedAt":{"type":"integer","format":"int64"},"status":{"type":"string"},"statusReason":{"type":"string"},"stoppedAt":{"type":"integer","format":"int64"}}}}"##,
    },
    ManagedEventType {
        source: "aws.codebuild",
        detail_type: "CodeBuild Build State Change",
        event_name: "CodeBuildBuildStateChange",
        components: r##"{"CodeBuildBuildStateChange":{"type":"object","required":["build-id","build-status","project-name","version"],"properties":{"build-id":{"type":"string"},"build-status":{"type":"string"},"current-phase":{"type":"string"},"current-phase-context":{"type":"string"},"project-name":{"type":"string"},"version":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.codepipeline",
        detail_type: "CodePipeline Pipeline Execution State Change",
        event_name: "CodePipelinePipelineExecutionStateChange",
        components: r##"{"CodePipelinePipelineExecutionStateChange":{"type":"object","required":["execution-id","pipeline","state","version"],"properties":{"execution-id":{"type":"string"},"pipeline":{"type":"string"},"state":{"type":"string"},"version":{"type":"number"}}}}"##,
    },
    ManagedEventType {
        source: "aws.ecs",
        detail_type: "ECS Task State Change",
        event_name: "ECSTaskStateChange",
        components: r##"{"ECSContainer":{"type":"object","required":["containerArn","lastStatus","name","taskArn"],"properties":{"containerArn":{"type":"string"},"exitCode":{"type":"integer"},"lastStatus":{"type":"string"},"name":{"type":"string"},"reason":{"type":"string"},"taskArn":{"type":"string"}}},"ECSTaskStateChange":{"type":"object","required":["clusterArn","desiredStatus","lastStatus","taskArn","taskDefinitionArn"],"properties":{"clusterArn":{"type":"string"},"containers":{"type":"array","items":{"$ref":"#/components/schemas/ECSContainer"}},"createdAt":{"type":"string","format":"date-time"},"desiredStatus":{"type":"string"},"group":{"type":"string"},"lastStatus":{"type":"string"},"launchType":{"type":"string"},"startedBy":{"type":"string"},"stoppedReason":{"type":"string"},"taskArn":{"type":"string"},"taskDefinitionArn":{"type":"string"},"updatedAt":{"type":"string","format":"date-time"}}}}"##,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "Scheduled Event",
        event_name: "ScheduledEvent",
        components: r##"{"ScheduledEvent":{"type":"object","properties":{}}}"##,
    },
    ManagedEventType {
        source: "aws.health",
        detail_type: "AWS Health Event",
        event_name: "AWSHealthEvent",
        components: r##"{"AWSHealthEvent":{"type":"object","required":["eventArn","eventTypeCategory","eventTypeCode","service","startTime"],"properties":{"affectedEntities":{"type":"array","items":{"$ref":"#/components/schemas/AWSHealthAffectedEntity"}},"endTime":{"type":"string"},"eventArn":{"type":"string"},"eventDescription":{"type":"array","items":{"$ref":"#/components/schemas/AWSHealthEventDescription"}},"eventTypeCategory":{"type":"string"},"eventTypeCode":{"type":"string"},"service":{"type":"string"},"startTime":{"type":"string"}}},"AWSHealthAffectedEntity":{"type":"object","properties":{"entityValue":{"type":"string"},"status":{"type":"string"}}},"AWSHealthEventDescription":{"type":"object","properties":{"language":{"type":"string"},"latestDescription":{"type":"string"}}}}"##,
    },
    // The tag-change event, published under BOTH spellings AWS has published it as:
    // the `detail-type` has been written with a lower-case and with a capital "on", and
    // the catalogue holds the entry each spelling yields — see
    // [`aws_managed_event_types`] on why two spellings are two entries. One components
    // template serves them, because they describe the SAME event.
    ManagedEventType {
        source: "aws.tag",
        detail_type: "Tag Change on Resource",
        event_name: "TagChangeonResource",
        components: TAG_CHANGE_COMPONENTS,
    },
    ManagedEventType {
        source: "aws.tag",
        detail_type: "Tag Change On Resource",
        event_name: "TagChangeOnResource",
        components: TAG_CHANGE_COMPONENTS,
    },
    ManagedEventType {
        source: "aws.acm",
        detail_type: "ACM Certificate Available",
        event_name: "ACMCertificateAvailable",
        components: r##"{"ACMCertificateAvailable":{"type":"object","required":["Action","CertificateType","CommonName"],"properties":{"Action":{"type":"string"},"CertificateType":{"type":"string"},"CommonName":{"type":"string"},"DomainValidationMethod":{"type":"string"}}}}"##,
    },
    // The event types below are published by the catalogue with their identity (the
    // source and the `detail-type` EventBridge delivers them under) but with an
    // unconstrained `detail`: AWS documents that these events exist, not the members
    // their `detail` carries. [`UNDOCUMENTED_DETAIL`] says what that means for a
    // reader, and [`ManagedEventType::identified`] is how such a row is written.
    //
    // They are grouped by publishing source, because WHICH event types a source
    // publishes is per-source published data: a source publishes the event types AWS
    // publishes for it and no others, so a row is added for an event type AWS
    // publishes rather than derived from a template over sources.
    ManagedEventType::identified("aws.a4b", "Room State Change", "RoomStateChange"),
    ManagedEventType::identified(
        "aws.a4b",
        "Room Device Calendar Error Status",
        "RoomDeviceCalendarErrorStatus",
    ),
    // `aws.access-analyzer` publishes its finding events and NOTHING of the
    // CloudTrail-delivered family, which is why it is absent from
    // [`AWS_EVENT_SOURCES`].
    ManagedEventType::identified(
        "aws.access-analyzer",
        "Access Analyzer Finding",
        "AccessAnalyzerFinding",
    ),
    ManagedEventType::identified(
        "aws.access-analyzer",
        "Unused Access Finding for IAM entities",
        "UnusedAccessFindingforIAMentities",
    ),
    ManagedEventType::identified(
        "aws.account",
        "Region Opt-In Status Change",
        "RegionOpt-InStatusChange",
    ),
    // The certificate lifecycle event types `aws.acm` publishes. The two revocation
    // entries whose names differ only in a letter's case are two entries, not one: AWS
    // has published the revocation event under more than one `detail-type` spelling and
    // the catalogue holds each of them.
    ManagedEventType::identified(
        "aws.acm",
        "ACM Certificate Approaching Expiration",
        "ACMCertificateApproachingExpiration",
    ),
    ManagedEventType::identified("aws.acm", "ACM Certificate Expired", "ACMCertificateExpired"),
    ManagedEventType::identified(
        "aws.acm",
        "ACM Certificate Renewal Action Required",
        "ACMCertificateRenewalActionRequired",
    ),
    ManagedEventType::identified("aws.acm", "ACM Certificate Revoked", "ACMCertificateRevoked"),
    ManagedEventType::identified(
        "aws.acm",
        "ACM Certificate Revoked By CA",
        "ACMCertificateRevokedByCA",
    ),
    ManagedEventType::identified(
        "aws.acm",
        "ACM Certificate Revoked by Certificate Authority CA",
        "ACMCertificateRevokedbyCertificateAuthorityCA",
    ),
    ManagedEventType::identified(
        "aws.acm-pca",
        "ACM Private CA Audit Report Generation",
        "ACMPrivateCAAuditReportGeneration",
    ),
    ManagedEventType::identified(
        "aws.acm-pca",
        "ACM Private CA CRL Generation",
        "ACMPrivateCACRLGeneration",
    ),
    ManagedEventType::identified(
        "aws.acm-pca",
        "ACM Private CA Certificate Issuance",
        "ACMPrivateCACertificateIssuance",
    ),
    ManagedEventType::identified(
        "aws.acm-pca",
        "ACM Private CA Certificate Revocation",
        "ACMPrivateCACertificateRevocation",
    ),
    ManagedEventType::identified(
        "aws.acm-pca",
        "ACM Private CA Creation",
        "ACMPrivateCACreation",
    ),
    ManagedEventType::identified(
        "aws.act",
        "Trust Bundle Association Version Updated",
        "TrustBundleAssociationVersionUpdated",
    ),
    // The investigation lifecycle `aws.aidevops` publishes: one event type per state an
    // investigation reaches.
    ManagedEventType::identified(
        "aws.aidevops",
        "Investigation Cancelled",
        "InvestigationCancelled",
    ),
    ManagedEventType::identified(
        "aws.aidevops",
        "Investigation Completed",
        "InvestigationCompleted",
    ),
    ManagedEventType::identified(
        "aws.aidevops",
        "Investigation Created",
        "InvestigationCreated",
    ),
    ManagedEventType::identified("aws.aidevops", "Investigation Failed", "InvestigationFailed"),
    ManagedEventType::identified(
        "aws.aidevops",
        "Investigation In Progress",
        "InvestigationInProgress",
    ),
    ManagedEventType::identified("aws.aidevops", "Investigation Linked", "InvestigationLinked"),
    ManagedEventType::identified(
        "aws.aidevops",
        "Investigation Pending Triage",
        "InvestigationPendingTriage",
    ),
    ManagedEventType::identified(
        "aws.appflow",
        "AppFlow Automatic Field Import Report",
        "AppFlowAutomaticFieldImportReport",
    ),
    ManagedEventType::identified(
        "aws.application-autoscaling",
        "Application Auto Scaling Scaling Activity State Change",
        "ApplicationAutoScalingScalingActivityStateChange",
    ),
    // The spelling of the API-call `detail-type` the catalogue publishes for
    // `aws.autoscaling` BESIDE the one [`CLOUD_TRAIL_EVENT_FAMILY`] carries: AWS has
    // published this event with a capital and with a lower-case "via", and a client
    // resolving either spelling by name reads the entry for it.
    ManagedEventType::cloud_trail(
        "aws.autoscaling",
        "AWS API Call Via CloudTrail",
        "AWSAPICallViaCloudTrail",
    ),
    // `aws.b2bi` publishes the acknowledgement event of its own BESIDE the
    // CloudTrail-delivered family [`AWS_EVENT_SOURCES`] carries for it.
    ManagedEventType::identified(
        "aws.b2bi",
        "Acknowledgement Completed",
        "AcknowledgementCompleted",
    ),
    ManagedEventType::identified(
        "aws.cleanrooms",
        "Analysis Template Created",
        "AnalysisTemplateCreated",
    ),
    ManagedEventType::identified(
        "aws.cleanrooms",
        "Analysis Template Deleted",
        "AnalysisTemplateDeleted",
    ),
    ManagedEventType::identified(
        "aws.cleanrooms",
        "Analysis Template Updated",
        "AnalysisTemplateUpdated",
    ),
    ManagedEventType::identified(
        "aws.deadline",
        "Budget Threshold Reached",
        "BudgetThresholdReached",
    ),
    ManagedEventType::identified(
        "aws.deadline",
        "Fleet Size Recommendation Change",
        "FleetSizeRecommendationChange",
    ),
    ManagedEventType::identified(
        "aws.deadline",
        "Job Lifecycle Status Change",
        "JobLifecycleStatusChange",
    ),
    ManagedEventType::identified(
        "aws.deadline",
        "Job Run Status Change",
        "JobRunStatusChange",
    ),
    ManagedEventType::identified(
        "aws.deadline",
        "Step Lifecycle Status Change",
        "StepLifecycleStatusChange",
    ),
    ManagedEventType::identified(
        "aws.deadline",
        "Step Run Status Change",
        "StepRunStatusChange",
    ),
    // The fleet event types `aws.ec2fleet` publishes: the five an EC2 Fleet delivers
    // and the five a Spot Fleet delivers, both under this one source. It publishes
    // nothing of the CloudTrail-delivered family, which is why it is absent from
    // [`AWS_EVENT_SOURCES`].
    ManagedEventType::identified("aws.ec2fleet", "EC2 Fleet Error", "EC2FleetError"),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Fleet Information",
        "EC2FleetInformation",
    ),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Fleet Instance Change",
        "EC2FleetInstanceChange",
    ),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Fleet Spot Instance Request Change",
        "EC2FleetSpotInstanceRequestChange",
    ),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Fleet State Change",
        "EC2FleetStateChange",
    ),
    ManagedEventType::identified("aws.ec2fleet", "EC2 Spot Fleet Error", "EC2SpotFleetError"),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Spot Fleet Information",
        "EC2SpotFleetInformation",
    ),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Spot Fleet Instance Change",
        "EC2SpotFleetInstanceChange",
    ),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Spot Fleet Spot Instance Request Change",
        "EC2SpotFleetSpotInstanceRequestChange",
    ),
    ManagedEventType::identified(
        "aws.ec2fleet",
        "EC2 Spot Fleet State Change",
        "EC2SpotFleetStateChange",
    ),
    // `aws.epfs` publishes the job and session lifecycle event types below and nothing
    // of the CloudTrail-delivered family.
    ManagedEventType::identified(
        "aws.epfs",
        "Node Identification Job State Changed",
        "NodeIdentificationJobStateChanged",
    ),
    ManagedEventType::identified(
        "aws.epfs",
        "Power Flow Job State Changed",
        "PowerFlowJobStateChanged",
    ),
    ManagedEventType::identified(
        "aws.epfs",
        "Power Flow Node Job State Changed",
        "PowerFlowNodeJobStateChanged",
    ),
    ManagedEventType::identified(
        "aws.epfs",
        "Session State Changed",
        "SessionStateChanged",
    ),
    ManagedEventType::identified(
        "aws.epfs",
        "Subsystem Analysis Job State Changed",
        "SubsystemAnalysisJobStateChanged",
    ),
    ManagedEventType::identified(
        "aws.events",
        "API Destination Activated",
        "APIDestinationActivated",
    ),
    ManagedEventType::identified(
        "aws.events",
        "API Destination Deactivated",
        "APIDestinationDeactivated",
    ),
    // The connection event types `aws.events` publishes: a connection's authorization
    // and its lifecycle, each delivered under its own `detail-type`. The pair of
    // "started" events ("Connection Creation Started", "Connection Update Started",
    // "Connection Deletion Started") report work beginning, and the rest report the
    // state a connection reached.
    ManagedEventType::identified("aws.events", "Connection Activated", "ConnectionActivated"),
    ManagedEventType::identified(
        "aws.events",
        "Connection Authorization Started",
        "ConnectionAuthorizationStarted",
    ),
    ManagedEventType::identified(
        "aws.events",
        "Connection Authorized",
        "ConnectionAuthorized",
    ),
    ManagedEventType::identified(
        "aws.events",
        "Connection Creation Started",
        "ConnectionCreationStarted",
    ),
    ManagedEventType::identified(
        "aws.events",
        "Connection Deauthorization Started",
        "ConnectionDeauthorizationStarted",
    ),
    ManagedEventType::identified(
        "aws.events",
        "Connection Deauthorized",
        "ConnectionDeauthorized",
    ),
    ManagedEventType::identified(
        "aws.events",
        "Connection Deletion Started",
        "ConnectionDeletionStarted",
    ),
    ManagedEventType::identified(
        "aws.events",
        "Connection Failed Connectivity",
        "ConnectionFailedConnectivity",
    ),
    ManagedEventType::identified(
        "aws.events",
        "Connection Update Started",
        "ConnectionUpdateStarted",
    ),
    // The scheduled events `aws.events` delivers to a rule's targets: the plain
    // scheduled event ([`AWS_SERVICE_EVENT_TYPES`] above, which carries a document)
    // and the one whose payload is JSON.
    ManagedEventType::identified("aws.events", "Scheduled Json", "ScheduledJson"),
    // `aws.factstore` publishes one event type of its own and nothing of the
    // CloudTrail-delivered family.
    ManagedEventType::identified("aws.factstore", "New Commit Pushed", "NewCommitPushed"),
    // The event types `aws.glue-autodebug` publishes — the auto-version job lifecycle,
    // the data-quality results and the metering event — and nothing of the
    // CloudTrail-delivered family. The entries whose published names run words
    // together carry the `detail-type` they were published under, spelling for
    // spelling: the catalogue derives the name from the detail-type by taking the
    // spaces out, so the name is what fixes the spelling of each word.
    ManagedEventType::identified(
        "aws.glue-autodebug",
        "Auto Version Job State Change",
        "AutoVersionJobStateChange",
    ),
    ManagedEventType::identified(
        "aws.glue-autodebug",
        "Autodebug metadata key shared from Orca",
        "AutodebugmetadatakeysharedfromOrca",
    ),
    ManagedEventType::identified(
        "aws.glue-autodebug",
        "Autoversion job run completed",
        "Autoversionjobruncompleted",
    ),
    ManagedEventType::identified(
        "aws.glue-autodebug",
        "Autoversion job version updated",
        "Autoversionjobversionupdated",
    ),
    ManagedEventType::identified(
        "aws.glue-autodebug",
        "Data Quality Evaluation Results Available",
        "DataQualityEvaluationResultsAvailable",
    ),
    ManagedEventType::identified(
        "aws.glue-autodebug",
        "Data Quality Task State Change",
        "DataQualityTaskStateChange",
    ),
    ManagedEventType::identified(
        "aws.glue-autodebug",
        "Entity Metering Event",
        "EntityMeteringEvent",
    ),
    // `aws.goshawk` publishes these two repository event types and nothing of the
    // CloudTrail-delivered family.
    ManagedEventType::identified(
        "aws.goshawk",
        "CodeArtifact Package Version State Change",
        "CodeArtifactPackageVersionStateChange",
    ),
    ManagedEventType::identified(
        "aws.goshawk",
        "Goshawk Repository State Change",
        "GoshawkRepositoryStateChange",
    ),
    // The abuse event `aws.health` publishes beside its health event (above, which
    // carries a document).
    ManagedEventType::identified(
        "aws.health",
        "AWS Health Abuse Event",
        "AWSHealthAbuseEvent",
    ),
    // `aws.labs` and `aws.maester` publish their own event types and NOTHING of the
    // CloudTrail-delivered family, which is why neither is in [`AWS_EVENT_SOURCES`].
    ManagedEventType::identified("aws.labs", "Lab Auto Cancelled", "LabAutoCancelled"),
    ManagedEventType::identified("aws.labs", "Lab Cost Alert", "LabCostAlert"),
    ManagedEventType::identified("aws.labs", "Lab Cost Update", "LabCostUpdate"),
    ManagedEventType::identified(
        "aws.labs",
        "Lab Launch Status Change",
        "LabLaunchStatusChange",
    ),
    ManagedEventType::identified(
        "aws.labs",
        "Lab Termination Status Change",
        "LabTerminationStatusChange",
    ),
    ManagedEventType::identified("aws.maester", "Document Created", "DocumentCreated"),
    ManagedEventType::identified("aws.maester", "Document Deleted", "DocumentDeleted"),
    ManagedEventType::identified("aws.maester", "Document Updated", "DocumentUpdated"),
];

/// The `components.schemas` of an event type whose `detail` AWS publishes no member
/// list for: one component, named after the event type, constrained only to be an
/// object.
///
/// The catalogue's membership and an event's identity are what a discovery flow reads
/// (which event types exist, under which names, published by which source and
/// `detail-type`); the members of a `detail` AWS does not document cannot be invented
/// here, and a document that named members the real event does not carry would make a
/// generated model reject real events. So the component says exactly what is known:
/// the `detail` is a JSON object.
const UNDOCUMENTED_DETAIL: &str = r##"{"{EventName}":{"type":"object"}}"##;

/// The `components.schemas` of the tag-change event, as
/// [`event_schema_document`] publishes them.
///
/// Source: the `detail` AWS documents for the tag-change event (the EventBridge event
/// reference for tag changes). The component is named by
/// [`EVENT_COMPONENT_PLACEHOLDER`], because the catalogue publishes this one event under
/// each spelling of its `detail-type`, and each spelling is published under its own
/// component name.
const TAG_CHANGE_COMPONENTS: &str = r##"{"{EventName}":{"type":"object","required":["changed-tag-keys","resource-type","service","version"],"properties":{"changed-tag-keys":{"type":"array","items":{"type":"string"}},"resource-type":{"type":"string"},"service":{"type":"string"},"tags":{"type":"object"},"version":{"type":"integer"}}}}"##;

/// The token a components template carries where the event type's own component name
/// goes, substituted by [`ManagedEventType::components_json`].
///
/// A template rather than a literal name, because one template serves a whole FAMILY
/// of event types: every CloudTrail-delivered event carries a CloudTrail record as its
/// `detail`, and each of them is published under its own component name.
const EVENT_COMPONENT_PLACEHOLDER: &str = "{EventName}";

/// How many versions the AWS-managed catalogue publishes for a schema, where it
/// publishes more than one.
///
/// `VersionCount` is "the number of versions available for the schema" — a property of
/// the PUBLISHED schema, not of this process: AWS has revised some of its event schemas
/// and still serves the earlier revisions, so a client that pins a version has one to
/// pin. The count is therefore catalogue data, seeded like a schema's name and its
/// document, and [`aws_managed_registries`] seeds exactly this many versions so the
/// count a listing reports stays what the store actually holds.
///
/// Source: the counts AWS reports for these entries. Anything not named here is
/// published at one version, which is the catalogue's norm. The count is PER ENTRY and
/// not a property of the source: entries of one source carry different counts, so a row
/// is written for each entry the catalogue publishes more than one version of.
const MANAGED_VERSION_COUNTS: &[(&str, u64)] = &[
    ("aws.a4b@RoomStateChange", 3),
    ("aws.acm@ACMCertificateApproachingExpiration", 3),
    ("aws.acm@ACMCertificateAvailable", 3),
    ("aws.acm@ACMCertificateExpired", 3),
    ("aws.acm@ACMCertificateRenewalActionRequired", 3),
    ("aws.acm@ACMCertificateRevoked", 2),
    ("aws.aidevops@InvestigationCancelled", 2),
    ("aws.aidevops@InvestigationCompleted", 2),
    ("aws.aidevops@InvestigationCreated", 2),
    ("aws.aidevops@InvestigationFailed", 2),
    ("aws.aidevops@InvestigationInProgress", 2),
    ("aws.aidevops@InvestigationLinked", 2),
    ("aws.aidevops@InvestigationPendingTriage", 2),
    ("aws.b2bi@AcknowledgementCompleted", 2),
    ("aws.health@AWSHealthAbuseEvent", 4),
    ("aws.tag@TagChangeOnResource", 4),
];

/// The number of versions the catalogue publishes for the schema named — see
/// [`MANAGED_VERSION_COUNTS`]. Never zero: a schema with no version could not be
/// described at all.
pub fn managed_version_count(schema_name: &str) -> u64 {
    MANAGED_VERSION_COUNTS
        .iter()
        .find(|(name, _)| *name == schema_name)
        .map_or(1, |(_, count)| *count)
}

/// The `Type` every schema of the service-owned catalogue is stored as: AWS stores its
/// event schemas as OpenAPI 3 documents, and `ExportSchema` is what converts one to
/// JSON Schema draft 4. Read from [`SchemaType`], so the catalogue is seeded with a
/// language a request can actually name.
pub const AWS_MANAGED_SCHEMA_TYPE: &str = SchemaType::OpenApi3.as_wire();

/// The registry AWS publishes its discovered event schemas in — the read-only
/// catalogue of every AWS service event type, which every account sees without
/// creating anything.
pub const AWS_MANAGED_REGISTRY: &str = "aws.events";

/// Where an AWS event schema's OpenAPI document keeps its reusable components, as a
/// `$ref` names one. Shared with `ExportSchema`, which rewrites these refs when it
/// converts a document to JSON Schema draft 4.
pub const OPENAPI_COMPONENT_REF: &str = "#/components/schemas/";

/// The component an OpenAPI event document names its EventBridge envelope by, where it
/// declares one: the envelope whose `detail` references the event-specific component.
/// Shared with `ExportSchema`, which promotes this component to the root of its draft-4
/// export — a document that declares no envelope names no root among its co-equal
/// components, and the published catalogue documents are of that kind (see
/// [`event_schema_document`]).
pub const EVENT_ENVELOPE_COMPONENT: &str = "AWSEvent";

/// The event name the CloudTrail-delivered API-call schema carries in the catalogue,
/// i.e. the `<EventName>` half of `<source>@<EventName>`.
///
/// Source: the catalogue names a schema after its event's `detail-type` with the
/// spaces taken out, so the `AWS API Call via CloudTrail` detail-type
/// ([`CLOUD_TRAIL_DETAIL_TYPE`]) is published as `AWSAPICallviaCloudTrail` — the
/// lower-case `via` is AWS's own, because the detail-type spells it that way.
pub const CLOUD_TRAIL_EVENT_NAME: &str = "AWSAPICallviaCloudTrail";

/// The `detail-type` EventBridge delivers a CloudTrail-recorded API call under.
pub const CLOUD_TRAIL_DETAIL_TYPE: &str = "AWS API Call via CloudTrail";

/// The CloudTrail-delivered event family the catalogue publishes for a source in
/// [`AWS_EVENT_SOURCES`], as `(detail-type, EventName)` pairs.
///
/// CloudTrail records more than API calls, and EventBridge delivers each KIND of record
/// under its own `detail-type`: a console action, a console sign-in, an Insights event,
/// a network-activity event and a service event beside the API call itself. The
/// catalogue holds one schema per kind for a source it publishes these for, so such a
/// source's entries are this whole family, not the API call alone; WHICH sources those
/// are is [`AWS_EVENT_SOURCES`].
///
/// The `detail` of every one of them IS a CloudTrail record, whose contents are the
/// same whichever kind it records (CloudTrail User Guide, "CloudTrail record
/// contents"), so one components template ([`CLOUD_TRAIL_COMPONENTS`]) serves the whole
/// family — published under each event's own component name.
///
/// The `EventName` beside each `detail-type` is that detail-type with its spaces taken
/// out, which is how the catalogue derives a schema's name; the pairs are written out
/// rather than computed so the published spelling (AWS's own lower-case `via`) is the
/// one in the table, and `the_cloud_trail_family_is_named_after_its_detail_types` below
/// holds the two to that rule.
pub const CLOUD_TRAIL_EVENT_FAMILY: &[(&str, &str)] = &[
    (CLOUD_TRAIL_DETAIL_TYPE, CLOUD_TRAIL_EVENT_NAME),
    (
        "AWS Console Action via CloudTrail",
        "AWSConsoleActionviaCloudTrail",
    ),
    (
        "AWS Console Sign In via CloudTrail",
        "AWSConsoleSignInviaCloudTrail",
    ),
    ("AWS Insight via CloudTrail", "AWSInsightviaCloudTrail"),
    (
        "AWS Network Activity Event via CloudTrail",
        "AWSNetworkActivityEventviaCloudTrail",
    ),
    (
        "AWS Service Event via CloudTrail",
        "AWSServiceEventviaCloudTrail",
    ),
];

/// The `components.schemas` of a CloudTrail-delivered event's document. The record
/// component is named by [`EVENT_COMPONENT_PLACEHOLDER`], because the same record is
/// published under each member of [`CLOUD_TRAIL_EVENT_FAMILY`].
///
/// Source: the document AWS PUBLISHES for a CloudTrail-delivered event, as the recorded
/// `DescribeSchema` of `aws.events@AWSAPICallviaCloudTrail` serves it — the model states
/// nothing about a document's text, so the published document is the reference, and it
/// is what this operation serves. The `detail` of a CloudTrail-delivered event IS a
/// CloudTrail record (CloudTrail User Guide, "CloudTrail record contents") and the
/// record is the same whichever source published it and whichever kind of record it is,
/// which is why one template serves all of [`AWS_EVENT_SOURCES`] and the whole family.
///
/// What the published document says about that record, and this template with it:
///
/// * it constrains the record's members but marks none of them required — CloudTrail
///   fills a member only for the records it applies to;
/// * the record's nested structures (a resource of `resources`, the `tlsDetails`, the
///   `userIdentity` and the structures nested in that identity) are declared INLINE, so
///   the document carries no component beside the event's own;
/// * the four free-form members of a record — `addendum`, `additionalEventData`,
///   `requestParameters` and `responseElements`, whose contents are the called API's
///   rather than CloudTrail's — are marked `nullable`, OpenAPI 3.0's way of saying the
///   member may be present and null; `ExportSchema` lowers that to draft-04's type
///   union. The document marks nothing else nullable: a member of a declared shape is
///   either written or absent;
/// * a member whose own members the published document does not declare (the
///   edge-device and event contexts, the identity an `inScopeOf`/`invokedByDelegate`
///   names, the provider attributes of a web-identity federation) is constrained to be
///   an object and no further;
/// * the session a temporary credential belongs to (`userIdentity.sessionContext`) is a
///   nested structure like the others, so it is declared inline WITH its own members —
///   the root-session marker, the session's attributes, the EC2 role-delivery version,
///   the entity that issued the session, the source identity it carries and its
///   web-identity federation data (CloudTrail User Guide, "CloudTrail userIdentity
///   element");
/// * the identity that made the call is declared WHOLE: every member a record's
///   `userIdentity` carries is declared, the kind of principal it is (`type`) and the
///   friendly name of an identity that has one (`userName`) included — those two are
///   the first facts a record states about a caller (CloudTrail User Guide, "CloudTrail
///   userIdentity element"), and a record this emulator itself writes carries them
///   (`services/common`'s CloudTrail record writer), so a binding or validator built
///   from this document has to know them;
/// * `eventTime` carries both the `date-time` format and the `yyyy-MM-dd'T'HH:mm:ss'Z'`
///   pattern the record writes it in, which is what a generated binding parses it with.
///
/// The members keep the order the published document writes them in: it is served as
/// written (see [`JsonNode`]), not re-ordered by the map it passes through.
const CLOUD_TRAIL_COMPONENTS: &str = r##"{
  "{EventName}": {
    "type": "object",
    "properties": {
      "addendum": {"type": "object", "nullable": true},
      "additionalEventData": {"type": "object", "nullable": true},
      "annotation": {"type": "string"},
      "apiVersion": {"type": "string"},
      "awsRegion": {"type": "string"},
      "edgeDeviceDetails": {"type": "object"},
      "errorCode": {"type": "string"},
      "errorMessage": {"type": "string"},
      "eventCategory": {"type": "string"},
      "eventContext": {"type": "object"},
      "eventID": {"type": "string"},
      "eventName": {"type": "string"},
      "eventSource": {"type": "string"},
      "eventTime": {"pattern": "yyyy-MM-dd'T'HH:mm:ss'Z'", "type": "string", "format": "date-time"},
      "eventType": {"type": "string"},
      "eventVersion": {"type": "string"},
      "managementEvent": {"type": "boolean"},
      "readOnly": {"type": "boolean"},
      "recipientAccountId": {"type": "string"},
      "requestID": {"type": "string"},
      "requestParameters": {"type": "object", "nullable": true},
      "resources": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "accountId": {"type": "string"},
            "ARN": {"type": "string"},
            "ARNPrefix": {"type": "string"},
            "type": {"type": "string"}
          }
        }
      },
      "responseElements": {"type": "object", "nullable": true},
      "sessionCredentialFromConsole": {"type": "string"},
      "sharedEventID": {"type": "string"},
      "sourceIPAddress": {"type": "string"},
      "tlsDetails": {
        "type": "object",
        "properties": {
          "cipherSuite": {"type": "string"},
          "clientProvidedHostHeader": {"type": "string"},
          "keyExchange": {"type": "string"},
          "tlsVersion": {"type": "string"}
        }
      },
      "userAgent": {"type": "string"},
      "userIdentity": {
        "type": "object",
        "properties": {
          "accessKeyId": {"type": "string"},
          "accountId": {"type": "string"},
          "arn": {"type": "string"},
          "credentialId": {"type": "string"},
          "identityProvider": {"type": "string"},
          "inScopeOf": {"type": "object"},
          "invokedBy": {"type": "string"},
          "invokedByDelegate": {"type": "object"},
          "onBehalfOf": {
            "type": "object",
            "properties": {
              "identityStoreArn": {"type": "string"},
              "userId": {"type": "string"}
            }
          },
          "principalId": {"type": "string"},
          "sessionContext": {
            "type": "object",
            "properties": {
              "assumedRoot": {"type": "string"},
              "attributes": {
                "type": "object",
                "properties": {
                  "creationDate": {"type": "string"},
                  "mfaAuthenticated": {"type": "string"}
                }
              },
              "ec2RoleDelivery": {"type": "string"},
              "sessionIssuer": {
                "type": "object",
                "properties": {
                  "accountId": {"type": "string"},
                  "arn": {"type": "string"},
                  "principalId": {"type": "string"},
                  "type": {"type": "string"},
                  "userName": {"type": "string"}
                }
              },
              "sourceIdentity": {"type": "string"},
              "webIdFederationData": {
                "type": "object",
                "properties": {
                  "attributes": {"type": "object"},
                  "federatedProvider": {"type": "string"}
                }
              }
            }
          },
          "type": {"type": "string"},
          "userName": {"type": "string"}
        }
      }
    }
  }
}"##;

/// The AWS event sources the managed catalogue holds a CloudTrail-delivered schema
/// FOR — the `aws.<service>` namespaces whose CloudTrail records EventBridge delivers,
/// under the detail-types of [`CLOUD_TRAIL_EVENT_FAMILY`].
///
/// Source: the event-source namespaces the published catalogue carries those schemas
/// under. WHICH sources they are is published data, read off the catalogue one source
/// at a time — it is not every `aws.<service>` namespace there is, and a source's
/// entry here says nothing about which of its OWN event types the catalogue publishes
/// ([`AWS_SERVICE_EVENT_TYPES`] carries those, per source, and a source may appear
/// there and not here). Kept in alphabetical order for reading; the store sorts by
/// name on its own, and a name repeated here seeds one schema, not two.
pub const AWS_EVENT_SOURCES: &[&str] = &[
    "aws.a4b",
    "aws.abuse",
    // `aws.access-analyzer` is deliberately NOT here: the catalogue publishes its
    // finding event types ([`AWS_SERVICE_EVENT_TYPES`]) and no CloudTrail-delivered
    // schema for it. Membership of this list is per-source published data, not every
    // event source there is.
    "aws.access-insights",
    "aws.account",
    "aws.acm",
    "aws.acm-pca",
    "aws.act",
    // `aws.aidevops` is deliberately NOT here: the catalogue publishes its investigation
    // lifecycle event types ([`AWS_SERVICE_EVENT_TYPES`]) and no CloudTrail-delivered
    // schema for it, so its block of the catalogue is those entries alone.
    "aws.airflow",
    "aws.amazon.connect",
    "aws.amazonmq",
    "aws.amazonq",
    "aws.amplify",
    "aws.amplifybackend",
    "aws.amplifyuibuilder",
    "aws.apigateway",
    "aws.appconfig",
    "aws.appflow",
    "aws.appintegrations",
    "aws.application-autoscaling",
    "aws.application-insights",
    "aws.applicationcostprofiler",
    "aws.appmesh",
    "aws.apprunner",
    "aws.appstream",
    "aws.appsync",
    "aws.aps",
    "aws.artifact",
    "aws.athena",
    "aws.auditmanager",
    "aws.autoscaling",
    "aws.autoscaling-plans",
    "aws.b2bi",
    "aws.backup",
    "aws.backup-gateway",
    "aws.batch",
    "aws.billingconductor",
    "aws.braket",
    "aws.budgets",
    "aws.cassandra",
    "aws.ce",
    "aws.chatbot",
    "aws.chime",
    "aws.chime-sdk-media-pipelines",
    "aws.chime-sdk-voice",
    "aws.cleanrooms",
    "aws.cloud9",
    "aws.clouddirectory",
    "aws.cloudformation",
    "aws.cloudfront",
    "aws.cloudhsm",
    "aws.cloudsearch",
    "aws.cloudshell",
    "aws.cloudtrail",
    "aws.cloudwatch",
    "aws.codeartifact",
    "aws.codebuild",
    "aws.codecommit",
    "aws.codedeploy",
    "aws.codeguru-profiler",
    "aws.codeguru-reviewer",
    "aws.codepipeline",
    "aws.codestar",
    "aws.codestar-connections",
    "aws.codestar-notifications",
    "aws.cognito-identity",
    "aws.cognito-idp",
    "aws.cognito-sync",
    "aws.comprehend",
    "aws.comprehendmedical",
    "aws.compute-optimizer",
    "aws.config",
    "aws.connect",
    "aws.connect-contact-lens",
    "aws.controltower",
    "aws.databrew",
    "aws.dataexchange",
    "aws.datapipeline",
    "aws.datasync",
    "aws.dax",
    "aws.deadline",
    "aws.deepracer",
    "aws.detective",
    "aws.devicefarm",
    "aws.devops-guru",
    "aws.directconnect",
    "aws.discovery",
    "aws.dlm",
    "aws.dms",
    "aws.drs",
    "aws.ds",
    "aws.dynamodb",
    "aws.ebs",
    "aws.ec2",
    "aws.ec2-instance-connect",
    "aws.ecr",
    "aws.ecr-public",
    "aws.ecs",
    "aws.eks",
    "aws.elasticache",
    "aws.elasticbeanstalk",
    "aws.elasticfilesystem",
    "aws.elasticloadbalancing",
    "aws.elasticmapreduce",
    "aws.elastictranscoder",
    "aws.email",
    "aws.emr-containers",
    "aws.emr-serverless",
    "aws.es",
    "aws.events",
    "aws.evidently",
    "aws.finspace",
    "aws.finspace-api",
    "aws.firehose",
    "aws.fis",
    "aws.fms",
    "aws.forecast",
    "aws.frauddetector",
    "aws.fsx",
    "aws.gamelift",
    "aws.glacier",
    "aws.globalaccelerator",
    "aws.glue",
    "aws.grafana",
    "aws.greengrass",
    "aws.groundstation",
    "aws.guardduty",
    "aws.health",
    "aws.health-agent",
    "aws.healthlake",
    "aws.iam",
    "aws.identitystore",
    "aws.imagebuilder",
    "aws.inspector",
    "aws.inspector-scan",
    "aws.inspector2",
    "aws.iot",
    "aws.iot1click",
    "aws.iotanalytics",
    "aws.iotdeviceadvisor",
    "aws.iotevents",
    "aws.iotfleethub",
    "aws.iotsecuredtunneling",
    "aws.iotsitewise",
    "aws.iottwinmaker",
    "aws.iotwireless",
    "aws.ivs",
    "aws.kafka",
    "aws.kafkaconnect",
    "aws.kendra",
    "aws.kinesis",
    "aws.kinesisanalytics",
    "aws.kinesisvideo",
    "aws.kms",
    "aws.lakeformation",
    "aws.lambda",
    "aws.lex",
    "aws.license-manager",
    "aws.lightsail",
    "aws.location",
    "aws.logs",
    "aws.lookoutequipment",
    "aws.lookoutmetrics",
    "aws.lookoutvision",
    "aws.machinelearning",
    "aws.macie",
    "aws.managedblockchain",
    "aws.mediaconnect",
    "aws.mediaconvert",
    "aws.medialive",
    "aws.mediapackage",
    "aws.mediapackage-vod",
    "aws.mediastore",
    "aws.mediatailor",
    "aws.memorydb",
    "aws.mgh",
    "aws.mgn",
    "aws.mq",
    "aws.mturk",
    "aws.neptune",
    "aws.network-firewall",
    "aws.networkmanager",
    "aws.nimble",
    "aws.opensearch",
    "aws.opsworks",
    "aws.organizations",
    "aws.outposts",
    "aws.panorama",
    "aws.personalize",
    "aws.pi",
    "aws.pinpoint",
    "aws.polly",
    "aws.pricing",
    "aws.proton",
    "aws.qldb",
    "aws.quicksight",
    "aws.ram",
    "aws.rbin",
    "aws.rds",
    "aws.redshift",
    "aws.redshift-data",
    "aws.redshift-serverless",
    "aws.rekognition",
    "aws.resiliencehub",
    "aws.resource-groups",
    "aws.robomaker",
    "aws.route53",
    "aws.route53domains",
    "aws.route53resolver",
    "aws.rum",
    "aws.s3",
    "aws.s3-outposts",
    "aws.sagemaker",
    "aws.savingsplans",
    "aws.scheduler",
    "aws.schemas",
    "aws.sdb",
    "aws.secretsmanager",
    "aws.securityhub",
    "aws.serverlessrepo",
    "aws.servicecatalog",
    "aws.servicediscovery",
    "aws.servicequotas",
    "aws.ses",
    "aws.shield",
    "aws.signer",
    "aws.signin",
    "aws.sms",
    "aws.snowball",
    "aws.sns",
    "aws.sqs",
    "aws.ssm",
    "aws.ssm-contacts",
    "aws.ssm-incidents",
    "aws.sso",
    "aws.states",
    "aws.storagegateway",
    "aws.sts",
    "aws.support",
    "aws.swf",
    "aws.synthetics",
    "aws.tag",
    "aws.textract",
    "aws.timestream",
    "aws.transcribe",
    "aws.transfer",
    "aws.translate",
    "aws.trustedadvisor",
    "aws.voiceid",
    "aws.waf",
    "aws.waf-regional",
    "aws.wafv2",
    "aws.wellarchitected",
    "aws.wisdom",
    "aws.workdocs",
    "aws.worklink",
    "aws.workmail",
    "aws.workspaces",
    "aws.xray",
];

/// The service-owned registries a fresh store holds. Every entry of the catalogue
/// lives in [`AWS_MANAGED_REGISTRY`], the one registry AWS publishes event schemas in.
pub const AWS_MANAGED_REGISTRIES: &[&str] = &[AWS_MANAGED_REGISTRY];

/// One AWS event type the service-owned catalogue holds a schema for.
///
/// It is the event's IDENTITY plus the shape of its `detail`; the OpenAPI document
/// itself is rendered from them by [`event_schema_document`], because the header the
/// catalogue publishes a document under is the same for every AWS event.
#[derive(Debug, Clone, Copy)]
pub struct ManagedEventType {
    /// The event source that publishes it: `aws.<service>`.
    pub source: &'static str,
    /// The `detail-type` EventBridge delivers the event under.
    pub detail_type: &'static str,
    /// The name the catalogue publishes the event type under: the `<EventName>` half
    /// of `<source>@<EventName>`, which AWS derives from the `detail_type`.
    pub event_name: &'static str,
    /// The document's `components.schemas`: the component describing the event's
    /// `detail` (the one named [`Self::event_name`]) together with everything that
    /// component references in turn.
    pub components: &'static str,
}

impl ManagedEventType {
    /// One member of the CloudTrail-delivered event family of `source` — the family
    /// every AWS event source publishes, so it is rendered per source rather than
    /// written out per source. See [`CLOUD_TRAIL_EVENT_FAMILY`].
    pub const fn cloud_trail(
        source: &'static str,
        detail_type: &'static str,
        event_name: &'static str,
    ) -> Self {
        Self {
            source,
            detail_type,
            event_name,
            components: CLOUD_TRAIL_COMPONENTS,
        }
    }

    /// An event type the catalogue publishes by its IDENTITY alone — the source and
    /// the `detail-type` EventBridge delivers it under — because AWS documents that
    /// the event exists without documenting the members of its `detail`. See
    /// [`UNDOCUMENTED_DETAIL`] for what the rendered document then says.
    pub const fn identified(
        source: &'static str,
        detail_type: &'static str,
        event_name: &'static str,
    ) -> Self {
        Self {
            source,
            detail_type,
            event_name,
            components: UNDOCUMENTED_DETAIL,
        }
    }

    /// `<source>@<EventName>` — the name the catalogue publishes this event type
    /// under, which is what `ListSchemas` reports and `DescribeSchema` takes.
    pub fn schema_name(&self) -> String {
        format!("{}@{}", self.source, self.event_name)
    }

    /// This event type's `components.schemas`, with the event's own component name
    /// substituted for [`EVENT_COMPONENT_PLACEHOLDER`]: one template serves a whole
    /// family of event types, each published under its own component name.
    pub fn components_json(&self) -> String {
        self.components
            .replace(EVENT_COMPONENT_PLACEHOLDER, self.event_name)
    }

    /// This event type as the catalogue holds it. `None` when its document cannot be
    /// rendered — see [`event_schema_document`].
    pub fn schema(&self) -> Option<ManagedSchema> {
        let name = self.schema_name();
        Some(ManagedSchema {
            description: managed_schema_description(&name),
            event: PublishedEvent {
                source: self.source.to_string(),
                detail_type: self.detail_type.to_string(),
            },
            content: event_schema_document(
                self.source,
                self.detail_type,
                self.event_name,
                &self.components_json(),
            )?,
            last_modified: managed_last_modified(&name),
            version_count: managed_version_count(&name),
            name,
        })
    }
}

/// One schema of the service-owned catalogue, as [`aws_managed_schemas`] renders it
/// and the store seeds it.
#[derive(Debug, Clone)]
pub struct ManagedSchema {
    /// `<source>@<EventName>`, the naming the catalogue publishes under.
    pub name: String,
    /// The sentence AWS describes a catalogue entry with — see
    /// [`managed_schema_description`].
    pub description: Option<String>,
    /// The `OpenApi3` document of the event type, as `DescribeSchema` serves it and
    /// `ExportSchema` converts it.
    pub content: String,
    /// When the published schema was last modified — see [`managed_last_modified`].
    pub last_modified: SystemTime,
    /// How many versions the catalogue publishes for it — see
    /// [`managed_version_count`].
    pub version_count: u64,
    /// The event this entry publishes a schema FOR — see [`PublishedEvent`].
    pub event: PublishedEvent,
}

/// The event a published catalogue entry describes: the `source` that delivers it and
/// the `detail-type` it is delivered under.
///
/// A published document describes the event's `detail` and states the event's identity
/// nowhere in its text (see [`event_schema_document`]), so the identity is held beside
/// the document — it is what `SearchSchemas` indexes a published schema under beside its
/// name and the text of its definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedEvent {
    /// The event source that publishes it: `aws.<service>`.
    pub source: String,
    /// The `detail-type` the event is delivered under.
    pub detail_type: String,
}

/// When the AWS-managed catalogue's entry named `schema_name` was last modified.
///
/// `LastModified` is "the date and time that schema was modified": a property of the
/// PUBLISHED schema, which is what a client compares against its cached copy to decide
/// whether that copy is stale. So what the member has to satisfy is that it is the
/// instant that schema was PUBLISHED at — fixed, at whole-second precision, and the same
/// in every process and across every reset of this store. A reading of the clock
/// satisfies none of that: it moves the instant on every boot, and so invalidates every
/// client's cache on every run.
///
/// The instant is catalogue DATA and is computed from NOTHING: a publication writes a
/// SET of entries at one instant, so the entries of one publication all report that
/// instant and an entry's own name says nothing about when it was published. The
/// publications the catalogue publishes an instant of their own for are
/// [`MANAGED_PUBLICATIONS`]; every entry none of them names belongs to the publication
/// that wrote the rest of the catalogue, [`CATALOGUE_PUBLICATION_INSTANT`].
pub fn managed_last_modified(schema_name: &str) -> SystemTime {
    published_instant(managed_publication_instant(schema_name))
}

/// The `date-time` the catalogue publishes as the `LastModified` of the entry named
/// `schema_name`: the instant of the publication that wrote that entry.
fn managed_publication_instant(schema_name: &str) -> &'static str {
    MANAGED_PUBLICATIONS
        .iter()
        .find(|(_, published)| published.contains(&schema_name))
        .map_or(CATALOGUE_PUBLICATION_INSTANT, |(instant, _)| *instant)
}

/// The publications the catalogue publishes an instant OF THEIR OWN for: each one's
/// instant and the entries it published.
///
/// A publication instant is a fact about the entries that publication wrote and no
/// function of their names — two entries of one source, named alike but for a letter's
/// case, report instants years apart, while entries of one publication whose names have
/// nothing in common report the same instant to the second. So it is CATALOGUE DATA,
/// seeded like an entry's name, its version count and its document.
///
/// Source: the instants the published catalogue reports for these entries, which are
/// historical — a publication of the past, years before the catalogue was otherwise
/// written, reported identically on every request.
///
/// Each instant is a whole-second `date-time`, as [`published_instant`] reads it.
const MANAGED_PUBLICATIONS: &[(&str, &[&str])] = &[
    (
        "2019-12-01T00:59:43Z",
        &["aws.autoscaling@AWSAPICallViaCloudTrail"],
    ),
    // The EC2 Fleet entries: the fleet events were published in one pass and the spot
    // fleet events in the next second, so each set reports its own pass's instant —
    // years before the rest of their source's entries.
    (
        "2020-12-17T18:43:39Z",
        &[
            "aws.ec2fleet@EC2FleetError",
            "aws.ec2fleet@EC2FleetInformation",
            "aws.ec2fleet@EC2FleetInstanceChange",
            "aws.ec2fleet@EC2FleetSpotInstanceRequestChange",
            "aws.ec2fleet@EC2FleetStateChange",
        ],
    ),
    (
        "2020-12-17T18:43:40Z",
        &[
            "aws.ec2fleet@EC2SpotFleetError",
            "aws.ec2fleet@EC2SpotFleetInformation",
            "aws.ec2fleet@EC2SpotFleetInstanceChange",
            "aws.ec2fleet@EC2SpotFleetSpotInstanceRequestChange",
            "aws.ec2fleet@EC2SpotFleetStateChange",
        ],
    ),
    ("2022-03-24T16:40:20Z", &["aws.tag@TagChangeOnResource"]),
    ("2026-03-12T21:12:29Z", &["aws.health@AWSHealthAbuseEvent"]),
];

/// One of [`MANAGED_PUBLICATIONS`]' instants as an instant.
///
/// Read by the same `date-time` reader the wire uses, so a seeded instant is exactly
/// the instant its text names rather than one hand-computed from calendar arithmetic
/// here. Panics for text that is not a `date-time`: these are constants, so that is a
/// typo in the table rather than a runtime condition, and it fails in every build
/// instead of making an entry report an instant the catalogue does not publish.
fn published_instant(text: &str) -> SystemTime {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("the seeded publication instant {text:?} must be a date-time: {error}")
    });
    let seconds = u64::try_from(parsed.secs()).unwrap_or_else(|_| {
        panic!("the seeded publication instant {text:?} must be after the Unix epoch")
    });
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The instant of the publication that wrote the catalogue this store seeds: what every
/// entry [`MANAGED_PUBLICATIONS`] names no earlier publication of its own for reports.
///
/// The entries of one publication report one instant, so a catalogue written by a single
/// publication reports a single instant for the entries it wrote — which is what the
/// published catalogue does for the entries of any one of its passes, whatever their
/// names. Nothing here is computed from an entry: a value derived from the name would
/// scatter the entries of one publication over unrelated moments, which is precisely
/// what `LastModified` is read to rule out.
///
/// It is 2019-12-01T00:00:00Z, when the event schema registry became available, so the
/// catalogue this store seeds was published as that registry came up. Fixed seed data,
/// so it reports the same instant in every process and across every reset of the store.
/// It bounds no other member: an earlier VERSION of an entry was created before the
/// publication that wrote the entry, which is where [`MANAGED_VERSION_INTERVAL`] lays
/// each one.
const CATALOGUE_PUBLICATION_INSTANT: &str = "2019-12-01T00:00:00Z";

/// How far apart the catalogue's versions of one schema are published.
///
/// AWS publishes no interval, so this span is the emulator's own: what the store owes
/// is that version N of a schema was created before version N+1 and that the newest
/// version's creation is the schema's `LastModified`, so the versions are laid out
/// backwards from that instant, one month apart.
const MANAGED_VERSION_INTERVAL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The description a catalogue entry carries, derived from its name.
///
/// AWS describes every schema in the managed catalogue with one sentence built from
/// the schema's own name: a `<source>@<EventName>` schema is "Schema for event type
/// `<EventName>`, published by AWS service `<source>`". `None` for a name that is not
/// of that form, which describes no event type and so has no such sentence.
pub fn managed_schema_description(schema_name: &str) -> Option<String> {
    let (source, event_name) = schema_name.split_once('@')?;
    Some(format!(
        "Schema for event type {event_name}, published by AWS service {source}"
    ))
}

/// Render the `OpenApi3` document of one AWS event type: the `components.schemas` of
/// its `detail`, under the document header the catalogue publishes them with.
///
/// A published catalogue document describes the EVENT TYPE, i.e. the `detail` AWS
/// delivers for it: its `components.schemas` are the component named after the event
/// together with whatever that component references in turn, and nothing else. It
/// declares no EventBridge envelope around them and stamps no `x-amazon-events-*`
/// extension on them — the document AWS publishes for a catalogue entry carries one
/// component per schema the event's `detail` needs, with `type` before `properties`,
/// and that is the whole of it.
///
/// The document's `info.title` is the sentence the catalogue titles a published event
/// schema with: the event's `detail-type`, then which source publishes it
/// (`AWS API Call via CloudTrail published by aws.events`). It names the EVENT, not the
/// schema's component, so a client that renders the title shows the event the document
/// describes; `ExportSchema` carries the same title onto the draft-04 translation.
///
/// The document is assembled through [`JsonNode`], not [`serde_json::Value`], because
/// the published document's MEMBER ORDER is part of it: it opens with `openapi`, then
/// `info` (its `title` before its `version`), then `paths`, then `components`, and each
/// component's `type` precedes its `properties`. A `serde_json::Map` cannot state that
/// — it holds members by name or by insertion depending on a workspace-wide feature —
/// and `Content` is text a client caches, diffs and hashes.
///
/// `None` when `components` is not a JSON object, or does not define the component the
/// document is published for (the one named after the event) as an object: that is a
/// document with nothing to describe, and the catalogue leaves it out rather than
/// seeding a schema that describes no event.
fn event_schema_document(
    source: &str,
    detail_type: &str,
    event_name: &str,
    components: &str,
) -> Option<String> {
    let JsonNode::Object(schemas) = JsonNode::parse(components)? else {
        return None;
    };
    let event = schemas
        .iter()
        .find(|(name, _)| name == event_name)
        .map(|(_, component)| component)?;
    if !matches!(event, JsonNode::Object(_)) {
        return None;
    }
    Some(
        JsonNode::Object(vec![
            ("openapi".to_string(), JsonNode::text(MANAGED_OPENAPI_VERSION)),
            (
                "info".to_string(),
                JsonNode::Object(vec![
                    (
                        "title".to_string(),
                        JsonNode::Text(managed_schema_title(source, detail_type)),
                    ),
                    ("version".to_string(), JsonNode::text(MANAGED_DOCUMENT_VERSION)),
                ]),
            ),
            ("paths".to_string(), JsonNode::Object(Vec::new())),
            (
                "components".to_string(),
                JsonNode::Object(vec![("schemas".to_string(), JsonNode::Object(schemas))]),
            ),
        ])
        .to_json(),
    )
}

/// The OpenAPI dialect every published event document declares.
const MANAGED_OPENAPI_VERSION: &str = "3.0.0";

/// The `info.version` every published event document carries: the document describes
/// one event type, which the catalogue publishes one revision of at a time.
const MANAGED_DOCUMENT_VERSION: &str = "1.0.0";

/// The title the catalogue gives the document of one published event type:
/// `<detail-type> published by <source>`.
///
/// Shared with `ExportSchema`, which reports the source document's title on the
/// draft-04 translation it renders, so the two cannot drift.
pub fn managed_schema_title(source: &str, detail_type: &str) -> String {
    format!("{detail_type} published by {source}")
}

/// Every event type the service-owned catalogue holds a schema for: the whole
/// CloudTrail-delivered event family ([`CLOUD_TRAIL_EVENT_FAMILY`]) of every source in
/// [`AWS_EVENT_SOURCES`], followed by the service-specific event types in
/// [`AWS_SERVICE_EVENT_TYPES`].
///
/// The service-specific entries come last, so one of them wins over a generated entry
/// of the same name: the store seeds entries in this order and keys them by name.
pub fn aws_managed_event_types() -> Vec<ManagedEventType> {
    AWS_EVENT_SOURCES
        .iter()
        .flat_map(|source| {
            CLOUD_TRAIL_EVENT_FAMILY
                .iter()
                .map(move |(detail_type, event_name)| {
                    ManagedEventType::cloud_trail(source, detail_type, event_name)
                })
        })
        .chain(AWS_SERVICE_EVENT_TYPES.iter().copied())
        .collect()
}

/// The whole service-owned catalogue, one entry per schema: every event type
/// [`aws_managed_event_types`] names, as the name, description and document the store
/// seeds and the read operations report.
///
/// Rendered ONCE per process and handed out by reference afterwards: the catalogue is
/// built from constants, so it is the same in every store, and rendering it renders
/// every member's document.
pub fn aws_managed_schemas() -> &'static [ManagedSchema] {
    static CATALOGUE: ::std::sync::OnceLock<Vec<ManagedSchema>> = ::std::sync::OnceLock::new();
    CATALOGUE.get_or_init(|| {
        aws_managed_event_types()
            .iter()
            .map(|event| {
                // A row whose `components` do not parse, or do not define the
                // component named after the event, describes no event type. That is a
                // typo in the seed data above rather than a runtime condition — the
                // rows are constants — so it fails here, in every build, rather than
                // quietly shrinking the catalogue the service serves.
                event.schema().unwrap_or_else(|| {
                    panic!(
                        "the seeded event type {}@{} must render a document",
                        event.source, event.event_name
                    )
                })
            })
            .collect()
    })
}

/// Build the service-owned registries and seed them with [`aws_managed_schemas`].
///
/// Their ARNs are left empty: a service-owned registry is minted by no request, so no
/// request's region is stored for it, and [`Registry::arn_in`] derives the ARN from
/// the region of the request asking. A service-owned SCHEMA carries no ARN at all —
/// see [`Registry::schema_arn_in`].
///
/// Nothing here reads the clock: every instant a catalogue entry reports is the
/// published schema's own ([`managed_last_modified`]), so two stores seeded at two
/// different moments report the same catalogue.
fn aws_managed_registries() -> BTreeMap<String, Registry> {
    let mut registries: BTreeMap<String, Registry> = AWS_MANAGED_REGISTRIES
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                Registry {
                    name: (*name).to_string(),
                    arn: String::new(),
                    description: None,
                    tags: BTreeMap::new(),
                    scope: RegistryScope::Aws,
                    // A service-owned registry carries no caller-written policy.
                    policy: None,
                    schemas: BTreeMap::new(),
                },
            )
        })
        .collect();

    let Some(registry) = registries.get_mut(AWS_MANAGED_REGISTRY) else {
        return registries;
    };
    for entry in aws_managed_schemas() {
        // Every version the catalogue publishes is stored, so the `VersionCount` a
        // listing reports is the count of versions this store actually holds and a
        // client that pins one of them can read it. They are laid out backwards from
        // the schema's last-modified instant: the newest version was created then, and
        // each earlier one a [`MANAGED_VERSION_INTERVAL`] before its successor. The
        // document is the one AWS publishes for the schema, which is the document of
        // its newest version — AWS publishes no earlier revision of it, so an earlier
        // version carries the same document rather than one invented here.
        let versions: BTreeMap<u64, SchemaVersion> = (1..=entry.version_count.max(1))
            .map(|version| {
                let age = MANAGED_VERSION_INTERVAL * u32::try_from(entry.version_count - version)
                    .unwrap_or(u32::MAX);
                (
                    version,
                    SchemaVersion {
                        version,
                        content: entry.content.clone(),
                        r#type: AWS_MANAGED_SCHEMA_TYPE.to_string(),
                        created_date: entry
                            .last_modified
                            .checked_sub(age)
                            .unwrap_or(entry.last_modified),
                    },
                )
            })
            .collect();
        registry.schemas.insert(
            entry.name.clone(),
            Schema {
                name: entry.name.clone(),
                arn: String::new(),
                description: entry.description.clone(),
                r#type: AWS_MANAGED_SCHEMA_TYPE.to_string(),
                tags: BTreeMap::new(),
                latest_version: entry.version_count.max(1),
                last_modified: entry.last_modified,
                versions,
                minted_by_token: BTreeMap::new(),
                code_bindings: BTreeMap::new(),
                // A published entry's document does not state the event it describes,
                // so the identity the catalogue publishes it under is stored beside the
                // document: it is what a keyword search over this registry matches.
                published_event: Some(entry.event.clone()),
            },
        );
    }
    registries
}

/// The languages this service generates code bindings for, in the identifiers the
/// API takes in the `Language` URI label.
///
/// Source: the `Language` member is modelled as a plain `__string` with no enum and
/// no pattern (`smithy_query(query="operation",operation="PutCodeBinding")`), so the
/// accepted set is the SERVICE's, not the model's. The service states it in the
/// refusal it answers a bad `Language` with — "Parameter CodeBindingLanguage doesn't
/// match pattern `^(Java8|TypeScript3|Python36|Go1)$`" — i.e. the code-binding
/// languages EventBridge Schemas generates for: Java 8+, TypeScript 3+, Python 3.6+
/// and Go 1.x. Anything else is a language this service cannot generate, and is
/// refused rather than recorded.
///
/// The order is the pattern's: the alternation in the refusal below is built from
/// this list, so the two cannot drift apart.
pub const CODE_BINDING_LANGUAGES: &[&str] = &["Java8", "TypeScript3", "Python36", "Go1"];

/// The name the service's own validation gives the `Language` URI label it checks
/// against [`code_binding_language_pattern`].
pub const CODE_BINDING_LANGUAGE_PARAMETER: &str = "CodeBindingLanguage";

/// The pattern an acceptable `Language` matches: the alternation of
/// [`CODE_BINDING_LANGUAGES`], anchored at both ends, which is why a language family
/// without its version suffix and a differently-cased spelling are both refused.
pub fn code_binding_language_pattern() -> String {
    format!("^({})$", CODE_BINDING_LANGUAGES.join("|"))
}

/// The canonical language identifier a request named, or `None` when it named a
/// language this service does not generate bindings for.
///
/// The comparison is exact: these identifiers are wire values, and a service that
/// quietly accepted another spelling would persist a binding under a language a
/// later read cannot ask for.
pub fn code_binding_language(requested: &str) -> Option<&'static str> {
    CODE_BINDING_LANGUAGES
        .iter()
        .copied()
        .find(|language| *language == requested)
}

/// A schema LANGUAGE this service knows, as a request's `Type` names it.
///
/// One variant per value of [`TYPE_VALUES`], the model's `Type` enum, and the
/// correspondence is checked at COMPILE time below — so the set a `Type` is admitted
/// from, the set a refusal names ([`unsupported_schema_type`]) and the branches the
/// handlers take are one list, not three that can drift apart.
///
/// The languages differ in more than their name — each spells a nullable position its
/// own way, and only one of them is a language an export can be rendered FROM — so
/// every operation that decides something from a `Type` branches on this rather than
/// comparing wire strings of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaType {
    /// JSON Schema draft 4.
    JsonSchemaDraft4,
    /// OpenAPI 3.
    OpenApi3,
}

impl SchemaType {
    /// Every language this service knows, in [`TYPE_VALUES`] order.
    pub const ALL: &'static [Self] = &[Self::JsonSchemaDraft4, Self::OpenApi3];

    /// This language as `Type` renders it on the wire — its value in [`TYPE_VALUES`].
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::JsonSchemaDraft4 => "JSONSchemaDraft4",
            Self::OpenApi3 => "OpenApi3",
        }
    }

    /// The language a request's `Type` names, or the refusal of a value the model's
    /// `Type` enum does not hold.
    ///
    /// Every operation whose `Type` the boundary validator cannot check for it —
    /// because the member is modelled a plain `__string` there — reaches the admitted
    /// set through here, so no two of them can disagree about which languages this
    /// service takes. The comparison is exact: these are wire values, and a service
    /// that quietly accepted another spelling would answer a language the request did
    /// not ask for.
    pub fn from_modeled(requested: &str) -> Result<Self, BadRequestException> {
        Self::ALL
            .iter()
            .copied()
            .find(|language| language.as_wire() == requested)
            .ok_or_else(unsupported_schema_type)
    }
}

/// [`SchemaType`] holds exactly the values the model's `Type` enum does, in its order.
/// A model that gains or renames one fails the BUILD here rather than leaving a handler
/// refusing a value the refusal it answers with says is admitted.
const _: () = {
    assert!(SchemaType::ALL.len() == TYPE_VALUES.len());
    let mut index = 0;
    while index < SchemaType::ALL.len() {
        assert!(same_text(SchemaType::ALL[index].as_wire(), TYPE_VALUES[index]));
        index += 1;
    }
};

/// Whether two strings are the same text, decidable while the crate is being compiled —
/// which `==` on `str` is not. See the assertion above, its only caller.
const fn same_text(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// Refuse a `Type` that is not a schema language this service knows.
///
/// Names the member and the values it admits — [`TYPE_VALUES`], the model's own `Type`
/// enum, which is also the set [`SchemaType::from_modeled`] tests against — as a
/// validation refusal does. The rejected value is not echoed back: the caller already
/// holds what it sent, and the admitted set is what it needs.
pub fn unsupported_schema_type() -> BadRequestException {
    bad_request(format!("Type must be one of {}.", TYPE_VALUES.join(", ")))
}

/// The request parameter an export's target language arrives in, as the service's own
/// refusal of a bad value names it.
///
/// Source: the model binds `ExportSchema`'s `Type` to `@httpQuery("type")`
/// (`smithy_query(query="operation",operation="ExportSchema")`), and a value constraint
/// is reported against the member as the request spells it.
pub const EXPORT_TYPE_PARAMETER: &str = "type";

/// The languages an export can be rendered INTO: JSON Schema draft 4 and nothing else.
///
/// Narrower than [`SchemaType::ALL`], which is the set of languages a schema may be
/// STORED as: `ExportSchema`'s `Type` names the export TARGET, and the service produces
/// one kind of export — a schema asked for in the language it is already stored in is
/// not an export of it, it is a read.
pub const EXPORT_TARGETS: &[SchemaType] = &[SchemaType::JsonSchemaDraft4];

/// The export target a request's `Type` names, or the refusal of a value that names
/// none.
///
/// Separate from [`SchemaType::from_modeled`], the service-wide set of schema
/// languages: a value that is a language this service stores is still refused here
/// unless it is also a language an export is rendered into.
pub fn export_target(requested: &str) -> Result<SchemaType, BadRequestException> {
    EXPORT_TARGETS
        .iter()
        .copied()
        .find(|target| target.as_wire() == requested)
        .ok_or_else(|| not_an_export_target(requested))
}

/// Refuse a `Type` that names no export target ([`EXPORT_TARGETS`]).
///
/// Worded the way this service reports a member whose value is outside the set the
/// member admits: the number of values that failed, the value itself as the request
/// sent it, the member it was checked at, and the set it must come from.
pub fn not_an_export_target(requested: &str) -> BadRequestException {
    let admitted: Vec<&str> = EXPORT_TARGETS
        .iter()
        .map(|target| target.as_wire())
        .collect();
    bad_request(format!(
        "1 validation error detected: Value '{requested}' at '{EXPORT_TYPE_PARAMETER}' failed \
         to satisfy constraint: Member must satisfy enum value set: [{}]",
        admitted.join(", ")
    ))
}

/// Refuse an `ExportSchema` of a schema the service did not write itself.
///
/// An export is served only for a schema whose definition the SERVICE authored — one
/// AWS publishes, or one schema discovery wrote — because the export is the service's
/// own rendering of its own document; a definition a caller wrote is served back as the
/// caller wrote it ([`crate::ops::describe_schema`]) and is not exported. The sentence
/// is entirely fixed text: it names no request member.
pub fn schema_is_not_exportable() -> ForbiddenException {
    forbidden("You cannot export non discovered or non aws managed schemas.")
}

/// The constraint this service holds one of its caller-supplied resource NAMES to:
/// the parameter a refusal names it by, the characters an acceptable name is built
/// from, and the longest name the service accepts.
///
/// Both names a caller supplies here — `RegistryName` and `SchemaName` — are modelled
/// as a plain `__string` with no `@length` and no `@pattern`
/// (`smithy_query(query="shape",shape="CreateRegistryRequest,CreateSchemaRequest")`),
/// so the boundary validator enforces nothing and the constraint is the SERVICE's,
/// exactly as it is for the `Language` label ([`CODE_BINDING_LANGUAGES`]). A name the
/// service does not accept names no resource it could ever have created, so it is
/// refused on the REQUEST — before any resource is resolved and before anything is
/// stored — rather than recorded under a name whose ARN this service could not mint.
#[derive(Debug)]
pub struct NameConstraint {
    /// The request parameter the name arrives in, as the service's own refusal names
    /// it.
    pub parameter: &'static str,
    /// The characters an acceptable name is built from, as inclusive ranges in the
    /// order the service's published pattern lists them (a single character is a range
    /// of itself).
    ///
    /// This is the ONE place the accepted set is written down: the check reads these
    /// ranges ([`NameConstraint::validate`]) and the pattern the refusal quotes is
    /// rendered from them ([`NameConstraint::pattern`]), so the sentence cannot state a
    /// rule the check does not apply.
    pub characters: &'static [(char, char)],
    /// The longest name the service accepts, in characters.
    pub max_length: usize,
}

impl NameConstraint {
    /// The pattern an acceptable name matches, as the refusal quotes it: the accepted
    /// ranges as one anchored character class.
    pub fn pattern(&self) -> String {
        let mut class = String::new();
        for (first, last) in self.characters {
            Self::write_class_character(&mut class, *first);
            if first != last {
                class.push('-');
                Self::write_class_character(&mut class, *last);
            }
        }
        format!("^[{class}]+$")
    }

    /// One character of the class, escaped where a class would otherwise read it as
    /// syntax rather than as itself.
    fn write_class_character(class: &mut String, character: char) {
        if matches!(character, '.' | '-' | '^' | ']' | '\\') {
            class.push('\\');
        }
        class.push(character);
    }

    /// Whether `character` is one of the characters this constraint accepts.
    fn permits(&self, character: char) -> bool {
        self.characters
            .iter()
            .any(|(first, last)| (*first..=*last).contains(&character))
    }

    /// Check a name this constraint governs.
    ///
    /// `Err` is the refusal to answer, in the service's own parameter-validation voice:
    /// it names the parameter it checked and the rule that parameter broke, so a caller
    /// can fix the request from the message alone. One rule per refusal, and the length
    /// is decided first, so a name that breaks both is reported against its length.
    ///
    /// The rejected value is never echoed back: the caller already holds what it sent,
    /// and the rule is what it needs.
    pub fn validate(&self, name: &str) -> Result<(), BadRequestException> {
        if name.chars().count() > self.max_length {
            return Err(bad_request(format!(
                "Parameter {} must have length less than or equal to {}",
                self.parameter, self.max_length
            )));
        }
        // The class is `+`-quantified, so the empty name satisfies no pattern either.
        if name.is_empty() || !name.chars().all(|character| self.permits(character)) {
            return Err(bad_request(format!(
                "Parameter {} doesn't match pattern {}",
                self.parameter,
                self.pattern()
            )));
        }
        Ok(())
    }
}

/// The characters a registry name is built from, which a schema name extends: letters,
/// digits and the three separators, in the order the published pattern lists them.
const REGISTRY_NAME_CHARACTERS: &[(char, char)] = &[
    ('.', '.'),
    ('-', '-'),
    ('_', '_'),
    ('A', 'Z'),
    ('a', 'z'),
    ('0', '9'),
];

/// [`REGISTRY_NAME_CHARACTERS`] plus the `@` a schema name may also carry.
const SCHEMA_NAME_CHARACTERS: &[(char, char)] = &[
    ('.', '.'),
    ('-', '-'),
    ('_', '_'),
    ('A', 'Z'),
    ('a', 'z'),
    ('0', '9'),
    ('@', '@'),
];

/// The constraint a `RegistryName` a caller supplies must satisfy: letters, digits and
/// the three separators `.`, `-` and `_`, up to 64 characters.
///
/// Source: the constraint the service publishes for a registry's name —
/// `AWS::EventSchemas::Registry`'s `RegistryName` (pattern `[\.\-_A-Za-z0-9]+`,
/// maximum length 64), which is the same name this API takes in its `RegistryName`
/// label. The service-owned registry [`AWS_MANAGED_REGISTRY`] satisfies it, as every
/// name this service could have issued an ARN for does.
pub static REGISTRY_NAME: NameConstraint = NameConstraint {
    parameter: "RegistryName",
    characters: REGISTRY_NAME_CHARACTERS,
    max_length: 64,
};

/// The constraint a `SchemaName` a caller supplies must satisfy: the registry name's
/// character set plus `@`, up to 385 characters.
///
/// Source: the constraint the service publishes for a schema's name —
/// `AWS::EventSchemas::Schema`'s `SchemaName` (pattern `[\.\-_A-Za-z0-9@]+`, maximum
/// length 385). `@` is in the set because it is the separator the published catalogue's
/// own names carry between an event source and its event type
/// (`aws.events@ScheduledEvent`), and those are schema names of this service.
pub static SCHEMA_NAME: NameConstraint = NameConstraint {
    parameter: "SchemaName",
    characters: SCHEMA_NAME_CHARACTERS,
    max_length: 385,
};

/// Check the caller-supplied NAMES one request carries, in the order the operation's
/// URI states them: the registry first, then the schema.
///
/// Every operation that takes one of these names routes through here — a reporting one
/// as much as a mutating one, the way both of the code-binding operations check their
/// `Language` label ([`unsupported_code_binding_language`]) — so no two of them can
/// disagree about which names this service accepts. A name the service does not accept
/// names no resource anywhere, so the request is refused on its form rather than
/// answered as if the name simply named nothing yet.
///
/// `None` is a name the operation does not take, or an optional one the request left
/// out; there is nothing to check in either case.
pub fn validate_names(
    registry: Option<&str>,
    schema: Option<&str>,
) -> Result<(), BadRequestException> {
    if let Some(registry) = registry {
        REGISTRY_NAME.validate(registry)?;
    }
    if let Some(schema) = schema {
        SCHEMA_NAME.validate(schema)?;
    }
    Ok(())
}

/// The registry a POLICY request names, or why it names none this service accepts.
///
/// The three resource-policy operations — the put, the get and the delete — all bind
/// `RegistryName` as an OPTIONAL query parameter, and all three take the same two steps
/// before they reach the store: a resource-based policy is a property of ONE registry,
/// so a request carrying no name names no resource to read, write or remove
/// ([`registry_name_required`]), and a name the service does not accept names no
/// registry anywhere, so it is refused on its form rather than answered as if the
/// registry merely did not exist yet ([`validate_names`]). Requiring and checking
/// together is what keeps the second step from being forgotten by a handler that
/// remembers the first — the reason
/// [`ComAmazonawsSchemasSchemasState::writable_registry`] and
/// [`ComAmazonawsSchemasSchemasState::readable_schema_version`] resolve and refuse in
/// one place too, and what a fourth policy-scoped operation would inherit by calling
/// this.
///
/// The refusal is RETURNED rather than worded at the call site, because each operation
/// answers it through its own modelled error enum; both sentences are the service's
/// constants, so every call site words them alike.
pub fn policy_registry_name(registry: Option<String>) -> Result<String, BadRequestException> {
    let registry = registry.ok_or_else(registry_name_required)?;
    validate_names(Some(&registry), None)?;
    Ok(registry)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSchemasSchemasState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSchemasSchemasState>)
}

/// The region a request's ARNs name when the request carries no SigV4 scope to read
/// it from. It is the region the emulator writes into the shared AWS config it hands
/// clients (`vendor/server/src/lib.rs`, `[default] region = us-east-1`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for.
///
/// An ARN is regional, and the region a client's ARNs must carry is the one it
/// signed with, so it is read from the SigV4 credential scope rather than fixed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The `service` field every ARN of this service carries — its signing name.
pub const ARN_SERVICE: &str = "schemas";

/// `arn:aws:schemas:<region>:<account>:registry/<registry>`.
pub fn registry_arn(region: &str, registry: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("registry/{registry}"))
}

/// `arn:aws:schemas:<region>:<account>:schema/<registry>/<schema>`.
pub fn schema_arn(region: &str, registry: &str, schema: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("schema/{registry}/{schema}"))
}

/// The `DiscovererId` this service gives the discoverer of `source_arn`: the source
/// ARN's service namespace followed by the resource it names, with every separator
/// inside the resource rendered as `-`.
///
/// A discoverer IS the subscription to ONE event source, so its id is DERIVED from that
/// source rather than drawn from a counter or a digest: a caller can compute the id of
/// the discoverer it asked for from the source alone, the id carries which source it
/// discovers, and re-creating a discoverer for a source gets the same id back whatever
/// else the store has been asked for in between.
///
/// `DiscovererId` is modelled as a plain `__string` with no pattern, so nothing in the
/// model constrains the rendering; what constrains it is that the id must identify the
/// source. A value with no ARN fields to read carries no source to derive from — a
/// create refuses such a `SourceArn` on its form ([`source_arn_not_an_arn`]) before
/// this is reached — so the fallback is the whole value, rendered the same way, which
/// keeps the mapping total.
///
/// Collapsing the separators is not injective: two sources whose resources differ only
/// in which separator they use derive the same id. The create resolves that by looking
/// the derived id up in the store, so the second such source is refused as a resource
/// that already exists rather than overwriting the first.
pub fn discoverer_id_for_source(source_arn: &str) -> String {
    let mut fields = source_arn.splitn(6, ':');
    // Fields 0..=5 of an ARN are `arn`, the partition, the service, the region, the
    // account and the resource: the id is built from the service (field 2) and the
    // resource (field 5), which are what say WHICH source this is.
    let derived = match (fields.nth(2), fields.nth(2)) {
        (Some(service), Some(resource)) => format!("{service}:{resource}"),
        _ => source_arn.to_string(),
    };
    derived.replace(['/', ':'], "-")
}

/// `arn:aws:schemas:<region>:<account>:discoverer/<id>`.
pub fn discoverer_arn(region: &str, discoverer_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("discoverer/{discoverer_id}"))
}

/// A resource of this service that carries a tag set: what a `ResourceArn` names.
///
/// The three resource types this service's ARNs name — the registry, the schema and
/// the discoverer — each identified by the part of the ARN after the account field,
/// which is what tells one resource of a type from another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `registry/<name>`.
    Registry { registry: String },
    /// `schema/<registry>/<name>` — a schema is named within its registry.
    Schema { registry: String, schema: String },
    /// `discoverer/<id>`, by the id the service minted.
    Discoverer { discoverer_id: String },
}

/// Why a `ResourceArn` names no tag set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnError {
    /// The value is no resource ARN of this service, so it could name no resource in
    /// any store: the request's own form is wrong.
    NotAResourceArn,
    /// A resource ARN of this service, naming a resource this store does not hold.
    NoSuchResource,
}

/// Why a `ResourceArn` names no tag set a caller may WRITE.
///
/// The two refusals a read has ([`ResourceArnError`]), plus the one only a write can
/// meet: the ARN names content AWS owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceArnWriteError {
    /// The value is no resource ARN of this service: the request's own form is wrong.
    NotAResourceArn,
    /// A resource ARN of this service, naming a resource this store does not hold.
    NoSuchResource,
    /// A resource of a service-owned registry, named here — AWS owns what is in it,
    /// so a caller may read it but not write to it.
    ServiceOwned(String),
}

/// Why a registry a request NAMES is no registry its write can reach.
///
/// The two refusals [`ComAmazonawsSchemasSchemasState::writable_registry`] resolves,
/// each carrying the registry name the request supplied so the call site can word the
/// service's sentence for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryWriteError {
    /// This store holds no registry of that name, so the request names nothing to
    /// write to.
    NoSuchRegistry(String),
    /// A service-owned registry — AWS owns what is in it, so a caller may read it but
    /// not write to it.
    ServiceOwned(String),
}

/// Which step of the registry → schema → schema-version resolution reached nothing.
///
/// The three refusals
/// [`ComAmazonawsSchemasSchemasState::readable_schema_version`] resolves, each carrying
/// the request members its sentence is built from, so the call site can word the
/// service's sentence for it without repeating the choice of which refusal the step
/// answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadResolutionError {
    /// This store holds no registry of that name, so the request names nothing at all.
    NoSuchRegistry(String),
    /// The registry the request named holds no schema of that name.
    NoSuchSchema(String),
    /// The schema the request named holds no such version — including a `SchemaVersion`
    /// that names no version at all, and a schema left holding none.
    NoSuchSchemaVersion {
        schema: String,
        /// The request's `SchemaVersion`, absent where the request named none and the
        /// schema holds no version to answer as its latest.
        requested: Option<String>,
    },
}

impl ReadResolutionError {
    /// The service's sentence for this refusal.
    ///
    /// Every step of the resolution reaches a resource that does not exist, so each is
    /// the `NotFoundException` that step's own message helper words — which is why a
    /// handler maps the whole enum in one place and cannot answer one step's sentence
    /// for another's.
    pub fn not_found(&self) -> NotFoundException {
        match self {
            Self::NoSuchRegistry(registry) => registry_not_found(registry),
            Self::NoSuchSchema(schema) => schema_not_found(schema),
            Self::NoSuchSchemaVersion { schema, requested } => {
                schema_version_not_found(schema, requested.as_deref())
            }
        }
    }
}

/// A write meets every refusal a read of the same ARN meets, and words it the same
/// way, so the one resolution serves both.
impl From<ResourceArnError> for ResourceArnWriteError {
    fn from(error: ResourceArnError) -> Self {
        match error {
            ResourceArnError::NotAResourceArn => Self::NotAResourceArn,
            ResourceArnError::NoSuchResource => Self::NoSuchResource,
        }
    }
}

/// The resource an ARN of THIS service names, or `None` for a value that names none of
/// them: one that is not an ARN at all ([`is_arn`]), one whose `service` field is
/// another service's, or one whose resource part is not a resource type this service
/// has — such a value can name no resource of this service in any account or region.
///
/// The partition, region and account fields are not read here: they say WHERE a
/// resource lives, not which resource it is, and whether one of this store's resources
/// reports exactly this ARN is what
/// [`ComAmazonawsSchemasSchemasState::tags_for_arn`] then decides.
pub fn parse_resource_arn(candidate: &str) -> Option<TaggedResource> {
    if !is_arn(candidate) {
        return None;
    }
    let mut fields = candidate.splitn(6, ':');
    // `arn`, partition, service, region, account, resource — the six an ARN has.
    fields.next();
    fields.next();
    if fields.next() != Some(ARN_SERVICE) {
        return None;
    }
    fields.next();
    fields.next();
    let resource = fields.next()?;
    // A registry and a schema name are each ONE path segment of the resource part (the
    // model binds both to a non-greedy URI label, so neither can hold a `/`), so a
    // resource part carrying a segment more than its type takes names no resource.
    let mut parts = resource.splitn(4, '/');
    let kind = parts.next()?;
    let (first, second, extra) = (parts.next(), parts.next(), parts.next());
    if extra.is_some() {
        return None;
    }
    match (kind, first, second) {
        ("registry", Some(registry), None) if !registry.is_empty() => {
            Some(TaggedResource::Registry {
                registry: registry.to_string(),
            })
        }
        ("schema", Some(registry), Some(schema))
            if !registry.is_empty() && !schema.is_empty() =>
        {
            Some(TaggedResource::Schema {
                registry: registry.to_string(),
                schema: schema.to_string(),
            })
        }
        ("discoverer", Some(discoverer_id), None) if !discoverer_id.is_empty() => {
            Some(TaggedResource::Discoverer {
                discoverer_id: discoverer_id.to_string(),
            })
        }
        _ => None,
    }
}

/// Store the `Tags` member of a request. An absent map and an empty one are the same
/// thing: a resource with no tags.
pub fn tags_from_input(tags: Option<&HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>()
    })
    .unwrap_or_default()
}

/// Render a stored tag set for a response. Always present — an untagged resource
/// reports an empty map, not an absent member.
pub fn tags_to_output(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

/// One page of a key-ordered listing.
#[derive(Debug)]
pub struct Page<T> {
    /// The items this page returns, in key order.
    pub items: Vec<T>,
    /// The token that resumes after the last item, or `None` on the last page.
    pub next_token: Option<String>,
}

/// Collate a listing of a registry's schemas: the schemas `entries` names, each paired
/// with its own `SchemaName`, in the CODEPOINT order of those names — which is the
/// order `ListSchemas` and `SearchSchemas` serve a registry's schemas in and
/// [`page_by_key`] then pages over.
///
/// The order is the names' codepoints and nothing else: a name's own characters decide
/// its place, so an upper-case letter precedes the lower-case one at the same position
/// and punctuation participates like any other character. That makes the order total
/// over distinct names — a `NextToken` names a POSITION in it, so an order two distinct
/// names could share would make a resumed walk skip or repeat one of them — and it is
/// exactly how [`page_by_key`] compares keys, as strings.
///
/// The names are collated, not the values: this orders the WHOLE selection (so the
/// caller filters cheaply before it, or lazily over the result) and touches nothing but
/// each schema's name.
pub fn in_schema_name_order<'a, V>(
    entries: impl Iterator<Item = (&'a String, V)>,
) -> Vec<(String, V)> {
    let mut collated: Vec<(String, V)> = entries
        .map(|(name, value)| (name.clone(), value))
        .collect();
    collated.sort_by(|(left, _), (right, _)| left.cmp(right));
    collated
}

/// Render the fingerprint of a listing's selection: the listing itself, and the
/// request members that decide WHICH items it contains, each with the value this
/// request sent (an absent member and the value that selects the same items render
/// alike, because they ARE the same listing).
///
/// The rendering is UNAMBIGUOUS — two selections that differ in any value, in the
/// member a value was sent for, or in the listing they select, render different
/// fingerprints — because every value is preceded by its length in bytes, so a
/// reader knows where it ends. Separating the values alone cannot promise that: a
/// member modelled as a free-form string may hold the separator itself, and then one
/// member's value runs into the next member's name and two different selections
/// render alike — the very confusion binding the selection into a `NextToken` exists
/// to prevent.
pub fn selection_fingerprint(listing: &str, members: &[(&str, &str)]) -> String {
    let mut fingerprint = String::new();
    let mut push_field = |name: &str, value: &str| {
        fingerprint.push_str(name);
        fingerprint.push('=');
        fingerprint.push_str(&value.len().to_string());
        fingerprint.push(':');
        fingerprint.push_str(value);
        fingerprint.push(';');
    };
    push_field("listing", listing);
    for (member, value) in members {
        push_field(member, value);
    }
    fingerprint
}

/// How long a `NextToken` this service mints resumes its listing for.
///
/// The window is the model's, not a number the emulator picked: the documentation of
/// every `NextToken` member of this service states that the token expires in 24
/// hours. A token therefore carries the instant it was minted at, and one presented
/// once this much has passed no longer names a position in any listing — it is
/// refused, like any other token this service will not resume.
pub const NEXT_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Mint a `NextToken` that resumes strictly after `last_key`, for a listing selected
/// by `selection` and served at `issued_at`.
///
/// Every field is hex, so the token is a single opaque word: it is safe in a query
/// string whatever the key or the selection contains, a client cannot mistake it for
/// a value it may construct, and the fields cannot run into each other.
///
/// The selection is carried IN the token because a token names a position within one
/// listing: resuming a differently-filtered listing from it would skip or repeat
/// items with no error. A token presented with another selection is therefore not a
/// token for that listing, and is refused.
///
/// The issue instant is carried for the same kind of reason: a position is only
/// meaningful for as long as the token lives, which is [`NEXT_TOKEN_TTL`], so the
/// token has to say when it was minted. Handlers reach this through [`page_by_key`];
/// it is public so a test can mint a token for a chosen instant.
pub fn encode_next_token(selection: &str, last_key: &str, issued_at: SystemTime) -> String {
    let issued_at_seconds = issued_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!(
        "{}.{}.{issued_at_seconds:016x}",
        ::hex::encode(selection),
        ::hex::encode(last_key),
    )
}

/// How many digits [`encode_next_token`] renders a token's issue instant with: the
/// widest a `u64` of seconds can be in hex, and the minter pads every instant out to
/// it, so that field of a token this service minted is always exactly this long.
const NEXT_TOKEN_INSTANT_DIGITS: usize = 16;

/// Whether `field` is spelled the way [`encode_next_token`] renders a token's
/// fields: lower-case hex digits, which is all `::hex::encode` and `{:x}` emit.
///
/// Screened before the digits are handed to a parser, exactly as this service reads
/// its other opaque label ([`Schema::version_number`]): `::hex::decode` and
/// `u64::from_str_radix` both also read `A`-`F`, and the latter a leading `+` as
/// well, so without the screen a spelling the minter can never render would name
/// the position a token the service DID mint names.
fn is_rendered_hex(field: &str) -> bool {
    field
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Read back the key a `NextToken` [`encode_next_token`] minted for `selection`
/// resumes after, as of `now`. `None` for anything else: a token minted for a
/// different selection, one this service never minted, and one minted more than
/// [`NEXT_TOKEN_TTL`] before `now`.
///
/// "Never minted" is held to the MINTER's rendering, field by field: a token names a
/// position only when it is exactly how [`encode_next_token`] writes that position —
/// two hex fields of lower-case digit pairs and an instant of exactly
/// [`NEXT_TOKEN_INSTANT_DIGITS`] lower-case digits — so any other spelling of the
/// same bytes (upper-case or mixed-case digits, a sign, a different number of
/// digits) names no position in any listing.
fn decode_next_token(selection: &str, token: &str, now: SystemTime) -> Option<String> {
    let mut fields = token.split('.');
    let (encoded_selection, encoded_key, encoded_issued_at) =
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(selection), Some(key), Some(issued_at), None) => (selection, key, issued_at),
            _ => return None,
        };
    // The spelling of every field is settled FIRST, before any of them is read: what
    // the fields say only matters for a token spelled the way the minter renders one.
    if !is_rendered_hex(encoded_selection)
        || !is_rendered_hex(encoded_key)
        || !is_rendered_hex(encoded_issued_at)
        || encoded_issued_at.len() != NEXT_TOKEN_INSTANT_DIGITS
    {
        return None;
    }
    if ::hex::decode(encoded_selection).ok()? != selection.as_bytes() {
        return None;
    }
    let issued_at = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(
            u64::from_str_radix(encoded_issued_at, 16).ok()?,
        ))?;
    // The age runs forward from the instant in the token, so a token the clock has
    // not reached yet is as young as a fresh one rather than older than any.
    if now.duration_since(issued_at).unwrap_or_default() >= NEXT_TOKEN_TTL {
        return None;
    }
    String::from_utf8(::hex::decode(encoded_key).ok()?).ok()
}

/// Take one page out of a key-ordered sequence.
///
/// `entries` must be ordered by key (a `BTreeMap`'s iteration order is, and
/// [`in_schema_name_order`]'s output is ordered by the name it pairs each item with).
/// The page starts strictly after the key `token` encodes and holds at most
/// `page_size` items; `next_token` is minted only when a further entry exists, so the
/// last page of a listing carries none.
///
/// `page_size` is a count, never "unbounded": a listing that materialised its whole
/// selection into one response would be a response-amplification path out of a store
/// the caller fills. Callers resolve it through [`requested_page_size`].
///
/// `selection` identifies the listing being walked — the listing itself and every
/// request member that decides WHICH items it contains (the parent resource, a name
/// prefix, a scope), as [`selection_fingerprint`] renders them. It is bound into the
/// token and checked when one comes back, so a token cannot resume a listing it was
/// not issued for.
///
/// `now` is the instant this request is served: it stamps the token this page mints
/// and ages the one it was given, so a token older than [`NEXT_TOKEN_TTL`] resumes
/// nothing.
///
/// `token` is the `NextToken` the request carried, and a request that carried NONE —
/// whether the member was absent or present with nothing in it — asks for the FIRST
/// page: the documentation of every `NextToken` member of this service says the first
/// page is requested by leaving the token empty, and a parameter sent with an empty
/// value has been left empty exactly as an omitted one has. That is how the siblings
/// that select these same listings already read an empty value, so no listing of this
/// service refuses a value the rest of its members treat as "nothing was sent".
///
/// Returns `Err` for a token this service did not mint for this selection, and for
/// one that has expired — the caller answers that with its own modelled client error.
pub fn page_by_key<K: AsRef<str>, V>(
    entries: impl Iterator<Item = (K, V)>,
    selection: &str,
    token: Option<&str>,
    page_size: usize,
    now: SystemTime,
) -> Result<Page<V>, InvalidToken> {
    let after = match token {
        // No token at all, and a token left empty, both name no position: the page
        // starts at the beginning of the selection.
        None | Some("") => None,
        Some(token) => Some(decode_next_token(selection, token, now).ok_or(InvalidToken)?),
    };
    let mut remaining =
        entries.filter(|(key, _)| after.as_deref().is_none_or(|after| key.as_ref() > after));

    let mut items = Vec::new();
    let mut last_key = String::new();
    while items.len() < page_size {
        match remaining.next() {
            Some((key, value)) => {
                last_key = key.as_ref().to_string();
                items.push(value);
            }
            // The sequence ran out before the page filled: this is the last page.
            None => {
                return Ok(Page {
                    items,
                    next_token: None,
                })
            }
        }
    }

    // The page filled. Only a sequence that still has an entry gets a token, so a
    // listing whose total is an exact multiple of `limit` does not hand back a token
    // that would resolve to an empty page.
    let next_token = remaining
        .next()
        .map(|_| encode_next_token(selection, &last_key, now));
    Ok(Page { items, next_token })
}

/// A `NextToken` the listing it was presented to will not resume from: one this
/// service did not mint for that listing's selection, or one that has expired.
#[derive(Debug)]
pub struct InvalidToken;

/// The ceiling on how many registries this store holds, on how many schemas one
/// registry holds, and on the total schema content it retains: the caps that bound
/// caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// a create is the only thing that adds one, so the store needs explicit maxima —
/// neither a TTL nor an eviction policy is available to it, because `DescribeSchema`
/// must still answer what `CreateSchema` created. Without them, one `CreateRegistry`
/// per caller-chosen `RegistryName` and one `CreateSchema` per caller-chosen
/// `SchemaName` grow the store for as long as a client keeps calling.
///
/// The entry counts bound the PRODUCT at
/// `MAX_REGISTRIES * MAX_SCHEMAS_PER_REGISTRY` schemas, and
/// [`MAX_STORED_CONTENT_BYTES`] bounds what those entries can retain: the model caps
/// one `Content` at 100 000 characters, so the entry counts alone would still admit
/// that many hundred-kilobyte payloads. Both creates refuse past their cap with the
/// service's own `BadRequestException` (HTTP 400) — this model declares no quota
/// error, and a client error is the honest class: the store cannot accept the
/// request, and no retry of it will succeed.
///
/// The numbers are the emulator's own bound, not a published AWS default: AWS
/// publishes these quotas as adjustable, so no single value is the contract, and they
/// are set high enough that no ordinary workflow meets one.
pub const MAX_REGISTRIES: usize = 500;

/// See [`MAX_REGISTRIES`].
pub const MAX_SCHEMAS_PER_REGISTRY: usize = 500;

/// See [`MAX_REGISTRIES`]. The ceiling on how many discoverers a caller may hold:
/// `CreateDiscoverer` mints one record per call and only a delete removes one, so
/// without it the discoverer map grows for as long as a client keeps creating.
pub const MAX_DISCOVERERS: usize = 500;

/// See [`MAX_REGISTRIES`]. Counted over every version of every schema in every
/// registry a CALLER owns, which is everything a caller's `Content` can make this
/// service retain; the service-owned catalogue is fixed at compile time, so it is not
/// part of what this budget bounds.
///
/// `CreateSchema` writes a schema's first version and `UpdateSchema` writes every
/// later one, so a single schema's versions are bounded on their own by
/// [`MAX_VERSIONS_PER_SCHEMA`] beside this byte budget.
pub const MAX_STORED_CONTENT_BYTES: usize = 64 * 1024 * 1024;

/// The most versions one schema may hold. See [`MAX_REGISTRIES`] for why the store
/// needs explicit maxima at all: `UpdateSchema` mints a version per call, so without
/// this one schema grows for as long as a client keeps updating it, under the byte
/// budget alone.
///
/// Like the other entry caps this is the emulator's own bound, not a published AWS
/// default, and is set high enough that no ordinary workflow meets it.
pub const MAX_VERSIONS_PER_SCHEMA: usize = 500;

/// The largest `Policy` document `PutResourcePolicy` will store for one registry.
///
/// `Policy` is modelled as a plain `__string`, so the model bounds it nowhere, and a
/// policy is retained for the life of the process like every other resource here —
/// the same reason [`MAX_REGISTRIES`] exists. One policy per registry, each at most
/// this many bytes, bounds what policies can make this store retain. The number is
/// the emulator's own bound rather than a published AWS quota, and is far larger than
/// any real policy document.
pub const MAX_POLICY_BYTES: usize = 64 * 1024;

/// See [`MAX_REGISTRIES`]. The ceiling on how many code bindings one schema holds.
///
/// A binding is keyed by schema version and language. The language is one of the
/// finitely many [`CODE_BINDING_LANGUAGES`], so what a caller can grow is the number
/// of VERSIONS it starts bindings for; this cap is the explicit ceiling on the
/// product, kept so the binding map is bounded however many versions a schema comes
/// to hold. A put past the cap is refused with the service's own
/// `BadRequestException`, as the other caps are.
pub const MAX_CODE_BINDINGS_PER_SCHEMA: usize = 500;

/// The most items one page of a listing may hold, whatever `Limit` asks for.
///
/// Without it, an absent or very large `Limit` would materialise the whole selected
/// store into a single response — the amplifying half of an unbounded store. A
/// listing that has more to give says so with a `NextToken`, which is how a caller
/// reads past one page, so clamping loses nothing.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most keywords one `SearchSchemas` request may name.
///
/// A keyword search costs the PRODUCT of two factors: how many keywords the request
/// names, and how much stored text each one is matched against. The stored side is
/// already bounded ([`MAX_STORED_CONTENT_BYTES`]), but the request side is not —
/// `Keywords` is one query-string value that yields one more keyword for every
/// separator a caller puts in it, so without this cap a single small GET could ask
/// for an arbitrary multiple of a scan of the whole store. With it, one request costs
/// at most `MAX_KEYWORDS_PER_SEARCH` passes over the text of the registry it names.
///
/// Refused past the cap with the service's own `BadRequestException` (HTTP 400): this
/// model declares no quota error, and the store cannot serve the request as asked, so
/// no retry of it will succeed. The number is the emulator's own bound, not a
/// published AWS limit, and is set high enough that no ordinary search meets it.
pub const MAX_KEYWORDS_PER_SEARCH: usize = 10;

/// How long after a definition is written it is reachable through the registry's
/// KEYWORD INDEX.
///
/// `SearchSchemas` searches an index, and an index catches up with a write rather than
/// being part of it: a search made straight after a `CreateSchema` reports nothing for
/// the schema it just stored, and a later search reports it — which is why a client
/// polls. AWS publishes no duration for that catch-up, so this span is the emulator's
/// own choice. It is deliberately NOT zero, so a caller searching immediately after a
/// write observes the same transient absence the real service shows, and it is short
/// enough that a client polling for its schema finds it on an early poll.
///
/// Nothing waits for it and nothing writes it: a definition carries the instant it was
/// written ([`SchemaVersion::created_date`]), so whether it is in the index is DERIVED
/// at search time ([`is_keyword_indexed`]).
pub const SCHEMA_INDEXING_DELAY: Duration = Duration::from_secs(3);

/// Whether a definition written at `written` is in the registry's keyword index as of
/// `now` — [`SCHEMA_INDEXING_DELAY`] after it was written.
///
/// An instant so far in the future that the span cannot be added to it is one no clock
/// reading can be past, so such a definition is not indexed yet.
pub fn is_keyword_indexed(written: SystemTime, now: SystemTime) -> bool {
    written
        .checked_add(SCHEMA_INDEXING_DELAY)
        .is_some_and(|due| now >= due)
}

/// The most named schemas one inferred document may carry — one per object position
/// the sample shows, plus the root.
///
/// The model bounds the sample itself: `Events` is `@length(min: 1, max: 10)` and an
/// entry is `@length(min: 1, max: 100000)`
/// (`smithy_query(query="shape", shape="__listOfGetDiscoveredSchemaVersionItemInput")`),
/// so the framework refuses more than a megabyte of sampled events before any handler
/// runs. What that does NOT bound is how many distinct OBJECTS a sample of that size
/// describes — a bare `{"m":{}}` is a dozen bytes — and each one becomes a named schema
/// the document holds, names and writes. A thousand distinct shapes in one sample is
/// already past anything a schema a client generates code from describes.
///
/// Refused past the cap with `BadRequestException` (HTTP 400), as the other bounds of
/// this service are: the request cannot be served as asked, so no retry of it will
/// succeed. The emulator's own bound, not a published AWS limit.
pub const MAX_INFERRED_SCHEMAS: usize = 1_000;

/// Refuse a sample that would describe more schemas than one document may carry. Names
/// the member, the bound and what the sample showed — never the sample itself, which is
/// the caller's data.
pub fn too_many_inferred_schemas(named: usize) -> BadRequestException {
    bad_request(format!(
        "Events describe {named} distinct objects, but an inferred schema document \
         carries at most {MAX_INFERRED_SCHEMAS}."
    ))
}

/// The page size to serve a request whose `Limit` member is `limit`.
///
/// `Err` when the request asked for a page of fewer than one item: the model gives
/// `Limit` no range, so that bound is the handler's, and a page of zero items is not
/// a page. Anything larger than [`MAX_PAGE_SIZE`] is served as `MAX_PAGE_SIZE`, and
/// an absent `Limit` means "as much as one page may hold".
pub fn requested_page_size(limit: Option<i32>) -> Result<usize, BadRequestException> {
    match limit {
        None => Ok(MAX_PAGE_SIZE),
        Some(limit) if limit >= 1 => Ok(usize::try_from(limit)
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE)),
        Some(limit) => Err(limit_out_of_range(limit)),
    }
}

/// How many bytes of schema `Content` this store currently retains on a caller's
/// behalf, across every version of every schema in every registry the caller owns —
/// what [`MAX_STORED_CONTENT_BYTES`] bounds.
///
/// Walked on demand rather than tracked in a counter: a counter has to be decremented
/// on every delete path too, and a counter that drifts reports a budget the store is
/// not actually using.
pub fn stored_content_bytes(state: &ComAmazonawsSchemasSchemasState) -> usize {
    state
        .registries
        .values()
        .filter(|registry| !registry.scope.is_managed())
        .flat_map(|registry| registry.schemas.values())
        .flat_map(|schema| schema.versions.values())
        .map(|version| version.content.len())
        .sum()
}

/// How many registries the caller owns — what [`MAX_REGISTRIES`] bounds. The
/// service-owned catalogue is not a caller's and does not consume the caller's
/// allowance.
pub fn local_registry_count(state: &ComAmazonawsSchemasSchemasState) -> usize {
    state
        .registries
        .values()
        .filter(|registry| !registry.scope.is_managed())
        .count()
}

/// The `$schema` a JSON Schema draft-4 document carries — what makes a document
/// draft 4 rather than some other draft.
///
/// Shared: `GetDiscoveredSchema` stamps it on the document it infers and
/// `ExportSchema` on the document it converts, so one constant keeps the two from
/// naming two different drafts.
pub const JSON_SCHEMA_DRAFT4_URI: &str = "http://json-schema.org/draft-04/schema#";

// ---------------------------------------------------------------------------
// Error codes
//
// Every error shape this service models carries two REQUIRED members, `Code`
// ("The error code.") and `Message`, and the two say different things. The
// exception SHAPE name is what the protocol already carries, in the
// `x-amzn-errortype` header (`OperationError::shape_name`, see
// `crate::scaffolded`); `Code` carries the service's own code for the CONDITION
// that was raised, which is what a client branching on the SDK's error-code
// accessor reads. So the two are never the same string: a condition-level code
// goes in `Code` and the shape name is left to the header.
//
// The codes below are the ones this service answers with; each is named once
// here so no handler can spell one differently.
// ---------------------------------------------------------------------------

/// `Code` for a resource the store does not hold — a registry, a schema, a schema
/// version, a code binding or a resource-based policy. One code for every
/// not-found condition: what is missing is said in the `Message`, not the code.
pub const CODE_RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// `Code` for a create whose resource already exists, the conflict an idempotent
/// create-or-reuse caller keys on.
pub const CODE_RESOURCE_ALREADY_EXIST: &str = "ResourceAlreadyExist";

/// `Code` for a request the service will not act on as sent: a member outside the
/// constraint the service enforces, or content that is not the document it must be.
pub const CODE_MALFORMED_REQUEST: &str = "MalformedRequest";

/// `Code` for a request that conflicts with the state of the resource it names —
/// both the 409 conflicts that are not "already exists" and the 412 a stale
/// `RevisionId` precondition fails with, which an optimistic-concurrency retry loop
/// keys on.
pub const CODE_CONFLICT: &str = "Conflict";

/// `Code` for a caller that may not perform the action on this resource.
pub const CODE_FORBIDDEN: &str = "Forbidden";

/// `Code` for a fault on the service's side rather than in the request.
pub const CODE_INTERNAL_SERVER_ERROR: &str = "InternalServerError";

/// A `NotFoundException` for a resource this store does not hold.
///
/// `Code` is [`CODE_RESOURCE_NOT_FOUND`], never the shape name: see the note above.
pub fn not_found(message: impl Into<String>) -> NotFoundException {
    NotFoundException {
        code: CODE_RESOURCE_NOT_FOUND.to_string(),
        message: message.into(),
    }
}

/// A `ConflictException` for a create whose resource already exists.
/// `Code` is [`CODE_RESOURCE_ALREADY_EXIST`].
///
/// Every create this service serves refuses a name that is already taken with this:
/// the request is well-formed and what stops it is the stored resource, so the class
/// is a conflict whatever the one operation's `errors` list happens to enumerate.
pub fn already_exists(message: impl Into<String>) -> ConflictException {
    ConflictException {
        code: CODE_RESOURCE_ALREADY_EXIST.to_string(),
        message: message.into(),
    }
}

/// A `ConflictException` for a request that conflicts with the state of the
/// resource it names, other than a resource that already exists.
/// `Code` is [`CODE_CONFLICT`].
pub fn conflict(message: impl Into<String>) -> ConflictException {
    ConflictException {
        code: CODE_CONFLICT.to_string(),
        message: message.into(),
    }
}

/// A `BadRequestException`. `Code` is [`CODE_MALFORMED_REQUEST`].
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: CODE_MALFORMED_REQUEST.to_string(),
        message: message.into(),
    }
}

/// A `ForbiddenException`. `Code` is [`CODE_FORBIDDEN`].
pub fn forbidden(message: impl Into<String>) -> ForbiddenException {
    ForbiddenException {
        code: CODE_FORBIDDEN.to_string(),
        message: message.into(),
    }
}

/// A `PreconditionFailedException`: the request's precondition is not the state the
/// resource is in. `Code` is [`CODE_CONFLICT`] — the condition is a conflict with
/// the stored resource, and the 412 is what says the request stated a precondition.
pub fn precondition_failed(message: impl Into<String>) -> PreconditionFailedException {
    PreconditionFailedException {
        code: CODE_CONFLICT.to_string(),
        message: message.into(),
    }
}

/// An `InternalServerErrorException`. `Code` is [`CODE_INTERNAL_SERVER_ERROR`].
///
/// Raised only where a handler cannot reach its own store, which is a server fault.
pub fn internal_error() -> InternalServerErrorException {
    InternalServerErrorException {
        code: CODE_INTERNAL_SERVER_ERROR.to_string(),
        message: "The schemas emulator could not reach its own state store.".to_string(),
    }
}

/// Message for a registry no caller has created. The template is AWS's own, built
/// from the request's `RegistryName`.
pub fn registry_not_found(registry: &str) -> NotFoundException {
    not_found(format!("Registry with name {registry} does not exist."))
}

/// Message for a registry that holds no resource-based policy. The template is
/// AWS's own, built from the request's `RegistryName`.
pub fn policy_not_found(registry: &str) -> NotFoundException {
    not_found(format!(
        "Policy for registry with name '{registry}' does not exist."
    ))
}

/// Refuse a request that carries no `RegistryName`.
///
/// A resource policy is a property of ONE registry, so a request that does not name
/// a registry names no resource to read or write. Names the member, as a validation
/// error does.
pub fn registry_name_required() -> BadRequestException {
    bad_request(
        "RegistryName is required: a resource-based policy belongs to one registry, \
         so the request must name the registry it applies to.",
    )
}

/// Refuse a write whose `RevisionId` precondition is not the revision the stored
/// policy is at.
///
/// The service's sentence for this refusal is a CONSTANT: it names neither the
/// revision the caller sent nor the one the store holds. Nothing request-derived or
/// environment-derived belongs in it — the caller already holds what it sent, a
/// refusal is not a read of the resource (which is what tells the caller the current
/// revision), and a message carrying a minted id would differ from run to run for a
/// client or test that compares one.
pub fn stale_policy_revision() -> PreconditionFailedException {
    precondition_failed(
        "The RevisionId provided does not match the latest RevisionId for the registry policy.",
    )
}

/// Refuse a `Policy` larger than the store will retain for one registry. Names the
/// cap, so the caller knows a retry cannot help.
pub fn policy_too_large(sent: usize) -> BadRequestException {
    bad_request(format!(
        "Policy must be at most {MAX_POLICY_BYTES} bytes; the document sent is {sent} bytes."
    ))
}

/// Refuse a `Policy` that is not a JSON object.
///
/// `Policy` is modelled `@mediaType("application/json")` and a resource-based policy
/// document is a JSON object, so a value that is not one cannot be stored as a
/// policy. Names the member and what it must carry.
pub fn policy_not_json(detail: &str) -> BadRequestException {
    bad_request(format!(
        "Policy must be a JSON object holding a resource-based policy document: {detail}."
    ))
}

/// Message for a schema the named registry does not hold.
///
/// The sentence is the service's own template for a missing resource — "«Resource»
/// with name «name» does not exist.", as [`registry_not_found`] carries it — built
/// from the request's `SchemaName` and from that member ALONE, the way
/// [`schema_already_exists`] keeps the registry out of its sentence.
pub fn schema_not_found(schema: &str) -> NotFoundException {
    not_found(format!("Schema with name {schema} does not exist."))
}

/// The sentence a duplicate create is refused with, for any resource kind: the
/// service's own template, built from the name the request sent and naming the kind
/// of resource that name is taken for. One template here, so no two creates can
/// drift apart on the wording.
fn already_exists_sentence(resource: &str, name: &str) -> String {
    format!("{resource} with name {name} already exists.")
}

/// Refuse a `CreateRegistry` for a name the store already holds. The template is
/// AWS's own, built from the request's `RegistryName`.
pub fn registry_already_exists(registry: &str) -> ConflictException {
    already_exists(already_exists_sentence("Registry", registry))
}

/// Message for a discoverer this store does not hold, named by the request's
/// `DiscovererId`.
///
/// A discoverer is refused by its id ALONE — the sentence is "Discoverer «id» does not
/// exist.", not the "«Resource» with «identifier» «value»" template
/// [`registry_not_found`] and [`schema_not_found`] carry — because the id is the whole
/// of what a discoverer operation names.
pub fn discoverer_not_found(discoverer_id: &str) -> NotFoundException {
    not_found(format!("Discoverer {discoverer_id} does not exist."))
}

/// Refuse a `CreateDiscoverer` for a source this store already discovers.
///
/// A discoverer IS the subscription to one event source, so a second one for the same
/// source is the same resource asked for twice. Built from the `DiscovererId` of the
/// discoverer the store already holds, so a caller that parses the sentence gets the
/// handle it needs to describe or reuse the colliding resource.
pub fn discoverer_already_exists(discoverer_id: &str) -> ConflictException {
    already_exists(format!("Discoverer {discoverer_id} already exists."))
}

/// Refuse a `SourceArn` that is not an ARN.
///
/// A discoverer discovers schemas from an event source IDENTIFIED BY ITS ARN, so a
/// value that is not an ARN names no source. The model bounds only the length
/// (`__stringMin20Max1600`), so the form is the handler's to refuse. Names the member
/// and what it must carry, never the value.
pub fn source_arn_not_an_arn() -> BadRequestException {
    bad_request(
        "SourceArn must be the ARN of the event source to discover schemas from, \
         in the form arn:<partition>:<service>:<region>:<account>:<resource>.",
    )
}

/// Whether `candidate` is an ARN: the `arn` scheme and the six colon-separated fields
/// every ARN has, the last of which (the resource) is non-empty.
///
/// The partition, region and account fields are NOT required to be non-empty — a
/// service-wide or account-wide ARN leaves some of them blank — so what this checks is
/// the form, not which resource the ARN names.
pub fn is_arn(candidate: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next().is_some()
        && fields.next().is_some_and(|service| !service.is_empty())
        && fields.next().is_some()
        && fields.next().is_some()
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

/// Refuse a `ResourceArn` that is no resource ARN of this service
/// ([`parse_resource_arn`]).
///
/// A tagging operation acts on the resource its ARN IDENTIFIES, so a value that is not
/// an ARN, or is an ARN of another service or of no resource type this service has,
/// names no resource anywhere and is refused on its form. Names the member and what it
/// must carry, never the value — as [`source_arn_not_an_arn`] does for the other ARN
/// member this service takes.
pub fn resource_arn_not_a_resource_arn() -> BadRequestException {
    bad_request(format!(
        "ResourceArn must be the ARN of a {ARN_SERVICE} registry, schema or discoverer, \
         in the form arn:<partition>:{ARN_SERVICE}:<region>:<account>:registry/<name>, \
         arn:<partition>:{ARN_SERVICE}:<region>:<account>:schema/<registry>/<name> or \
         arn:<partition>:{ARN_SERVICE}:<region>:<account>:discoverer/<id>."
    ))
}

/// Message for a resource ARN this store holds no resource for. The template is the
/// service's own for a missing resource — "«Resource» with «identifier» «value» does
/// not exist.", as [`registry_not_found`] carries it — built from the request's
/// `ResourceArn`, which is the only identifier such a request names.
pub fn tagged_resource_not_found(resource_arn: &str) -> NotFoundException {
    not_found(format!(
        "Resource with ARN {resource_arn} does not exist."
    ))
}

/// Refuse a `CreateSchema` for a name its registry already holds. The template is
/// AWS's own, built from the request's `SchemaName` — and from that member ALONE:
/// AWS keeps the registry out of this sentence.
///
/// The class is the conflict every duplicate create here takes: see
/// [`already_exists`].
pub fn schema_already_exists(schema: &str) -> ConflictException {
    already_exists(already_exists_sentence("Schema", schema))
}

/// Refuse a `DeleteRegistry` that would take a registry's schemas with it. Names no
/// request member: AWS's sentence for this refusal names none.
pub fn registry_not_empty() -> ConflictException {
    conflict("The registry you tried to delete is not empty.")
}

/// Refuse a caller's write to a service-owned registry or one of its schemas: AWS
/// owns that content, so a caller may read it but never change it.
///
/// The content AWS owns is the registry, its schemas, their versions, their metadata
/// and its policy. Code generated FROM one of those schemas is not that content — it is
/// the asking caller's artifact — so a code-binding put is not refused here; see
/// [`ComAmazonawsSchemasSchemasState::schema_at_version_mut`].
pub fn managed_registry_is_read_only(registry: &str) -> ForbiddenException {
    forbidden(format!(
        "Registry {registry} is an AWS-managed registry; its contents are owned by AWS \
         and cannot be created, updated or deleted."
    ))
}

/// Message for a version the named schema does not hold — including a `SchemaVersion`
/// that is not a version number at all.
///
/// The template is the service's own for a missing schema version, built from the
/// request's `SchemaName` and `SchemaVersion` and from those two members ALONE: the
/// registry holding the schema is no part of the sentence, the way
/// [`schema_not_found`] and [`schema_already_exists`] keep it out of theirs.
///
/// `requested` is absent only where the request named no version AND the schema holds
/// none to answer as its latest — a schema with no definition at all, which no request
/// can produce (a delete of a schema's last version is refused:
/// [`cannot_delete_every_version`]) — so that case answers the sentence for the schema
/// itself rather than inventing a second template for a resource this service cannot
/// hold.
pub fn schema_version_not_found(schema: &str, requested: Option<&str>) -> NotFoundException {
    match requested {
        Some(version) => not_found(format!(
            "Schema with name {schema} and version {version} does not exist."
        )),
        None => schema_not_found(schema),
    }
}

/// Refuse a `DeleteSchemaVersion` that would take a schema's LAST definition.
///
/// A schema always holds at least one definition, so the operation that removes one
/// version cannot be the one that empties it; the sentence names no request member —
/// the service's wording for this refusal is entirely fixed text — and points the
/// caller at the operation that does remove a schema with all of its versions.
pub fn cannot_delete_every_version() -> ConflictException {
    conflict(
        "Cannot delete all versions of a Schema. To delete a Schema use the DeleteSchema API.",
    )
}

/// Message for a code binding nobody has started for this schema version and
/// language.
///
/// The template is AWS's own, built from the request's `SchemaName`, the resolved
/// version and the request's `Language`. It names no registry — AWS's sentence for
/// this refusal names none — so this helper takes none.
pub fn code_binding_not_found(schema: &str, version: u64, language: &str) -> NotFoundException {
    not_found(format!(
        "Code bindings for schema with name {schema} and version {version} language \
         {language} not found."
    ))
}

/// Refuse a `Language` this service generates no code bindings for.
///
/// The model gives `Language` no enum, so this is the service's own constraint, and
/// the refusal states it the way the service does: it names the parameter it checked
/// and the pattern that parameter must match ([`code_binding_language_pattern`]), so
/// a caller can fix the request from the message alone. The rejected value is not
/// echoed back — the pattern is what the caller needs.
pub fn unsupported_code_binding_language() -> BadRequestException {
    bad_request(format!(
        "Parameter {CODE_BINDING_LANGUAGE_PARAMETER} doesn't match pattern {}",
        code_binding_language_pattern()
    ))
}

/// Refuse a `Limit` the model cannot express a bound for: a page of fewer than one
/// item is not a page. Names the member and the bound, as a validation error does.
pub fn limit_out_of_range(limit: i32) -> BadRequestException {
    bad_request(format!("Limit must be at least 1, but was {limit}."))
}

/// Refuse a create that would take the store past a cap it keeps so caller-driven
/// growth stays bounded. Names the cap, so the caller knows a retry cannot help.
pub fn store_cap_reached(what: &str, cap: usize) -> BadRequestException {
    bad_request(format!(
        "This emulator holds at most {cap} {what}; delete one before creating another."
    ))
}

/// The JSON type of a value, for a refusal that names what the caller sent without
/// echoing it back. One wording for the whole service, so the refusals of
/// `UpdateSchema`'s `Content`, `PutResourcePolicy`'s `Policy` and
/// `GetDiscoveredSchema`'s `Events` cannot drift apart.
pub fn json_type_name(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "a boolean",
        ::serde_json::Value::Number(_) => "a number",
        ::serde_json::Value::String(_) => "a string",
        ::serde_json::Value::Array(_) => "an array",
        ::serde_json::Value::Object(_) => "an object",
    }
}

/// Refuse `Content` that is not a schema document at all.
///
/// A schema registry holds schema SOURCE, so content an `UpdateSchema` call would
/// store is parsed before it enters the store rather than retained blind. Both types
/// this service models are JSON documents whose top level is an object: "A JSON Schema
/// is a JSON document, and that document MUST be an object" (JSON Schema draft-04,
/// section 3.2), and an OpenAPI document "is itself a JSON object" (OpenAPI
/// Specification 3.0, "Document Structure"/"Format"). Content that does not parse as
/// JSON, or parses as a value that is not an object, is therefore a document of
/// neither modelled `Type`, and no reader of it could treat it as a schema.
///
/// What this does NOT do is check a document against the whole of its specification —
/// that every member OpenAPI requires is present, or that every draft-04 keyword is
/// well-formed. Content is stored verbatim and handed back verbatim by
/// `DescribeSchema` and `ExportSchema`, and partial documents are legitimate input to
/// those, so the refusal is scoped to content that is not a schema document at all.
///
/// `CreateSchema` and `UpdateSchema` both route through here: the requirement is a
/// property of the request, so it cannot differ between the two.
///
/// `schema_type` is named in the message only — the requirement is the same for both
/// of [`crate::ops::types::TYPE_VALUES`], and a third type cannot reach a handler
/// because the modelled enum is refused at the boundary before one runs.
///
/// The message names the member and what was wrong with it, the way a validation
/// error does; it echoes no content back. The refusal carries
/// [`CODE_MALFORMED_REQUEST`] (see [`bad_request`]): the body is not the document it
/// was declared to be.
pub fn validate_content(content: &str, schema_type: &str) -> Result<(), BadRequestException> {
    let document: ::serde_json::Value = ::serde_json::from_str(content).map_err(|error| {
        // AWS's own template for a body it cannot read as JSON, built from the
        // request: the member, then the character the body stops being JSON at, what
        // was being read there and that character's byte offset into `Content` — see
        // [`json_parse_detail`], which is where every refusal of an unreadable body in
        // this service gets its wording, so no two of them can point a client at two
        // different coordinate systems.
        bad_request(format!(
            "Content is not valid JSON: {}",
            json_parse_detail(content, &error)
        ))
    })?;
    if document.is_object() {
        return Ok(());
    }
    // The JSON type the content turned out to be, so the caller can see what it sent
    // without the content being echoed back.
    let kind = json_type_name(&document);
    Err(bad_request(format!(
        "Content is not a valid {schema_type} document: a schema document is a JSON object, \
         but this Content is {kind}."
    )))
}

/// Refuse one of `GetDiscoveredSchema`'s `Events` that discovery cannot read.
///
/// The message is built from the request: it names the INDEX of the event at
/// fault — `Events` is a list, so the index is what tells a caller sending a batch
/// WHICH event to fix — and then the reason that event could not be read. The
/// event itself is never echoed back: it is the caller's data.
pub fn event_not_parsable(index: usize, reason: impl std::fmt::Display) -> BadRequestException {
    bad_request(format!(
        "Fail to parse event index: [{index}]. Reason: {reason}"
    ))
}

/// Refuse a search that names more keywords than one request may carry. Names the
/// member, the bound and what the request asked for, as a validation error does.
pub fn too_many_keywords(named: usize) -> BadRequestException {
    bad_request(format!(
        "Keywords may name at most {MAX_KEYWORDS_PER_SEARCH} keywords, but this request named {named}."
    ))
}

/// Refuse a `NextToken` this listing will not resume from — one this service did not
/// mint for this selection, or one that has expired — without echoing it back.
pub fn invalid_next_token() -> BadRequestException {
    let hours = NEXT_TOKEN_TTL.as_secs() / 3600;
    bad_request(format!(
        "The NextToken provided is not a token this service issued for this listing. \
         A NextToken resumes the listing it was returned by, so every request that \
         follows it must repeat that listing's selection unchanged, and it expires \
         {hours} hours after the listing returned it."
    ))
}

/// The bounds a tag set is kept within, and the refusals past them: the one
/// caller-driven growth dimension the model constrains nowhere, so it carries its own
/// ceilings beside [`MAX_REGISTRIES`] and the budgets around it. Re-exported, so every
/// handler reaches them as `crate::state::…` like the rest of this store's helpers.
mod tags;
pub use tags::*;

#[cfg(test)]
mod tests;

// SPDX-License-Identifier: CC0-1.0

//! How a request is resolved against the store: the case fold the Hive-compatible catalog
//! resources are keyed by, the `CatalogId` and principal a request addresses, the name
//! patterns a listing filters with, and the instants and cursors a time-ordered listing
//! pages by.

use super::*;

/// Fold a database or table name the way the catalog stores it: ASCII-lowercased.
/// Applied on both write and lookup, so a name is reachable by any casing the client
/// sends. The fold is ASCII because Hive compatibility is an ASCII rule, and because
/// ASCII folding is length-stable: full Unicode lowercasing can change a name's length
/// and so its modelled `length` constraint.
pub fn fold_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The `@pattern` a catalog's `Name` must satisfy: `^(?!(.*[.\/\\]|aws:)).*$`
/// (`CatalogNameString`, `smithy_query shape=CatalogNameString`) — a name carrying none
/// of the three path separators anywhere in it, and not opening with the `aws:` prefix
/// AWS reserves for its own resources.
pub const CATALOG_NAME_PATTERN: &str = r"^(?!(.*[.\/\\]|aws:)).*$";

/// The prefix AWS reserves for its own resources: a catalog's own name may not open with
/// it ([`CATALOG_NAME_PATTERN`]), and the connections of AWS's own managed federation
/// integrations are named under it ([`is_managed_federation_connection`]).
pub const RESERVED_NAME_PREFIX: &str = "aws:";

/// Whether `name` satisfies [`CATALOG_NAME_PATTERN`].
///
/// Checked here rather than left to the boundary validator because that pattern uses a
/// negative look-ahead, which the framework's regex engine cannot compile, and a
/// `@pattern` it cannot compile is treated as matching every value — which would let a
/// name the model forbids through to the store. The `@length(1, 64)` half of the
/// constraint IS enforced upstream, so it is not repeated here.
pub fn is_catalog_name(name: &str) -> bool {
    !name.contains(['.', '/', '\\']) && !name.starts_with(RESERVED_NAME_PREFIX)
}

/// Whether the connection `name` is one of the federation connections AWS's OWN managed
/// integrations use, rather than one an account creates for itself.
///
/// Such a connection is named under the prefix AWS reserves for its own resources
/// ([`RESERVED_NAME_PREFIX`]) — the prefix a tenant's catalog name may not take either —
/// and no `CreateConnection` of this account can hold it.
pub fn is_managed_federation_connection(name: &str) -> bool {
    name.starts_with(RESERVED_NAME_PREFIX)
}

/// Whether the data source `federated` points at is REGISTERED for the account this
/// endpoint serves, and so one a catalog of that account may be federated to.
///
/// The sources an account holds a registration for without having registered anything are
/// the ones AWS's own managed integrations register for it, and an account reaches those
/// through the integration's own federation connection
/// ([`is_managed_federation_connection`]). Every other source is one the account has to
/// register with the data lake itself before a catalog may be federated to it: the Glue
/// connection a federation names is a connection TO the source
/// (`FederatedCatalog$ConnectionName`: "The name of the connection to an external data
/// source, for example a Redshift-federated catalog"), not a registration OF it, so
/// holding one does not make its source available — and a federation naming no connection
/// at all names nothing through which a source could be reached.
///
/// This emulator records no registration of its own, so a source outside AWS's managed
/// integrations is unregistered here, which is what a `CreateCatalog` naming one is
/// refused for ([`messages::federation_source_not_registered`]).
pub fn is_registered_federation_source(federated: &FederatedCatalog) -> bool {
    federated
        .connection_name
        .as_deref()
        .is_some_and(is_managed_federation_connection)
}

/// Ceiling on the size of the matcher one name pattern compiles to, in bytes.
///
/// A pattern is request text and every stored name is matched against it, so the work it
/// can buy is bounded here rather than left to grow with a model's pattern length: the
/// compiled matcher runs in time linear in the name and never backtracks, and this
/// ceiling bounds how large a program a pattern may build — so this ceiling and the
/// per-database table ceiling together bound one request. It lives beside the matcher
/// because every listing that takes such a pattern needs the same bound; each refuses a
/// pattern past it with its OWN modelled client error.
pub const MAX_PATTERN_PROGRAM_BYTES: usize = 64 * 1024;

/// The matcher a name pattern selects names with, or the reason the value is no pattern
/// this catalog can match.
///
/// The pattern language is the one the Data Catalog's listings document: "A regular
/// expression pattern. If present, only those tables whose names match the pattern are
/// returned" (`GetTablesRequest$Expression`), in which `*` additionally stands for any run
/// of characters (including none) — the Hive-metastore substitution that makes `events*`
/// select `events` and `events_raw` while `ev_.*` selects `ev_one` and `ev_two`. The
/// pattern is anchored at both ends, because a listing selects the names that match it as
/// a WHOLE, and compared case-sensitively: a stored catalog name is folded to lowercase
/// ([`fold_name`]), the pattern is not, and a pattern in another case selects nothing.
///
/// A `*` that is ESCAPED (`\*`) or inside a character class (`[*]`) is the regular
/// expression's own star rather than the wildcard, so it is left as written: substituting
/// there would change `[*]` into a class of dot-or-star and `\*` into any run of dots,
/// i.e. a different pattern from the one the caller sent.
///
/// It lives here rather than in one operation's file because every listing of these
/// tables has to select them the same way — `GetTables`'s own `Expression`, and Athena's
/// `ListTableMetadata` over this same catalog — so there is ONE pattern language and two
/// listings of one set of tables can never disagree.
pub fn compile_name_pattern(pattern: &str) -> Result<::regex::Regex, ::regex::Error> {
    let mut translated = String::with_capacity(pattern.len() + 1);
    let mut escaped = false;
    let mut in_class = false;
    for character in pattern.chars() {
        let wildcard = character == '*' && !escaped && !in_class;
        if wildcard {
            translated.push_str(".*");
        } else {
            translated.push(character);
        }
        // The state for the NEXT character: a backslash escapes exactly the one after it,
        // and a class runs from an unescaped `[` to the next unescaped `]`.
        if escaped {
            escaped = false;
        } else {
            match character {
                '\\' => escaped = true,
                '[' => in_class = true,
                ']' => in_class = false,
                _ => {}
            }
        }
    }
    ::regex::RegexBuilder::new(&format!("^(?:{translated})$"))
        .size_limit(MAX_PATTERN_PROGRAM_BYTES)
        .build()
}

/// The IAM user name of the identity this emulator's long-term credentials speak for.
///
/// The emulator publishes exactly ONE long-term identity, and STS is what defines it:
/// `GetCallerIdentity` answers `arn:aws:iam::<account>:user/emulator` for every request
/// whose access key id names no session (`AMBIENT_USER_NAME` in
/// `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs`).
pub(super) const AMBIENT_USER_NAME: &str = "emulator";

/// The principal that made this request, as the RELATIVE id IAM gives that identity: the
/// resource part of its ARN, `user/<userName>`.
///
/// This is the form Glue records in `Connection$LastUpdatedBy` — "the user, group, or role
/// that last updated this connection definition", a `NameString` rather than an ARN — so a
/// client matching the updater against the identity's own short form matches.
/// [`request_principal`] renders the same identity as the full ARN for the members that
/// carry one.
pub fn request_principal_id() -> String {
    format!("user/{AMBIENT_USER_NAME}")
}

/// The principal that made this request, as the ARN the catalog records in
/// `Table$CreatedBy` ("The person or entity who created the table").
///
/// Server-derived from the emulator's own identity contract, never from the request: it is
/// the ARN of the identity a request signed with this emulator's credentials speaks
/// for, which is the one [`AMBIENT_USER_NAME`] names — so Glue and STS agree on who the
/// caller is. An access key id is a credential id, not a user name, so it is never
/// rendered into an IAM ARN: that is a principal no AWS rule produces. The value is
/// therefore the same on every call and across restarts, exactly as the IAM identity
/// behind one set of credentials is.
pub fn request_principal() -> String {
    ::engine::emulator_arn("iam", "", &request_principal_id())
}

/// The `CatalogId` a request addresses: the one it names, else this emulator's
/// account id, which is what the model documents as the default
/// ("If none is provided, the Amazon Web Services account ID is used by default").
pub fn resolve_catalog_id(catalog_id: Option<&String>) -> String {
    match catalog_id.map(String::as_str).filter(|id| !id.is_empty()) {
        Some(id) => id.to_string(),
        None => ::engine::emulator_account_id().to_string(),
    }
}

/// Whether `catalog_id` names the ACCOUNT-level Data Catalog this emulator holds.
///
/// It holds exactly one of those: the one for its own account, which is the catalog the
/// model documents as the default. A catalog id of any other account belongs to that
/// account's estate, reachable on the real service only through a Data Catalog resource
/// policy, so a request that names one is refused rather than quietly given a new, empty
/// catalog. The catalog RESOURCES a `CreateCatalog` registers inside this account carry
/// their own ids ([`child_catalog_id`]) and are addressed through the catalog operations
/// that take one ([`ComAmazonawsGlueAwsglueState::catalog_definition`]); the databases of
/// such a catalog are not served through the account-level operations yet.
pub fn is_known_catalog(catalog_id: &str) -> bool {
    catalog_id == ::engine::emulator_account_id()
}

/// `Catalog$CatalogType` for a catalog whose databases live in the Glue Data Catalog
/// itself: the account's own Data Catalog is one of these.
pub const CATALOG_TYPE_NATIVE: &str = "NATIVE";

/// `Catalog$CatalogType` for a catalog whose resources come from a source OUTSIDE the
/// Glue Data Catalog — a federated source or a Redshift resource link.
pub const CATALOG_TYPE_FEDERATED: &str = "FEDERATED";

/// `Catalog$Description` of the account's own Data Catalog: Glue's own fixed text for the
/// catalog every account has, which no `CreateCatalog` wrote and no client can set.
pub const DEFAULT_CATALOG_DESCRIPTION: &str = "This is the Default Catalog.";

/// The account's own Data Catalog as the modelled `Catalog` shape, as addressed in
/// `region`.
///
/// Every account HAS this catalog: it is the default a request that names no `CatalogId`
/// resolves to ([`resolve_catalog_id`]) and the container the account's databases and its
/// created catalogs live in, so an operation that addresses it by the account's own id
/// reports it instead of searching the records a `CreateCatalog` wrote
/// ([`ComAmazonawsGlueAwsglueState::catalog_definition`]).
///
/// Its name and id are the account's own number, and its ARN is the Data Catalog's own
/// ([`catalog_arn`]) rather than one naming a catalog resource inside it. It carries no
/// instants, because no `CreateCatalog` wrote it and so there is no write to timestamp,
/// and no definition a client chose: its description is Glue's own
/// ([`DEFAULT_CATALOG_DESCRIPTION`]), it is a [`CATALOG_TYPE_NATIVE`] catalog, and it
/// configures no properties and no Lake Formation default permissions — reported as the
/// empty map and the empty lists, the same way a stored catalog reports them
/// ([`CatalogDefinition::to_catalog`]).
pub fn account_data_catalog(region: &str) -> crate::ops::types::Catalog {
    let account_id = ::engine::emulator_account_id().to_string();
    crate::ops::types::Catalog {
        catalog_id: Some(account_id.clone()),
        catalog_identifier: Some(account_id.clone()),
        catalog_type: Some(CATALOG_TYPE_NATIVE.to_string()),
        name: account_id.clone(),
        resource_arn: Some(catalog_arn(region, &account_id)),
        description: Some(DEFAULT_CATALOG_DESCRIPTION.to_string()),
        catalog_properties: Some(CatalogPropertiesOutput::default()),
        create_table_default_permissions: Some(Vec::new()),
        create_database_default_permissions: Some(Vec::new()),
        ..crate::ops::types::Catalog::default()
    }
}

/// The `CatalogId` of the catalog `name` inside the Data Catalog of `account_id`: the
/// account the catalog was created in, then the catalog's own name.
///
/// A `CreateCatalog` carries no parent id — it creates the catalog in the Data Catalog of
/// the account it is served for — and the catalog it created is then addressed by this
/// id, which is what [`parse_catalog_reference`] reads back apart.
pub fn child_catalog_id(account_id: &str, name: &str) -> String {
    format!("{account_id}:{name}")
}

/// A `CatalogId` naming one catalog RESOURCE, split into the account whose Data Catalog
/// holds it and the catalog's own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogReference {
    /// The account the catalog was created in: the part before the separator, or this
    /// emulator's own account for an id that is a bare catalog name, which names a
    /// catalog of the account the request is served for.
    pub account_id: String,
    /// The catalog's name, as the request spelled it.
    pub name: String,
}

/// The catalog resource `catalog_id` names ([`child_catalog_id`] read back apart), for
/// every operation that addresses one.
///
/// A bare name is a catalog of the account this endpoint serves, which is the default the
/// model documents for a `CatalogId` ("If none is provided, the Amazon Web Services
/// Account Number is used by default") — so `GetCatalog` reaches a catalog of its own
/// account by name alone, and a catalog of another account only by naming that account.
///
/// The account prefix is told apart from a catalog name that itself carries a colon by
/// what an account prefix IS: an Amazon Web Services account number
/// ([`ACCOUNT_ID_DIGITS`]). An id that is such a number and nothing else names that
/// account's own Data Catalog, which HOLDS catalogs but is not one of them, so it is
/// resolved into that account and a name no catalog can carry — the account's id, which
/// `Catalog$Name` cannot be.
pub fn parse_catalog_reference(catalog_id: &str) -> CatalogReference {
    if let Some((account_id, name)) = catalog_id
        .split_once(':')
        .filter(|(prefix, _)| is_account_id(prefix))
    {
        return CatalogReference {
            account_id: account_id.to_string(),
            name: name.to_string(),
        };
    }
    if is_account_id(catalog_id) {
        return CatalogReference {
            account_id: catalog_id.to_string(),
            name: catalog_id.to_string(),
        };
    }
    CatalogReference {
        account_id: ::engine::emulator_account_id().to_string(),
        name: catalog_id.to_string(),
    }
}

/// How many digits an Amazon Web Services account number carries. It is what tells a
/// `CatalogId`'s optional account prefix from a catalog name, and so from a name that
/// carries a colon of its own (`CatalogNameString`'s pattern forbids the three path
/// separators and the `aws:` prefix, not a colon).
pub(super) const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is spelled as an Amazon Web Services account number: twelve decimal
/// digits ([`ACCOUNT_ID_DIGITS`]).
pub(super) fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The resource portion of a Data Catalog's ARN: the catalog itself, which is the
/// resource a Data Catalog resource policy is attached to and the one an authorization
/// refusal names.
pub const CATALOG_ARN_RESOURCE: &str = "catalog";

/// The IAM action namespace every Glue action is spelled under, as an authorization
/// refusal names the action (`glue:GetTables`).
pub const IAM_ACTION_PREFIX: &str = "glue:";

/// The ARN of the Data Catalog `catalog_id` names, as addressed in `region`.
///
/// The canonical layout and partition come from the shared minter, so this ARN matches
/// every other ARN this emulator emits; only the ACCOUNT field is replaced, because a
/// request may address the catalog of another account and that account is what such an
/// ARN names.
pub fn catalog_arn(region: &str, catalog_id: &str) -> String {
    glue_arn_for_account(region, catalog_id, CATALOG_ARN_RESOURCE)
}

/// `Catalog$ResourceArn`: "the Amazon Resource Name (ARN) assigned to the catalog
/// resource" — the ARN of the catalog `name` created in the Data Catalog of `account_id`,
/// as addressed in `region`.
///
/// The catalog itself is the resource TYPE ([`CATALOG_ARN_RESOURCE`]) and the catalog's
/// name the resource, which is the layout every other Glue resource ARN follows
/// ([`TaggedResourceKind::arn`]).
pub fn catalog_resource_arn(region: &str, account_id: &str, name: &str) -> String {
    glue_arn_for_account(region, account_id, &format!("{CATALOG_ARN_RESOURCE}/{name}"))
}

/// A Glue ARN for `resource` in the estate of `account_id`, as addressed in `region`.
///
/// The canonical layout and partition come from the shared minter, so this ARN matches
/// every other ARN this emulator emits; only the ACCOUNT field is replaced, because a
/// request may address a catalog of another account and that account is what such an ARN
/// names.
pub(super) fn glue_arn_for_account(region: &str, account_id: &str, resource: &str) -> String {
    let canonical = ::engine::emulator_arn(ARN_NAMESPACE, region, resource);
    let mut fields: Vec<&str> = canonical.split(':').collect();
    // Field 5 of an ARN is the account; the canonical form always has it.
    if let Some(account) = fields.get_mut(4) {
        *account = account_id;
    }
    fields.join(":")
}

/// The AUTHORIZATION refusal alone, for the operations whose modelled error set has no
/// entity miss in it: `CreateDatabase`, `CreateConnection`, `BatchDeleteConnection`,
/// `SearchTables`, `GetDataCatalogEncryptionSettings`,
/// `PutDataCatalogEncryptionSettings`, `GetCatalogImportStatus` and `ImportCatalogToGlue`
/// model no `EntityNotFoundException`, so a catalog that does not exist cannot be reported
/// to their callers as a missing entity — an error code outside an operation's modelled
/// set is one no strongly typed client can map — and the one thing they can state about a
/// `CatalogId` that is not this account's own is that the caller may not act on it.
///
/// Every other catalog-addressed operation goes through [`catalog_refusal`], which tells
/// the missing catalog and the foreign one apart; the sentence here is the same one that
/// decision carries for a foreign catalog, so the two cannot disagree about it.
pub fn foreign_catalog_refusal(catalog_id: &str, region: &str, action: &str) -> Option<String> {
    (!is_known_catalog(catalog_id))
        .then(|| messages::not_authorized_on_catalog(action, &catalog_arn(region, catalog_id)))
}

/// Why a request addressed to the Data Catalog `catalog_id` cannot be served, decided
/// BEFORE the entity lookup ([`catalog_refusal`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogRefusal {
    /// `catalog_id` names no Data Catalog at all: it is not an Amazon Web Services account
    /// number, so it addresses no account's Data Catalog, and it names no catalog this
    /// account holds either — there is no catalog for an entity to be looked up in.
    /// Carries the sentence the miss is reported with ([`messages::CATALOG_NOT_FOUND`]),
    /// to be answered as the operation's own `EntityNotFoundException`.
    NotFound(String),
    /// `catalog_id` names the Data Catalog of ANOTHER account. Carries the authorization
    /// sentence, to be answered as `AccessDeniedException`.
    NotAuthorized(String),
}

/// The refusal a request addressed to the Data Catalog `catalog_id` earns before any
/// entity of it is looked up, or `None` when that catalog is one this endpoint serves —
/// the account's own Data Catalog ([`is_known_catalog`]) or a catalog resource a
/// `CreateCatalog` registered in it ([`ComAmazonawsGlueAwsglueState::catalog_definition`]).
///
/// The two refusals answer two different faults, and WHICH one an id earns is decided by
/// what the id addresses rather than by what the store happens to hold:
///
/// * An id that is an account number other than this endpoint's own addresses that
///   account's Data Catalog, which is reachable on the real service only through that
///   catalog's own resource-based policy. A caller holding no grant on it is refused as
///   unauthorized rather than told the entity does not exist — which would also answer a
///   cross-account read out of the caller's OWN catalog whenever the two hold a same-named
///   entity. This store can hold no resource policy of a FOREIGN catalog —
///   `PutResourcePolicy` attaches a policy only to a resource of the account this endpoint
///   serves ([`resolve_policy_resource_arn`]) — so no such grant can exist here and every
///   cross-account request is refused.
/// * An id that addresses no account's Data Catalog and no catalog of this account names
///   NOTHING, and a request naming nothing is a miss: the catalog does not exist, so
///   authorization over it is not a question the service can even reach, and reporting one
///   would send a client to write IAM policy for a catalog that is absent.
///
/// `action` is the operation's own name, which the authorization refusal names as the IAM
/// action ([`IAM_ACTION_PREFIX`]) the caller may not perform.
pub fn catalog_refusal(
    store: &ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    region: &str,
    action: &str,
) -> Option<CatalogRefusal> {
    if is_known_catalog(catalog_id) {
        return None;
    }
    let reference = parse_catalog_reference(catalog_id);
    if !is_served_account(&reference.account_id) {
        return Some(CatalogRefusal::NotAuthorized(
            messages::not_authorized_on_catalog(action, &catalog_arn(region, catalog_id)),
        ));
    }
    if store.catalog_definition(&reference).is_some() {
        return None;
    }
    Some(CatalogRefusal::NotFound(
        messages::CATALOG_NOT_FOUND.to_string(),
    ))
}

/// Whether `account_id` — field 5 of an ARN — is the account this emulator serves, and
/// so the only account whose Glue resources this store can hold. An ARN naming any other
/// account names a resource of another account's estate, which this process does not
/// hold.
pub fn is_served_account(account_id: &str) -> bool {
    account_id == ::engine::emulator_account_id()
}

/// The Region a request is scoped to: the one its SigV4 credential scope names, or
/// [`DEFAULT_REGION`] for a request that carries no scope (an unsigned call).
///
/// A Glue record carries no Region of its own — every operation that addresses a resource
/// names it by name — so the Region a resource's ARN is minted for, and compared against,
/// is the one its request is served in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a request with no SigV4 credential scope is scoped to. Matches the Region
/// the emulator's own generated client config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace every Glue resource lives in, as the modelled
/// `GlueResourceArn` pattern spells it
/// (`^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:`).
pub const ARN_NAMESPACE: &str = "glue";

/// One kind of Glue resource that carries its OWN tag set, and so can be named by a
/// tagging call's `ResourceArn`.
///
/// The kinds are the ones whose create carries a `Tags` map and whose record in this
/// store therefore holds a tag set: a Data Catalog database (`CreateDatabase$Tags`), a
/// connection (`CreateConnection$Tags`), a crawler (`CreateCrawler$Tags`) and a job
/// definition (`CreateJob$Tags`). A table is NOT one of them — `CreateTable` models no
/// `Tags` member — and a Glue resource this emulator does not store yet has no tag set
/// to name either.
///
/// A schema-registry record holds a tag set too ([`SchemaEntry::tags`]), but is not a kind
/// here: its ARN's resource portion carries two path segments
/// (`schema/<registry>/<schema>`), which this one-segment table does not spell, so
/// resolving one through the tagging operations belongs to those operations' own issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// `connection/<name>`.
    Connection,
    /// `crawler/<name>`.
    Crawler,
    /// `database/<name>`.
    Database,
    /// `job/<name>`.
    Job,
}

/// The ARN resource-type segment each taggable kind carries: the catalog
/// [`parse_tagged_resource`] resolves an ARN against and [`TaggedResourceKind::arn`]
/// mints one from.
///
/// Source: the resource portion of each resource's documented Glue ARN
/// (`database/<name>`, `connection/<name>`, `crawler/<name>`, `job/<name>`). The model
/// states only the prefix every Glue ARN shares (`GlueResourceArn`'s pattern, see
/// [`ARN_NAMESPACE`]), so the segments are listed here once and every mint and every
/// lookup goes through this one table.
pub const TAGGED_RESOURCE_TYPES: [(&str, TaggedResourceKind); 4] = [
    ("connection", TaggedResourceKind::Connection),
    ("crawler", TaggedResourceKind::Crawler),
    ("database", TaggedResourceKind::Database),
    ("job", TaggedResourceKind::Job),
];

/// How many tags one resource's tag set may hold at once.
///
/// Source: the modelled `TagsMap` bound (len 0..50), which is the shape BOTH a tag
/// write's `TagsToAdd` and `GetTags`'s `Tags` target — so a tag set past this many
/// entries could not be reported by the read that serves it. A write that would carry a
/// resource past it is refused instead of stored.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

impl TaggedResourceKind {
    /// This kind's ARN resource-type segment, from [`TAGGED_RESOURCE_TYPES`].
    pub fn arn_segment(self) -> &'static str {
        TAGGED_RESOURCE_TYPES
            .iter()
            .find(|(_, kind)| *kind == self)
            .map(|(segment, _)| *segment)
            // Every variant has a row above, so this is unreachable; answering the
            // service namespace rather than panicking keeps a request path alive.
            .unwrap_or(ARN_NAMESPACE)
    }

    /// The ARN of the resource `name` of this kind, as served in `region`.
    ///
    /// The account segment is this emulator's own ([`::engine::emulator_arn`]), which is
    /// also the only `CatalogId` a catalog resource can live under here (see
    /// [`is_known_catalog`]), so a minted ARN and a stored record always agree on whose
    /// resource it is.
    pub fn arn(self, region: &str, name: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("{}/{name}", self.arn_segment()),
        )
    }
}

/// A Glue resource a tagging call named by ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    /// Which kind of resource the ARN's resource-type segment names.
    pub kind: TaggedResourceKind,
    /// Field 4 of the ARN: the Region whose estate the resource belongs to.
    pub region: String,
    /// Field 5 of the ARN: the account that owns the resource, which for a Data Catalog
    /// resource is also the `CatalogId` it lives under ("If none is provided, the Amazon
    /// Web Services account ID is used by default").
    pub account_id: String,
    /// The resource's name, folded exactly as the store keys it: lowercased for a
    /// database ([`fold_name`]), as written for the kinds the model documents no fold
    /// for.
    pub name: String,
}

/// The Glue resource `resource_arn` names, or `None` when it is not the ARN of a taggable
/// Glue resource at all — a value in another service's namespace, a malformed ARN, or a
/// Glue resource kind this store holds no tag set for.
///
/// Only the KIND and the identity fields are read here; whether this store holds the
/// resource is [`ComAmazonawsGlueAwsglueState::resource_tags`]'s answer, so an ARN of a
/// known kind that names nothing earns that kind's not-found refusal rather than being
/// called malformed.
pub fn parse_tagged_resource(resource_arn: &str) -> Option<TaggedResource> {
    let parsed = parse_glue_arn(resource_arn)?;
    let (segment, name) = parsed.resource.split_once('/')?;
    if name.is_empty() {
        return None;
    }
    let (_, kind) = TAGGED_RESOURCE_TYPES
        .iter()
        .find(|(candidate, _)| *candidate == segment)?;
    Some(TaggedResource {
        kind: *kind,
        region: parsed.region.clone(),
        account_id: parsed.account_id.clone(),
        name: match kind {
            // The two Hive-compatible catalog resources are the ones the catalog folds;
            // a crawler, job or connection name is keyed as written.
            TaggedResourceKind::Database => fold_name(name),
            TaggedResourceKind::Connection
            | TaggedResourceKind::Crawler
            | TaggedResourceKind::Job => name.to_string(),
        },
    })
}

/// The message a refusal carries for a `ResourceArn` of a known kind that names no
/// resource this store holds.
///
/// Each sentence is the one this crate already answers wherever that kind fails to
/// resolve ([`messages`]), so "this ARN names no database" reads the same from a tagging
/// operation as from `GetDatabase`. The two kinds whose refusal wording no recording or
/// doc states take a sentence DERIVED from the request instead: it names the ARN that
/// resolved to nothing, and what is pinned is the behaviour — 400 with
/// `EntityNotFoundException`.
pub fn tagged_resource_not_found_message(resource: &TaggedResource, resource_arn: &str) -> String {
    match resource.kind {
        TaggedResourceKind::Database => messages::database_not_found(&resource.name),
        TaggedResourceKind::Crawler => messages::crawler_not_found(&resource.name),
        TaggedResourceKind::Connection | TaggedResourceKind::Job => {
            format!("Entity not found for resource {resource_arn}")
        }
    }
}

/// Whether the refusal of a `ResourceArn` that is not the ARN of a taggable Glue resource
/// ([`parse_tagged_resource`] answers `None`) states any prose at all: it does NOT.
///
/// RECORDED behaviour of the tagging surface: such a request is answered with the
/// operation's modelled `InvalidInputException` carrying its CODE and no message member at
/// all — the refusal says that the ARN is not one of this service's, and states neither the
/// value it was handed nor the resource types the member admits. One copy, shared by the
/// three tagging operations that resolve a `ResourceArn` through [`parse_tagged_resource`],
/// so none of them invents prose for a refusal the service words silently.
pub const INVALID_RESOURCE_ARN_MESSAGE: Option<String> = None;

/// The message a refusal carries for a tag write that would leave the resource holding
/// more tags than one tag set may have ([`MAX_TAGS_PER_RESOURCE`]).
///
/// Derived from the request, like this crate's other refusals whose wording no recording
/// or doc states: it names how many tags the write would leave and the bound it passes.
/// What is pinned is the behaviour — 400 with `InvalidInputException`, and nothing
/// stored.
pub fn tag_limit_exceeded_message(would_hold: usize) -> String {
    format!(
        "TagsToAdd would leave the resource holding {would_hold} tags, more than the \
         {MAX_TAGS_PER_RESOURCE} allowed per resource"
    )
}

/// A wall-clock instant as the modelled timestamp type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The same instant as the `LongValue` members that carry "the unix time and date"
/// (`Statement$StartedOn`) report it: milliseconds since the epoch, which is the scale a
/// Glue statement's instants are stated in.
pub fn to_unix_millis(at: SystemTime) -> i64 {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// How many digits the instant part of a continuation token is padded to, so the token
/// orders the same way as a string as the instant it names orders as a number. Twenty
/// digits hold any nanosecond count this clock can report.
pub(super) const CURSOR_INSTANT_DIGITS: usize = 20;

/// One record's place in a time-ordered listing, as that listing's continuation token
/// renders it: the instant the listing orders by, zero-padded so the string orders as the
/// number does, then the record's own id to separate two records stamped in the same
/// nanosecond.
///
/// ONE copy, shared by every listing of this crate that pages over a run history
/// (`GetJobRuns`, `GetColumnStatisticsTaskRuns`), so the token format cannot drift between
/// them. Comparing a token against this key is what resumes a page, which is why any
/// string a request sends names a well-defined place in the order.
pub fn history_page_cursor(ordered_by: SystemTime, id: &str) -> String {
    let at = ordered_by
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{at:0CURSOR_INSTANT_DIGITS$}:{id}")
}

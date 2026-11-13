// SPDX-License-Identifier: CC0-1.0

//! The metadata records of this service's data-asset surface: form types, asset types,
//! assets, glossaries and glossary terms, with the refusals each miss answers with.

use super::*;

/// Whether an `UpdateWorkflow` request names a member of the workflow to change at all.
///
/// Every member of that request other than the `Name` addressing the workflow is optional,
/// so a request carrying none of them asks for no change: nothing is written, so the
/// workflow's `LastModifiedOn` stays where the last write that did change something put it
/// — the same way an `UpdateCrawler` naming no member leaves its crawler's `LastUpdated`
/// alone ([`crawler_update_names_a_change`]). Written as an exhaustive destructuring so a
/// member added to the request shape cannot be silently left out of the answer.
pub fn workflow_update_names_a_change(input: &UpdateWorkflowInput) -> bool {
    let UpdateWorkflowInput {
        name: _,
        description,
        default_run_properties,
        max_concurrent_runs,
    } = input;

    description.is_some() || default_run_properties.is_some() || max_concurrent_runs.is_some()
}

/// The message the catalog's metadata form type surface refuses an identifier it holds no
/// form type under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// This surface's refusal text is itself a JSON document carrying the sentence under a
/// `Message` key, so that text — braces and all — is what the modelled member carries, and
/// a client reading `Message` reads the document rather than a bare sentence. It is NOT an
/// envelope this emulator adds around the member: the member's own value is this text, as
/// the surface words it. `serde_json` renders it, so an identifier carrying a quote or a
/// backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a form type identifier
/// ([`ComAmazonawsGlueAwsglueState::form_type_by_reference`]), so two of them cannot
/// disagree about how the one miss is worded.
pub fn form_type_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Metadata form type {identifier} not found."))
}

/// `sentence` as the business-catalog surface puts a refusal on the wire: a JSON document
/// carrying it under a `Message` key.
///
/// One copy, shared by every message builder of that surface below, so the DOCUMENT shape
/// cannot drift between them while each keeps the sentence its own refusal words. Rendered
/// by `serde_json`, so a sentence carrying a quote or a backslash still yields a
/// well-formed document.
pub(super) fn catalog_refusal_document(sentence: &str) -> String {
    serde_json::json!({ "Message": sentence }).to_string()
}

/// The message the catalog's business glossary surface refuses an identifier it holds no
/// glossary under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type one above does
/// ([`form_type_not_found_message`]): the refusal text is itself a JSON document carrying
/// the sentence under a `Message` key, so that text — braces and all — is what the modelled
/// member carries, and the resource is named as the service names it, a business glossary.
/// `serde_json` renders it, so an identifier carrying a quote or a backslash still yields a
/// well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a `GlossaryId`
/// ([`ComAmazonawsGlueAwsglueState::glossary`]) — the glossary's own reader and the term
/// create that puts a term inside one — so two of them cannot disagree about how the one
/// miss is worded.
pub fn glossary_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Business glossary {identifier} not found."))
}

/// The message every refusal of a glossary NAME this account already holds a glossary
/// under carries: the value of that refusal's `AlreadyExistsException$Message`.
///
/// Worded the way this surface words every refusal ([`glossary_not_found_message`],
/// [`form_type_not_found_message`]): the refusal text is itself a JSON document carrying
/// the sentence under a `Message` key, so that text — braces and all — is what the
/// modelled member carries. The sentence names the glossary the write asked for, in
/// single quotes, which is its one variable part; `serde_json` renders it, so a name
/// carrying a quote or a backslash still yields a well-formed document.
///
/// One copy, shared by the create that would mint a second glossary under a stored name
/// and the update that would rename one onto it, so the two cannot disagree about how the
/// one collision is worded.
pub fn glossary_already_exists_message(name: &str) -> String {
    catalog_refusal_document(&format!("A glossary with name '{name}' already exists."))
}

/// The message every refusal of a glossary TERM name the addressed glossary already holds
/// a term under carries: the value of that refusal's `AlreadyExistsException$Message`.
///
/// Worded the way this surface words a name collision one level up
/// ([`glossary_already_exists_message`]): the refusal text is itself a JSON document
/// carrying the sentence under a `Message` key, and the sentence names the resource kind the
/// collision is about — a glossary term — the name the write asked for, and the GLOSSARY it
/// asked for it in, because a term's name identifies it within one glossary and is free in
/// every other. Both variable parts come from the request; `serde_json` renders the
/// document, so a name carrying a quote or a backslash still yields a well-formed one.
pub fn glossary_term_already_exists_message(glossary: &str, name: &str) -> String {
    catalog_refusal_document(&format!(
        "A glossary term with name '{name}' already exists in glossary '{glossary}'."
    ))
}

/// A stored form type: the schema for structured metadata `PutFormType` registers in the
/// Data Catalog.
///
/// Every member of the modelled form type is held here — its id, its name and its schema
/// — and nothing else: the model gives a form type no status, owner or timestamp. The
/// name and the schema come from the request, and the id is derived from the name (see
/// [`Self::id`]).
#[derive(Debug, Clone)]
pub struct FormTypeEntry {
    /// `FormTypeId`: the identifier a client addresses this form type by, which is the
    /// form type's own NAME.
    ///
    /// Not an opaque per-record id: the catalog can word a refusal for a form type it no
    /// longer holds with that form type's name while all it was given is the identifier,
    /// so the identifier it hands out carries the name. `FormTypeName`'s `length 1..128`
    /// sits inside `FormTypeId`'s `length 1..256`, so every name is a usable identifier,
    /// and because the name is also the namespace's key, two form types never share one
    /// identifier and an update under a name keeps the identifier an earlier put reported.
    pub id: String,
    /// The form type's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::form_types`].
    pub name: String,
    /// "The Smithy IDL schema definition for the form type", stored exactly as sent: the
    /// catalog reports back the schema text the client wrote, not a re-rendering of it.
    pub schema: String,
}

impl FormTypeEntry {
    /// The record a put stores for a name this store does not hold yet: the request's
    /// name and schema, under a freshly minted id.
    pub fn new(name: String, schema: String) -> Self {
        Self {
            id: name.clone(),
            name,
            schema,
        }
    }
}

/// `FormTypeName`'s modelled `@pattern` and `@length` bounds, as the model writes them
/// (`smithy_query(query="shape",shape="FormTypeName")`: pattern `^[A-Z]\w*$`,
/// `length 1..128`) — held here exactly as declared, because a refusal of this member
/// quotes the expression the way the model spells it.
const FORM_TYPE_NAME_PATTERN: &str = r"^[A-Z]\w*$";

/// See [`FORM_TYPE_NAME_PATTERN`].
const FORM_TYPE_NAME_MAX_CHARS: i64 = 128;

/// See [`FORM_TYPE_NAME_PATTERN`].
const FORM_TYPE_NAME_MIN_CHARS: i64 = 1;

/// Whether `name` satisfies the expression [`FORM_TYPE_NAME_PATTERN`] declares: it opens
/// with an upper-case letter and carries nothing but letters, digits and underscores after
/// it, which is what `[A-Z]\w*` admits over the whole value.
fn matches_form_type_name_pattern(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_uppercase())
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Every requirement of `FormTypeName` the `Name` of a form type write breaks, in the order
/// the member declares them — empty when the name is one this surface admits.
///
/// Checked HERE, and not at the framework boundary, because this surface answers a name it
/// will not take with its OWN client-input error rather than with the cross-service
/// `ValidationException` (`skip_validation` on `PutFormTypeRequest` in `src/ops/types/`),
/// the same division every other member Glue judges itself follows. The requirements are
/// the member's own, so the clauses a refusal states are the clauses the boundary would
/// have stated for the same value.
fn form_type_name_violations(name: &str) -> Vec<::smithy_server_runtime::Violation> {
    let path = ::aws_smithy_schema_server::constraints::path::member("Name");
    let characters = i64::try_from(name.chars().count()).unwrap_or(i64::MAX);
    let mut broken = Vec::new();
    if characters < FORM_TYPE_NAME_MIN_CHARS {
        broken.push(::smithy_server_runtime::ViolationKind::MinLength(
            FORM_TYPE_NAME_MIN_CHARS,
        ));
    }
    if characters > FORM_TYPE_NAME_MAX_CHARS {
        broken.push(::smithy_server_runtime::ViolationKind::MaxLength(
            FORM_TYPE_NAME_MAX_CHARS,
        ));
    }
    if !matches_form_type_name_pattern(name) {
        broken.push(::smithy_server_runtime::ViolationKind::Pattern(
            FORM_TYPE_NAME_PATTERN.to_string(),
        ));
    }
    broken
        .into_iter()
        .map(|kind| ::smithy_server_runtime::Violation {
            path: path.clone(),
            kind,
            value: None,
            sensitive: false,
        })
        .collect()
}

/// The message the form type surface refuses a `Name` outside what `FormTypeName` admits
/// with — the value of that refusal's `InvalidInputException$Message` — or `None` when the
/// name is one it admits.
///
/// RECORDED shape, and it is the two renderings of this surface put together: the text is
/// itself a JSON document carrying the refusal under a `Message` key
/// ([`catalog_refusal_document`]), and the refusal inside it is AWS's own constraint report
/// over the requirements the name broke ([`stated_business_metadata_constraint`]) — the
/// member path, the requirement, the value WITHHELD and a `@pattern` quoted exactly as the
/// model declares it, anchors included.
pub fn malformed_form_type_name_message(name: &str) -> Option<String> {
    stated_business_metadata_constraint(form_type_name_violations(name))
}

/// One constraint report as the business-metadata surface states it: the shared renderer's
/// envelope and clauses, in THIS surface's dialect, wrapped in its JSON document.
///
/// The envelope, the clause and every requirement are the shared renderer's
/// (`service_common::validation_message`), so a report this surface states reads like the
/// one the boundary states for the same violation. The two choices are this surface's and
/// are what its recordings show: the clause WITHHOLDS the value — it names the member and
/// the requirement and quotes nothing of what was sent — and a `@pattern` is quoted as the
/// model declares it, anchoring pair included, rather than in the unanchored rendering the
/// catalog surface's refusals quote.
///
/// `None` for no violation at all, so a caller cannot state an empty report.
fn stated_business_metadata_constraint(
    violations: Vec<::smithy_server_runtime::Violation>,
) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let stated = ::service_common::validation_message::aws_validation_refusal_withholding_with(
        &::smithy_server_runtime::Refusal::Invalid(violations),
        |pattern| pattern.to_string(),
        |_| true,
    )?;
    Some(catalog_refusal_document(&stated.message))
}

/// Which records still USE one form type, and the sentence a delete held up by such a use
/// is refused with.
impl ComAmazonawsGlueAwsglueState {
    /// Every ASSET TYPE that still references the form type `identifier` addresses,
    /// deduplicated and in the order this namespace holds those records — empty when nothing
    /// references it, and when `identifier` names no form type this store holds at all.
    ///
    /// A form type "cannot be deleted if it is still referenced by an asset type"
    /// (`DeleteFormType`), so the reference that holds a delete up is an asset type's: one
    /// that DECLARES the form type among its forms references it for as long as that
    /// declaration stands, and the delete of the form type is refused until the asset type
    /// itself is gone. An ASSET that carries a form of the type references it through the
    /// asset type it is an instance of, so a stored asset is named by that asset type too —
    /// which keeps the dependency observable where the asset outlives its type's record. The
    /// delete asks this before removing anything, and the refusal names every dependency that
    /// holds it up rather than one of them.
    ///
    /// An asset type's forms carry the `FormTypeIdentifier` its own put wrote
    /// ([`AssetTypeEntry::forms`]), an asset's carry the one its put wrote
    /// ([`AssetEntry::forms`]), and either is resolved the way every reader of a form type
    /// reference resolves one ([`Self::form_type_by_reference`]), so another spelling of one
    /// stored form type still counts as the same reference. The asset type an asset is an
    /// instance of is resolved the same way ([`Self::asset_type_by_reference`]), falling back
    /// to the identifier the put stored for an asset type that is no longer there. The walk is
    /// bounded by [`MAX_ASSET_TYPES`] and [`MAX_ASSETS`] records of [`MAX_ASSET_FORMS`] forms.
    pub fn asset_types_using_form_type(&self, identifier: &str) -> Vec<String> {
        let Some(form_type) = self.form_type_by_reference(identifier) else {
            return Vec::new();
        };
        let references_it = |reference: &str| {
            self.form_type_by_reference(reference)
                .is_some_and(|stored| stored.name == form_type.name)
        };
        let mut dependencies: Vec<String> = Vec::new();
        let mut hold_up = |asset_type: String| {
            if !dependencies.contains(&asset_type) {
                dependencies.push(asset_type);
            }
        };
        for asset_type in self.asset_types.values() {
            if asset_type
                .forms
                .values()
                .any(|reference| references_it(reference))
            {
                hold_up(asset_type.name.clone());
            }
        }
        for asset in self.assets.values() {
            let uses_it = asset
                .forms
                .values()
                .any(|form| form.form_type_id.as_deref().is_some_and(references_it));
            if !uses_it {
                continue;
            }
            hold_up(
                self.asset_type_by_reference(&asset.asset_type_id)
                    .map_or_else(|| asset.asset_type_id.clone(), |stored| stored.name.clone()),
            );
        }
        dependencies
    }
}

/// The message the form type surface refuses a DELETE held up by an asset's use of it with:
/// the value of that refusal's `ConflictException$Message`.
///
/// RECORDED template, worded the way this surface words every refusal
/// ([`form_type_not_found_message`]): the text is itself a JSON document carrying the
/// sentence under a `Message` key. The sentence states the rule the operation documents
/// ("A form type cannot be deleted if it is still referenced by an asset type",
/// `DeleteFormType`) and then LISTS the dependencies that hold the delete up, each named the
/// way this surface names an asset type as a dependency ([`asset_type_dependency`]). Both
/// variable parts are read off the store: the form type the delete addressed, and the asset
/// type of every asset still using it.
pub fn form_type_referenced_message(form_type: &str, asset_types: &[String]) -> String {
    let dependencies: Vec<String> = asset_types
        .iter()
        .map(|asset_type| asset_type_dependency(asset_type))
        .collect();
    catalog_refusal_document(&format!(
        "Form type {form_type} cannot be deleted because of the following dependencies: {}",
        dependencies.join(", ")
    ))
}

/// How this surface names an ASSET TYPE as a DEPENDENCY of another record: the kind of
/// dependency it is (`gluetype`, an asset type being a type of the business catalog), then
/// the asset type's address — the entity kind, the catalog that holds it
/// ([`CATALOG_QUALIFIER_PREFIX`] and the account, as every catalog-qualified identifier of
/// this surface spells it) and the asset type's own name, joined by `#`.
fn asset_type_dependency(asset_type: &str) -> String {
    format!(
        "gluetype glueAsset#{CATALOG_QUALIFIER_PREFIX}{}#{asset_type}",
        ::engine::emulator_account_id()
    )
}

/// The prefix a catalog-qualified identifier opens with: the catalog the entity lives in
/// is this emulator's own Data Catalog, which is named for the service and the account
/// that holds it.
///
/// One half of [`catalog_qualified_id`]; see that function for the whole form.
pub const CATALOG_QUALIFIER_PREFIX: &str = "glue_";

/// What separates the catalog an entity lives in from the entity's own name inside it.
pub const CATALOG_QUALIFIER_SEPARATOR: &str = "::";

/// The catalog-qualified identifier of a business-catalog entity called `name`:
/// `glue_<accountId>::<name>`, i.e. the catalog that holds it and its name within that
/// catalog, which is the form a READ of such an entity reports its id in.
///
/// Derived, not minted: every caller that knows the account and the name derives the same
/// identifier, so two reads of one entity report one id and a cross-reference built from
/// that id resolves ([`asset_type_name_from_qualified_id`] reads the name back out of it).
/// The account is this emulator's own ([`::engine::emulator_account_id`]), the only account
/// whose catalog it serves.
pub fn catalog_qualified_id(name: &str) -> String {
    format!(
        "{CATALOG_QUALIFIER_PREFIX}{}{CATALOG_QUALIFIER_SEPARATOR}{name}",
        ::engine::emulator_account_id()
    )
}

/// The name inside the catalog that `identifier` names, when it is the catalog-qualified
/// form [`catalog_qualified_id`] builds for the account this emulator serves, or `None`
/// when it is not that form at all.
///
/// This is what lets an entity be ADDRESSED by the identifier a read of it reported: the
/// qualified id is not stored, so resolving one is reading the name back out of it.
pub fn asset_type_name_from_qualified_id(identifier: &str) -> Option<&str> {
    let qualifier = format!(
        "{CATALOG_QUALIFIER_PREFIX}{}{CATALOG_QUALIFIER_SEPARATOR}",
        ::engine::emulator_account_id()
    );
    identifier
        .strip_prefix(&qualifier)
        .filter(|name| !name.is_empty())
}

/// A stored asset type: the structure of an asset `PutAssetType` registers in the Data
/// Catalog, as the set of forms an asset of that type includes.
///
/// Every member of the modelled asset type is held here — its id, its name and its forms
/// — and nothing else: the model gives an asset type no status, owner or timestamp. The
/// name and the forms come from the request, and the id is derived from the name (see
/// [`Self::id`]).
#[derive(Debug, Clone)]
pub struct AssetTypeEntry {
    /// `AssetTypeId` as the put that registers an asset type reports it: the NAME the
    /// caller supplied.
    ///
    /// Not an opaque per-record id: "if an asset type with the given name already exists,
    /// it is updated" makes the name the record's identity, and `AssetTypeName`'s
    /// `length 1..128` sits inside `AssetTypeId`'s `length 1..256`, so every name is a
    /// usable identifier — exactly as a form type's is ([`FormTypeEntry::id`]). A read of
    /// the asset type reports the same identity in its catalog-qualified form
    /// ([`Self::qualified_id`]), and both forms address this record
    /// ([`ComAmazonawsGlueAwsglueState::asset_type_by_reference`]).
    pub id: String,
    /// The asset type's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::asset_types`].
    pub name: String,
    /// "The forms that make up the asset type, keyed by form name", each value the
    /// identifier of the form type that defines that form's schema, stored exactly as the
    /// put sent it (`AssetTypeFormReference$FormTypeIdentifier`, the reference's only
    /// member) so a read reports the identifier the client wrote. It resolved against this
    /// store's form types when it was stored
    /// ([`ComAmazonawsGlueAwsglueState::form_type_by_reference`]), so it is either a
    /// minted `FormTypeId` or the `FormTypeName` the form type was registered under.
    /// Ordered by form name, so two reads of one asset type report the forms in one order.
    pub forms: BTreeMap<String, String>,
}

impl AssetTypeEntry {
    /// The record a put stores for a name this store does not hold yet: the request's
    /// name and form references, under the identifier that name derives.
    pub fn new(name: String, forms: BTreeMap<String, String>) -> Self {
        Self {
            id: name.clone(),
            name,
            forms,
        }
    }

    /// This asset type's identity in the catalog-qualified form a READ of it reports:
    /// `glue_<accountId>::<name>` ([`catalog_qualified_id`]).
    ///
    /// Derived from the name each time rather than stored, so it cannot drift from the
    /// record's own name, and the catalog's own reference to the asset type — the `Type` a
    /// read of an asset reports — is the same text.
    pub fn qualified_id(&self) -> String {
        catalog_qualified_id(&self.name)
    }

    /// This record's forms as the modelled `AssetTypeFormsMap`, which is what every
    /// operation that reports an asset type answers with.
    pub fn forms_map(&self) -> HashMap<String, AssetTypeFormReference> {
        self.forms
            .iter()
            .map(|(form_name, form_type_id)| {
                (
                    form_name.clone(),
                    AssetTypeFormReference {
                        form_type_identifier: form_type_id.clone(),
                    },
                )
            })
            .collect()
    }
}

/// The message the catalog's asset type surface refuses an identifier it holds no asset
/// type under with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type, asset and business
/// glossary ones do ([`form_type_not_found_message`]): the refusal text is itself a JSON
/// document carrying the sentence under a `Message` key, so that text — braces and all —
/// is what the modelled member carries, and a client reading `Message` reads the document
/// rather than a bare sentence. `serde_json` renders it, so an identifier carrying a quote
/// or a backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves an asset type identifier
/// ([`ComAmazonawsGlueAwsglueState::asset_type_by_reference`]), so two of them cannot
/// disagree about how the one miss is worded.
pub fn asset_type_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Asset type {identifier} not found."))
}

/// The message the catalog's asset surface refuses an identifier it holds no asset under
/// with: the value of that refusal's `EntityNotFoundException$Message`.
///
/// Worded the way this surface words a miss, exactly as the form type and business
/// glossary ones above do ([`form_type_not_found_message`]): the refusal text is itself a
/// JSON document carrying the sentence under a `Message` key, so that text — braces and
/// all — is what the modelled member carries. This surface words the sentence the other
/// way round from those two, naming the resource kind and then the identifier that
/// resolved to nothing. `serde_json` renders it, so an identifier carrying a quote or a
/// backslash still yields a well-formed document.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves an `AssetId`
/// ([`ComAmazonawsGlueAwsglueState::asset`]) — the asset's own reader and the glossary
/// term association that addresses one — so two of them cannot disagree about how the one
/// miss is worded.
pub fn asset_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Asset not found: {identifier}"))
}

/// The message the catalog's business glossary surface refuses an identifier it holds no
/// glossary TERM under with: the value of that refusal's
/// `EntityNotFoundException$Message`.
///
/// Worded the way the glossary itself words a miss ([`glossary_not_found_message`]) — the
/// refusal text is a JSON document carrying the sentence under a `Message` key — with the
/// resource named as the service names it, a business glossary term.
///
/// `identifier` is the text's one variable part and is taken from the request.
///
/// One copy, shared by every operation that resolves a `GlossaryTermId`
/// ([`ComAmazonawsGlueAwsglueState::glossary_term`]), so two of them cannot disagree about
/// how the one miss is worded.
pub fn glossary_term_not_found_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Business glossary term {identifier} not found."))
}

/// The message the catalog refuses a request that asks to associate a glossary term this
/// store holds none of with: the value of that refusal's `InvalidInputException$Message`.
///
/// The refusal is the ASSET surface's reading of a reference it cannot resolve, so it is
/// worded the way that surface words a miss ([`asset_not_found_message`]) rather than the
/// way the term's own reader words one: the text is a JSON document carrying the sentence
/// under a `Message` key, and the sentence names the kind of thing that was looked for and
/// then the identifier that resolved to nothing.
///
/// `identifier` is the text's one variable part and is the entry of the request's
/// identifier list that resolved to nothing, taken verbatim.
pub fn unassociable_glossary_term_message(identifier: &str) -> String {
    catalog_refusal_document(&format!("Glossary term not found: {identifier}"))
}

/// The message this surface refuses a request that names an `IterableFormName` without the
/// `ItemIdentifier` beside it with: the value of that refusal's
/// `InvalidInputException$Message`.
///
/// "The identifier of the item within the iterable form. Required when iterableFormName is
/// specified" (`ItemIdentifier`), and the form name is what says the request targets an item
/// "rather than the asset itself" (`IterableFormName`), so the two name ONE target together
/// and a request carrying one without the other names no target at all.
///
/// One copy, shared by every operation of this surface that takes the pair — the glossary
/// term association, the attachment write and the attachment delete — so three of them
/// cannot disagree about how the one bad request is worded.
pub fn item_identifier_required_message() -> String {
    "ItemIdentifier is required when IterableFormName is specified".to_string()
}

/// The same for the other half of that pair: an `ItemIdentifier` named without the
/// `IterableFormName` the item lives in.
pub fn iterable_form_name_required_message() -> String {
    "IterableFormName is required when ItemIdentifier is specified".to_string()
}

/// The message this surface refuses a request that targets an item inside an iterable form
/// an asset does not carry with: the value of that refusal's
/// `EntityNotFoundException$Message`.
///
/// Nothing writes an iterable form onto an asset — the model binds no operation that stores
/// one, so every asset this store holds carries none — so such a request names an entity
/// that does not exist. The wording is pinned by no recording or doc, so it is derived from
/// the request: it names the iterable form and the asset it was looked for on, which are its
/// two variable parts.
///
/// One copy, shared by every operation of this surface that takes that target, so none of
/// them can disagree about how the one miss is worded.
pub fn iterable_form_not_found_message(iterable_form_name: &str, asset_identifier: &str) -> String {
    format!("Iterable form {iterable_form_name} not found on asset {asset_identifier}")
}

/// How many forms one asset may hold.
///
/// The same ceiling the model puts on the forms of an ASSET TYPE — "len 1..100" on
/// `AssetTypeFormsMap` (`smithy_query shape=AssetTypeFormsMap`) — which is the structure an
/// asset is an instance of. `AssetFormMap` carries no length of its own, so without this one
/// put would decide how much every later read of that asset serialises: a read reports the
/// forms TWICE, as the map and as the list beside it
/// ([`AssetEntry::reported_forms_map`], [`AssetEntry::reported_form_list`]), so this bound is
/// what keeps the answer to one small read proportional to it, and what bounds the walk a put
/// spends resolving the form types its entries name.
pub const MAX_ASSET_FORMS: usize = 100;

/// A stored asset: one catalog entity `PutAsset` registers as an instance of an asset
/// type, carrying that entity's metadata in forms keyed by form name.
///
/// Every member of the modelled asset is held here — its identifier, the asset type it is
/// an instance of, its name, its description, its forms and the two instants the SERVER
/// stamps on it — so a read reports the asset the last write stored and nothing invented on
/// the way out. The identifier, the asset type, the name, the description and the forms
/// come from the request; the two instants are the clock's.
#[derive(Debug, Clone)]
pub struct AssetEntry {
    /// `AssetId`: the identifier the PUT's caller chose, which is also this record's key in
    /// [`ComAmazonawsGlueAwsglueState::assets`] and the address every later operation
    /// carries. Stored exactly as sent.
    pub id: String,
    /// "The identifier of the asset type for the asset" (`PutAssetRequest$AssetTypeId`),
    /// stored exactly as the put addressed it — either the `AssetTypeId` the catalog minted
    /// for that asset type or the `AssetTypeName` it was registered under, the two forms
    /// [`ComAmazonawsGlueAwsglueState::asset_type_by_reference`] resolves — because it
    /// resolved against this store when it was stored, so a read reports the identifier the
    /// client wrote.
    pub asset_type_id: String,
    /// "The name of the asset" (`PutAssetRequest$Name`), as the latest put sent it.
    pub name: String,
    /// "The description of the asset", or `None` when the latest put carried none: the
    /// member is optional and the model gives it no default.
    pub description: Option<String>,
    /// "The forms to set on the asset, keyed by form name", each entry the form type that
    /// defines the form's schema and the form's JSON content, stored exactly as the put
    /// sent them. A put SETS the whole map, so a form the latest put did not send is not
    /// held, and it never holds more than [`MAX_ASSET_FORMS`] entries. Ordered by form name,
    /// so two reads of one asset report its forms in one order.
    pub forms: BTreeMap<String, AssetFormEntry>,
    /// "The timestamp at which the asset was created": the instant the put that first
    /// stored this identifier was served, kept by every later put under it so every read
    /// reports the same creation instant.
    pub created_at: SystemTime,
    /// "The timestamp at which the asset was last updated" (`GetAssetOutput$UpdatedAt`):
    /// the instant the latest put that changed this record was served, which for a record
    /// no put has updated yet is [`Self::created_at`].
    pub updated_at: SystemTime,
    /// The `ClientToken` of the put that last wrote this record, when it carried one.
    ///
    /// `PutAssetRequest$ClientToken` is "a unique, case-sensitive identifier that you
    /// provide to ensure the idempotency of the request", so a retry carrying the token of
    /// the put already stored is answered with that record rather than writing it again.
    pub client_token: Option<String>,
    /// The glossary terms associated with this asset, as the `GlossaryTermId` of each, in
    /// the order they were first associated.
    ///
    /// A SET, held in insertion order: associating a term already held changes nothing, so
    /// the same association sent twice leaves one entry, and the order two reads report is
    /// the order the associations happened in. The ids are stored exactly as the
    /// association sent them and are NOT resolved against the term namespace afterwards,
    /// because the association is to the id: a term deleted after being associated leaves
    /// its id on the asset, which is why this is not a list of references into
    /// [`ComAmazonawsGlueAwsglueState::glossary_terms`]. An asset no association has
    /// reached holds none, and the list never grows past
    /// [`MAX_ASSET_GLOSSARY_TERMS`].
    ///
    /// A put under an identifier already stored does not touch it: `PutAsset` sets an
    /// asset's forms and carries no glossary term member at all, so the association
    /// surface is the only writer.
    pub glossary_terms: Vec<String>,
    /// The attachments on this asset, keyed by the `AttachmentName` each was written under,
    /// as the form type the attachment was created against and the document it carries.
    ///
    /// "Additional attachments on the asset for more context, keyed by attachment name"
    /// (`GetAssetOutput$Attachments`): `PutAttachment` is the writer, and "if an attachment
    /// with the same name already exists, it is overwritten", so one name holds one
    /// attachment however many writes land on it, and the map never grows past
    /// [`MAX_ASSET_ATTACHMENTS`]. Ordered by attachment name, so two reads of one asset
    /// report its attachments in one order.
    ///
    /// A put of the ASSET does not touch them: `PutAsset` sets an asset's forms and carries
    /// no attachment member at all, so the attachment surface is the only writer.
    pub attachments: BTreeMap<String, AssetAttachment>,
}

/// One attachment held on an asset: the form `PutAttachment` attached to it under an
/// `AttachmentName`.
///
/// Every member of the modelled attachment is held here — the form type it was created
/// against and its JSON content — beside the idempotency token of the write that last
/// stored it, so a read reports the attachment the last write stored and a retry of that
/// write is answered rather than stored twice.
#[derive(Debug, Clone)]
pub struct AssetAttachment {
    /// "The identifier of the form type for this attachment"
    /// (`PutAttachmentRequest$FormTypeId`), stored exactly as the write addressed it —
    /// either the minted `FormTypeId` or the `FormTypeName` the form type was registered
    /// under, the two forms [`ComAmazonawsGlueAwsglueState::form_type_by_reference`]
    /// resolves — so a read reports the identifier the client wrote.
    pub form_type_id: String,
    /// "The JSON content of the form, conforming to the schema of the specified form type"
    /// (`PutAttachmentRequest$Content`), stored exactly as the write sent the text; a read
    /// of the asset reports the DOCUMENT that text carries ([`compact_json_content`]).
    pub content: String,
    /// The `ClientToken` of the write that last stored this attachment, when it carried one,
    /// so a retry of that very write is answered with this record rather than rewriting it.
    pub client_token: Option<String>,
}

/// How many attachments one asset may hold.
///
/// The attachment map is reported as an `AssetFormMap`, which carries no length of its own
/// (`smithy_query shape=AssetFormMap`), so the ceiling is the one the model puts on the
/// forms of the asset TYPE an asset is an instance of, exactly as the forms of an asset are
/// bounded by it ([`MAX_ASSET_FORMS`]). A read reports every attachment TWICE, as the keyed
/// entry and as the entry of the list beside it
/// ([`AssetEntry::attachments_map`], [`AssetEntry::attachment_form_list`]), so this bound is
/// what keeps the answer to one small read proportional to it.
pub const MAX_ASSET_ATTACHMENTS: usize = MAX_ASSET_FORMS;

impl AssetEntry {
    /// The record a put stores for an identifier this store does not hold yet: the
    /// request's members, stamped with the instant `now` the put was served at. A fresh
    /// asset carries no glossary term: the put models no member to send one with.
    pub fn new(
        id: String,
        asset_type_id: String,
        name: String,
        description: Option<String>,
        forms: BTreeMap<String, AssetFormEntry>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            asset_type_id,
            name,
            description,
            forms,
            created_at: now,
            updated_at: now,
            client_token,
            glossary_terms: Vec::new(),
            attachments: BTreeMap::new(),
        }
    }

    /// This record's glossary terms as the modelled `GlossaryTermIdList`, which is what
    /// every operation that reports an asset's associations answers with.
    ///
    /// Always a list, EMPTY for an asset no association has reached: the member is how a
    /// client reads which terms an asset carries, so "none" is reported as the empty list
    /// rather than by leaving the member out, which a client cannot tell from "not
    /// reported".
    pub fn glossary_terms_list(&self) -> Vec<String> {
        self.glossary_terms.clone()
    }

    /// Merge `identifiers` into this asset's glossary term set, reporting the terms the
    /// asset carries afterwards, or `None` when the merge would take it past
    /// [`MAX_ASSET_GLOSSARY_TERMS`].
    ///
    /// A union, not a replacement: an id the asset already holds keeps its place and is
    /// not repeated, and an id it does not hold is appended, so the terms an earlier
    /// association added survive a later one and an association sent twice is the same
    /// state as sending it once. The bound is checked against the UNION, so re-associating
    /// terms an asset already holds is served however full it is.
    ///
    /// The ceiling is also what bounds the WORK: a request can only carry as many new ids
    /// as the union admits, so the walk stops at the first one past it rather than merging
    /// the whole request first — one call cannot spend more comparisons or more memory than
    /// [`MAX_ASSET_GLOSSARY_TERMS`] allows, whatever it sends.
    pub fn associate_glossary_terms(&mut self, identifiers: &[String]) -> Option<Vec<String>> {
        let mut merged = self.glossary_terms.clone();
        for identifier in identifiers {
            if merged.iter().any(|held| held == identifier) {
                continue;
            }
            if merged.len() == MAX_ASSET_GLOSSARY_TERMS {
                return None;
            }
            merged.push(identifier.clone());
        }
        self.glossary_terms = merged;
        Some(self.glossary_terms.clone())
    }

    /// This record's forms as the modelled `AssetFormMap`, with every form's content
    /// EXACTLY as the put that stored it sent the text: what the put that accepted that
    /// text answers with.
    pub fn forms_map(&self) -> HashMap<String, AssetFormEntry> {
        self.forms
            .iter()
            .map(|(form_name, form)| (form_name.clone(), form.clone()))
            .collect()
    }

    /// This record's forms as a READ of the asset reports them: the stored forms, each
    /// one's content rendered from the JSON document it carries
    /// ([`compact_json_content`]), beside the catalog's own form on the asset
    /// ([`CATALOG_RESOURCE_FORM_NAME`]).
    ///
    /// A read answers from the catalog's own record of the asset rather than from the text
    /// one put supplied, which is why the content is the document and not the bytes: two
    /// puts sending the same document with different spacing are the same asset, and a read
    /// of either reports one content.
    ///
    /// A form this asset STORES under the catalog's form name wins: `AssetFormKey` carries
    /// no pattern, so a put may name a form that way, and what a read reports for a form the
    /// asset holds is always the form the asset holds — the catalog's own form is added only
    /// where the asset has no form of that name, so no stored form is shadowed by it.
    pub fn reported_forms_map(&self) -> HashMap<String, AssetFormEntry> {
        let mut reported: HashMap<String, AssetFormEntry> = self.reported_forms().collect();
        if let Some(catalog_form) = self.unshadowed_catalog_resource_form() {
            reported.insert(CATALOG_RESOURCE_FORM_NAME.to_string(), catalog_form);
        }
        reported
    }

    /// Every form a read of the asset reports, as the LIST that read answers with beside
    /// the map (`AssetForms`): one named entry per stored form, in form-name order, and the
    /// catalog's own form on the asset LAST — the forms the asset carries first, then the
    /// one the catalog put there.
    ///
    /// The same entries as [`Self::reported_forms_map`], so the two answers cannot disagree
    /// about one form — a form stored under the catalog's own name included.
    pub fn reported_form_list(&self) -> Vec<AssetFormRecord> {
        self.reported_forms()
            .chain(
                self.unshadowed_catalog_resource_form()
                    .map(|form| (CATALOG_RESOURCE_FORM_NAME.to_string(), form)),
            )
            .map(|(form_name, form)| AssetFormRecord {
                form_name: Some(form_name),
                form_type_id: form.form_type_id,
                content: form.content,
            })
            .collect()
    }

    /// The catalog's own form on this asset when the asset itself holds no form under that
    /// name, and `None` when it does: a stored form is never replaced by it.
    fn unshadowed_catalog_resource_form(&self) -> Option<AssetFormEntry> {
        if self.forms.contains_key(CATALOG_RESOURCE_FORM_NAME) {
            return None;
        }
        Some(self.catalog_resource_form())
    }

    /// The stored forms as a read reports each of them: the form type the put named, and
    /// the content rendered from the JSON document that put supplied. Ordered by form name,
    /// so the list a read reports follows one order.
    fn reported_forms(&self) -> impl Iterator<Item = (String, AssetFormEntry)> + '_ {
        self.forms.iter().map(|(form_name, form)| {
            (
                form_name.clone(),
                AssetFormEntry {
                    form_type_id: form.form_type_id.clone(),
                    content: form.content.as_deref().map(compact_json_content),
                },
            )
        })
    }

    /// The catalog's own form on this asset, as every read of it reports it: the form type
    /// the catalog names it with ([`CATALOG_RESOURCE_FORM_TYPE_ID`]) and, as its content,
    /// the catalog's own document for the asset ([`Self::catalog_resource_document`]).
    ///
    /// Recorded behaviour, not a record of this store: the form and its form type are what
    /// the recorded reads of an asset carry (see [`CATALOG_RESOURCE_FORM_NAME`]), and no
    /// operation of this surface writes or clears one.
    fn catalog_resource_form(&self) -> AssetFormEntry {
        AssetFormEntry {
            form_type_id: Some(CATALOG_RESOURCE_FORM_TYPE_ID.to_string()),
            content: Some(self.catalog_resource_document()),
        }
    }

    /// The document the catalog's OWN form on this asset carries: the asset's resource
    /// members — its name, and its description when the asset holds one — as a JSON object,
    /// name first.
    ///
    /// The form names the catalog's `Resource` form type, so what it carries is the
    /// catalog's own record of the asset as a RESOURCE, which is the pair of members the
    /// asset surface stores for it. It is computed from the record on each read rather than
    /// stored, so it cannot drift from the `Name` and `Description` the same read reports:
    /// a put that renames the asset or drops its description moves this document with it,
    /// and an asset with no description carries a document with no `description` member
    /// rather than a null one.
    ///
    /// `serde_json` renders each value, so a name or a description carrying a quote, a
    /// backslash or a control character still yields a well-formed document; the members
    /// are laid out here rather than through a map so the order is the catalog's and not
    /// the alphabet's.
    fn catalog_resource_document(&self) -> String {
        let name = serde_json::Value::String(self.name.clone()).to_string();
        match &self.description {
            Some(description) => {
                let description = serde_json::Value::String(description.clone()).to_string();
                format!("{{\"name\":{name},\"description\":{description}}}")
            }
            None => format!("{{\"name\":{name}}}"),
        }
    }

    /// The attachment this asset holds under `name`, or `None` when it holds none under
    /// that name.
    pub fn attachment(&self, name: &str) -> Option<&AssetAttachment> {
        self.attachments.get(name)
    }

    /// This record's attachments as the modelled `AssetFormMap` a read of the asset reports,
    /// keyed by attachment name: each entry the form type the attachment was created against
    /// and the content rendered from the JSON document the write supplied, exactly as a
    /// stored form's content is rendered ([`compact_json_content`]).
    ///
    /// Always a map, EMPTY for an asset no attachment has been written to: the member is how
    /// a client reads which attachments an asset carries, so "none" is reported as the empty
    /// map rather than by leaving the member out.
    pub fn attachments_map(&self) -> HashMap<String, AssetFormEntry> {
        self.reported_attachments().collect()
    }

    /// The same attachments as the LIST a read answers with beside the map, in
    /// attachment-name order: one named entry per attachment, exactly as the form list
    /// mirrors the form map ([`Self::reported_form_list`]).
    ///
    /// The same entries as [`Self::attachments_map`], so the two answers cannot disagree
    /// about one attachment. What the recorded reads pin about this list is its SIZE — one
    /// entry per attachment the asset holds, and none at all for an asset no attachment has
    /// reached — so each entry states the attachment it stands for the way the form list
    /// beside the map states a form: its name, its form type and its document, rather than an
    /// object saying nothing about the attachment it is there for.
    pub fn attachment_form_list(&self) -> Vec<AssetFormRecord> {
        self.reported_attachments()
            .map(|(name, attachment)| AssetFormRecord {
                form_name: Some(name),
                form_type_id: attachment.form_type_id,
                content: attachment.content,
            })
            .collect()
    }

    /// The stored attachments as a read reports each of them. Ordered by attachment name, so
    /// the map and the list a read reports follow one order.
    fn reported_attachments(&self) -> impl Iterator<Item = (String, AssetFormEntry)> + '_ {
        self.attachments.iter().map(|(name, attachment)| {
            (
                name.clone(),
                AssetFormEntry {
                    form_type_id: Some(attachment.form_type_id.clone()),
                    content: Some(compact_json_content(&attachment.content)),
                },
            )
        })
    }

    /// Write `attachment` onto this asset under `name`, reporting the record the asset holds
    /// afterwards, or `None` when the asset holds no attachment under that name yet and one
    /// more would take it past [`MAX_ASSET_ATTACHMENTS`].
    ///
    /// "If an attachment with the same name already exists, it is overwritten"
    /// (`PutAttachment`), so a write under a name the asset already holds REPLACES that
    /// record and leaves the count where it is — which is why the ceiling is only consulted
    /// for a name the asset does not hold yet, and why a client can rewrite an attachment
    /// however full the asset is.
    pub fn put_attachment(
        &mut self,
        name: &str,
        attachment: AssetAttachment,
    ) -> Option<&AssetAttachment> {
        if !self.attachments.contains_key(name) && self.attachments.len() >= MAX_ASSET_ATTACHMENTS
        {
            return None;
        }
        self.attachments.insert(name.to_string(), attachment);
        self.attachments.get(name)
    }

    /// Remove the attachment this asset holds under `name`, reporting the record that was
    /// removed, or `None` when the asset holds no attachment under that name.
    ///
    /// The name resolves exactly as a read of the attachment resolves one
    /// ([`Self::attachment`]) — it is the attachment map's own key — so a delete reaches the
    /// entry a read of the asset reports under that name, and the attachments the asset
    /// holds under every other name are left where they are.
    pub fn remove_attachment(&mut self, name: &str) -> Option<AssetAttachment> {
        self.attachments.remove(name)
    }
}

/// The form name the catalog's OWN form on an asset carries: every read of an asset reports
/// a form of this name beside the forms the asset holds, which is recorded behaviour of this
/// surface (corpus `simparity-wire-aws-v1`, `glue-asset-crud-with-forms`) rather than
/// anything the model declares or any write of this emulator produces.
///
/// Namespaced (`<namespace>::<name>`), which is what tells it apart from a form a client's
/// put stored: `AssetFormKey` carries no pattern, while the form names a client addresses
/// elsewhere in this surface do (`IterableFormName`'s
/// `^[a-zA-Z][a-zA-Z0-9_]*(::[a-zA-Z][a-zA-Z0-9_]*)?$` admits the same two-part spelling).
/// A put MAY still store a form under this name, and then that form is what a read reports
/// ([`AssetEntry::reported_forms_map`]).
pub const CATALOG_RESOURCE_FORM_NAME: &str = "catalog::Resource";

/// The `FormTypeId` the catalog's own form on an asset names.
///
/// Recorded behaviour: this is the form type the recorded reads of an asset report that form
/// under (corpus `simparity-wire-aws-v1`), and it is the `<name>` half of
/// [`CATALOG_RESOURCE_FORM_NAME`] — the form's name carries the namespace, the form type is
/// named inside it.
///
/// It is NOT a record of [`ComAmazonawsGlueAwsglueState::form_types`] (that namespace holds
/// what `PutFormType` stores), so it is not addressable through the form type surface and
/// cannot be referenced by a client's own form.
pub const CATALOG_RESOURCE_FORM_TYPE_ID: &str = "Resource";

/// `content` as a read of the form that carries it reports it: the same JSON document,
/// rendered without the whitespace between its tokens.
///
/// A form's content is "the JSON content of the form, conforming to the schema of the
/// specified form type" (`AssetFormEntry$Content`), so what the catalog holds for a form is
/// that DOCUMENT: a read reports the document, which is the text with the whitespace
/// between its tokens dropped. The scan that drops it is the shared one
/// ([`::service_common::json_document::minified`]), which every service that stores a JSON
/// document as an opaque string and reports it back uses — member order, string literals
/// and number spellings are the document's own there, which is exactly what a form's
/// content must keep.
///
/// Text that is not a JSON document at all is reported exactly as it was stored: the
/// catalog has no document to render, so there is nothing to drop. That decision is this
/// surface's own, which is why it is made here and not in the shared scan (whose pass is
/// total over any text).
pub fn compact_json_content(content: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(content).is_err() {
        return content.to_string();
    }
    ::service_common::json_document::minified(content)
}

/// How many glossary ids this process has minted. A nonce source for
/// [`new_glossary_id`], exactly as [`MINTED_DATABASE_IDS`] is for a database id.
pub(super) static MINTED_GLOSSARY_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `GlossaryId`: the identifier the Data Catalog assigns a business glossary when
/// `CreateGlossary` stores it.
///
/// The identifier shape this surface's entities carry: [`CATALOG_ENTITY_ID_LENGTH`]
/// lowercase base-36 characters, which `GlossaryId` — a string the model gives no length or
/// pattern constraint — admits. The characters come from [`mint_base36_id`], so two creates
/// landing in the same nanosecond cannot be given the same id. It is minted ONCE, at
/// create, and kept on the [`GlossaryEntry`] through every later update — a rename included
/// — so every read of that glossary reports the same id and two glossaries never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_glossary_id() -> String {
    mint_base36_id(&MINTED_GLOSSARY_IDS, CATALOG_ENTITY_ID_LENGTH)
}

/// How many characters the identifier of a business-catalog entity — a glossary, a
/// glossary term — carries: 14, the width of the identifiers this surface hands out. They
/// are DRAWN rather than derived from the request, because nothing a caller submits
/// identifies one of these entities: a glossary's name can be changed by an update, and a
/// term's name is unique only inside its glossary.
const CATALOG_ENTITY_ID_LENGTH: usize = 14;

/// `length` characters of lowercase base-36 (the ten digits and the 26 lowercase letters):
/// how this surface spells the identifier it draws for an entity, so every id it hands out
/// reads the same way.
///
/// The bits of each draw come from [`id_draw`], the same one `mint_hex_id` renders in
/// hexadecimal — one entropy rule for every id this crate mints, and only the alphabet and
/// the width differ here.
///
/// This mints a RESOURCE id, not a credential: a credential would come from the credential
/// store, never from here.
fn mint_base36_id(minted: &AtomicU64, length: usize) -> String {
    /// The alphabet of lowercase base-36, in ASCII order.
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    /// How many base-36 digits one 64-bit draw is spent on: 36^12 is below 2^64, so each
    /// of twelve digits taken from a draw is spread over the whole alphabet.
    const DIGITS_PER_DRAW: usize = 12;

    let sequence = minted.fetch_add(1, Ordering::Relaxed);
    let mut id = String::with_capacity(length);
    let mut draw = 0u64;
    for position in 0..length {
        if position % DIGITS_PER_DRAW == 0 {
            draw = id_draw(sequence, position);
        }
        let digit = usize::try_from(draw % 36).unwrap_or(0);
        id.push(char::from(DIGITS[digit]));
        draw /= 36;
    }
    id
}

/// A stored business glossary: the container for glossary terms `CreateGlossary`
/// registers.
///
/// Every member of the modelled glossary is held here — its id, its name and its
/// description — beside the two instants the SERVER stamps on it: the name and the
/// description come from the request, the id is minted by [`new_glossary_id`] when the
/// glossary is stored, and the two instants are the clock's.
#[derive(Debug, Clone)]
pub struct GlossaryEntry {
    /// `GlossaryId`, minted by [`new_glossary_id`] at create and unchanged afterwards, so
    /// a caller that kept the id from the create can still address the glossary after it
    /// has been updated.
    pub id: String,
    /// "The name of the glossary" (`CreateGlossaryRequest$Name`), unique over this
    /// account's glossaries. Stored exactly as sent — the model documents no case fold
    /// for `GlossaryName`.
    pub name: String,
    /// "The description of the glossary", absent when the create carried none: the member
    /// is optional and the model gives it no default, so a glossary created without one
    /// reports none rather than an empty string.
    pub description: Option<String>,
    /// The instant the create that stored this glossary was served, which every read of it
    /// reports and no later write moves.
    pub created_at: SystemTime,
    /// The instant the latest update that CHANGED this glossary was served, which for a
    /// glossary no update has changed is [`Self::created_at`].
    pub updated_at: SystemTime,
    /// The `ClientToken` of the request that created this glossary, when it carried one.
    ///
    /// `CreateGlossaryRequest$ClientToken` is "a unique, case-sensitive identifier that
    /// you provide to ensure the idempotency of the request", so a retry of that same
    /// request is answered with this record instead of being refused as a duplicate name.
    pub client_token: Option<String>,
}

impl GlossaryEntry {
    /// The record a create stores: the name and description it carried, under a freshly
    /// minted id, stamped with the instant `now` the create was served at, beside the
    /// idempotency token a retry of that create is recognised by.
    pub fn new(
        name: String,
        description: Option<String>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            id: new_glossary_id(),
            name,
            description,
            created_at: now,
            updated_at: now,
            client_token,
        }
    }

    /// The same, read off a `CreateGlossary` request: every member comes from the request
    /// except the id, which the catalog assigns, and the instants, which are the clock's.
    pub fn from_create_input(input: &CreateGlossaryInput, now: SystemTime) -> Self {
        Self::new(
            input.name.clone(),
            input.description.clone(),
            input.client_token.clone(),
            now,
        )
    }

    /// Apply an `UpdateGlossary` request to this record, as served at `now`.
    ///
    /// Every updatable member is optional on the request ("The updated name of the
    /// glossary", "The updated description of the glossary"), so the ones the request
    /// NAMES replace what is stored and the ones it omits are left where the last write
    /// that named them put them: an update naming one member changes that one alone.
    ///
    /// [`Self::updated_at`] moves to `now` when the request named a member and stays where
    /// it is for one that named none: it reports when this glossary last CHANGED.
    ///
    /// [`Self::id`] and [`Self::created_at`] are no members of the request and so cannot
    /// move, and [`Self::client_token`] keeps the CREATE's token, which is what a retry of
    /// that create is recognised by and not something an update of the glossary restates.
    pub fn apply_update(&mut self, input: &UpdateGlossaryInput, now: SystemTime) {
        let mut changed = false;
        if let Some(name) = input.name.clone() {
            self.name = name;
            changed = true;
        }
        if let Some(description) = input.description.clone() {
            self.description = Some(description);
            changed = true;
        }
        if changed {
            self.updated_at = now;
        }
    }
}

/// How many glossary term ids this process has minted. A nonce source for
/// [`new_glossary_term_id`], exactly as [`MINTED_GLOSSARY_IDS`] is for a glossary id.
pub(super) static MINTED_GLOSSARY_TERM_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `GlossaryTermId`: the identifier the Data Catalog assigns a glossary term when
/// `CreateGlossaryTerm` stores it.
///
/// The same shape as a glossary id ([`new_glossary_id`]) and for the same reason:
/// `GlossaryTermId` carries no length or pattern constraint of its own, and the characters
/// come from [`mint_base36_id`], so two creates landing in the same nanosecond cannot be
/// given the same id. It is minted ONCE, at create, and kept on the [`GlossaryTermEntry`]
/// through every later update — a rename included — so every read of that term reports the
/// same id and two terms never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_glossary_term_id() -> String {
    mint_base36_id(&MINTED_GLOSSARY_TERM_IDS, CATALOG_ENTITY_ID_LENGTH)
}

/// A stored glossary term: one business concept `CreateGlossaryTerm` registers within a
/// glossary.
///
/// Every member of the modelled term is held here — its id, the glossary it belongs to, its
/// name and its two descriptions — beside the two instants the SERVER stamps on it: the
/// glossary, the name and the descriptions come from the request, the id is minted by
/// [`new_glossary_term_id`] when the term is stored, and the two instants are the clock's.
#[derive(Debug, Clone)]
pub struct GlossaryTermEntry {
    /// `GlossaryTermId`, minted by [`new_glossary_term_id`] at create and unchanged
    /// afterwards, so a caller that kept the id from the create can still address the term
    /// after it has been updated.
    pub id: String,
    /// "The unique identifier of the glossary in which to create the term"
    /// (`CreateGlossaryTermRequest$GlossaryIdentifier`), recorded on the term because a
    /// term is addressed by its own id alone and the glossary it belongs to is reported by
    /// every read of it.
    pub glossary_id: String,
    /// "The name of the glossary term" (`CreateGlossaryTermRequest$Name`), unique within
    /// [`Self::glossary_id`]. Stored exactly as sent — the model documents no case fold for
    /// `GlossaryTermName`.
    pub name: String,
    /// "A short description of the glossary term", absent when the create carried none: the
    /// member is optional and the model gives it no default, so a term created without one
    /// reports none rather than an empty string.
    pub short_description: Option<String>,
    /// "A long description of the glossary term", absent for the same reason as
    /// [`Self::short_description`].
    pub long_description: Option<String>,
    /// The instant the create that stored this term was served, which every read of it
    /// reports and no later write moves.
    pub created_at: SystemTime,
    /// The instant the latest update that CHANGED this term was served, which for a term no
    /// update has changed is [`Self::created_at`].
    pub updated_at: SystemTime,
    /// The `ClientToken` of the request that created this term, when it carried one.
    ///
    /// `CreateGlossaryTermRequest$ClientToken` is "a unique, case-sensitive identifier that
    /// you provide to ensure the idempotency of the request", so a retry of that same
    /// request is answered with this record instead of being refused as a duplicate name.
    pub client_token: Option<String>,
}

impl GlossaryTermEntry {
    /// The record a create stores: the glossary, name and descriptions it carried, under a
    /// freshly minted id, stamped with the instant `now` the create was served at, beside
    /// the idempotency token a retry of that create is recognised by.
    pub fn new(
        glossary_id: String,
        name: String,
        short_description: Option<String>,
        long_description: Option<String>,
        client_token: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            id: new_glossary_term_id(),
            glossary_id,
            name,
            short_description,
            long_description,
            created_at: now,
            updated_at: now,
            client_token,
        }
    }

    /// The same, read off a `CreateGlossaryTerm` request: every member comes from the
    /// request except the id, which the catalog assigns, and the instants, which are the
    /// clock's.
    pub fn from_create_input(input: &CreateGlossaryTermInput, now: SystemTime) -> Self {
        Self::new(
            input.glossary_identifier.clone(),
            input.name.clone(),
            input.short_description.clone(),
            input.long_description.clone(),
            input.client_token.clone(),
            now,
        )
    }
}

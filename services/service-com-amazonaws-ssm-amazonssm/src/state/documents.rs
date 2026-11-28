// SPDX-License-Identifier: CC0-1.0

//! The DOCUMENTS this service's state holds, and everything the document operations read
//! and write them through: the bounds a document's untrusted content is guarded by, the
//! parse that turns it into the modelled description, the bodies of the documents AWS
//! itself publishes, and the stored record one document version is kept as. The CATALOGUE
//! those bodies belong to — every document that exists without a create, with its type,
//! target type and publication date — is the child module [`published_catalogue`].
//!
//! Moved out of [`super`] unchanged so each file of the state module keeps one concern;
//! every name stays reachable under `crate::state` through that module's re-export.

use super::*;

mod published_catalogue;
pub use published_catalogue::*;

/// The `CloudProvider` a document version runs against when the call that minted it named
/// none: "If you don't specify a value, the default is `AWS`"
/// (`CreateDocumentRequest$CloudProviders`) — a value of the modelled `CloudProvider` enum
/// (`crate::ops::types::CLOUD_PROVIDER_VALUES`).
pub const DEFAULT_CLOUD_PROVIDER: &str = "AWS";

/// The cloud providers a version a create minted runs against: the ones it named, or the
/// default for a create that named none ([`DEFAULT_CLOUD_PROVIDER`]).
///
/// `CloudProviderList` is `@length(min: 1, max: 10)`, so a list that reaches a handler at
/// all names at least one provider; the empty case is the member being absent.
#[must_use]
pub fn created_cloud_providers(stated: Option<Vec<String>>) -> Vec<String> {
    stated
        .filter(|providers| !providers.is_empty())
        .unwrap_or_else(|| vec![DEFAULT_CLOUD_PROVIDER.to_string()])
}

/// The cloud providers a version an UPDATE minted runs against: the ones it named, or
/// `inherited` — "if you don't specify a value, the new version uses the cloud providers of
/// the latest version" (`UpdateDocumentRequest$CloudProviders`).
#[must_use]
pub fn updated_cloud_providers(stated: Option<Vec<String>>, inherited: Vec<String>) -> Vec<String> {
    match stated.filter(|providers| !providers.is_empty()) {
        Some(stated) => stated,
        None => created_cloud_providers(Some(inherited).filter(|held| !held.is_empty())),
    }
}

/// Pre-flight bounds on untrusted content, applied BEFORE any parser builds a tree
/// from it: the YAML parser descends recursively, so the nesting the bytes can ask for
/// has to be bounded while there is still no tree to measure.
///
/// This is a GUARD, not a parser: it knows nothing about YAML context and measures only
/// quantities a hiding trick can push UP but never DOWN — the content's length, the
/// number of flow openers (whose count IS their depth bound, since flow nesting costs
/// neither a line nor a column), the compact indicators on any one line, and a line's
/// leading indentation. Closers are never subtracted, so a `]` inside a scalar cannot
/// buy depth back. The same guard the Greengrass recipe parser
/// (`service-com-amazonaws-greengrassv2-greengrassv2/src/state.rs`) and the
/// CloudFormation template parser apply, for the same reason and with the same bounds;
/// the three copies are to be migrated onto one shared implementation rather than
/// copied a fourth time.
pub(super) fn guard_document_content(content: &str) -> Result<(), ContentRejection> {
    content_within_size(content)?;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    for byte in content.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_DOCUMENT_INDENT {
                    return Err(ContentRejection::Content(format!(
                        "a line of Content must not be indented past {MAX_DOCUMENT_INDENT} columns"
                    )));
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_openers += 1;
                if flow_openers > MAX_DOCUMENT_FLOW_OPENERS {
                    return Err(ContentRejection::Content(format!(
                        "Content must not carry more than {MAX_DOCUMENT_FLOW_OPENERS} '[' or '{{' \
                         openers"
                    )));
                }
            }
            b'-' | b'?' => {
                counting_indent = false;
                compact_on_line += 1;
                if compact_on_line > MAX_DOCUMENT_COMPACT_PER_LINE {
                    return Err(ContentRejection::Content(format!(
                        "a line of Content must not carry more than \
                         {MAX_DOCUMENT_COMPACT_PER_LINE} '-' or '?' indicators"
                    )));
                }
            }
            _ => counting_indent = false,
        }
    }
    Ok(())
}

/// Refuses content that uses a YAML alias, BEFORE the loader can expand one.
///
/// `yaml-rust2`'s loader RESOLVES an alias by copying the anchored node, so by the
/// time a tree exists the expansion has already happened — a short alias chain can
/// expand to gigabytes. The parser's own event stream reports an alias as
/// `Event::Alias` and copies nothing, so the content is parsed once for this check and
/// only alias-free content is handed to the loader. An anchor no alias refers to
/// expands nothing and is accepted.
pub(super) fn reject_document_aliases(content: &str) -> Result<(), ContentRejection> {
    #[derive(Default)]
    struct AliasDetector {
        saw_alias: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.saw_alias = true;
            }
        }
    }

    let mut detector = AliasDetector::default();
    ::yaml_rust2::parser::Parser::new_from_str(content)
        .load(&mut detector, true)
        .map_err(|why| {
            ContentRejection::Content(format!("Content could not be parsed as a document: {why}"))
        })?;
    if detector.saw_alias {
        return Err(ContentRejection::Content(
            "Content must not use YAML aliases".to_string(),
        ))
    }
    Ok(())
}

/// Look a key up in a document mapping.
pub(super) fn mapping_get<'a>(
    map: &'a ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'a ::yaml_rust2::Yaml> {
    map.iter()
        .find(|(held, _)| held.as_str() == Some(key))
        .map(|(_, value)| value)
}

/// A document scalar as the string the API reports it as.
pub(super) fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => Some(text.clone()),
        ::yaml_rust2::Yaml::Boolean(flag) => Some(flag.to_string()),
        ::yaml_rust2::Yaml::Integer(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A parameter's `default` as the single string `DefaultValue` reports: the scalar
/// itself, or the JSON rendering of a list or mapping default.
///
/// `budget` is the recursion left, so a deeply nested default cannot drive this into
/// the stack even though [`guard_document_content`] already bounded the nesting; a
/// default that exhausts it is REFUSED rather than reported with the member missing.
pub(super) fn default_value_string(
    name: &str,
    value: &::yaml_rust2::Yaml,
    budget: usize,
) -> Result<String, ContentRejection> {
    if let Some(scalar) = scalar_string(value) {
        return Ok(scalar);
    }
    let rendered = document_as_json(value, budget).ok_or_else(|| {
        ContentRejection::Content(format!(
            "the default of parameter '{name}' is not a value this service can read: it nests \
             past {MAX_DOCUMENT_DEPTH} levels or is keyed by a non-string"
        ))
    })?;
    Ok(rendered.to_string())
}

/// A document node as the JSON value that renders it.
pub(crate) fn document_as_json(
    node: &::yaml_rust2::Yaml,
    budget: usize,
) -> Option<::serde_json::Value> {
    use ::serde_json::Value;
    use ::yaml_rust2::Yaml;
    let budget = budget.checked_sub(1)?;
    Some(match node {
        Yaml::Null | Yaml::BadValue => Value::Null,
        Yaml::Boolean(flag) => Value::Bool(*flag),
        Yaml::Integer(number) => Value::Number((*number).into()),
        Yaml::Real(real) => real
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or_else(|| Value::String(real.clone()), Value::Number),
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| document_as_json(item, budget))
                .collect::<Option<Vec<_>>>()?,
        ),
        Yaml::Hash(map) => {
            let mut object = ::serde_json::Map::new();
            for (key, value) in map {
                object.insert(key.as_str()?.to_string(), document_as_json(value, budget)?);
            }
            Value::Object(object)
        }
        // An alias is refused before the load by `reject_document_aliases`, so a
        // resolved alias node can never reach here.
        Yaml::Alias(_) => return None,
    })
}

/// What one declared parameter ADMITS as a value, beyond its name, its type and its
/// default.
///
/// A document's `parameters` entry may state what the values supplied for it have to be —
/// the enumeration they are drawn from, the expression they match, how long each may be and
/// how many there may be (AWS Systems Manager User Guide, "Creating SSM document content",
/// the `parameters` top-level element). The modelled `DocumentParameter` a read reports
/// carries none of that, so it is kept beside the reported list and read by the one place
/// that holds a call's values to what the document declares
/// ([`super::commands::parameter_rejection`]).
#[derive(Debug, Clone, Default)]
pub struct DeclaredParameterConstraints {
    /// The parameter this constrains, as the content names it.
    pub name: String,
    /// The content's `type` for it: `String`, `StringList`, `Integer`, `Boolean`,
    /// `StringMap` or `MapList` — the types that element documents.
    pub r#type: Option<String>,
    /// `allowedValues`: the values the parameter admits, or empty when it admits any.
    pub allowed_values: Vec<String>,
    /// `allowedPattern`: a regular expression every value has to match.
    pub allowed_pattern: Option<String>,
    /// `minChars` / `maxChars`: how long each value may be.
    pub min_chars: Option<usize>,
    pub max_chars: Option<usize>,
    /// `minItems` / `maxItems`: how many values there may be.
    pub min_items: Option<usize>,
    pub max_items: Option<usize>,
}

/// The parameter types that are a LIST of values; every other type is one value.
///
/// From the `type` values the `parameters` element documents: `StringList` is "a list of
/// strings" and `MapList` "a list of maps", while `String`, `Integer`, `Boolean` and
/// `StringMap` are each one value.
pub const LIST_PARAMETER_TYPES: &[&str] = &["StringList", "MapList"];

impl DeclaredParameterConstraints {
    /// Whether this parameter is one SEVERAL values may be supplied for
    /// ([`LIST_PARAMETER_TYPES`]). A parameter that declares no type is left unrestricted,
    /// so it takes however many were supplied.
    pub fn takes_several_values(&self) -> bool {
        match self.r#type.as_deref() {
            None => true,
            Some(declared) => LIST_PARAMETER_TYPES.contains(&declared),
        }
    }
}

/// A document scalar as the count it states, or `None` when it states no count.
pub(super) fn scalar_usize(value: &::yaml_rust2::Yaml) -> Option<usize> {
    match value {
        ::yaml_rust2::Yaml::Integer(number) => usize::try_from(*number).ok(),
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => text.parse().ok(),
        _ => None,
    }
}

/// The parameters `map` declares: the list a read REPORTS, and what each of them admits as
/// a value ([`DeclaredParameterConstraints`]), in the content's own order.
///
/// The `type` is reported verbatim rather than narrowed, because an Automation document's
/// parameters carry types (`Integer`, `StringMap`, …) beyond the two a Command document's
/// do.
pub(super) fn declared_parameters(
    map: &::yaml_rust2::yaml::Hash,
) -> Result<(Vec<DocumentParameter>, Vec<DeclaredParameterConstraints>), ContentRejection> {
    let Some(declared) = mapping_get(map, "parameters") else {
        return Ok((Vec::new(), Vec::new()));
    };
    if matches!(declared, ::yaml_rust2::Yaml::Null) {
        return Ok((Vec::new(), Vec::new()));
    }
    let Some(declared) = declared.as_hash() else {
        return Err(ContentRejection::Content(
            "the parameters of a document must be a mapping of parameter name to its definition"
                .to_string(),
        ));
    };
    let mut parameters = Vec::with_capacity(declared.len());
    let mut constraints = Vec::with_capacity(declared.len());
    for (name, definition) in declared {
        let Some(name) = name.as_str() else {
            return Err(ContentRejection::Content(
                "every parameter of a document must be named by a string".to_string(),
            ));
        };
        let Some(definition) = definition.as_hash() else {
            return Err(ContentRejection::Content(format!(
                "the definition of parameter '{name}' must be a mapping"
            )));
        };
        let default_value = match mapping_get(definition, "default") {
            None => None,
            Some(default) => Some(default_value_string(name, default, MAX_DOCUMENT_DEPTH)?),
        };
        let declared_type = mapping_get(definition, "type").and_then(scalar_string);
        constraints.push(DeclaredParameterConstraints {
            name: name.to_string(),
            r#type: declared_type.clone(),
            allowed_values: mapping_get(definition, "allowedValues")
                .and_then(::yaml_rust2::Yaml::as_vec)
                .map(|values| values.iter().filter_map(scalar_string).collect())
                .unwrap_or_default(),
            allowed_pattern: mapping_get(definition, "allowedPattern").and_then(scalar_string),
            min_chars: mapping_get(definition, "minChars").and_then(scalar_usize),
            max_chars: mapping_get(definition, "maxChars").and_then(scalar_usize),
            min_items: mapping_get(definition, "minItems").and_then(scalar_usize),
            max_items: mapping_get(definition, "maxItems").and_then(scalar_usize),
        });
        parameters.push(DocumentParameter {
            name: Some(name.to_string()),
            r#type: declared_type,
            description: mapping_get(definition, "description").and_then(scalar_string),
            default_value,
        });
    }
    Ok((parameters, constraints))
}

/// The top-level member a schema 2.x or 0.3 document declares its steps in: the steps
/// Systems Manager runs for that document, each with its own `name` (the `mainSteps`
/// top-level element of those schemas, "Systems Manager document schemas and features" in
/// the AWS Systems Manager User Guide).
pub const DOCUMENT_MAIN_STEPS: &str = "mainSteps";

/// The top-level member the schema 1.2 generation declares what a document runs in: the
/// plugin entries, whose keys are the plugin names (the `runtimeConfig` top-level element
/// of that schema).
pub const DOCUMENT_RUNTIME_CONFIG: &str = "runtimeConfig";

/// The document types whose body declares WHAT SYSTEMS MANAGER RUNS, so a body of one has
/// to carry the steps member its schema version names ([`required_steps_member`]).
///
/// Source: the per-type document schemas of the AWS Systems Manager User Guide ("Systems
/// Manager document schemas and features", and "Creating SSM document content" for the
/// top-level elements of each schema): a command document, an inventory policy document, an
/// Automation runbook and a change template each declare the steps to run, while a session
/// document declares the session it opens (`sessionType`) and a Distributor package declares
/// the packages it installs, so neither of those carries steps at all. Every other document
/// type carries no Systems Manager document schema ([`supported_schema_versions`]) and so
/// nothing this service requires of its body.
pub const STEP_DOCUMENT_TYPES: &[&str] = &[
    "Command",
    "Policy",
    "Automation",
    "Automation.ChangeTemplate",
];

/// The top-level member a document of `document_type` on `schema_version` MUST declare, or
/// `None` when that pair requires none.
///
/// Systems Manager validates a document body against the schema its `schemaVersion`
/// declares, and the member a step document ([`STEP_DOCUMENT_TYPES`]) declares its steps in
/// is mandatory in every generation of that schema: the 2.x and 0.3 schemas declare them as
/// [`DOCUMENT_MAIN_STEPS`] and the 1.2 schema as [`DOCUMENT_RUNTIME_CONFIG`]. A body that
/// declares no such member is what `InvalidDocumentContent` reports
/// ([`missing_document_member_message`]).
pub fn required_steps_member(document_type: &str, schema_version: &str) -> Option<&'static str> {
    if !STEP_DOCUMENT_TYPES.contains(&document_type) {
        return None;
    }
    match schema_version {
        "0.3" | "2.0" | "2.2" => Some(DOCUMENT_MAIN_STEPS),
        "1.2" => Some(DOCUMENT_RUNTIME_CONFIG),
        _ => None,
    }
}

/// How this service states that a document body declares no member its schema requires.
///
/// `InvalidDocumentContent` is "the content for the document isn't valid" (its own
/// documentation), and the sentence Systems Manager answers a body missing a mandatory
/// top-level member with is fixed text naming that MEMBER — `Missing "mainSteps" in the
/// document.` — with nothing of the request in it.
pub fn missing_document_member_message(member: &str) -> String {
    format!("Missing \"{member}\" in the document.")
}

/// The names of the steps `content` declares, in its own order.
///
/// A schema 2.x document declares its steps as `mainSteps`, each with its own `name`
/// ("The name of the step"), and a schema 1.2 document declares them as the `runtimeConfig`
/// plugin entries, whose keys are the plugin names. A `GetCommandInvocation` asks for one
/// step's detailed results by that name, so both spellings are read.
pub(super) fn declared_step_names(map: &::yaml_rust2::yaml::Hash) -> Vec<String> {
    if let Some(steps) =
        mapping_get(map, DOCUMENT_MAIN_STEPS).and_then(::yaml_rust2::Yaml::as_vec)
    {
        return steps
            .iter()
            .filter_map(|step| mapping_get(step.as_hash()?, "name").and_then(scalar_string))
            .collect();
    }
    mapping_get(map, DOCUMENT_RUNTIME_CONFIG)
        .and_then(::yaml_rust2::Yaml::as_hash)
        .map(|plugins| {
            plugins
                .keys()
                .filter_map(::yaml_rust2::Yaml::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The ONE document tree `content` holds, or why it is not one document tree this
/// service accepts.
///
/// This is what "one document tree" means for this crate: the create that derives a
/// document's report from its content and the read that re-renders that content in
/// another format ([`document_format`]) both read it through here.
///
/// The pre-flight bounds are applied HERE, before the loader is handed anything, so no
/// caller can reach it without them: the parser descends recursively
/// ([`guard_document_content`]) and resolves an alias by copying the anchored node
/// ([`reject_document_aliases`]), so content that was stored without being measured — a
/// `TEXT` entry a later read re-renders, say — is measured on the way in rather than
/// trusted. All three checks are pure functions of `content`, so applying them again
/// where a caller has already applied them changes nothing.
pub fn load_single_document(content: &str) -> Result<::yaml_rust2::Yaml, ContentRejection> {
    content_within_size(content)?;
    guard_document_content(content)?;
    reject_document_aliases(content)?;
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(content).map_err(|why| {
        ContentRejection::Content(format!("Content could not be parsed as a document: {why}"))
    })?;
    if documents.len() != 1 {
        return Err(ContentRejection::Content(format!(
            "Content must be exactly one document; found {}",
            documents.len()
        )));
    }
    Ok(documents.remove(0))
}

/// What `content` declares, or why it is not a document this service accepts.
///
/// "The content for the new SSM document in JSON or YAML format"
/// (`CreateDocument`'s `Content` doc), so the content is parsed in the format the
/// request declared — YAML 1.2 is a superset of JSON, so one parser reads either, and
/// `JSON` is additionally required to BE JSON rather than only YAML that resembles it.
/// `TEXT` content (a Change Calendar entry) is free text with no document tree, so
/// nothing is derived from it and nothing in it is refused.
pub fn parse_document_content(
    content: &str,
    format: &str,
    document_type: &str,
) -> Result<ParsedDocumentContent, ContentRejection> {
    // The documented size bound is measured for content of every format, including the
    // one with no document tree to parse.
    content_within_size(content)?;
    if format == DOCUMENT_FORMAT_TEXT {
        return Ok(ParsedDocumentContent::default());
    }
    // The nesting guard runs before the JSON reading below, which builds a tree of its
    // own from the same bytes; [`load_single_document`] applies it (and the alias
    // rejection) again before the YAML loader sees them.
    guard_document_content(content)?;
    if format == DEFAULT_DOCUMENT_FORMAT
        && ::serde_json::from_str::<::serde_json::Value>(content).is_err()
    {
        return Err(ContentRejection::Content(
            "Content isn't a valid JSON document".to_string(),
        ));
    }
    let document = load_single_document(content)?;
    let Some(map) = document.as_hash() else {
        return Err(ContentRejection::Content(
            "Content must be a mapping of document keys to values".to_string(),
        ));
    };

    let schema_version = mapping_get(map, "schemaVersion").and_then(scalar_string);
    let supported = supported_schema_versions(document_type);
    if !supported.is_empty() {
        match schema_version.as_deref() {
            None => {
                return Err(ContentRejection::Content(format!(
                    "Content declares no schemaVersion; a {document_type} document must declare \
                     one of {}",
                    supported.join(", ")
                )))
            }
            Some(declared) if !supported.contains(&declared) => {
                return Err(ContentRejection::SchemaVersion(format!(
                    "Document schema version '{declared}' isn't supported for a {document_type} \
                     document; the supported schema versions are {}",
                    supported.join(", ")
                )))
            }
            Some(_) => {}
        }
    }

    let (parameters, parameter_constraints) = declared_parameters(map)?;
    Ok(ParsedDocumentContent {
        schema_version,
        description: mapping_get(map, "description").and_then(scalar_string),
        parameters,
        parameter_constraints,
        platform_types: document_platforms::declared_platform_types(map),
        step_names: declared_step_names(map),
        steps_member: [DOCUMENT_MAIN_STEPS, DOCUMENT_RUNTIME_CONFIG]
            .into_iter()
            .find(|member| mapping_get(map, member).is_some())
            .map(str::to_string),
    })
}

/// What the content a CALL supplies declares, or why it is not a document body this service
/// accepts from the caller.
///
/// Everything [`parse_document_content`] derives, plus the requirement that the body declare
/// the member its schema version makes mandatory ([`required_steps_member`]): Systems
/// Manager validates the body a `CreateDocument` or an `UpdateDocument` carries against the
/// schema that body declares, and refuses one missing a mandatory member with
/// `InvalidDocumentContent` rather than registering a document nothing can run.
///
/// The one entry point for caller-supplied content, so a create and an update hold a body to
/// the same rules; this crate's own published bodies are read through
/// [`parse_document_content`], which derives what a read reports without re-validating a
/// body no request supplied.
pub fn parse_supplied_document_content(
    content: &str,
    format: &str,
    document_type: &str,
) -> Result<ParsedDocumentContent, ContentRejection> {
    let parsed = parse_document_content(content, format, document_type)?;
    if let Some(member) = parsed.missing_steps_member(document_type) {
        return Err(ContentRejection::Content(missing_document_member_message(
            member,
        )));
    }
    Ok(parsed)
}

impl ParsedDocumentContent {
    /// The steps member a body of a `document_type` document has to declare and this body
    /// does not ([`required_steps_member`]), or `None` when it declares the member its
    /// schema version requires.
    ///
    /// A body that declares the OTHER generation's steps member still declares none of the
    /// one its own schema version names, so it is missing it: the schema the body declares
    /// is what decides which member it has to carry.
    pub fn missing_steps_member(&self, document_type: &str) -> Option<&'static str> {
        let required = required_steps_member(document_type, self.schema_version.as_deref()?)?;
        (self.steps_member.as_deref() != Some(required)).then_some(required)
    }
}

// ---------------------------------------------------------------------------
// The SSM documents Systems Manager publishes
// ---------------------------------------------------------------------------

/// The `Owner` a document Systems Manager publishes reports.
///
/// `Owner` is "the Amazon Web Services user that created the document"
/// (`DocumentDescription$Owner`), and a published document was created by Amazon rather
/// than by the account that reads it — the owner a listing selects it by
/// (`ListDocuments`' `Owner` filter: `Self`, `Amazon`, `Public`, `Private`,
/// `ThirdParty`).
pub const PUBLISHED_DOCUMENT_OWNER: &str = "Amazon";

/// What the `content` of a catalogue entry IS.
///
/// Systems Manager publishes hundreds of documents, and the bodies it publishes are not in
/// this service's model, so the catalogue carries two kinds of body — and a read reports
/// the same members from either, derived by the one content reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishedBody {
    /// A body THIS CRATE authors for the document, declaring the parameters the "Systems
    /// Manager Automation runbook reference" (or the command-document reference) documents
    /// for it, so a read of that document reports its real parameter list.
    Authored,
    /// The stand-in body of the document's FAMILY: it names the document and declares the
    /// schema version of its type, which is what the catalogue's own metadata (the name,
    /// the type, the target type) is known for; the parameters the published body declares
    /// are not restated, so a read reports none for it.
    FamilyStandIn,
}

/// ONE version Systems Manager has published of a catalogue document: the version number it
/// publishes that revision as, and when it published it.
///
/// A published document is republished as Amazon revises it — `ListDocumentVersions` of one
/// "lists all versions for a document", and the recorded catalogue reports several for the
/// runbooks Amazon has revised — so the versions an entry declares are a FACT OF THE
/// CATALOGUE carried as data, the way the publication date and the target type are. The
/// version a read that names no selector resolves to is the NEWEST of them
/// ([`PublishedDocument::default_published_version`]): Amazon publishes its latest revision
/// as the default.
#[derive(Debug, Clone, Copy)]
pub struct PublishedVersion {
    /// "The document version", counting from [`FIRST_DOCUMENT_VERSION`].
    pub version: u64,
    /// When Amazon published THIS version, in milliseconds since the Unix epoch (see
    /// [`PublishedDocument::created_date_millis`] for why a date is data here).
    pub created_date_millis: u64,
}

/// One document of the catalogue Systems Manager publishes: present in every account and
/// Region without anybody creating it, and readable — never writable — by the account
/// that reads it.
#[derive(Debug, Clone, Copy)]
pub struct PublishedDocument {
    /// The name the document resolves by, which is also what a read reports.
    pub name: &'static str,
    /// A `DocumentType` value (`crate::ops::types::DOCUMENT_TYPE_VALUES`).
    pub document_type: &'static str,
    /// The document's body, in the format Amazon authored it in
    /// ([`PublishedDocument::document_format`]). Everything a read reports beyond the name,
    /// the type, the target type and the publication date is derived from it by the one
    /// content reader ([`parse_document_content`]), exactly as it is for a document a caller
    /// created — the reported `DocumentFormat` included.
    pub content: &'static str,
    /// Which kind of body [`PublishedDocument::content`] is.
    pub body: PublishedBody,
    /// "The target type which defines the kinds of resources the document can run on. For
    /// example, `/AWS::EC2::Instance`" (`DocumentIdentifier$TargetType`), or `None` for a
    /// published document the catalogue names none for.
    ///
    /// A fact of the catalogue, like the publication date: a published document's target
    /// type is Amazon's to declare, so it is carried here as data rather than read off a
    /// request — and `ListDocuments`' `TargetType` filter selects on it.
    pub target_type: Option<&'static str>,
    /// When Amazon published this document, in MILLISECONDS since the Unix epoch.
    ///
    /// `CreatedDate` is "the date the document was created"
    /// (`DocumentVersionInfo$CreatedDate`, `DocumentDescription$CreatedDate`), and nobody
    /// in this account created a published document: Amazon did, once, before any account
    /// read it. So the date is a FACT OF THE CATALOGUE carried here as data rather than a
    /// clock reading taken when this store came into being — identical in every account,
    /// Region and run of the emulator, and unchanged by a restart. Milliseconds, because
    /// that is the precision Systems Manager reports a document date at.
    pub created_date_millis: u64,
    /// Every version Amazon has published of this document, OLDEST FIRST, or EMPTY for an
    /// entry whose history this crate does not hold.
    ///
    /// "List all versions for a document" (`ListDocumentVersions`) reports one entry per
    /// published version, and Amazon republishes a document as it revises it, so the number
    /// of versions a catalogue entry has and the date of each are the catalogue's own data —
    /// held here for the entries this crate holds them for. An entry declaring NONE is the
    /// one version its publication date dates ([`PublishedDocument::published_versions`]),
    /// which is what a document Amazon has published once has.
    pub versions: &'static [PublishedVersion],
}

impl PublishedDocument {
    /// The `DocumentFormat` Amazon AUTHORED this document in, which is what every read of
    /// it reports: "the document format, either JSON or YAML"
    /// (`DocumentVersionInfo$DocumentFormat`).
    ///
    /// A published document was written by Amazon rather than created here, so its format
    /// is a fact of the catalogue the way its type and its publication date are — and the
    /// catalogue carries it in the only place it cannot disagree with: the BODY itself. YAML
    /// 1.2 is a superset of JSON, so a body that reads as JSON is a JSON document and any
    /// other body this service's one parser reads is a YAML one. Nothing here is the
    /// format a create of this account's own document falls back to
    /// ([`DEFAULT_DOCUMENT_FORMAT`]): that is the default of a REQUEST that declares none.
    pub fn document_format(&self) -> &'static str {
        if ::serde_json::from_str::<::serde_json::Value>(self.content).is_ok() {
            DEFAULT_DOCUMENT_FORMAT
        } else {
            DOCUMENT_FORMAT_YAML
        }
    }

    /// The instant this document was published, as the stored record and every projection
    /// of it carry it ([`PublishedDocument::created_date_millis`]).
    pub fn created_date(&self) -> SystemTime {
        ::std::time::UNIX_EPOCH + ::std::time::Duration::from_millis(self.created_date_millis)
    }

    /// The versions this entry publishes, oldest first and never empty: the history the
    /// catalogue declares, or the ONE version its publication date dates for an entry that
    /// declares none.
    pub fn published_versions(&self) -> Vec<PublishedVersion> {
        if self.versions.is_empty() {
            return vec![PublishedVersion {
                version: FIRST_DOCUMENT_VERSION,
                created_date_millis: self.created_date_millis,
            }];
        }
        self.versions.to_vec()
    }

    /// The version a read of this document that names no selector resolves to: the NEWEST
    /// version the catalogue publishes, since Amazon publishes its latest revision of a
    /// document as that document's default version.
    pub fn default_published_version(&self) -> u64 {
        self.published_versions()
            .iter()
            .map(|published| published.version)
            .max()
            .unwrap_or(FIRST_DOCUMENT_VERSION)
    }
}

/// The body of the runbook that configures versioning on an S3 bucket.
///
/// The parameters each published runbook takes are the ones the "Systems Manager
/// Automation runbook reference" documents for it; the body below is this emulator's
/// stand-in for the published body, declaring those parameters so a read derives them.
pub(super) const CONFIGURE_S3_BUCKET_VERSIONING: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Configures versioning on an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket to configure versioning on."
    },
    "VersioningState": {
      "type": "String",
      "description": "(Optional) The versioning state of the bucket.",
      "default": "Enabled",
      "allowedValues": ["Enabled", "Suspended"]
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketVersioning",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutBucketVersioning",
        "Bucket": "{{ BucketName }}",
        "VersioningConfiguration": { "Status": "{{ VersioningState }}" }
      }
    },
    {
      "name": "VerifyBucketVersioning",
      "action": "aws:executeAwsApi",
      "inputs": { "Service": "s3", "Api": "GetBucketVersioning", "Bucket": "{{ BucketName }}" }
    }
  ]
}"#;

/// The body of the runbook that blocks public access on an S3 bucket. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
pub(super) const CONFIGURE_S3_BUCKET_PUBLIC_ACCESS_BLOCK: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Configures the public access block of an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket to configure."
    },
    "BlockPublicAcls": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 blocks public ACLs for this bucket.",
      "default": true
    },
    "IgnorePublicAcls": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 ignores public ACLs for this bucket.",
      "default": true
    },
    "BlockPublicPolicy": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 blocks public bucket policies for this bucket.",
      "default": true
    },
    "RestrictPublicBuckets": {
      "type": "Boolean",
      "description": "(Optional) Whether Amazon S3 restricts public bucket policies for this bucket.",
      "default": true
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketPublicAccessBlock",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutPublicAccessBlock",
        "Bucket": "{{ BucketName }}",
        "PublicAccessBlockConfiguration": {
          "BlockPublicAcls": "{{ BlockPublicAcls }}",
          "IgnorePublicAcls": "{{ IgnorePublicAcls }}",
          "BlockPublicPolicy": "{{ BlockPublicPolicy }}",
          "RestrictPublicBuckets": "{{ RestrictPublicBuckets }}"
        }
      }
    }
  ]
}"#;

/// The body of the runbook that removes a bucket's public read and write grants. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
pub(super) const DISABLE_S3_BUCKET_PUBLIC_READ_WRITE: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Removes the public read and write grants of an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "S3BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket whose public grants are removed."
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketAcl",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutBucketAcl",
        "Bucket": "{{ S3BucketName }}",
        "ACL": "private"
      }
    }
  ]
}"#;

/// The body of the runbook that turns on default encryption of an S3 bucket. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
pub(super) const ENABLE_S3_BUCKET_ENCRYPTION: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Turns on default server-side encryption of an Amazon S3 bucket.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "BucketName": {
      "type": "String",
      "description": "(Required) The name of the S3 bucket to encrypt."
    },
    "SSEAlgorithm": {
      "type": "String",
      "description": "(Optional) The server-side encryption algorithm to apply by default.",
      "default": "AES256"
    }
  },
  "mainSteps": [
    {
      "name": "PutBucketEncryption",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "s3",
        "Api": "PutBucketEncryption",
        "Bucket": "{{ BucketName }}",
        "ServerSideEncryptionConfiguration": {
          "Rules": [
            {
              "ApplyServerSideEncryptionByDefault": { "SSEAlgorithm": "{{ SSEAlgorithm }}" }
            }
          ]
        }
      }
    }
  ]
}"#;

/// The body of the runbook that publishes a notification to an SNS topic. See
/// [`CONFIGURE_S3_BUCKET_VERSIONING`] for where the parameters come from.
pub(super) const PUBLISH_SNS_NOTIFICATION: &str = r#"{
  "schemaVersion": "0.3",
  "description": "Publishes a notification to an Amazon SNS topic.",
  "assumeRole": "{{ AutomationAssumeRole }}",
  "parameters": {
    "AutomationAssumeRole": {
      "type": "String",
      "description": "(Optional) The ARN of the role that allows Automation to perform the actions on your behalf.",
      "default": ""
    },
    "TopicArn": {
      "type": "String",
      "description": "(Required) The ARN of the SNS topic to publish to."
    },
    "Message": {
      "type": "String",
      "description": "(Required) The message to publish."
    }
  },
  "mainSteps": [
    {
      "name": "Publish",
      "action": "aws:executeAwsApi",
      "inputs": {
        "Service": "sns",
        "Api": "Publish",
        "TopicArn": "{{ TopicArn }}",
        "Message": "{{ Message }}"
      }
    }
  ]
}"#;

/// The body of the command document that runs a shell script on a managed node.
///
/// Published at schema 1.2 — the schema the two script documents Systems Manager has
/// published since 2017 are written in — whose steps are the `runtimeConfig` PLUGIN
/// ENTRIES rather than `mainSteps`: the map is keyed by the plugin Run Command invokes
/// (`aws:runShellScript`) and that plugin's `properties` carry the script in
/// `runCommand`. The plugin is what restricts the document to a platform
/// ([`document_platforms::declared_platform_types`]), so the platforms the published
/// document is reported compatible with are declared by its own content, the way a
/// created document's are.
pub(super) const RUN_SHELL_SCRIPT: &str = r#"{
  "schemaVersion": "1.2",
  "description": "Run a shell script or specify the commands to run.",
  "parameters": {
    "commands": {
      "type": "StringList",
      "description": "(Required) Specify a shell script or a command to run."
    },
    "workingDirectory": {
      "type": "String",
      "description": "(Optional) The path to the working directory on your managed node.",
      "default": ""
    },
    "executionTimeout": {
      "type": "String",
      "description": "(Optional) The time in seconds for a command to be completed before it is considered to have failed.",
      "default": "3600"
    }
  },
  "runtimeConfig": {
    "aws:runShellScript": {
      "properties": [
        {
          "id": "0.aws:runShellScript",
          "runCommand": "{{ commands }}",
          "workingDirectory": "{{ workingDirectory }}",
          "timeoutSeconds": "{{ executionTimeout }}"
        }
      ]
    }
  }
}"#;

/// The body of the command document that runs a PowerShell script on a managed node.
/// Published at schema 1.2 as well — see [`RUN_SHELL_SCRIPT`] for how such a body
/// declares its plugin and the platforms that plugin restricts it to.
pub(super) const RUN_POWER_SHELL_SCRIPT: &str = r#"{
  "schemaVersion": "1.2",
  "description": "Run a PowerShell script or specify the commands to run.",
  "parameters": {
    "commands": {
      "type": "StringList",
      "description": "(Required) Specify a PowerShell script or a command to run."
    },
    "workingDirectory": {
      "type": "String",
      "description": "(Optional) The path to the working directory on your managed node.",
      "default": ""
    },
    "executionTimeout": {
      "type": "String",
      "description": "(Optional) The time in seconds for a command to be completed before it is considered to have failed.",
      "default": "3600"
    }
  },
  "runtimeConfig": {
    "aws:runPowerShellScript": {
      "properties": [
        {
          "id": "0.aws:runPowerShellScript",
          "runCommand": "{{ commands }}",
          "workingDirectory": "{{ workingDirectory }}",
          "timeoutSeconds": "{{ executionTimeout }}"
        }
      ]
    }
  }
}"#;

/// The body of the command document that updates the SSM Agent on a managed node. It
/// declares no precondition, so it is compatible with every platform the model lists.
pub(super) const UPDATE_SSM_AGENT: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Update the SSM Agent to the latest version or specify an older version.",
  "parameters": {
    "version": {
      "type": "String",
      "description": "(Optional) The version of the SSM Agent to install. An empty value installs the latest version.",
      "default": ""
    },
    "allowDowngrade": {
      "type": "String",
      "description": "(Optional) Whether the SSM Agent may be downgraded to an earlier version.",
      "default": "false",
      "allowedValues": ["true", "false"]
    }
  },
  "mainSteps": [
    {
      "name": "updateSSMAgent",
      "action": "aws:updateSsmAgent",
      "inputs": {
        "agentName": "amazon-ssm-agent",
        "source": "https://s3.amazonaws.com/ec2-downloads-windows/SSMAgent/{{version}}/amazon-ssm-agent.msi",
        "allowDowngrade": "{{ allowDowngrade }}",
        "targetVersion": "{{ version }}"
      }
    }
  ]
}"#;

/// The `DocumentType` of a document that OPENS A SESSION on a managed node rather than
/// running a command on it (from the modelled enum,
/// `crate::ops::types::DOCUMENT_TYPE_VALUES`): the type of the session documents Session
/// Manager starts a session with.
pub const DOCUMENT_TYPE_SESSION: &str = "Session";

/// The `DocumentType` of a document that tells the agent on a managed node WHAT TO COLLECT
/// rather than what to run (from the modelled enum,
/// `crate::ops::types::DOCUMENT_TYPE_VALUES`): the type of the inventory policy document
/// Systems Manager publishes ([`inventory::GATHER_SOFTWARE_INVENTORY_DOCUMENT`]).
pub const DOCUMENT_TYPE_POLICY: &str = "Policy";

/// The published document `name` names, or `None` when the catalogue holds none.
///
/// A name is resolved as written: an SSM document name is case-sensitive, so a
/// differently-cased spelling names no document.
pub fn published_document(name: &str) -> Option<&'static PublishedDocument> {
    PUBLISHED_DOCUMENTS
        .iter()
        .find(|published| published.name == name)
}

/// What each catalogue body yields: the members the content reader derives from it, its
/// hash, and the format it is authored in.
///
/// Derived ONCE for the process rather than per read. The bodies are `const`, so two
/// reads of one published document cannot differ, and no request pays for parsing them.
/// A body this reader cannot read — a mistake in a `const` of this crate, which the test
/// suite reads every entry to catch, never a caller's input — yields nothing derived
/// rather than panicking a live request: the document does exist either way.
///
/// Each body is read in ITS OWN format ([`PublishedDocument::document_format`]), so a
/// document Amazon authored in YAML yields the same description, parameters and platform
/// types as one it authored in JSON.
pub(super) static PUBLISHED_PROJECTIONS: ::std::sync::LazyLock<
    Vec<(ParsedDocumentContent, String, &'static str)>,
> = ::std::sync::LazyLock::new(|| {
    PUBLISHED_DOCUMENTS
        .iter()
        .map(|published| {
            let format = published.document_format();
            let parsed =
                parse_document_content(published.content, format, published.document_type)
                    .unwrap_or_default();
            (parsed, document_content_hash(published.content), format)
        })
        .collect()
});

/// The published document `name` names together with what its body yields — the derived
/// members, the body's hash and the format it is authored in — or `None` when the
/// catalogue holds no document of that name.
pub(super) fn published_document_projection(
    name: &str,
) -> Option<(
    &'static PublishedDocument,
    &'static ParsedDocumentContent,
    &'static str,
    &'static str,
)> {
    let index = PUBLISHED_DOCUMENTS
        .iter()
        .position(|published| published.name == name)?;
    let published = PUBLISHED_DOCUMENTS.get(index)?;
    let (parsed, hash, format) = PUBLISHED_PROJECTIONS.get(index)?;
    Some((published, parsed, hash.as_str(), format))
}

/// The hash the system takes over a document's content: "The Sha256 or Sha1 hash created
/// by the system when the document was created" (`DocumentDescription$Hash`), with "Sha1
/// hashes have been deprecated", rendered hex the way AWS reports it.
pub fn document_content_hash(content: &str) -> String {
    use ::sha2::Digest;
    ::hex::encode(::sha2::Sha256::digest(content.as_bytes()))
}

/// The attachment names `AttachmentsInformation` reports — "details about the document
/// attachments, including names" — for the attachment sources one call named.
///
/// An `AttachmentsSource` carries "the name of the document attachment file" in `Name`;
/// a source that names none is identified by the file its `Values` URL points at, which
/// is the last segment of that URL. Read by every call that supplies attachments for a
/// version of a document, so a create and an update identify them the same way.
pub fn attachment_names(attachments: Option<&[AttachmentsSource]>) -> Vec<String> {
    attachments
        .unwrap_or_default()
        .iter()
        .filter_map(|source| {
            source
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    let value = source.values.as_ref()?.first()?;
                    let file = value.rsplit('/').next().unwrap_or(value);
                    (!file.is_empty()).then(|| file.to_string())
                })
        })
        .collect()
}

/// ONE version of a stored SSM document: what the call that minted it supplied, what the
/// service derived from that version's content, and the status move the clock still owes
/// it.
///
/// Everything here is per version because that is what the model says varies between
/// them: `DisplayName` "can differ for each version of the document", a `VersionName` is
/// "unique across all versions of a document", and the content — and so the hash, the
/// schema version, the description, the parameters and the platform types derived from it
/// — is what an update supplies a new one of.
#[derive(Debug, Clone)]
pub struct DocumentVersionRecord {
    /// "The document version", counting from [`FIRST_DOCUMENT_VERSION`].
    pub version: u64,
    pub display_name: Option<String>,
    pub version_name: Option<String>,
    /// A `DocumentFormat` value: the format this version's content is written in.
    pub document_format: String,
    pub target_type: Option<String>,
    /// The content exactly as the call supplied it: what
    /// [`DocumentVersionRecord::hash`] is taken over, and what `GetDocument` reports.
    pub content: String,
    /// Hex-rendered Sha256 of [`DocumentVersionRecord::content`].
    pub hash: String,
    /// Everything derived from the content (see [`parse_document_content`]).
    pub parsed: ParsedDocumentContent,
    pub requires: Option<Vec<DocumentRequires>>,
    /// "The cloud providers that the returned version of the document can run against"
    /// (`DocumentDescription$CloudProviders`), as `CloudProvider` values.
    ///
    /// Per VERSION, because that is what the model says varies between them: a create's
    /// `CloudProviders` is "the cloud providers that the document can run against" and an
    /// update's is "the cloud providers that the NEW DOCUMENT VERSION can run against".
    /// Never empty — a version whose call named none runs against
    /// [`DEFAULT_CLOUD_PROVIDER`].
    pub cloud_providers: Vec<String>,
    /// The attachment names the call named, which is what `AttachmentsInformation`
    /// reports.
    pub attachment_names: Vec<String>,
    pub created_date: SystemTime,
    /// A `DocumentStatus` value.
    pub status: String,
    /// When the clock owes this version's move to `Active`, or `None` once that move has
    /// been materialised.
    pub active_due: Option<SystemTime>,
}

impl DocumentVersionRecord {
    /// Materialise the status move the clock already owes at `now` — the move to
    /// `Active` that was due whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.active_due.is_some_and(|due| now >= due) {
            self.active_due = None;
            self.status = DOCUMENT_STATUS_ACTIVE.to_string();
        }
    }
}

/// One stored SSM document: the identity and the metadata the whole document carries,
/// and every version of it the store holds.
///
/// A create mints the first version and each `UpdateDocument` appends the next one, so
/// `LatestVersion` is the last version held. The DEFAULT version moves only when
/// `UpdateDocumentDefaultVersion` moves it, which is why an update's new version is the
/// latest without becoming the default.
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    /// The identifier Systems Manager assigned the document when it was created: minted
    /// once, per document, so two reads of one document resolve the same id and no two
    /// documents share one ([`ComAmazonawsSsmAmazonssmState::mint_document_id`]). Nothing of
    /// the request is in it, and it is not the key the document is reached by — a document is
    /// addressed by name.
    ///
    /// Not on the wire: no shape of the model declares a `DocumentId` member, so no read
    /// reports it. It stays the document's stored identity, which is what keeps a published
    /// document's id stable across reads
    /// ([`ComAmazonawsSsmAmazonssmState::published_document_ids`]).
    pub document_id: String,
    /// The name the create supplied, which is what a read reports and part of the
    /// store key.
    pub name: String,
    /// The Region the document was created in; part of its store key and of its ARN.
    pub region: String,
    /// A `DocumentType` value. One document is of one type: `UpdateDocument` carries no
    /// `DocumentType` member, so a version never changes it.
    pub document_type: String,
    pub tags: BTreeMap<String, String>,
    /// "The Amazon Web Services user that created the document"
    /// (`DocumentDescription$Owner`): the account for a document a caller created here,
    /// and [`PUBLISHED_DOCUMENT_OWNER`] for one Systems Manager publishes.
    pub owner: String,
    /// Every version this document holds, oldest first and the newest last. Never empty
    /// for a record that is in the store.
    pub versions: Vec<DocumentVersionRecord>,
    /// "The default version", the one a read that names no version resolves to.
    pub default_version: u64,
    /// Who this document is SHARED with, as account id (or `all`) to the document version
    /// the share names — what `ModifyDocumentPermission` writes and
    /// `DescribeDocumentPermission` reports ([`super::document_permissions`]). Empty for a
    /// document nobody shared.
    pub shared_with: BTreeMap<String, String>,
}

/// Why a read's `DocumentVersion` / `VersionName` pair names no version of a document,
/// carrying what was asked for so each operation can report it in its own words.
#[derive(Debug, Clone)]
pub enum DocumentVersionRejection {
    /// No version of the document is the one this `DocumentVersion` selector names.
    Version(String),
    /// No version of the document carries this `VersionName`.
    VersionName(String),
}

impl DocumentRecord {
    /// This document's ARN: `arn:aws:ssm:<region>:<account>:document/<name>`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            &self.region,
            &format!("{ARN_DOCUMENT_RESOURCE_PREFIX}{}", self.name),
        )
    }

    /// Materialise the status move the clock already owes every version at `now` — the
    /// move to `Active` that was due whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for version in &mut self.versions {
            version.materialize_due(now);
        }
    }

    /// The newest version, which is "the latest version of the document"
    /// (`DocumentDescription$LatestVersion`).
    pub fn latest(&self) -> Option<&DocumentVersionRecord> {
        self.versions.last()
    }

    /// The number "the latest version of the document" carries.
    pub fn latest_version(&self) -> u64 {
        self.latest()
            .map_or(FIRST_DOCUMENT_VERSION, |latest| latest.version)
    }

    /// The number the next version of this document carries: each update mints the one
    /// after the latest.
    pub fn next_version(&self) -> u64 {
        self.latest_version().saturating_add(1)
    }

    /// The version `version` numbers, or `None` when this document holds no such version.
    pub fn version(&self, version: u64) -> Option<&DocumentVersionRecord> {
        self.versions.iter().find(|held| held.version == version)
    }

    /// The version `version_name` names: "This value is unique across all versions of a
    /// document" (`DocumentVersionInfo$VersionName`), so it names at most one of them.
    pub fn version_named(&self, version_name: &str) -> Option<&DocumentVersionRecord> {
        self.versions
            .iter()
            .find(|held| held.version_name.as_deref() == Some(version_name))
    }

    /// The version a `DocumentVersion` selector names, or `None` when this document
    /// holds none.
    ///
    /// The modelled `DocumentVersion` pattern admits three spellings: `$LATEST` names the
    /// newest version, `$DEFAULT` names "the default version", and a number names the
    /// version it counts.
    pub fn select(&self, selector: &str) -> Option<&DocumentVersionRecord> {
        match selector {
            DOCUMENT_VERSION_LATEST => self.latest(),
            DOCUMENT_VERSION_DEFAULT => self.version(self.default_version),
            number => self.version(number.parse().ok()?),
        }
    }

    /// The version a read's `DocumentVersion` and `VersionName` pair names, or why it
    /// names none of this document's versions.
    ///
    /// A read that names neither resolves to the default version. A read that names only
    /// a version NAME resolves to the version carrying it, because that name is "unique
    /// across all versions of a document"; one that names both has to name the same
    /// version twice, so a name the selected version does not carry names no version.
    pub fn select_requested(
        &self,
        document_version: Option<&str>,
        version_name: Option<&str>,
    ) -> Result<&DocumentVersionRecord, DocumentVersionRejection> {
        let document_version = document_version.filter(|selector| !selector.is_empty());
        let version_name = version_name.filter(|name| !name.is_empty());
        let selected = match (document_version, version_name) {
            (Some(selector), _) => self
                .select(selector)
                .ok_or_else(|| DocumentVersionRejection::Version(selector.to_string()))?,
            (None, Some(name)) => {
                return self
                    .version_named(name)
                    .ok_or_else(|| DocumentVersionRejection::VersionName(name.to_string()));
            }
            (None, None) => self.select(DOCUMENT_VERSION_DEFAULT).ok_or_else(|| {
                DocumentVersionRejection::Version(DOCUMENT_VERSION_DEFAULT.to_string())
            })?,
        };
        match version_name {
            Some(name) if selected.version_name.as_deref() != Some(name) => {
                Err(DocumentVersionRejection::VersionName(name.to_string()))
            }
            _ => Ok(selected),
        }
    }

    /// Whether `requested` names a version name this document carries.
    pub fn holds_version_name(&self, requested: &str) -> bool {
        self.version_named(requested).is_some()
    }

    /// The version whose content hashes to `hash`, or `None` when no version of this
    /// document holds that content.
    ///
    /// The hash is taken over the content ([`document_content_hash`]), so two versions
    /// hash alike exactly when their content is the same — which is what
    /// `DuplicateDocumentContent` is reported for.
    pub fn version_with_hash(&self, hash: &str) -> Option<&DocumentVersionRecord> {
        self.versions.iter().find(|held| held.hash == hash)
    }

    /// "The current review status of a new custom Systems Manager document"
    /// (`DocumentDescription$ReviewStatus`).
    ///
    /// A review is the change-template approval workflow, and `UpdateDocumentMetadata` —
    /// the operation that records one — is still a scaffold, so a change template created
    /// here has not been reviewed and no other document type carries a review at all.
    pub fn review_status(&self) -> Option<String> {
        (self.document_type == CHANGE_TEMPLATE_DOCUMENT_TYPE)
            .then(|| DOCUMENT_REVIEW_STATUS_NOT_REVIEWED.to_string())
    }

    /// The modelled `DocumentDescription` of `version` of this document: the version's
    /// own members beside the whole document's name, owner, type, tags, latest version
    /// and default version.
    pub fn description_of(&self, version: &DocumentVersionRecord) -> DocumentDescription {
        DocumentDescription {
            // "Sha1 hashes have been deprecated", so the system hash is the Sha256 in
            // `Hash` and the deprecated member is left unset.
            sha1: None,
            hash: Some(version.hash.clone()),
            hash_type: Some(DOCUMENT_HASH_TYPE.to_string()),
            name: Some(self.name.clone()),
            display_name: version.display_name.clone(),
            version_name: version.version_name.clone(),
            owner: Some(self.owner.clone()),
            created_date: Some(crate::state::wire_instant(version.created_date)),
            status: Some(version.status.clone()),
            // "A message returned by Amazon Web Services Systems Manager that explains
            // the Status value", which it sets for a failure; nothing here failed.
            status_information: None,
            document_version: Some(version.version.to_string()),
            description: version.parsed.description.clone(),
            parameters: (!version.parsed.parameters.is_empty())
                .then(|| version.parsed.parameters.clone()),
            platform_types: (!version.parsed.platform_types.is_empty())
                .then(|| version.parsed.platform_types.clone()),
            document_type: Some(self.document_type.clone()),
            schema_version: version.parsed.schema_version.clone(),
            latest_version: Some(self.latest_version().to_string()),
            default_version: Some(self.default_version.to_string()),
            document_format: Some(version.document_format.clone()),
            target_type: version.target_type.clone(),
            // "The tags, or metadata, that have been applied to the document"
            // (`DocumentDescription$Tags`, a `TagList` of length 0..1000): a document
            // carrying none has an EMPTY tag list rather than no list at all, so a caller
            // iterating `Tags` never has to tell a missing member from an empty one.
            tags: Some(tagging::to_tag_list(&self.tags)),
            attachments_information: (!version.attachment_names.is_empty()).then(|| {
                version
                    .attachment_names
                    .iter()
                    .map(|name| AttachmentInformation {
                        name: Some(name.clone()),
                    })
                    .collect()
            }),
            requires: version.requires.clone(),
            // "The user in your organization who created the document": the emulator
            // authenticates no human author, so there is none to report.
            author: None,
            // A review is the change-template approval workflow, and `UpdateDocument`
            // metadata — the operation that records one — is still a scaffold, so a
            // change template created here has not been reviewed and no other document
            // type carries a review at all.
            review_information: None,
            approved_version: None,
            pending_review_version: None,
            review_status: self.review_status(),
            // "The classification of a document to help you identify and categorize
            // its use", which Systems Manager assigns to the documents it publishes;
            // nothing classifies a document created through this API.
            category: None,
            category_enum: None,
            // "This field specifies the cloud providers that the returned version of the
            // document can run against": the version's own, which is what the call that
            // minted it named or the default it fell back to.
            cloud_providers: Some(version.cloud_providers.clone()),
        }
    }

    /// The modelled `DocumentDescription` of the version `selector` names, or `None` when
    /// this document holds no such version.
    pub fn description_of_version(&self, selector: &str) -> Option<DocumentDescription> {
        Some(self.description_of(self.select(selector)?))
    }

    /// The same description as a DESCRIBE of this document reports it: the document's
    /// classification members carried as the empty lists a document nothing classified
    /// has.
    ///
    /// `Category` is "the classification of a document to help you identify and
    /// categorize its use" and `CategoryEnum` "the value that identifies a document's
    /// category" (both `CategoryList`/`CategoryEnumList`, `@length` 0..3): Systems Manager
    /// classifies the documents it publishes, so a document created through this API
    /// carries NO category — which a describe reports as an empty list rather than as a
    /// missing member, the way it reports an untagged document's `Tags`. RECORDED
    /// BEHAVIOUR (corpus `simparity-wire-aws-v1`, fixture `ssm-association-create-and-list`
    /// step `setup#2`): the create's own response carries neither member while the describe
    /// carries both as `[]`, so this shape belongs to the describe rather than to every
    /// read.
    pub fn described_version(&self, version: &DocumentVersionRecord) -> DocumentDescription {
        DocumentDescription {
            category: Some(Vec::new()),
            category_enum: Some(Vec::new()),
            ..self.description_of(version)
        }
    }

    /// Drop the version `version` numbers from this document, reporting whether it held
    /// one. A document whose last version is dropped holds none and is removed whole by
    /// its store ([`ComAmazonawsSsmAmazonssmState::remove_document_version`]).
    pub fn remove_version(&mut self, version: u64) -> bool {
        let held = self.version(version).is_some();
        self.versions.retain(|candidate| candidate.version != version);
        held
    }
}

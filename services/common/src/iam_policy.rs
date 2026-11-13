// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of the IAM JSON policy language, for every service whose API takes a
//! policy document as a string and has to decide whether to accept it: the grammar of a
//! **resource-based** document, and the VALUES the language defines for any document's
//! elements.
//!
//! A member documented as "a policy document, formatted in JSON" is not satisfied by any
//! JSON value: it carries a document in the IAM JSON policy language, and a service that
//! only asks `serde_json` whether the text parses accepts `5`, `"hello"`, `[]` and `{}` as
//! policies and then reports them back as the resource's policy. The grammar below is what
//! separates a policy from any other JSON, stated once so two services handed the same
//! document cannot disagree about whether it is one.
//!
//! The grammar, in the IAM JSON policy reference's own order:
//!
//! * the document is a JSON **object** — the policy's top-level elements;
//! * a `Version` it declares is one of [`POLICY_VERSIONS`]; a document may omit it;
//! * `Statement` is **required** and holds either one statement object or a non-empty array
//!   of them;
//! * every statement carries an `Effect` of [`POLICY_EFFECTS`], at least one action
//!   (`Action` or its complement `NotAction`) and — because a RESOURCE-based policy is the
//!   one that states WHO the permission is granted to — at least one principal (`Principal`
//!   or its complement `NotPrincipal`).
//!
//! `Resource` is deliberately NOT required: in a resource-based policy the resource the
//! document is attached to is the implicit one, which is why IAM's resource-policy examples
//! omit it. A service whose API documents the resource element as the ARN of one of its
//! resources reads the VALUES of that element separately, through [`non_arn_resource`]:
//! the grammar here admits the wildcard the IAM language admits, and whether a particular
//! service accepts a non-ARN resource is that service's rule, not the language's.
//!
//! Which ELEMENTS a document must carry is the grammar above; which VALUES those elements
//! may declare is the reading beside it ([`identity_policy_value_defect`]) — the policy
//! versions that exist, the two effects a statement may state, and, for an IDENTITY-based
//! document, whose statements name the resources their actions apply to, resources named by
//! ARN or by the wildcard. The two are applied together by a service that STORES a policy,
//! because IAM refuses a document breaking either.
//!
//! Which NAMES a document may carry its elements under, and which of them it must carry at
//! all, is the third reading ([`attached_policy_defect`]): the validator a service runs
//! before it ATTACHES a document to one of its resources, which is stricter than the
//! grammar above in two places (the language enumerates a document's members —
//! [`POLICY_ELEMENTS`] and [`STATEMENT_ELEMENTS`] — so a name outside them describes
//! nothing a policy engine could enforce; and that validator requires a version string the
//! language itself lets a document omit). Its clauses carry AWS's own sentences, as
//! [`PolicyValueDefect`]'s do.
//!
//! This judges only the SHAPE of a document. What a policy then SAYS — whether it allows a
//! request — is [`super::iam_authorization`], and the two read the same elements in the
//! same forms (a string, a list of them, or the `{"AWS": …}` object a `Principal` is
//! written as), so a document this accepts is one that module can evaluate.
//!
//! Each caller words its own modelled fault: [`PolicyDefect`] says WHAT the document failed
//! to be and nothing about any one service's error shape, so a service that reports a
//! single opaque "invalid policy" phrase can ignore it and one that names the member can
//! render it. [`PolicyValueDefect`] is the one exception, and only because AWS words those
//! clauses identically wherever it reads a policy document: it carries IAM's own sentence
//! per clause ([`PolicyValueDefect::message`]).
//!
//! Beside the grammar, [`canonical_document`] is the one LAYOUT an attached policy document
//! is reported in: a service that stores a policy does not echo the caller's bytes but
//! re-serializes the parsed document, so the rendering lives here too rather than once per
//! service.

/// The policy-language versions an IAM JSON policy document may declare. The IAM JSON
/// policy reference documents exactly two `Version` values: `2012-10-17`, the current one,
/// and `2008-10-17`, the legacy one that no new policy should use.
pub const POLICY_VERSIONS: [&str; 2] = ["2012-10-17", "2008-10-17"];

/// The values an IAM statement's `Effect` may take — "the `Effect` element is required and
/// specifies whether the statement results in an allow or an explicit deny".
pub const POLICY_EFFECTS: [&str; 2] = ["Allow", "Deny"];

/// What a document that is not a resource-based policy failed to be.
///
/// One variant per clause of the grammar, so a caller can word a fault that says which
/// clause the document broke — or ignore it and state its own opaque phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDefect {
    /// The text is not JSON at all.
    NotJson,
    /// The text is JSON, but not an object: a scalar or an array is not a policy.
    NotAnObject,
    /// `Version` is declared as something other than one of [`POLICY_VERSIONS`].
    UnsupportedVersion,
    /// There is no `Statement`, or it holds no statement at all.
    NoStatement,
    /// A `Statement` entry is not an object.
    StatementNotAnObject,
    /// A statement declares no `Effect` of [`POLICY_EFFECTS`].
    MissingEffect,
    /// A statement names no action, by either `Action` or `NotAction`.
    MissingAction,
    /// A statement names no principal, by either `Principal` or `NotPrincipal`.
    MissingPrincipal,
}

impl PolicyDefect {
    /// The clause of the grammar this document broke, as one phrase a fault message can be
    /// built around. It describes the DOCUMENT, naming no member of any service's API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotJson => "the document is not JSON",
            Self::NotAnObject => "the document is not a JSON object",
            Self::UnsupportedVersion => "the document declares a policy version that does not exist",
            Self::NoStatement => "the document declares no statement",
            Self::StatementNotAnObject => "a statement is not a JSON object",
            Self::MissingEffect => "a statement declares no Effect of Allow or Deny",
            Self::MissingAction => "a statement names no Action or NotAction",
            Self::MissingPrincipal => "a statement names no Principal or NotPrincipal",
        }
    }
}

impl ::std::fmt::Display for PolicyDefect {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The clause of the resource-policy grammar `text` breaks, or `None` when `text` is a
/// resource-based policy document.
///
/// Parsing is `serde_json`'s; only the grammar on top of it is stated here. Nothing about a
/// document's SIZE is judged: a length bound belongs to the member that carries it, which
/// the framework's boundary validator already enforces from the model.
pub fn resource_policy_defect(text: &str) -> Option<PolicyDefect> {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(text) else {
        return Some(PolicyDefect::NotJson);
    };
    resource_policy_defect_of(&document)
}

/// [`resource_policy_defect`] for a document that is already parsed — for a caller that
/// parsed it to read something else out of it and must not parse it twice.
pub fn resource_policy_defect_of(document: &::serde_json::Value) -> Option<PolicyDefect> {
    if !document.is_object() {
        return Some(PolicyDefect::NotAnObject);
    }

    if declares_unsupported_version(document) {
        return Some(PolicyDefect::UnsupportedVersion);
    }

    let statements = statements(document);
    if statements.is_empty() {
        return Some(PolicyDefect::NoStatement);
    }

    statements.into_iter().find_map(statement_defect)
}

/// Whether `document` declares a `Version` that is not one of [`POLICY_VERSIONS`] — the one
/// reading of that clause, so the two readers of a document cannot come to differ about
/// which versions exist. A document declaring none is not it: `Version` may be omitted.
fn declares_unsupported_version(document: &::serde_json::Value) -> bool {
    document
        .get("Version")
        .is_some_and(|version| !POLICY_VERSIONS.contains(&version.as_str().unwrap_or_default()))
}

/// The prefix every Amazon Resource Name begins with, and so the one thing a value has to
/// carry to be read as an ARN at all.
pub const ARN_PREFIX: &str = "arn:";

/// The first value the statements' resource elements name that is not an ARN, or `None`
/// when every value they name is one — which includes a document naming no resource at
/// all, and a text that is not a policy.
///
/// `Resource` and its complement `NotResource` name RESOURCES, and a resource is named by
/// its ARN, so a service whose API documents that element as the ARN of one of its own
/// resources reads the values here: the IAM language admits a bare `*` there, which is a
/// wildcard and not an ARN, so this reading is separate from the grammar above and is
/// applied beside [`resource_policy_defect`] rather than instead of it. The value is
/// reported as submitted, so a caller can name it in the refusal it words.
pub fn non_arn_resource(text: &str) -> Option<String> {
    let document = ::serde_json::from_str::<::serde_json::Value>(text).ok()?;
    statements(&document)
        .into_iter()
        .flat_map(resource_values)
        .find(|value| !value.starts_with(ARN_PREFIX))
}

/// The statements `document` declares: the entries of its `Statement` list, or the one
/// statement written as the object itself rather than as a list. Empty for a document
/// that is not an object or declares no statement.
///
/// `pub(crate)` so the reading of WHO a document's grants reach
/// ([`super::public_resource_policy`]) walks the same statements this grammar does, rather
/// than carrying a second reading of that element.
pub(crate) fn statements(document: &::serde_json::Value) -> Vec<&::serde_json::Value> {
    match document.get("Statement") {
        Some(::serde_json::Value::Array(entries)) => entries.iter().collect(),
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement],
        _ => Vec::new(),
    }
}

/// Every value one statement's resource elements name, in the order they are written.
fn resource_values(statement: &::serde_json::Value) -> Vec<String> {
    ["Resource", "NotResource"]
        .into_iter()
        .filter_map(|key| statement.get(key))
        .flat_map(element_values)
        .collect()
}

/// `element`'s values: a policy element is written as one string or as a list of them,
/// and anything else names no value.
fn element_values(element: &::serde_json::Value) -> Vec<String> {
    match element {
        ::serde_json::Value::String(value) => vec![value.clone()],
        ::serde_json::Value::Array(entries) => entries.iter().flat_map(element_values).collect(),
        _ => Vec::new(),
    }
}

/// The clause one statement breaks, or `None` when it is a resource-policy statement.
fn statement_defect(statement: &::serde_json::Value) -> Option<PolicyDefect> {
    let Some(statement) = statement.as_object() else {
        return Some(PolicyDefect::StatementNotAnObject);
    };
    let effect = statement
        .get("Effect")
        .and_then(::serde_json::Value::as_str)
        .unwrap_or_default();
    if !POLICY_EFFECTS.contains(&effect) {
        return Some(PolicyDefect::MissingEffect);
    }
    let names = |keys: [&str; 2]| {
        keys.iter()
            .any(|key| statement.get(*key).is_some_and(names_values))
    };
    if !names(["Action", "NotAction"]) {
        return Some(PolicyDefect::MissingAction);
    }
    if !names(["Principal", "NotPrincipal"]) {
        return Some(PolicyDefect::MissingPrincipal);
    }
    None
}

/// Whether a policy element names at least one value: a non-empty string, a non-empty array
/// of them, or — as `Principal` is written — a non-empty object mapping a principal type to
/// either of those.
fn names_values(element: &::serde_json::Value) -> bool {
    match element {
        ::serde_json::Value::String(value) => !value.is_empty(),
        ::serde_json::Value::Array(entries) => {
            !entries.is_empty() && entries.iter().all(names_values)
        }
        ::serde_json::Value::Object(members) => {
            !members.is_empty() && members.values().all(names_values)
        }
        _ => false,
    }
}

/// The member names the IAM JSON policy language defines at a document's TOP level, in the
/// policy reference's own order: the policy-language version, the document's optional id,
/// and the statements themselves.
pub const POLICY_ELEMENTS: [&str; 3] = ["Version", "Id", "Statement"];

/// The member names the language defines INSIDE a statement, in the policy reference's own
/// order: the statement id, the effect, the principal and its complement, the action and
/// its complement, the resource and its complement, and the condition block.
pub const STATEMENT_ELEMENTS: [&str; 9] = [
    "Sid",
    "Effect",
    "Principal",
    "NotPrincipal",
    "Action",
    "NotAction",
    "Resource",
    "NotResource",
    "Condition",
];

/// The one top-level element the IAM JSON policy language documents as REQUIRED: the
/// `Statement` element, which holds the permissions the document grants. `Version` and `Id`
/// are both optional, so this is the only name a policy object must carry.
pub const REQUIRED_POLICY_ELEMENT: &str = "Statement";

/// What a document a service ATTACHES to one of its resources failed to be.
///
/// The reading of the validator those services run, stated as one clause per variant in the
/// order that validator reads them. It is stricter than the grammar above in two places,
/// because that validator is: the document's members must be names the language DEFINES
/// (the grammar reads which elements are present, not whether a name is one at all), and it
/// must declare a version string (the language lets a document omit `Version`, which is why
/// [`PolicyDefect`] does not require one). Every other clause the grammar states — a
/// statement's effect, its action, its principal — is [`resource_policy_defect`]'s, applied
/// beside this one by a service that wants it.
///
/// Unlike [`PolicyDefect`], each variant carries AWS's own sentence for its clause
/// ([`Self::message`]), because AWS words these identically wherever a service attaches a
/// policy document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachedPolicyDefect {
    /// The text does not open a JSON object, or is not JSON at all: a policy document's
    /// top-level elements are an object's members, so a text opening with anything else —
    /// an array, a scalar, whitespace, or no byte at all — carries no policy.
    NotAPolicyObject,
    /// A member name the language does not define, at the document's top level
    /// ([`POLICY_ELEMENTS`]) or inside one of its statements ([`STATEMENT_ELEMENTS`]),
    /// carried as submitted because AWS's sentence for this clause names it.
    UnknownElement(String),
    /// The document carries none of the element the language requires
    /// ([`REQUIRED_POLICY_ELEMENT`]). A document carrying an EMPTY statement list carries
    /// that element, so it is not missing it — whether the element names any statement is
    /// [`PolicyDefect::NoStatement`].
    MissingElement(&'static str),
    /// The document declares no `Version` of [`POLICY_VERSIONS`] — including declaring none
    /// at all.
    UndefinedVersion,
    /// An action element names a value the language cannot read as an action
    /// ([`readable_action`]), or that the owning service cannot grant.
    UnreadableAction,
}

impl AttachedPolicyDefect {
    /// How AWS words this clause in the malformed-document refusal it answers a document
    /// breaking it with. Only the two element clauses carry anything of the document — the
    /// member name they name; the rest is fixed text.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NotAPolicyObject => NOT_A_JSON_POLICY_MESSAGE.to_string(),
            Self::UnknownElement(name) => format!("Unknown field {name}"),
            Self::MissingElement(name) => format!("Missing required field {name}"),
            Self::UndefinedVersion => INVALID_POLICY_VERSION_MESSAGE.to_string(),
            Self::UnreadableAction => POLICY_INVALID_ACTION_MESSAGE.to_string(),
        }
    }
}

/// The sentence AWS words the refusal of a text that is not a JSON policy object with:
/// fixed text, carrying nothing of the submitted document. The first BYTE is what it states,
/// which is why that is what [`attached_policy_defect`] reads.
pub const NOT_A_JSON_POLICY_MESSAGE: &str =
    "Policies must be valid JSON and the first byte must be '{'";

/// The sentence AWS words the refusal of a document declaring no version the policy
/// language defines with: fixed text, carrying nothing of the submitted document.
pub const INVALID_POLICY_VERSION_MESSAGE: &str = "The policy must contain a valid version string";

/// The sentence AWS words the refusal of a document naming an action it will not read with:
/// fixed text, carrying nothing of the submitted document.
pub const POLICY_INVALID_ACTION_MESSAGE: &str = "Policy has invalid action";

/// The first clause of [`AttachedPolicyDefect`] `text` breaks, or `None` for a document the
/// attaching service stores.
///
/// `grantable` is the OWNING service's reading of which actions its resource can grant — an
/// access point grants its own API's actions, and which namespaces those are is that
/// service's rule rather than the language's, so it is passed in and applied beside the
/// language's own reading of an action's form ([`readable_action`]). Pass
/// `|_| true` to read the form alone.
///
/// The document is parsed ONCE here and every clause read off the parsed value, so a
/// service applying the whole reading does not re-parse it per clause.
pub fn attached_policy_defect(
    text: &str,
    grantable: impl Fn(&str) -> bool,
) -> Option<AttachedPolicyDefect> {
    if text.as_bytes().first() != Some(&b'{') {
        return Some(AttachedPolicyDefect::NotAPolicyObject);
    }
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(text) else {
        return Some(AttachedPolicyDefect::NotAPolicyObject);
    };
    // The names a document carries. Only the two levels the language ENUMERATES are read:
    // the names inside a `Condition` block are condition operators and condition keys, and
    // the names inside a `Principal` are principal types, neither of which the language
    // fixes.
    let statements = statements(&document);
    if let Some(name) = undefined_element_of(&document, &POLICY_ELEMENTS).or_else(|| {
        statements
            .iter()
            .find_map(|statement| undefined_element_of(statement, &STATEMENT_ELEMENTS))
    }) {
        return Some(AttachedPolicyDefect::UnknownElement(name));
    }
    if !document
        .as_object()
        .is_some_and(|members| members.contains_key(REQUIRED_POLICY_ELEMENT))
    {
        return Some(AttachedPolicyDefect::MissingElement(
            REQUIRED_POLICY_ELEMENT,
        ));
    }
    // The version clause, read through the grammar's own reading of which versions exist
    // plus the one thing this validator adds: a document declaring none declares no valid
    // one either.
    if document.get("Version").is_none() || declares_unsupported_version(&document) {
        return Some(AttachedPolicyDefect::UndefinedVersion);
    }
    let unreadable = statements.iter().any(|statement| {
        ACTION_ELEMENTS
            .into_iter()
            .any(|key| statement.get(key).is_some_and(element_names_no_action))
    });
    let ungrantable = statements
        .iter()
        .flat_map(|statement| {
            ACTION_ELEMENTS
                .into_iter()
                .filter_map(|key| statement.get(key))
                .flat_map(element_values)
                .collect::<Vec<String>>()
        })
        .any(|action| !grantable(&action));
    (unreadable || ungrantable).then_some(AttachedPolicyDefect::UnreadableAction)
}

/// The first member name `value` carries that is not one of `defined`, or `None` for a
/// value that carries only defined names — or that is not an object at all.
fn undefined_element_of(value: &::serde_json::Value, defined: &[&str]) -> Option<String> {
    value
        .as_object()?
        .keys()
        .find(|name| !defined.contains(&name.as_str()))
        .cloned()
}

/// The statement elements that name ACTIONS: the action element and its complement.
const ACTION_ELEMENTS: [&str; 2] = ["Action", "NotAction"];

/// The one value an IAM action element may name BESIDE a `<service>:<action>` pair: the
/// wildcard that stands for every action, which the language admits wherever an action is
/// named.
pub const ANY_ACTION: &str = "*";

/// Whether `value` is an action the IAM policy language can READ: [`ANY_ACTION`], or a
/// `<service>:<action>` pair whose service namespace and action name are both non-empty
/// and written in the characters each is written in.
///
/// A service namespace is written in lowercase letters, digits and hyphens (`s3`,
/// `s3-object-lambda`, `elasticloadbalancing`); an action name is written in letters and
/// digits, and may carry the `*` and `?` wildcards the language admits inside it
/// (`s3:Get*`). Neither half is read for CASE — IAM reads an action name
/// case-insensitively — and neither is read against which actions a service publishes:
/// that catalogue is the service's, not the language's.
#[must_use]
pub fn readable_action(value: &str) -> bool {
    if value == ANY_ACTION {
        return true;
    }
    let Some((namespace, action)) = value.split_once(':') else {
        return false;
    };
    let namespace_character = |character: char| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    };
    let action_character =
        |character: char| character.is_ascii_alphanumeric() || matches!(character, '*' | '?');
    !namespace.is_empty()
        && namespace.chars().all(namespace_character)
        && !action.is_empty()
        && action.chars().all(action_character)
}

/// Whether `element` — an action element that is present — names something other than one
/// action or a non-empty list of them.
fn element_names_no_action(element: &::serde_json::Value) -> bool {
    match element {
        ::serde_json::Value::String(value) => !readable_action(value),
        ::serde_json::Value::Array(entries) => {
            entries.is_empty() || entries.iter().any(element_names_no_action)
        }
        _ => true,
    }
}

/// The one value an IAM resource element may name BESIDE an ARN: the wildcard that stands
/// for every resource, which the IAM policy language admits wherever a resource is named.
pub const ANY_RESOURCE: &str = "*";

/// The sentence IAM words a document whose declared policy-language version does not exist
/// with: the version is read before anything the document says, so a document declaring one
/// the language does not define is reported as an error in the policy's syntax and nothing
/// of the document is named.
pub const SYNTAX_ERRORS_IN_POLICY: &str = "Syntax errors in policy.";

/// The sentence IAM words a statement whose `Effect` the policy language does not define
/// with: the document is JSON, but reading it as a policy fails, and the refusal carries
/// nothing of the document.
pub const POLICY_PARSE_FAILED: &str = "The policy failed legacy parsing";

/// What a document declares that the IAM policy language does not define.
///
/// [`PolicyDefect`] is about the ELEMENTS a document carries; this is about the VALUES those
/// elements declare, which IAM reads against the language before it accepts the document.
/// Each clause is worded with a sentence of its own ([`Self::message`]), so the refusal a
/// caller answers names the clause the document broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyValueDefect {
    /// `Version` is declared as something other than one of [`POLICY_VERSIONS`].
    UnsupportedVersion,
    /// A statement declares an `Effect` that is not one of [`POLICY_EFFECTS`].
    ///
    /// The two values are compared LITERALLY: the language defines `Allow` and `Deny`, so a
    /// statement whose effect is spelled in any other case declares an effect the language
    /// does not define. A statement that declares NO effect is not this clause — what a
    /// statement is REQUIRED to carry is [`PolicyDefect::MissingEffect`].
    UnreadableEffect,
    /// A statement's resource element names a value that is neither an ARN
    /// ([`ARN_PREFIX`]) nor [`ANY_RESOURCE`], so it names no resource. The value is carried
    /// as submitted, because IAM's sentence for this clause names it.
    NonResourceValue(String),
}

impl PolicyValueDefect {
    /// How IAM words this clause in the malformed-document refusal it answers a document
    /// breaking it with — AWS's own sentence per clause, as the recorded refusals of an
    /// inline policy write show them. Only the resource clause carries anything of the
    /// document: the other two are fixed text.
    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedVersion => SYNTAX_ERRORS_IN_POLICY.to_string(),
            Self::UnreadableEffect => POLICY_PARSE_FAILED.to_string(),
            Self::NonResourceValue(value) => {
                format!("Resource {value} must be in ARN format or \"{ANY_RESOURCE}\".")
            }
        }
    }
}

/// The clause of the IAM policy language an IDENTITY-based policy document declares a value
/// outside, or `None` when every value it declares is one the language defines.
///
/// A member documented as a policy document carries a document in the IAM JSON policy
/// language, and the language defines which values its elements may take: the policy
/// versions that exist ([`POLICY_VERSIONS`]), the two effects a statement may state
/// ([`POLICY_EFFECTS`]) and — for an identity-based policy, whose statements name the
/// resources their actions apply to — resources named by ARN or by the wildcard
/// ([`ANY_RESOURCE`]). A document that declares a value outside one of those describes no
/// permission IAM could enforce, so the operation taking it is refused rather than storing
/// it.
///
/// This reads the values; which elements a document must CARRY is the grammar above
/// ([`resource_policy_defect`] for a resource-based policy, and the owning service's own
/// requirement for an identity-based one), so the two are applied beside each other. A text
/// that is no JSON object at all declares nothing to read and is reported here as `None`:
/// that it is not a policy at all is the grammar's answer, not this one's.
pub fn identity_policy_value_defect(text: &str) -> Option<PolicyValueDefect> {
    let document = ::serde_json::from_str::<::serde_json::Value>(text).ok()?;
    if !document.is_object() {
        return None;
    }
    if declares_unsupported_version(&document) {
        return Some(PolicyValueDefect::UnsupportedVersion);
    }
    statements(&document)
        .into_iter()
        .find_map(identity_statement_value_defect)
}

/// The clause one statement of an identity-based policy declares a value outside.
///
/// The two clauses read here are about what the statement DECLARES, which is why neither
/// is the grammar's reading of the same element: a statement declaring NO effect is left to
/// the grammar ([`PolicyDefect::MissingEffect`]) rather than reported here, and a resource
/// element is read admitting the wildcard the language admits, where
/// [`non_arn_resource`] — the reading a service whose own API documents that element as one
/// of its ARNs asks — admits only an ARN.
fn identity_statement_value_defect(statement: &::serde_json::Value) -> Option<PolicyValueDefect> {
    if let Some(effect) = statement.as_object()?.get("Effect") {
        if !POLICY_EFFECTS.contains(&effect.as_str().unwrap_or_default()) {
            return Some(PolicyValueDefect::UnreadableEffect);
        }
    }
    // The resource elements are read through the one walk of them this module already
    // owns, so which spellings name a resource is stated once.
    resource_values(statement)
        .into_iter()
        .find(|value| !value.starts_with(ARN_PREFIX) && value != ANY_RESOURCE)
        .map(PolicyValueDefect::NonResourceValue)
}

/// The indentation one nesting level of a canonically rendered policy document is written
/// with: see [`canonical_document`].
const CANONICAL_INDENT: usize = 2;

/// What separates a canonically rendered document's key from its value: a colon with a
/// space on BOTH sides (`"Version" : "2012-10-17"`). See [`canonical_document`].
const CANONICAL_KEY_SEPARATOR: &str = " : ";

/// How many levels of nesting [`canonical_document`] renders.
///
/// The walk descends once per enclosing object or array of the submitted text, and that
/// nesting is the caller's, so it is bounded here rather than in whatever parsed the
/// document first: 128 levels, the depth a maintained JSON parser (`serde_json`) accepts
/// before it refuses a document for its nesting, which is far past any policy the IAM
/// grammar describes (a document, its statement list, a statement, a condition operator
/// and its key is five). Text nested past it is returned unrendered.
const MAX_CANONICAL_NESTING: usize = 128;

/// An IAM policy document in the layout the services that attach one REPORT it in: the
/// canonical rendering their policy engine re-serializes a stored document into, whatever
/// layout the caller submitted.
///
/// A service whose API takes a policy as a string does not echo the caller's bytes back: it
/// parses the document and renders it again on every read and write, so two clients that
/// submit the same policy written differently read the same text back. The layout, as the
/// recorded responses of those operations show it:
///
/// * each member of a JSON object on its own line, indented [`CANONICAL_INDENT`] spaces per
///   enclosing OBJECT (an enclosing array adds no indentation), with the closing brace on
///   its own line at the enclosing level;
/// * [`CANONICAL_KEY_SEPARATOR`] between a key and its value;
/// * arrays laid out inline — `[ ` before the first element, `, ` between elements and
///   ` ]` after the last — so a statement list reads `"Statement" : [ {` … `} ]`;
/// * an empty object and an empty array written `{ }` and `[ ]`;
/// * a VALUE LIST holding exactly one value written as that value alone, without the
///   brackets: the IAM policy language documents `Action`, `NotAction`, `Resource`,
///   `NotResource`, the per-type lists of `Principal`/`NotPrincipal` and a condition key's
///   values as "a string or an array of strings", and a one-value list of those is reported
///   in its string form. A list of STATEMENTS is not a value list, so a `Statement` carrying
///   one statement keeps its brackets — which is the difference a one-element array of
///   strings makes against one of objects.
///
/// Only the whitespace BETWEEN tokens, and the brackets of a one-value list, are this
/// function's: every token is copied out of the submitted text byte for byte, so nothing
/// else about the document's content changes. That is what keeps the caller's document the
/// caller's — member order survives, a number keeps the digits it was written with, and
/// every string literal keeps the escapes it was written with (`\u00e9` is not decoded, and
/// a literal non-ASCII character is not escaped). The rendering is therefore a fixed point:
/// re-rendering a rendered document returns it unchanged, so a client that puts back what it
/// read sees no drift.
///
/// `text` that is not well-formed JSON is returned as it came, and so is text nested past
/// [`MAX_CANONICAL_NESTING`]: this is a rendering, not a validation, and the policy grammar
/// ([`resource_policy_defect`]) is what refuses an unusable document.
pub fn canonical_document(text: &str) -> String {
    render_canonically(text, false)
}

/// [`canonical_document`], plus the one thing a policy ENGINE settles that re-indenting does
/// not: every value inside a statement's `Condition` is written as a **string**.
///
/// The policy language defines a condition's values as strings — "a string or an array of
/// strings" for a condition key's values, the same form `Action` and `Resource` take — so a
/// service whose policy engine parses the document and re-serializes it reports a value the
/// caller wrote as a JSON number or boolean in its string form (`300` comes back `"300"`,
/// `true` comes back `"true"`), and a one-value list of them collapses to that string like
/// any other value list. `null` is left as it was written: the language gives it no string
/// form.
///
/// Everything else is [`canonical_document`] exactly — member order, the statement list,
/// each string literal's own escapes, and the characters a number OUTSIDE a condition was
/// written with — because this is the SAME walk with one token rewritten rather than a
/// second reading of the document.
///
/// Taken by a service whose recorded reads show that coercion; a service whose reads report
/// the caller's own JSON types takes [`canonical_document`].
pub fn canonical_document_with_condition_values_as_strings(text: &str) -> String {
    render_canonically(text, true)
}

/// The rendering both entry points above share: the one walk, with or without the condition
/// coercion.
fn render_canonically(text: &str, condition_values_as_strings: bool) -> String {
    let mut renderer = PolicyRenderer {
        source: text,
        at: 0,
        nesting: 0,
        condition_values_as_strings,
        rendered: String::with_capacity(text.len() + text.len() / 2),
    };
    if renderer.render() {
        renderer.rendered
    } else {
        text.to_string()
    }
}

/// The element whose value is a statement's condition block: inside it, a condition's values
/// are the strings the policy language defines them as (see
/// [`canonical_document_with_condition_values_as_strings`]).
const CONDITION_ELEMENT: &str = "Condition";

/// The re-indenter behind [`canonical_document`]: walks the submitted document a token at a
/// time, copying every token verbatim into `rendered` and writing the canonical whitespace
/// between them.
struct PolicyRenderer<'a> {
    /// The document being rendered.
    source: &'a str,
    /// How far into `source` the walk has read. Only ever advanced over ASCII bytes or over
    /// a whole token, so it is always a character boundary.
    at: usize,
    /// How many enclosing objects and arrays the walk is inside, which is what
    /// [`MAX_CANONICAL_NESTING`] bounds. Counted separately from the INDENTATION depth,
    /// because an array adds a level of recursion while adding no indentation.
    nesting: usize,
    /// Whether a condition's values are rendered as strings
    /// ([`canonical_document_with_condition_values_as_strings`]).
    condition_values_as_strings: bool,
    /// What has been rendered so far.
    rendered: String,
}

impl<'a> PolicyRenderer<'a> {
    /// Render the whole document, reporting whether it was one JSON value and nothing else.
    /// `false` leaves `rendered` half-written, which [`canonical_document`] discards.
    fn render(&mut self) -> bool {
        if self.value(0, false).is_none() {
            return false;
        }
        self.skip_space();
        self.at == self.source.len()
    }

    /// The next byte of `source`, or `None` at its end.
    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.at).copied()
    }

    /// Step over the whitespace between two tokens: insignificant, and the whitespace this
    /// renderer replaces.
    fn skip_space(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    /// Step over `expected`, or report that the document does not carry it here.
    fn expect(&mut self, expected: u8) -> Option<()> {
        if self.peek()? != expected {
            return None;
        }
        self.at += 1;
        Some(())
    }

    /// Write a line break and the indentation `depth` enclosing objects are written with.
    fn newline(&mut self, depth: usize) {
        self.rendered.push('\n');
        for _ in 0..depth * CANONICAL_INDENT {
            self.rendered.push(' ');
        }
    }

    /// Render one JSON value, whose own members sit `depth` objects deep. `in_condition` says
    /// whether it sits inside a statement's condition block, where a scalar may be rendered
    /// as a string (see [`canonical_document_with_condition_values_as_strings`]).
    ///
    /// The one place the walk recurses, so the one place the nesting a document may carry
    /// is bounded ([`MAX_CANONICAL_NESTING`]): past it the render fails and
    /// [`canonical_document`] hands the text back unrendered, which is what keeps a
    /// caller-supplied run of opening brackets off the stack.
    fn value(&mut self, depth: usize, in_condition: bool) -> Option<()> {
        self.skip_space();
        if self.nesting == MAX_CANONICAL_NESTING {
            return None;
        }
        self.nesting += 1;
        let rendered = match self.peek()? {
            // An object's members are one level deeper than the object itself.
            b'{' => self.object(depth + 1, in_condition),
            // An array is inline, so it adds no level of its own.
            b'[' => self.array(depth, in_condition),
            b'"' => self.string().map(drop),
            _ => self.scalar(in_condition),
        };
        self.nesting -= 1;
        rendered
    }

    /// Render a JSON object whose members are written at `depth`. A member keyed
    /// [`CONDITION_ELEMENT`] opens a condition block, which every value under it is inside.
    fn object(&mut self, depth: usize, in_condition: bool) -> Option<()> {
        self.expect(b'{')?;
        self.rendered.push('{');
        self.skip_space();
        if self.peek()? == b'}' {
            self.at += 1;
            self.rendered.push_str(" }");
            return Some(());
        }
        loop {
            self.skip_space();
            self.newline(depth);
            let key = self.string()?;
            let inside = in_condition || key == CONDITION_ELEMENT;
            self.skip_space();
            self.expect(b':')?;
            self.rendered.push_str(CANONICAL_KEY_SEPARATOR);
            self.value(depth, inside)?;
            self.skip_space();
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.rendered.push(',');
                }
                b'}' => {
                    self.at += 1;
                    self.newline(depth - 1);
                    self.rendered.push('}');
                    return Some(());
                }
                _ => return None,
            }
        }
    }

    /// Render a JSON array inline, its elements' own members staying at `depth` — or, when
    /// it is a value list holding exactly one value, as that value alone (see
    /// [`canonical_document`]).
    fn array(&mut self, depth: usize, in_condition: bool) -> Option<()> {
        self.expect(b'[')?;
        self.skip_space();
        if self.peek()? == b']' {
            self.at += 1;
            self.rendered.push_str("[ ]");
            return Some(());
        }
        // Where this array's rendering begins: its brackets are written only once it is
        // known not to be a one-value list, which is rendered without them.
        let opens_at = self.rendered.len();
        let opened_with = self.peek()?;
        self.value(depth, in_condition)?;
        self.skip_space();
        // A VALUE list is one whose element is a string \u2014 as written, or as a condition's
        // own value was coerced to ([`canonical_document_with_condition_values_as_strings`]),
        // which is what makes a one-value list of them collapse like any other.
        let holds_values = opened_with == b'"'
            || (self.condition_values_as_strings
                && in_condition
                && self.rendered.as_bytes().get(opens_at) == Some(&b'"'));
        if holds_values && self.peek()? == b']' {
            self.at += 1;
            return Some(());
        }
        self.rendered.insert_str(opens_at, "[ ");
        loop {
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.rendered.push_str(", ");
                }
                b']' => {
                    self.at += 1;
                    self.rendered.push_str(" ]");
                    return Some(());
                }
                _ => return None,
            }
            self.value(depth, in_condition)?;
            self.skip_space();
        }
    }

    /// Copy a string literal — a key's or a value's — out verbatim, escapes and all, and
    /// answer the text between its quotes so a caller can tell which element it keys. A
    /// UTF-8 continuation byte is never one of the ASCII bytes scanned for, so reading the
    /// literal byte by byte cannot split a character.
    fn string(&mut self) -> Option<&'a str> {
        let source = self.source;
        if self.peek()? != b'"' {
            return None;
        }
        let start = self.at;
        self.at += 1;
        let mut escaped = false;
        loop {
            let byte = self.peek()?;
            self.at += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                break;
            }
        }
        self.rendered.push_str(&source[start..self.at]);
        Some(&source[start + 1..self.at - 1])
    }

    /// Copy a number, `true`, `false` or `null` out verbatim: a token that runs up to the
    /// next structural character.
    ///
    /// Inside a condition block, and only for a caller that asked for it
    /// ([`canonical_document_with_condition_values_as_strings`]), a number or a boolean is
    /// written back QUOTED — the string form the policy language gives a condition's values.
    /// `null` has no string form, so it is copied through as it is.
    fn scalar(&mut self, in_condition: bool) -> Option<()> {
        let source = self.source;
        let start = self.at;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b'.'))
        {
            self.at += 1;
        }
        if self.at == start {
            return None;
        }
        let token = &source[start..self.at];
        let coerced = self.condition_values_as_strings
            && in_condition
            && (matches!(token, "true" | "false") || token.parse::<f64>().is_ok());
        if coerced {
            self.rendered.push('"');
            self.rendered.push_str(token);
            self.rendered.push('"');
        } else {
            self.rendered.push_str(token);
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource-based policy with one statement, written as a client sends it.
    fn policy(statement: &str) -> String {
        format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement}]}}")
    }

    /// The statement every accepting row starts from: an `Effect`, an `Action` and a
    /// `Principal`.
    const STATEMENT: &str = "{\"Effect\":\"Allow\",\"Action\":\"svc:DoThing\",\
                             \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}";

    #[test]
    fn the_grammar_separates_a_resource_policy_from_any_other_json() {
        // The rows differ along what the rule generalises over: the JSON type of the
        // document, the form `Statement` takes (one object or a list), the number of
        // statements, which element is missing, and the form each element is written in.
        let rows: [(&str, String, Option<PolicyDefect>); 20] = [
            ("one statement in a list", policy(STATEMENT), None),
            (
                "two statements",
                policy(&format!("{STATEMENT},{STATEMENT}")),
                None,
            ),
            (
                "one statement written as the object itself",
                format!("{{\"Version\":\"2012-10-17\",\"Statement\":{STATEMENT}}}"),
                None,
            ),
            (
                "no Version at all, which a policy may omit",
                format!("{{\"Statement\":[{STATEMENT}]}}"),
                None,
            ),
            (
                "the legacy policy version",
                format!("{{\"Version\":\"2008-10-17\",\"Statement\":[{STATEMENT}]}}"),
                None,
            ),
            (
                "an Action list and a Principal string",
                policy(
                    "{\"Effect\":\"Deny\",\"Action\":[\"svc:A\",\"svc:B\"],\
                     \"Principal\":\"*\"}",
                ),
                None,
            ),
            (
                "the complements of Action and Principal",
                policy(
                    "{\"Effect\":\"Deny\",\"NotAction\":\"svc:A\",\
                     \"NotPrincipal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\"]}}",
                ),
                None,
            ),
            (
                "a statement carrying a Sid, a Resource and a Condition too",
                policy(
                    "{\"Sid\":\"one\",\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"Principal\":\"*\",\"Resource\":\"*\",\
                     \"Condition\":{\"StringEquals\":{\"aws:PrincipalOrgID\":\"o-1\"}}}",
                ),
                None,
            ),
            ("a bare number", "5".to_string(), Some(PolicyDefect::NotAnObject)),
            (
                "a bare string",
                "\"hello\"".to_string(),
                Some(PolicyDefect::NotAnObject),
            ),
            ("an empty array", "[]".to_string(), Some(PolicyDefect::NotAnObject)),
            (
                "an array of statements with no document around them",
                format!("[{STATEMENT}]"),
                Some(PolicyDefect::NotAnObject),
            ),
            ("null", "null".to_string(), Some(PolicyDefect::NotAnObject)),
            (
                "an empty object",
                "{}".to_string(),
                Some(PolicyDefect::NoStatement),
            ),
            (
                "a Statement that is an empty list",
                "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
                Some(PolicyDefect::NoStatement),
            ),
            (
                "a Statement that is a string",
                "{\"Statement\":\"let me in\"}".to_string(),
                Some(PolicyDefect::NoStatement),
            ),
            (
                "a version no policy language has",
                format!("{{\"Version\":\"2024-01-01\",\"Statement\":[{STATEMENT}]}}"),
                Some(PolicyDefect::UnsupportedVersion),
            ),
            (
                "a statement with no Effect",
                policy("{\"Action\":\"svc:A\",\"Principal\":\"*\"}"),
                Some(PolicyDefect::MissingEffect),
            ),
            (
                "a statement with no Action",
                policy("{\"Effect\":\"Allow\",\"Principal\":\"*\"}"),
                Some(PolicyDefect::MissingAction),
            ),
            (
                "a statement with no Principal",
                policy("{\"Effect\":\"Allow\",\"Action\":\"svc:A\"}"),
                Some(PolicyDefect::MissingPrincipal),
            ),
        ];

        for (case, document, expected) in rows {
            let defect = resource_policy_defect(&document);
            assert_eq!(
                defect, expected,
                "case {case:?}: expected {expected:?}, got {defect:?} for {document}"
            );
        }
    }

    #[test]
    fn a_defect_is_reported_for_a_statement_wherever_it_sits_in_the_list() {
        // The grammar holds of EVERY statement, not just the first, so the same broken
        // statement is caught at each position of the list.
        let broken = "{\"Effect\":\"Allow\",\"Action\":\"svc:A\"}";
        for (case, document) in [
            ("broken first", policy(&format!("{broken},{STATEMENT}"))),
            ("broken last", policy(&format!("{STATEMENT},{broken}"))),
            (
                "broken in the middle",
                policy(&format!("{STATEMENT},{broken},{STATEMENT}")),
            ),
        ] {
            let defect = resource_policy_defect(&document);
            assert_eq!(
                defect,
                Some(PolicyDefect::MissingPrincipal),
                "case {case:?}: every statement is judged, got {defect:?}"
            );
        }
    }

    #[test]
    fn the_first_resource_value_that_is_not_an_arn_is_reported() {
        // The rows differ along what the rule generalises over: which resource element
        // carries the value, the form it is written in (a string or a list), how many
        // statements the document has and where in them the value sits, and the kind of
        // value that is not an ARN (a wildcard, a bare name, a glob, a value whose ARN
        // prefix is misspelled).
        let identity = "arn:aws:svc:us-east-1:000000000000:identity/example.com";
        let rows: [(&str, String, Option<&str>); 14] = [
            ("no resource element at all", policy(STATEMENT), None),
            (
                "one resource named by its ARN",
                policy(&format!(
                    "{{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":\"{identity}\"}}"
                )),
                None,
            ),
            (
                "a list of ARNs",
                policy(&format!(
                    "{{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":[\"{identity}\",\"{identity}/other\"]}}"
                )),
                None,
            ),
            (
                "an ARN of another partition",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":\"arn:aws-cn:svc:cn-north-1:000000000000:identity/a\"}",
                ),
                None,
            ),
            (
                "a bare wildcard",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":\"*\"}",
                ),
                Some("*"),
            ),
            (
                "a bare resource name",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":\"example.com\"}",
                ),
                Some("example.com"),
            ),
            (
                "a glob over a resource path",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":\"identity/*\"}",
                ),
                Some("identity/*"),
            ),
            (
                "an ARN prefix that is misspelled",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":\"ARN:aws:svc:::thing\"}",
                ),
                Some("ARN:aws:svc:::thing"),
            ),
            (
                "the wildcard second in a list of resources",
                policy(&format!(
                    "{{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":[\"{identity}\",\"*\"]}}"
                )),
                Some("*"),
            ),
            (
                "a NotResource that is not an ARN",
                policy(
                    "{\"Effect\":\"Deny\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"NotResource\":\"*\"}",
                ),
                Some("*"),
            ),
            (
                "the second statement carrying the wildcard",
                policy(&format!(
                    "{{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                     \"Resource\":\"{identity}\"}},\
                     {{\"Effect\":\"Allow\",\"Action\":\"svc:B\",\"Principal\":\"*\",\
                     \"Resource\":\"everything\"}}"
                )),
                Some("everything"),
            ),
            (
                "one statement written as the object itself",
                "{\"Version\":\"2012-10-17\",\"Statement\":\
                 {\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":\"*\",\
                 \"Resource\":\"*\"}}"
                    .to_string(),
                Some("*"),
            ),
            // A text that is not a policy names no resource: whether it is a policy at
            // all is `resource_policy_defect`'s reading, not this one.
            ("text that is not JSON", "not a policy".to_string(), None),
            ("an empty object", "{}".to_string(), None),
        ];

        for (case, document, expected) in rows {
            let offending = non_arn_resource(&document);
            assert_eq!(
                offending.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {offending:?} for {document}"
            );
        }
    }

    #[test]
    fn text_that_is_not_json_is_reported_as_not_json() {
        for case in ["not a policy", "{", "[1,2", "\"unterminated", "", "{\"Statement\":}"] {
            let defect = resource_policy_defect(case);
            assert_eq!(
                defect,
                Some(PolicyDefect::NotJson),
                "case {case:?}: text that is not JSON is not a policy, got {defect:?}"
            );
        }
    }

    #[test]
    fn the_attaching_validator_reports_the_first_clause_a_document_breaks() {
        // The rows differ along what each clause generalises over: the first byte
        // and JSON type of the text, which undefined member name is carried and
        // at which level, which defined names a document carries beside the
        // required one, how a version string fails to be one, and the JSON form
        // an action element takes.
        let rows: [(&str, String, Option<AttachedPolicyDefect>); 29] = [
            ("one statement, every clause met", policy(STATEMENT), None),
            (
                "a statement written as the object itself",
                format!("{{\"Version\":\"2012-10-17\",\"Statement\":{STATEMENT}}}"),
                None,
            ),
            (
                "every top-level name the language defines",
                format!("{{\"Version\":\"2008-10-17\",\"Id\":\"one\",\"Statement\":[{STATEMENT}]}}"),
                None,
            ),
            (
                "every statement name the language defines",
                policy(
                    "{\"Sid\":\"s\",\"Effect\":\"Allow\",\"Principal\":\"*\",\
                     \"NotPrincipal\":\"*\",\"Action\":\"svc:A\",\"NotAction\":\"svc:B\",\
                     \"Resource\":\"*\",\"NotResource\":\"*\",\
                     \"Condition\":{\"StringEquals\":{\"aws:PrincipalAccount\":\"1\"}}}",
                ),
                None,
            ),
            (
                "a condition operator and key, which the language does not enumerate",
                policy(
                    "{\"Effect\":\"Deny\",\"Condition\":{\"ForAnyValue:StringLike\":\
                     {\"s3:prefix\":\"home/\"}}}",
                ),
                None,
            ),
            (
                "a principal type, which the language does not enumerate",
                policy("{\"Effect\":\"Deny\",\"Principal\":{\"Federated\":\"example.com\"}}"),
                None,
            ),
            (
                "an EMPTY statement list, which carries the required element",
                "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
                None,
            ),
            (
                "an array of statements with no document around them",
                format!("[{STATEMENT}]"),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "a number",
                "5".to_string(),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "a string",
                "\"hello\"".to_string(),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "a boolean",
                "true".to_string(),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "null",
                "null".to_string(),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "no text at all",
                String::new(),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "an object the text does not open with",
                " {\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "a truncated object",
                "{\"Statement\":[]".to_string(),
                Some(AttachedPolicyDefect::NotAPolicyObject),
            ),
            (
                "an undefined name at the document's own level",
                format!("{{\"hello\":\"world\",\"Version\":\"2012-10-17\",\"Statement\":[{STATEMENT}]}}"),
                Some(AttachedPolicyDefect::UnknownElement("hello".to_string())),
            ),
            (
                "a statement element written at the document's level",
                format!("{{\"Effect\":\"Allow\",\"Version\":\"2012-10-17\",\"Statement\":[{STATEMENT}]}}"),
                Some(AttachedPolicyDefect::UnknownElement("Effect".to_string())),
            ),
            (
                "a misspelled statement list",
                "{\"Statements\":[]}".to_string(),
                Some(AttachedPolicyDefect::UnknownElement("Statements".to_string())),
            ),
            (
                "an undefined name inside the only statement",
                policy("{\"Effect\":\"Deny\",\"Comment\":\"why\"}"),
                Some(AttachedPolicyDefect::UnknownElement("Comment".to_string())),
            ),
            (
                "an undefined name inside the second statement",
                policy(&format!(
                    "{STATEMENT},{{\"Effect\":\"Deny\",\"Actions\":\"svc:A\"}}"
                )),
                Some(AttachedPolicyDefect::UnknownElement("Actions".to_string())),
            ),
            (
                "an object declaring nothing",
                "{}".to_string(),
                Some(AttachedPolicyDefect::MissingElement("Statement")),
            ),
            (
                "a version and nothing else",
                "{\"Version\":\"2012-10-17\"}".to_string(),
                Some(AttachedPolicyDefect::MissingElement("Statement")),
            ),
            (
                "a version and an id but no statement",
                "{\"Version\":\"2012-10-17\",\"Id\":\"one\"}".to_string(),
                Some(AttachedPolicyDefect::MissingElement("Statement")),
            ),
            (
                "no version at all",
                format!("{{\"Statement\":[{STATEMENT}]}}"),
                Some(AttachedPolicyDefect::UndefinedVersion),
            ),
            (
                "a version date the language does not define",
                format!("{{\"Version\":\"2024-01-01\",\"Statement\":[{STATEMENT}]}}"),
                Some(AttachedPolicyDefect::UndefinedVersion),
            ),
            (
                "the current version without its separators",
                format!("{{\"Version\":\"20121017\",\"Statement\":[{STATEMENT}]}}"),
                Some(AttachedPolicyDefect::UndefinedVersion),
            ),
            (
                "a version that is not a string",
                format!("{{\"Version\":2012,\"Statement\":[{STATEMENT}]}}"),
                Some(AttachedPolicyDefect::UndefinedVersion),
            ),
            (
                "one unreadable action in a list of two",
                policy("{\"Effect\":\"Deny\",\"Action\":[\"svc:A\",\"nope\"]}"),
                Some(AttachedPolicyDefect::UnreadableAction),
            ),
            (
                "an action element naming nothing at all",
                policy("{\"Effect\":\"Deny\",\"NotAction\":[]}"),
                Some(AttachedPolicyDefect::UnreadableAction),
            ),
        ];
        for (case, document, defect) in rows {
            let found = attached_policy_defect(&document, |_| true);
            assert_eq!(
                found, defect,
                "case {case:?}: expected {defect:?}, got {found:?}"
            );
        }
    }

    #[test]
    fn each_attaching_clause_is_worded_with_its_own_sentence() {
        for (defect, sentence) in [
            (
                AttachedPolicyDefect::NotAPolicyObject,
                NOT_A_JSON_POLICY_MESSAGE.to_string(),
            ),
            (
                AttachedPolicyDefect::UnknownElement("hello".to_string()),
                "Unknown field hello".to_string(),
            ),
            (
                AttachedPolicyDefect::MissingElement("Statement"),
                "Missing required field Statement".to_string(),
            ),
            (
                AttachedPolicyDefect::UndefinedVersion,
                INVALID_POLICY_VERSION_MESSAGE.to_string(),
            ),
            (
                AttachedPolicyDefect::UnreadableAction,
                POLICY_INVALID_ACTION_MESSAGE.to_string(),
            ),
        ] {
            assert_eq!(
                defect.message(),
                sentence,
                "case {defect:?}: expected the clause's own sentence"
            );
        }
    }

    #[test]
    fn the_owning_service_decides_which_actions_its_resource_can_grant() {
        // The language reads an action's FORM; which actions the resource can
        // grant is the attaching service's own reading, passed in. One row per
        // element form, so the closure is applied through every one of them.
        let grantable = |action: &str| action.starts_with("svc:");
        for (case, document, defect) in [
            (
                "an action the service grants",
                policy("{\"Effect\":\"Allow\",\"Action\":\"svc:A\"}"),
                None,
            ),
            (
                "an action of another service",
                policy("{\"Effect\":\"Allow\",\"Action\":\"other:A\"}"),
                Some(AttachedPolicyDefect::UnreadableAction),
            ),
            (
                "one of each in a list",
                policy("{\"Effect\":\"Allow\",\"Action\":[\"svc:A\",\"other:A\"]}"),
                Some(AttachedPolicyDefect::UnreadableAction),
            ),
            (
                "another service's action in the complement element",
                policy("{\"Effect\":\"Deny\",\"NotAction\":\"other:A\"}"),
                Some(AttachedPolicyDefect::UnreadableAction),
            ),
        ] {
            let found = attached_policy_defect(&document, grantable);
            assert_eq!(
                found, defect,
                "case {case:?}: expected {defect:?}, got {found:?}"
            );
        }
    }

    #[test]
    fn an_action_is_read_as_a_service_namespace_and_an_action_name() {
        // The rows differ along the form of the value: the wildcard, a pair, a
        // pair carrying wildcards in its action name, a missing half, and
        // characters neither half is written in.
        let rows: [(&str, &str, bool); 13] = [
            ("the wildcard", "*", true),
            ("a pair", "s3:GetObject", true),
            ("a hyphenated namespace", "s3-object-lambda:GetObject", true),
            ("a wildcard inside the action name", "s3:Get*", true),
            ("a wildcard action name", "s3:*", true),
            ("a single-character wildcard", "s3:GetObjec?", true),
            ("no namespace at all", "GetObject", false),
            ("no action name", "s3:", false),
            ("no namespace before the separator", ":GetObject", false),
            ("a third field", "s3:GetObject:extra", false),
            ("a space inside the action name", "s3:Get Object", false),
            (
                "an action name written in lower case, which IAM reads case-insensitively",
                "s3:getObject",
                true,
            ),
            (
                "an ARN, which names a resource and not an action",
                "arn:aws:s3:::b",
                false,
            ),
        ];
        for (case, value, readable) in rows {
            assert_eq!(
                readable_action(value),
                readable,
                "case {case:?}: expected readable_action({value:?}) == {readable}, got {}",
                readable_action(value)
            );
        }
    }

    #[test]
    fn a_document_is_rendered_in_the_canonical_layout_whatever_layout_it_arrived_in() {
        // The rows differ along what the rendering generalises over: the submitted
        // whitespace, the arity of a value list (none, one, two), the element type of a
        // list (strings against statement objects), the nesting depth, the member order and
        // the value types a condition carries.
        let compact = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"One\",\
                       \"Effect\":\"Allow\",\"Principal\":\"*\",\
                       \"Action\":[\"svc:A\",\"svc:B\"]}]}";
        let rendered = "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {\n    \
                        \"Sid\" : \"One\",\n    \"Effect\" : \"Allow\",\n    \
                        \"Principal\" : \"*\",\n    \"Action\" : [ \"svc:A\", \"svc:B\" ]\n  \
                        } ]\n}";
        let rows: [(&str, &str, &str); 6] = [
            ("a compact document", compact, rendered),
            ("a document already rendered", rendered, rendered),
            (
                "a one-value list, which is written as the value alone",
                "{\"Statement\":{\"Effect\":\"Allow\",\"Principal\":\"*\",\
                 \"Action\":[\"svc:A\"]}}",
                "{\n  \"Statement\" : {\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : \"*\",\n    \"Action\" : \"svc:A\"\n  }\n}",
            ),
            (
                "a nested principal map and an empty list",
                "{\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"AWS\":[\"a\",\"b\"]},\"Action\":\"svc:A\",\
                 \"Resource\":[]}]}",
                "{\n  \"Statement\" : [ {\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : {\n      \"AWS\" : [ \"a\", \"b\" ]\n    },\n    \
                 \"Action\" : \"svc:A\",\n    \"Resource\" : [ ]\n  } ]\n}",
            ),
            (
                "a condition carrying a number and a boolean",
                "{\"Statement\":{\"Effect\":\"Allow\",\"Principal\":\"*\",\
                 \"Action\":\"svc:A\",\"Condition\":{\"NumericLessThan\":{\"k\":3600},\
                 \"Bool\":{\"b\":true},\"Null\":{}}}}",
                "{\n  \"Statement\" : {\n    \"Effect\" : \"Allow\",\n    \
                 \"Principal\" : \"*\",\n    \"Action\" : \"svc:A\",\n    \
                 \"Condition\" : {\n      \"NumericLessThan\" : {\n        \
                 \"k\" : 3600\n      },\n      \"Bool\" : {\n        \"b\" : true\n      \
                 },\n      \"Null\" : { }\n    }\n  }\n}",
            ),
            // Not JSON at all: a rendering is not a validation, so the text comes back as
            // it came.
            ("text that is not JSON", "not a policy", "not a policy"),
        ];

        for (case, submitted, expected) in rows {
            let actual = canonical_document(submitted);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            // The rendering is a fixed point, so a client that puts back what it read sees
            // no drift.
            let again = canonical_document(&actual);
            assert_eq!(
                again, actual,
                "case {case:?}: expected re-rendering to change nothing, got {again:?}"
            );
        }
    }

    #[test]
    fn a_condition_value_is_rendered_as_a_string_for_a_caller_that_asks() {
        // The rows differ along what the coercion generalises over: which JSON type the
        // value was written as, whether it stands alone or in a list of one or of several,
        // which operator carries it, a value already written as a string, `null` (which the
        // language gives no string form), and the same types OUTSIDE a condition block.
        let statement = |elements: &str| {
            format!("{{\"Statement\":{{\"Effect\":\"Allow\",\"Principal\":\"*\",{elements}}}}}")
        };
        let rows: [(&str, String, &str); 7] = [
            (
                "an integer under a numeric operator",
                statement(
                    "\"Action\":\"svc:A\",\
                     \"Condition\":{\"NumericLessThanEquals\":{\"aws:MultiFactorAuthAge\":300}}",
                ),
                "\"aws:MultiFactorAuthAge\" : \"300\"",
            ),
            (
                "a fractional number, with the digits it was written with",
                statement(
                    "\"Action\":\"svc:A\",\
                     \"Condition\":{\"NumericLessThan\":{\"k\":1.50}}",
                ),
                "\"k\" : \"1.50\"",
            ),
            (
                "a boolean",
                statement(
                    "\"Action\":\"svc:A\",\
                     \"Condition\":{\"Bool\":{\"aws:SecureTransport\":true}}",
                ),
                "\"aws:SecureTransport\" : \"true\"",
            ),
            (
                "a one-value list, which collapses to the string like any value list",
                statement(
                    "\"Action\":\"svc:A\",\
                     \"Condition\":{\"NumericEquals\":{\"k\":[300]}}",
                ),
                "\"k\" : \"300\"",
            ),
            (
                "a list of several values",
                statement(
                    "\"Action\":\"svc:A\",\
                     \"Condition\":{\"NumericEquals\":{\"k\":[60,300]}}",
                ),
                "\"k\" : [ \"60\", \"300\" ]",
            ),
            (
                "a value already written as a string",
                statement(
                    "\"Action\":\"svc:A\",\
                     \"Condition\":{\"StringEquals\":{\"k\":\"v\"}}",
                ),
                "\"k\" : \"v\"",
            ),
            (
                "null, which the language gives no string form",
                statement(
                    "\"Action\":\"svc:A\",\
                     \"Condition\":{\"Null\":{\"k\":null}}",
                ),
                "\"k\" : null",
            ),
        ];

        for (case, submitted, expected) in rows {
            let actual = canonical_document_with_condition_values_as_strings(&submitted);
            assert!(
                actual.contains(expected),
                "case {case:?}: expected a rendering carrying {expected:?}, got {actual:?}"
            );
            // The same walk, so the layout is the one every reader of a policy reports, and
            // re-rendering what was read changes nothing.
            let again = canonical_document_with_condition_values_as_strings(&actual);
            assert_eq!(
                again, actual,
                "case {case:?}: expected re-rendering to change nothing, got {again:?}"
            );
            assert_eq!(
                canonical_document(&actual),
                actual,
                "case {case:?}: expected the coerced rendering to be the canonical layout"
            );
        }
    }

    #[test]
    fn the_coercion_leaves_everything_outside_a_condition_as_it_was_written() {
        // A number and a boolean outside any condition block keep the characters they were
        // written with, under both entry points: the coercion is about a condition's values
        // and nothing else. (The policy grammar admits no such document, which is why this
        // is asserted on the rendering rather than through a service.)
        let submitted = "{\"Id\":3600,\"Flag\":true,\"Statement\":[]}";
        for (case, rendered) in [
            ("the plain rendering", canonical_document(submitted)),
            (
                "the coercing rendering",
                canonical_document_with_condition_values_as_strings(submitted),
            ),
        ] {
            assert!(
                rendered.contains("\"Id\" : 3600") && rendered.contains("\"Flag\" : true"),
                "case {case:?}: expected the values as submitted, got {rendered:?}"
            );
        }
    }

    #[test]
    fn a_document_nested_past_the_rendered_limit_comes_back_unrendered() {
        // Rows: exactly the nesting the renderer accepts, and one level past it. The
        // nesting of a submitted document is the caller's, so the bound is the renderer's
        // own rather than something it inherits from whoever parsed the text first.
        for (case, objects, renders) in [
            ("at the nesting bound", MAX_CANONICAL_NESTING - 1, true),
            ("one level past the bound", MAX_CANONICAL_NESTING, false),
        ] {
            let mut document = String::new();
            for _ in 0..objects {
                document.push_str("{\"a\":");
            }
            document.push_str("\"x\"");
            for _ in 0..objects {
                document.push('}');
            }

            let actual = canonical_document(&document);
            assert_eq!(
                actual != document,
                renders,
                "case {case:?}: expected a {objects}-deep document to be \
                 {}, got {} bytes",
                if renders { "rendered" } else { "returned as it came" },
                actual.len()
            );
            if renders {
                assert!(
                    actual.starts_with("{\n  \"a\" : "),
                    "case {case:?}: expected the canonical layout, got {:?}",
                    &actual[..actual.len().min(32)]
                );
            }
        }
    }

    #[test]
    fn an_element_written_as_an_empty_container_names_nothing() {
        for (case, statement, expected) in [
            (
                "an empty Action list",
                "{\"Effect\":\"Allow\",\"Action\":[],\"Principal\":\"*\"}",
                PolicyDefect::MissingAction,
            ),
            (
                "an empty Action string",
                "{\"Effect\":\"Allow\",\"Action\":\"\",\"Principal\":\"*\"}",
                PolicyDefect::MissingAction,
            ),
            (
                "an empty Principal object",
                "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":{}}",
                PolicyDefect::MissingPrincipal,
            ),
            (
                "a Principal whose type names nothing",
                "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":{\"AWS\":[]}}",
                PolicyDefect::MissingPrincipal,
            ),
            (
                "an Effect that is not Allow or Deny",
                "{\"Effect\":\"Maybe\",\"Action\":\"svc:A\",\"Principal\":\"*\"}",
                PolicyDefect::MissingEffect,
            ),
        ] {
            let defect = resource_policy_defect(&policy(statement));
            assert_eq!(
                defect,
                Some(expected),
                "case {case:?}: expected {expected:?}, got {defect:?}"
            );
        }
    }

    /// An identity-based policy's declared VALUES, held to the ones the policy language
    /// defines.
    #[test]
    fn a_value_the_policy_language_does_not_define_is_reported_with_its_clause() {
        /// An identity statement naming an action and a resource, as a client sends it.
        fn identity(statement: &str) -> String {
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement}]}}")
        }

        // The rows differ along what the rule generalises over: which clause the document
        // breaks, which published value it declares instead, the form the resource element
        // takes (a string or a list), which spelling of that element names the value, and
        // where in a two-statement document the offending value sits.
        let rows: [(&str, String, Option<PolicyValueDefect>); 14] = [
            (
                "every value the language defines",
                identity("{\"Effect\":\"Allow\",\"Action\":\"svc:DoThing\",\"Resource\":\"arn:aws:svc:::thing\"}"),
                None,
            ),
            (
                "the wildcard in place of an ARN",
                identity("{\"Effect\":\"Deny\",\"Action\":\"svc:DoThing\",\"Resource\":\"*\"}"),
                None,
            ),
            (
                "the legacy policy version",
                "{\"Version\":\"2008-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Resource\":\"*\"}]}".to_string(),
                None,
            ),
            (
                "no version at all",
                "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Resource\":\"*\"}]}".to_string(),
                None,
            ),
            (
                "a statement that declares no effect",
                identity("{\"Action\":\"svc:A\",\"Resource\":\"*\"}"),
                None,
            ),
            (
                "a version that does not exist",
                "{\"Version\":\"2012-10-18\",\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Resource\":\"*\"}]}".to_string(),
                Some(PolicyValueDefect::UnsupportedVersion),
            ),
            (
                "a version that is not a string",
                "{\"Version\":20121017,\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Resource\":\"*\"}]}".to_string(),
                Some(PolicyValueDefect::UnsupportedVersion),
            ),
            (
                "an effect written in another case",
                identity("{\"Effect\":\"allow\",\"Action\":\"svc:A\",\"Resource\":\"*\"}"),
                Some(PolicyValueDefect::UnreadableEffect),
            ),
            (
                "an effect the language does not define",
                identity("{\"Effect\":\"Permit\",\"Action\":\"svc:A\",\"Resource\":\"*\"}"),
                Some(PolicyValueDefect::UnreadableEffect),
            ),
            (
                "a resource that is no ARN",
                identity("{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Resource\":\"things/*\"}"),
                Some(PolicyValueDefect::NonResourceValue("things/*".to_string())),
            ),
            (
                "the second value of a resource list",
                identity("{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Resource\":[\"arn:aws:svc:::a\",\"b\"]}"),
                Some(PolicyValueDefect::NonResourceValue("b".to_string())),
            ),
            (
                "a NotResource that is no ARN",
                identity("{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"NotResource\":\"things\"}"),
                Some(PolicyValueDefect::NonResourceValue("things".to_string())),
            ),
            (
                "the second statement's resource",
                "{\"Statement\":[{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Resource\":\"*\"},\
                 {\"Effect\":\"Allow\",\"Action\":\"svc:B\",\"Resource\":\"thing\"}]}"
                    .to_string(),
                Some(PolicyValueDefect::NonResourceValue("thing".to_string())),
            ),
            (
                "a text that is no policy at all",
                "not json".to_string(),
                None,
            ),
        ];

        for (case, document, expected) in rows {
            let defect = identity_policy_value_defect(&document);
            assert_eq!(
                defect, expected,
                "case {case:?}: expected {expected:?}, got {defect:?} for {document}"
            );
        }
    }

    /// Each clause's sentence: fixed text for the two that carry nothing of the document,
    /// and the submitted value for the one that names it.
    #[test]
    fn each_clause_is_worded_with_its_own_sentence() {
        for (case, defect, expected) in [
            (
                "an unsupported version",
                PolicyValueDefect::UnsupportedVersion,
                "Syntax errors in policy.".to_string(),
            ),
            (
                "an effect the language does not define",
                PolicyValueDefect::UnreadableEffect,
                "The policy failed legacy parsing".to_string(),
            ),
            (
                "a resource that is no ARN",
                PolicyValueDefect::NonResourceValue("things/*".to_string()),
                "Resource things/* must be in ARN format or \"*\".".to_string(),
            ),
        ] {
            let message = defect.message();
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

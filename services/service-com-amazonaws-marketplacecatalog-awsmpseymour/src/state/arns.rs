// SPDX-License-Identifier: CC0-1.0

//! The ARNs this service MINTS, the identifiers that address its resources
//! through one, and the refusals for an ARN or identifier outside contract.
//!
//! Every ARN here is minted for the region of the request's own SigV4
//! credential scope ([`request_region`]), and that region is the one field of
//! these ARNs a caller chooses — so it is held to the characters the minted
//! ARN's own shape admits before it reaches one ([`is_arn_region`]).

use super::*;

/// The ARN of a change set, which `StartChangeSet`, `CancelChangeSet`,
/// `DescribeChangeSet` and `ListChangeSets` all report in a member of shape
/// `ARN`: `region` is the region of the request that started it
/// ([`request_region`]), so it is one [`is_arn_region`] admits and the result is
/// inside that shape's `^[a-zA-Z0-9:*/-]+$`.
pub fn change_set_arn(region: &str, change_set_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{AWS_MARKETPLACE_CATALOG}/ChangeSet/{change_set_id}"),
    )
}

/// The ARN of a catalog entity, reported in a member of shape `ARN` by every
/// entity read: `region` comes from the change set that applied the entity and
/// `entity_type` is one of [`ENTITY_TYPES`], so the result is inside that
/// shape's `^[a-zA-Z0-9:*/-]+$` (see [`change_set_arn`]).
pub fn entity_arn(region: &str, entity_type: &str, entity_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{AWS_MARKETPLACE_CATALOG}/{entity_type}/{entity_id}"),
    )
}

/// The ARN of an assessment, in the form
/// `DescribeAssessmentRequest$AssessmentIdentifier` documents
/// ("arn:aws:aws-marketplace:us-east-1::AWSMarketplace/Assessment/assessment-12345")
/// and `ARN`'s `^[a-zA-Z0-9:*/-]+$` allows.
pub fn assessment_arn(region: &str, assessment_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{AWS_MARKETPLACE_CATALOG}/{ASSESSMENT_ARN_RESOURCE_TYPE}/{assessment_id}"),
    )
}

/// True when `value` is an assessment id: [`ASSESSMENT_ID_PREFIX`] followed by at
/// least one alphanumeric character, which is `AssessmentIdentifier`'s
/// `assessment-[a-zA-Z0-9]+` alternative.
pub fn is_assessment_id(value: &str) -> bool {
    match value.strip_prefix(ASSESSMENT_ID_PREFIX) {
        Some(tail) => !tail.is_empty() && tail.chars().all(|c| c.is_ascii_alphanumeric()),
        None => false,
    }
}

/// The fields an ARN of THIS service's namespace carries beside the two fixed
/// ones: its partition, region, account and resource portion.
///
/// The two fixed fields are not members of this struct because a value that does
/// not carry them is no ARN of this service at all, so [`service_arn`] answers
/// `None` for it rather than reporting what they held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceArn<'a> {
    /// The partition field: where, in AWS's own partitioning, the resource lives.
    pub partition: &'a str,
    /// The region field, which an ARN of this service may leave empty.
    pub region: &'a str,
    /// The account field: which account the resource belongs to.
    pub account: &'a str,
    /// Everything after the account field, separators included — the resource
    /// portion [`entity_arn`] and [`change_set_arn`] write as
    /// `<catalog>/<type>/<id>`.
    pub resource: &'a str,
}

/// The variable fields of `value` when it is an ARN of this service's own
/// `arnNamespace` — the literal `arn`, then five `:`-separated fields of which
/// the second names [`ARN_NAMESPACE`] — or `None` when it is not one at all.
///
/// This is the one place this service decides what an ARN of its own namespace
/// is, so every surface addressed by one ([`assessment_id_of`],
/// [`is_resource_arn`], [`parse_resource_arn`]) agrees on the question and adds
/// only its OWN further conditions on the fields it gets back: the characters
/// each field may carry, which account owns the resource, or which resource the
/// resource portion names.
///
/// The resource portion is everything after the account field, so a resource
/// carrying `:` of its own stays whole.
///
/// [`arn_resource`] deliberately does NOT go through this: it reports the
/// resource portion of an ARN of ANY namespace, including one this service never
/// minted, so requiring this namespace there would change what it answers.
pub fn service_arn(value: &str) -> Option<ServiceArn<'_>> {
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(ARN_NAMESPACE),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    Some(ServiceArn {
        partition,
        region,
        account,
        resource,
    })
}

/// The assessment `identifier` names, for an identifier of either form
/// `AssessmentIdentifier` documents: the id itself, or the id at the end of an
/// assessment ARN of `catalog` in this service's own ARN namespace. `None` for
/// anything else, which is what the caller refuses the request for.
///
/// The ARN alternative of the modelled pattern constrains only the SHAPE of an
/// ARN (`arn:<partition>:<service>:<region>:<account>:<type>/Assessment/assessment-…`),
/// so an ARN naming another service's namespace or another catalog satisfies it
/// while naming no assessment of this catalog. The partition, region and account
/// are the caller's: this service serves one catalog, whatever region a caller
/// signs for, so they do not select the record.
///
/// Whether the identifier is an ARN of this service's namespace at all is
/// [`service_arn`]'s decision; the extra condition here is that its resource
/// portion names an assessment of `catalog`.
pub fn assessment_id_of<'a>(identifier: &'a str, catalog: &str) -> Option<&'a str> {
    if !(1..=ASSESSMENT_IDENTIFIER_MAX).contains(&identifier.chars().count()) {
        return None;
    }
    if is_assessment_id(identifier) {
        return Some(identifier);
    }
    let fields = service_arn(identifier)?;
    let id = fields
        .resource
        .strip_prefix(&format!("{catalog}/{ASSESSMENT_ARN_RESOURCE_TYPE}/"))?;
    is_assessment_id(id).then_some(id)
}

/// The syntax the service publishes for `AssessmentIdentifier`, which it reports
/// in the fault for one outside it: the two forms the member's doc documents
/// ("You can provide either the assessment ID … or the full assessment ARN"),
/// written as two SEPARATELY anchored alternatives joined by `|`, the ARN one
/// carrying no account field.
///
/// It is the service's own string, not the shape's modelled `@pattern`
/// (`smithy_query(query="shape", shape="AssessmentIdentifier")`, which spells
/// the same two forms as one anchored group with an `[0-9]*` account segment):
/// what a refusal quotes is the syntax the SERVICE publishes for the member, so
/// it is written out here rather than interpolated from the model — a caller
/// that pre-validates an identifier against the advertised regex gets the one
/// the service advertises.
///
/// [`assessment_id_of`] is wider than this string on the ARN alternative — the
/// account field of an ARN names where an assessment lives, not which one it is,
/// so an ARN carrying one resolves — and stricter on everything the shape of an
/// ARN cannot say: one naming another service's namespace or another catalog
/// names no assessment of this catalog and is refused too.
///
/// The string states no length, so the shape's `@length(min: 1, max: 2048)`
/// ([`ASSESSMENT_IDENTIFIER_MAX`]) is enforced by [`assessment_id_of`] beside it.
pub const ASSESSMENT_IDENTIFIER_PATTERN: &str = concat!(
    r"(^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::",
    r"[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$)"
);

/// How long an `AssessmentIdentifier` may be: that shape's modelled
/// `@length(min: 1, max: 2048)`, which [`ASSESSMENT_IDENTIFIER_PATTERN`] does
/// not state, so the bound is enforced where the identifier is read.
pub const ASSESSMENT_IDENTIFIER_MAX: usize = 2048;

/// The modelled `ValidationException` payload for an `AssessmentIdentifier` that
/// is neither of the forms [`assessment_id_of`] resolves — built from the
/// request, naming the identifier it rejected and the syntax it accepts.
pub fn invalid_assessment_identifier(identifier: &str) -> ValidationException {
    validation_message(quoting(
        "Requested assessment identifier '",
        identifier,
        &format!("' is invalid. It should match with {ASSESSMENT_IDENTIFIER_PATTERN}."),
    ))
}

/// The modelled `ResourceNotFoundException` payload for an assessment id that
/// names no assessment.
///
/// `AssessmentIdentifier` is `@length(max: 2048)`, the very length a `Message`
/// may be, so the id the sentence names is quoted through [`quoting`]: an
/// identifier a caller may legally send cannot take the fault out of contract.
pub fn assessment_not_found(assessment_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(quoting(
            "Requested assessment '",
            assessment_id,
            "' does not exist.",
        )),
    }
}

/// A resource of this catalog, as one of its resource ARNs names it.
///
/// `ListTagsForResource` is documented for "a resource (either an entity or
/// change set)", and [`entity_arn`] / [`change_set_arn`] / [`assessment_arn`]
/// write all three in the one form `<catalog>/<type>/<id>`, so one parse covers
/// them. Which KINDS an operation then acts on is the operation's own: the
/// tagging surface acts on any resource that CARRIES tags — an assessment does,
/// since `StartAssessmentRequest$Tags` applies tags to the assessment it starts
/// — while the resource-policy surface is documented for an entity and acts on
/// that alone
/// ([`ComAmazonawsMarketplacecatalogAwsmpseymourState::policy_key_for`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogResource {
    /// A catalog entity: its `EntityType` and its `ResourceId`.
    Entity { entity_type: String, id: String },
    /// A change set, by its `ResourceId`.
    ChangeSet { id: String },
    /// An assessment, by its `ResourceId`.
    Assessment { id: String },
}

impl CatalogResource {
    /// The id the ARN named — what a `ResourceNotFoundException` for it names.
    pub fn id(&self) -> &str {
        match self {
            Self::Entity { id, .. } | Self::ChangeSet { id } | Self::Assessment { id } => {
                id.as_str()
            }
        }
    }

    /// The key [`ComAmazonawsMarketplacecatalogAwsmpseymourState::resource_policies`]
    /// holds this resource's policy under: its type and id, so a policy
    /// attached through an ARN written for one region is the policy read back
    /// through an ARN written for another — the resource is the same resource.
    ///
    /// A resource of a kind the resource-policy operations are not documented
    /// for carries no resource-based policy at all, so it has no such key:
    /// `None`.
    pub fn policy_key(&self) -> Option<String> {
        match self {
            Self::Entity { entity_type, id } => Some(format!("{entity_type}/{id}")),
            Self::ChangeSet { .. } | Self::Assessment { .. } => None,
        }
    }
}

/// The resource portion of `arn` — everything after the account field — or the
/// whole value when it has no such field.
pub fn arn_resource(arn: &str) -> &str {
    let mut fields = arn.splitn(6, ':');
    match (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) {
        (Some("arn"), Some(_), Some(_), Some(_), Some(_), Some(resource)) => resource,
        _ => arn,
    }
}

/// The resource ID `arn` names: the last `/`-separated segment of its resource
/// portion, or the whole portion when it carries none.
///
/// A resource ARN of this catalog carries `<catalog>/<type>/<id>` there
/// ([`entity_arn`], [`change_set_arn`]), of which the ID is what names the
/// resource — the catalog and the type say WHERE it lives — so it is the ID that
/// an authorization refusal reports as the resource it refused.
pub fn arn_resource_id(arn: &str) -> &str {
    let resource = arn_resource(arn);
    resource.rsplit('/').next().unwrap_or(resource)
}

/// The syntax the service publishes for a resource ARN, which it reports in the
/// fault for one outside it: the modelled `ResourceARN` `@pattern`
/// (`smithy_query(query="shape", shape="ResourceARN")`), which is the syntax the
/// service's own recorded refusals quote for the member.
pub const RESOURCE_ARN_PATTERN: &str =
    r"^arn:[\w+=/,.@-]+:aws-marketplace:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// How long a resource ARN may be: the modelled `ResourceARN`
/// `@length(min: 1, max: 255)`, which [`RESOURCE_ARN_PATTERN`] does not state.
const RESOURCE_ARN_MAX: usize = 255;

/// Whether `value` is a resource ARN at all — [`RESOURCE_ARN_PATTERN`] spelled
/// out as a predicate, with the member's modelled `@length` beside it.
///
/// The six fields the pattern names: the literal `arn`, a partition, this
/// service's `arnNamespace`, a region (which may be empty), an account of
/// digits, and a resource of `/`-separated non-empty segments. The first three
/// are [`service_arn`]'s decision; the extra condition here is the characters
/// each remaining field may carry. WHICH resource an ARN inside this syntax
/// names is [`parse_resource_arn`]'s decision; this one says only that the value
/// is an ARN of this service's namespace, which is what a caller is told to
/// satisfy.
pub fn is_resource_arn(value: &str) -> bool {
    if !(1..=RESOURCE_ARN_MAX).contains(&value.chars().count()) {
        return false;
    }
    let field = |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | '/' | ',' | '.' | '@' | '-')
    };
    let segment = |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | ',' | '.' | '@' | '-')
    };
    let Some(fields) = service_arn(value) else {
        return false;
    };
    if fields.partition.is_empty() || !fields.partition.chars().all(field) {
        return false;
    }
    if !fields.region.chars().all(field) {
        return false;
    }
    if fields.account.is_empty() || !fields.account.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    fields
        .resource
        .split('/')
        .all(|part| !part.is_empty() && part.chars().all(segment))
}

/// The modelled `ValidationException` payload for a resource ARN outside
/// [`RESOURCE_ARN_PATTERN`] — built from the request, naming the value it
/// rejected and the syntax a resource ARN has to satisfy.
///
/// Every operation addressed by a resource ARN refuses a malformed one this way,
/// before it decides whether the ARN names a resource of this catalog: the
/// member is judged by the handler rather than at the boundary
/// (`skip_validation` on each of those inputs), so the refusal carries the
/// modelled `ValidationException` and its 422 rather than the protocol's status.
pub fn invalid_resource_arn(arn: &str) -> ValidationException {
    validation_message(quoting(
        "Requested resource arn '",
        arn,
        &format!("' is invalid. It should match with {RESOURCE_ARN_PATTERN}."),
    ))
}

/// The resource of THIS service `arn` names, or `None` when `arn` names none.
///
/// A resource ARN this service can act on is one it mints: this account, this
/// `arnNamespace`, and a resource portion of the `<catalog>/<type>/<id>` form
/// [`entity_arn`] and [`change_set_arn`] write. Every other well-formed ARN
/// names a resource outside this catalog, which no identity or resource policy
/// of this catalog can grant an action on — so the caller is not authorized to
/// act on it rather than it being absent.
///
/// The partition and region fields are not constrained: they name where the
/// resource lives, not what it is, and the modelled `ResourceARN` pattern
/// already bounds their characters.
///
/// Whether the value is an ARN of this service's namespace at all is
/// [`service_arn`]'s decision; the extra conditions here are the account and the
/// form of the resource portion.
pub fn parse_resource_arn(arn: &str) -> Option<CatalogResource> {
    let fields = service_arn(arn)?;
    if fields.account != ::engine::emulator_account_id() {
        return None;
    }
    let mut segments = fields.resource.split('/');
    let (Some(AWS_MARKETPLACE_CATALOG), Some(resource_type), Some(id), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if id.is_empty() {
        return None;
    }
    Some(match resource_type {
        CHANGE_SET_RESOURCE_TYPE => CatalogResource::ChangeSet { id: id.to_string() },
        ASSESSMENT_ARN_RESOURCE_TYPE => CatalogResource::Assessment { id: id.to_string() },
        entity_type => CatalogResource::Entity {
            entity_type: entity_type.to_string(),
            id: id.to_string(),
        },
    })
}

/// The region this request was signed for — what the ARNs it mints name.
///
/// A request's credential scope is the CALLER's own, and the region field in it
/// is bounded by nothing but being non-empty, while every ARN minted from it is
/// reported in a member whose shape is `ARN`. So a scope naming a region a
/// minted ARN cannot carry ([`is_arn_region`]) is attributed to
/// [`DEFAULT_REGION`], exactly as a request carrying no scope at all is: the
/// region a record stores, and every ARN built from it, stays inside the shape
/// that reports it.
///
/// Total: the scope is parsed by the server's own `Option`-answering parser, so
/// an absent, empty or malformed header resolves to the constant rather than
/// failing.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// How long an ARN this service reports may be: the `@length(min: 1, max: 2048)`
/// of `ARN`, the shape every member carrying one of the ARNs minted here targets
/// (`smithy_query(query="shape", shape="ARN")`).
const ARN_MAX: usize = 2048;

/// How long the region field of a minted ARN may be: what [`ARN_MAX`] leaves for
/// it once the longest ARN this service mints has taken its other fields — the
/// literal `arn`, the `aws` partition the emulator mints in
/// (`::engine::emulator_arn`), this service's [`ARN_NAMESPACE`], the account id,
/// and the widest resource these builders write
/// (`<catalog>/<entity type>/<entity id>`, each of the latter two at the
/// modelled [`ENTITY_MEMBER_MAX`]).
const ARN_REGION_MAX: usize = ARN_MAX
    - ("arn:aws:".len()
        + ARN_NAMESPACE.len()
        + ":".len()
        + ":".len()
        + ::engine::EMULATOR_ACCOUNT_ID.len()
        + ":".len()
        + AWS_MARKETPLACE_CATALOG.len()
        + "/".len()
        + ENTITY_MEMBER_MAX
        + "/".len()
        + ENTITY_MEMBER_MAX);

/// Whether an ARN this service mints can name `region`: a non-empty region of at
/// most [`ARN_REGION_MAX`] of the ASCII letters, digits and `-` that `ARN`'s
/// `^[a-zA-Z0-9:*/-]+$` admits INSIDE one of an ARN's fields.
///
/// The three characters of that class left out are the `:` that separates an
/// ARN's fields, the `/` that separates its resource segments and the `*` of a
/// wildcard: a region carrying one of them would not name a region in the field
/// it stands in, so the region a published region name is spelled with — letters,
/// digits and `-` — is the whole class a minted ARN can take here.
///
/// A member whose boundary validation this service replaces with a predicate of
/// its own is still bounded by it, length included ([`ENTITY_MEMBER_MAX`]); the
/// region reaches no boundary validator at all, so this predicate is what keeps
/// it inside the shape every ARN built from it is reported in.
pub fn is_arn_region(region: &str) -> bool {
    (1..=ARN_REGION_MAX).contains(&region.chars().count())
        && region
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

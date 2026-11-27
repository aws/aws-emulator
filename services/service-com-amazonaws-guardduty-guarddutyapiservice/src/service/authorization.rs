// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it names — the decision
//! taken BEFORE the operation reads or writes anything, as AWS takes it.
//!
//! An operation of this model that declares `AccessDeniedException` (`httpError` 403,
//! `smithy_query(operation="CreateInvestigation")`) has a modelled answer for a caller
//! whose permissions do not cover the action it asked for, and that answer is what AWS
//! gives: the request is judged at the IAM layer, ahead of the service resolving anything
//! the request named, so the refusal says nothing about whether the detector, the rule or
//! the plan it addressed exists. An operation that declares no 403 has no such answer and
//! is therefore not judged here at all — refusing it with an error its own model does not
//! declare would be a different divergence.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same record
//!    `sts:GetCallerIdentity` answers from), else the IAM user that owns the long-term
//!    key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's own accessors (`principal_policies`, `permissions_boundary_document`), so
//!    a document `iam:PutRolePolicy` wrote takes effect on the next request and deleting
//!    it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the wording of the refusal
//! does. That is the split every other gate in this tree makes
//! (`services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/service/authorization.rs`
//! and
//! `services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/service/authorization.rs`),
//! and the lookups stay in this crate for the reason those keep theirs there: hoisting
//! them into `service-common` would make every service depending on that crate link the
//! IAM and STS service crates.
//!
//! ## The action and the resource
//!
//! The action is `guardduty:<OperationName>` — this service's `arnNamespace`, which is
//! also its signing name ([`ACTION_PREFIX`]) — so one policy statement scoped to this
//! service's actions covers every operation of it and nothing outside it.
//!
//! The resource is the ARN of what the REQUEST ADDRESSES, spelled the way this service's
//! own ARN builders spell it and read off the request line:
//!
//! * the tagging surface binds its target's whole ARN as its one path label
//!   (`/tags/{ResourceArn}` — `smithy_query(operation="TagResource")`), so that ARN is
//!   already the resource;
//! * a detector and everything under it, and one Malware Protection plan, are addressed by
//!   paths that ARE the resource paths this service's ARNs carry — so
//!   `/detector/{DetectorId}/ipset/{IpSetId}` is judged against the `ipset` resource and
//!   `/malware-protection-plan/{MalwareProtectionPlanId}` against the plan's own ARN
//!   ([`crate::state::malware_protection_plan_arn`] builds exactly that);
//! * one association of a custom detection rule has an ARN whose resource path is NOT its
//!   URI path, so it is built by the one function that builds it
//!   ([`crate::state::custom_detection_rule_association_arn`]);
//! * an operation whose URI binding carries no label for the association it acts on names
//!   the rule the association belongs to in ANOTHER binding of the same request — the
//!   `ruleId` query parameter of the listing and the `ruleId` body member of the create
//!   ([`RULE_ID_BINDINGS`], which is read from the member's own binding traits) — so the
//!   resource is that rule's association ARN with [`ANY_RESOURCE`] in the place of the
//!   association id the request does not carry, which for a create the service has yet to
//!   mint;
//! * a request that addresses no single resource of this account in this Region — a
//!   collection, and the whole published-rule catalogue, whose rules are AWS-MANAGED so
//!   that their ARNs carry neither this account nor this Region
//!   ([`crate::state::custom_detection_rule_arn`]) — is judged against every resource of
//!   this service in the account and Region addressed ([`ANY_RESOURCE`]), which is how IAM
//!   writes the resource of such a request.
//!
//! Taking the identifiers from the REQUEST rather than from a stored record is also what
//! keeps a refused call from telling an unauthorized caller whether what it named exists.
//!
//! This service publishes no resource-based policy of its own — the model declares no
//! policy operation and no policy member — so the decision here is the identity-based one
//! alone.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no user can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, ServerHttpProtocol, Service,
};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `guardduty:<OperationName>` ("Actions
/// defined by Amazon GuardDuty", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` / `endpointPrefix` (`smithy_query(query="service_info")`) and
/// also the name every request to it is signed under.
pub const ACTION_PREFIX: &str = "guardduty";

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this restJson1 service renders an error in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The resource an action that addresses no single resource of this account in this Region
/// is judged against, as IAM writes it for such a request: every resource of this service
/// in the account and Region addressed.
pub const ANY_RESOURCE: &str = "*";

/// The first path segment of the tagging surface, whose one label is the whole ARN of the
/// resource the call addresses (`/tags/{ResourceArn}` —
/// `smithy_query(operation="TagResource")`).
const TAGS_PATH_SEGMENT: &str = "tags";

/// The first path segment every detector-rooted operation is bound under
/// (`/detector/{DetectorId}…` — `smithy_query(operation="GetDetector")`), which is also
/// the first segment of the `detector` resource path its ARNs carry.
const DETECTOR_PATH_SEGMENT: &str = "detector";

/// The first path segment the Malware Protection plan operations are bound under
/// (`/malware-protection-plan/{MalwareProtectionPlanId}` —
/// `smithy_query(operation="GetMalwareProtectionPlan")`), which is also the plan's own ARN
/// resource path ([`crate::state::malware_protection_plan_arn`]).
const MALWARE_PROTECTION_PLAN_PATH_SEGMENT: &str = "malware-protection-plan";

/// The path the custom detection rule operations are bound under, and the segment one
/// rule's ASSOCIATIONS hang off
/// (`/custom-detection-rule/rule/{RuleId}/association/{AssociationId}` —
/// `smithy_query(operation="GetCustomDetectionRuleAssociation")`).
const CUSTOM_DETECTION_RULE_PATH_SEGMENT: &str = "custom-detection-rule";
const RULE_PATH_SEGMENT: &str = "rule";
const ASSOCIATION_PATH_SEGMENT: &str = "association";

/// Every operation of this model that declares `AccessDeniedException`, i.e. the whole set
/// this gate is asked about, as `Operation::NAME` spells each one.
///
/// Source: the modelled error set of each operation
/// (`smithy_query(query="operation",operation="…")`, which is also what each handler's own
/// `…ErrorModeled` enum lists). An operation absent from this list models no 403, so there
/// is no modelled answer for it to refuse an unauthorized caller with and it is left
/// unjudged; the list is asserted against the model by this module's own test.
pub const GATED_OPERATIONS: &[&str] = &[
    "CreateCustomDetectionRuleAssociation",
    "CreateCustomDetectionRuleOrgConfiguration",
    "CreateIPSet",
    "CreateInvestigation",
    "CreateMalwareProtectionPlan",
    "CreateThreatIntelSet",
    "DeleteCustomDetectionRuleAssociation",
    "DeleteCustomDetectionRuleOrgConfiguration",
    "DeleteMalwareProtectionPlan",
    "GetCustomDetectionRule",
    "GetCustomDetectionRuleAssociation",
    "GetCustomDetectionRuleOrgConfiguration",
    "GetInvestigation",
    "GetMalwareProtectionPlan",
    "ListCustomDetectionRuleAssociations",
    "ListCustomDetectionRuleOrgConfigurations",
    "ListCustomDetectionRules",
    "ListInvestigationRecords",
    "ListInvestigations",
    "ListMalwareProtectionPlans",
    "ListTagsForResource",
    "SendObjectMalwareScan",
    "TagResource",
    "UntagResource",
    "UpdateCustomDetectionRuleAssociation",
    "UpdateCustomDetectionRuleOrgConfiguration",
    "UpdateIPSet",
    "UpdateMalwareProtectionPlan",
    "UpdateThreatIntelSet",
];

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The resource the request's URI addresses, in the form the decision needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Addressed {
    /// A whole ARN: one the request itself carries (the tagging surface binds its target
    /// that way), or one built by the function that owns that resource's ARN.
    Arn(String),
    /// A resource of THIS service, as the resource portion of its ARN — which for the
    /// detector-rooted and Malware Protection plan spaces is the request path itself.
    Resource(String),
    /// The request addresses no single resource of this account in this Region — a
    /// collection, or the AWS-managed published-rule catalogue — so the action is judged
    /// against every resource of this service in the account and Region addressed.
    Account,
}

/// The ARN the request NAMES, which is what the action is judged against.
///
/// Read from the REQUEST rather than from a stored record, so the decision does not depend
/// on whether that resource exists — which is also what keeps a refused call from telling
/// an unauthorized caller whether it does.
#[must_use]
pub fn judged_resource(ctx: &OperationContext<'_>, operation: &str, req: &RtRequest) -> String {
    let region = crate::state::request_region(ctx);
    match addressed(
        ::service_common::request_path(req.uri()),
        named_rule(operation, req).as_deref(),
        &region,
    ) {
        // Already whole: the tagging surface carries the ARN of its target, and an
        // association's ARN is built by the one function that builds it.
        Addressed::Arn(arn) => arn,
        Addressed::Resource(resource) => ::engine::emulator_arn(ACTION_PREFIX, &region, &resource),
        Addressed::Account => ::engine::emulator_arn(ACTION_PREFIX, &region, ANY_RESOURCE),
    }
}

/// Where an operation of this model carries its `RuleId` when its URI binding carries no
/// label for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleIdBinding {
    /// `@httpQuery("ruleId")`: the value travels in the request's query string.
    Query,
    /// A body member (`@jsonName("ruleId")`): the value travels in the restJson1 document.
    Body,
}

/// The operations whose `RuleId` is NOT a URI label, with the binding each one's model
/// gives that member.
///
/// Source: the member's own HTTP binding traits
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,ListCustomDetectionRuleAssociations")`),
/// asserted against the model by this module's own test — so the id is read from the
/// binding the request actually carries it in and one operation's query string can never
/// decide a resource another operation takes from its body. Every other operation of this
/// service takes its identifiers from its URI path ([`addressed`]).
const RULE_ID_BINDINGS: &[(&str, RuleIdBinding)] = &[
    (
        "CreateCustomDetectionRuleAssociation",
        RuleIdBinding::Body,
    ),
    (
        "ListCustomDetectionRuleAssociations",
        RuleIdBinding::Query,
    ),
];

/// The `RuleId` `operation` names OUTSIDE its URI path, read from the binding its model
/// gives that member ([`RULE_ID_BINDINGS`]), or `None` for an operation that binds the
/// member nowhere but its path.
///
/// Only the member that identifies the resource is read: the decision rests on the action
/// and the resource, and nothing else of the query or the body takes part. The value is
/// decoded the way the codec that deserializes that binding decodes it, so the id read
/// here is the one the handler reads.
fn named_rule(operation: &str, req: &RtRequest) -> Option<String> {
    let member = crate::state::RULE_ID_MEMBER;
    let binding = RULE_ID_BINDINGS
        .iter()
        .find_map(|(name, binding)| (*name == operation).then_some(*binding))?;
    let named = match binding {
        RuleIdBinding::Query => req
            .uri()
            .split_once('?')
            .and_then(|(_, query)| ::service_common::query_parameter(query, member))
            .map(::smithy_server_runtime::percent_decode),
        RuleIdBinding::Body => match ::serde_json::from_slice(req.body().bytes()?) {
            Ok(::serde_json::Value::Object(members)) => members
                .get(member)?
                .as_str()
                .map(::std::string::ToString::to_string),
            _ => None,
        },
    };
    named.filter(|rule_id| !rule_id.is_empty())
}

/// What `path` addresses, through `region`'s endpoint, where `named_rule` is the `RuleId`
/// the rest of the request names ([`named_rule`]).
///
/// Every label is decoded the way the codec that deserializes the request decodes it
/// ([`::service_common::percent_decode_path_label`]), so an ARN label and a detector id
/// are read exactly as the handler reads them.
fn addressed(path: &str, named_rule: Option<&str>, region: &str) -> Addressed {
    let segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(::service_common::percent_decode_path_label)
        .collect();
    let segment = |index: usize| segments.get(index).map(String::as_str);
    match segment(0) {
        // The tagging surface's one label IS the target's ARN; without one it names no
        // resource at all.
        Some(TAGS_PATH_SEGMENT) => match segment(1) {
            Some(arn) => Addressed::Arn(arn.to_string()),
            None => Addressed::Account,
        },
        // A detector is addressed by the id GuardDuty minted for it, and every
        // sub-resource of it by a path that is also its ARN's resource path. A request
        // carrying no detector id addresses the collection.
        Some(DETECTOR_PATH_SEGMENT) if segment(1).is_some() => {
            Addressed::Resource(segments.join("/"))
        }
        // Likewise one Malware Protection plan, whose path is exactly the resource path
        // its own ARN carries.
        Some(MALWARE_PROTECTION_PLAN_PATH_SEGMENT) if segment(1).is_some() => {
            Addressed::Resource(segments.join("/"))
        }
        // One association of a published rule is the ACCOUNT's own resource, and its ARN
        // resource path is not its URI path — so it is built by the function that owns it.
        Some(CUSTOM_DETECTION_RULE_PATH_SEGMENT)
            if segment(1) == Some(RULE_PATH_SEGMENT)
                && segment(3) == Some(ASSOCIATION_PATH_SEGMENT) =>
        {
            match (segment(2), segment(4)) {
                (Some(rule_id), Some(association_id)) => Addressed::Arn(
                    crate::state::custom_detection_rule_association_arn(
                        region,
                        rule_id,
                        association_id,
                    ),
                ),
                _ => Addressed::Account,
            }
        }
        // The association collection, whose operations carry the rule their associations
        // belong to in the request's query string or its body rather than in its path
        // ([`named_rule`]): the resource is that rule's own association ARN, with the
        // ANY-association wildcard where the request names no association id — which a
        // create has not minted yet. A request naming no rule addresses every association
        // of the account in this Region.
        Some(CUSTOM_DETECTION_RULE_PATH_SEGMENT)
            if segment(1) == Some(ASSOCIATION_PATH_SEGMENT) && segment(2).is_none() =>
        {
            match named_rule {
                Some(rule_id) => {
                    Addressed::Arn(crate::state::custom_detection_rule_association_arn(
                        region,
                        rule_id,
                        ANY_RESOURCE,
                    ))
                }
                None => Addressed::Account,
            }
        }
        // Everything else names no single resource of this account in this Region: a
        // collection, the organization's own surfaces, and the published-rule catalogue,
        // whose rules are AWS-managed.
        _ => Addressed::Account,
    }
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request
/// ADDRESSES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. Taken BEFORE the store is reached, so
/// a refused call writes nothing.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation of this
/// service may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this service publishes no resource-based policy, so the decision
    // below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow is
    // not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it — the one sentence every authorizing
/// service in this tree words it as ([`iam::not_authorized_by_identity_policy`]). Every
/// value in it comes from the request and from the stored policies.
///
/// `type` carries the error class the body's `__type` discriminator also carries
/// (`smithy_query(shape="AccessDeniedException")` — `Type` is "The error type"), the same
/// way every modelled refusal this service answers does.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, reason,
        )),
        r#type: Some(ACCESS_DENIED_EXCEPTION.to_string()),
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled 403, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in the layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status,
/// the shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer
/// can fail: a value it cannot render.
#[must_use]
pub fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    denial: &AccessDeniedException,
) -> RtResponse {
    protocol
        .serialize_error(
            denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|_| {
            let body = ::serde_json::json!({
                "__type": ACCESS_DENIED_EXCEPTION,
                "message": denial.message.clone().unwrap_or_default(),
                "type": ACCESS_DENIED_EXCEPTION,
            });
            let mut response = RtResponse::new(
                StatusCode::try_from(ACCESS_DENIED_STATUS)
                    .expect("the status is a constant of this module"),
                SdkBody::from(body.to_string()),
            );
            response
                .headers_mut()
                .insert("content-type", JSON_CONTENT_TYPE);
            response
                .headers_mut()
                .insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
            response
        })
}

/// One modelled operation, served behind IAM's decision about the action it names.
///
/// It carries the operation TYPE — which is what names the action and resolves the delegate
/// — and a handle on the ONE shared engine, which is what the decision reads the caller's
/// identity and its policies through. Every request it does not refuse is delegated to the
/// one operation instance the generated routing table built from that same engine, so there
/// is no second instance of any operation and no second store.
pub struct Gated<Op, Eng> {
    operation: PhantomData<fn() -> Op>,
    engine: Arc<Eng>,
}

impl<Op, Eng> Gated<Op, Eng> {
    /// Wrap one operation, judged against the policies held in `engine`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: PhantomData,
            engine,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for Gated<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // IAM's decision, taken before the operation reads or writes anything: the
            // action is this operation's own name under this service's prefix, and the
            // resource is the one the REQUEST names. A refused request never reaches the
            // delegate, so it mints nothing, moves no status and reports nothing.
            let resource = judged_resource(ctx, Op::NAME, req);
            if let Err(denial) = authorize(&*self.engine, ctx, Op::NAME, &resource) {
                // The protocol is read, used and dropped inside this one statement:
                // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
                // await below would make this future non-`Send`.
                return refusal_response(svc.definition().protocol(), &denial);
            }
            match svc.definition().routing_table().get(Op::NAME) {
                // The modelled walk — deserialize, validate, handle, serialize — runs
                // behind the delegate exactly as it did before, so nothing of how this
                // service refuses a bad request changes.
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key the
                // generated table registered. Answer the framework's own no-such-operation
                // response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed as the MODEL
        // says for the operation it names, so authentication is decided by the modelled
        // operation this wrapper stands in front of.
        None
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the IAM
    /// role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(engine: &Eng, ctx: &OperationContext<'_>) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            let role_arn = held.arn.clone();
            caller.principals.push(role_arn.clone());
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json"
    );

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: every operation in [`RULE_ID_BINDINGS`] is read from the binding its own model
    /// gives `RuleId`, so the id the decision uses is the one the request carries rather
    /// than one another operation's binding would have carried.
    ///
    /// The bindings come from the MODEL rather than from a second hand list: an operation
    /// that moves the member fails here instead of letting the gate judge a resource the
    /// request never named.
    #[test]
    fn each_operation_is_read_from_the_binding_its_model_gives_the_rule_id() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = &model["shapes"];
        for (operation, binding) in RULE_ID_BINDINGS {
            let input = shapes[format!("com.amazonaws.guardduty#{operation}")]["input"]["target"]
                .as_str()
                .unwrap_or_else(|| panic!("case {operation:?}: the model binds an input shape"))
                .to_string();
            let traits = &shapes[&input]["members"]["RuleId"]["traits"];
            let query = traits["smithy.api#httpQuery"].as_str();
            let json_name = traits["smithy.api#jsonName"].as_str();
            let label = traits["smithy.api#httpLabel"].is_object();
            let member = crate::state::RULE_ID_MEMBER;
            match binding {
                RuleIdBinding::Query => assert_eq!(
                    (query, label),
                    (Some(member), false),
                    "case {operation:?}: expected RuleId bound to the query parameter \
                     {member:?}, got {traits:?}"
                ),
                RuleIdBinding::Body => assert_eq!(
                    (json_name, query, label),
                    (Some(member), None, false),
                    "case {operation:?}: expected RuleId bound to the body member {member:?}, \
                     got {traits:?}"
                ),
            }
        }
    }

    /// RULE applied to the whole surface: [`GATED_OPERATIONS`] is EXACTLY the set of
    /// operations whose model declares `AccessDeniedException`.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// that gains or loses the 403 fails here instead of silently serving an unauthorized
    /// caller (or answering an error its own model does not declare).
    #[test]
    fn the_gated_set_is_exactly_the_operations_modelling_the_refusal() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let mut modelled: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter(|(_, shape)| {
                shape["errors"].as_array().is_some_and(|errors| {
                    errors.iter().any(|error| {
                        error["target"].as_str().is_some_and(|target| {
                            target.ends_with(&format!("#{ACCESS_DENIED_EXCEPTION}"))
                        })
                    })
                })
            })
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        modelled.sort_unstable();
        let mut gated: Vec<&str> = GATED_OPERATIONS.to_vec();
        gated.sort_unstable();
        assert_eq!(
            gated, modelled,
            "case \"the whole surface\": expected the gated set to be the operations \
             modelling {ACCESS_DENIED_EXCEPTION}, got {gated:?} against {modelled:?}"
        );
    }

    /// RULE: the action this gate asks about is `guardduty:<OperationName>`, so a policy
    /// scoped to this service's actions covers every gated operation and nothing outside
    /// this service.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"{ACTION_PREFIX}:*","Resource":"*"}}]}}"#);
        for operation in GATED_OPERATIONS {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(&document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected {ACTION_PREFIX}:* to allow {action}, got \
                 {reached:?}"
            );
        }
        let outside = decision(&document, "iam:CreateUser", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected {ACTION_PREFIX}:* to grant nothing outside \
             this service, got {outside:?}"
        );
    }

    /// RULE: the resource a call is judged against is the one the REQUEST names — the
    /// detector-rooted and Malware Protection plan paths, which are the resource paths
    /// this service's ARNs carry; the ARN the tagging surface binds as its label; the ARN
    /// the association builder spells, from the rule the request names in whichever
    /// binding carries it and with the any-association wildcard where it names no
    /// association id; and the any-resource wildcard for a call that names no single
    /// resource of this account in this Region.
    #[test]
    fn the_request_decides_the_resource_the_action_is_judged_against() {
        /// The Region every row is read through, so an account-scoped ARN carries it.
        const REGION: &str = "us-east-1";
        // A detector id of the minted form, and the ids of the sub-resources under it.
        let detector = "11111111111111111111111111111111";
        let plan_arn = crate::state::malware_protection_plan_arn(REGION, "plan-0001");
        // (the case, the request path, the rule the rest of the request names, what it
        // names)
        let rows: &[(&str, String, Option<&str>, Addressed)] = &[
            (
                "an investigation created under one detector",
                format!("/detector/{detector}/investigation"),
                None,
                Addressed::Resource(format!("detector/{detector}/investigation")),
            ),
            (
                "one investigation of one detector",
                format!("/detector/{detector}/investigation/abc-123"),
                None,
                Addressed::Resource(format!("detector/{detector}/investigation/abc-123")),
            ),
            (
                "one IPSet of one detector",
                format!("/detector/{detector}/ipset/ip-0001"),
                None,
                Addressed::Resource(format!("detector/{detector}/ipset/ip-0001")),
            ),
            (
                "one threat intel set of one detector",
                format!("/detector/{detector}/threatintelset/ti-0001"),
                None,
                Addressed::Resource(format!("detector/{detector}/threatintelset/ti-0001")),
            ),
            (
                "the detector collection, which names no detector",
                "/detector".to_string(),
                None,
                Addressed::Account,
            ),
            (
                "one Malware Protection plan",
                "/malware-protection-plan/plan-0001".to_string(),
                None,
                Addressed::Resource("malware-protection-plan/plan-0001".to_string()),
            ),
            (
                "the Malware Protection plan collection",
                "/malware-protection-plan".to_string(),
                None,
                Addressed::Account,
            ),
            (
                "one AWS-managed published rule, which is no resource of this account",
                "/custom-detection-rule/rule/suspicious-login".to_string(),
                None,
                Addressed::Account,
            ),
            (
                "the published rule catalogue",
                "/custom-detection-rule/rule".to_string(),
                None,
                Addressed::Account,
            ),
            (
                "an organization configuration of a published rule",
                "/custom-detection-rule/org-configuration/suspicious-login".to_string(),
                None,
                Addressed::Account,
            ),
            (
                "one association of a published rule, which the account owns",
                "/custom-detection-rule/rule/suspicious-login/association/as-0001".to_string(),
                None,
                Addressed::Arn(crate::state::custom_detection_rule_association_arn(
                    REGION,
                    "suspicious-login",
                    "as-0001",
                )),
            ),
            (
                "the association collection, for the rule the request names",
                "/custom-detection-rule/association".to_string(),
                Some("suspicious-login"),
                Addressed::Arn(crate::state::custom_detection_rule_association_arn(
                    REGION,
                    "suspicious-login",
                    ANY_RESOURCE,
                )),
            ),
            (
                "the association collection, for another rule",
                "/custom-detection-rule/association".to_string(),
                Some("credential-exfiltration"),
                Addressed::Arn(crate::state::custom_detection_rule_association_arn(
                    REGION,
                    "credential-exfiltration",
                    ANY_RESOURCE,
                )),
            ),
            (
                "the association collection, which names no rule at all",
                "/custom-detection-rule/association".to_string(),
                None,
                Addressed::Account,
            ),
            (
                "the tagging surface, whose label is the target's ARN",
                format!("/tags/{}", encoded(&plan_arn)),
                None,
                Addressed::Arn(plan_arn.clone()),
            ),
            (
                "the object malware scan path, which names no resource",
                "/object-malware-scan/send".to_string(),
                None,
                Addressed::Account,
            ),
        ];
        for (case, path, rule, expected) in rows {
            let reached = addressed(path, *rule, REGION);
            assert_eq!(
                &reached, expected,
                "case {case:?}: expected {path:?} to name {expected:?}, got {reached:?}"
            );
        }
        // The plan's own ARN builder and the gate's reading of the plan's path agree, so a
        // statement written about the ARN a read reports covers the call that addresses it.
        let plan_resource = match addressed("/malware-protection-plan/plan-0001", None, REGION) {
            Addressed::Resource(resource) => resource,
            other => panic!("a plan path names a resource of this account, got {other:?}"),
        };
        let judged = ::engine::emulator_arn(ACTION_PREFIX, REGION, &plan_resource);
        assert_eq!(
            judged, plan_arn,
            "case \"the plan ARN\": expected the judged resource to be the ARN the read \
             reports, got {judged:?}"
        );
    }

    /// Percent-encode one path label the way a client does, so the test's tagging row
    /// arrives as a client sends it.
    fn encoded(label: &str) -> String {
        label
            .chars()
            .map(|character| match character {
                ':' => "%3A".to_string(),
                '/' => "%2F".to_string(),
                other => other.to_string(),
            })
            .collect()
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused it — every part taken from the request
    /// and the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/investigator/probe",
            ::engine::emulator_account_id()
        );
        let action = format!("{ACTION_PREFIX}:CreateInvestigation");
        let resource = ::engine::emulator_arn(
            ACTION_PREFIX,
            "us-east-1",
            "detector/11111111111111111111111111111111/investigation",
        );
        for reason in [
            iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
            iam::no_identity_policy_allows(&action),
            iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            iam::no_permissions_boundary_allows(&action),
        ] {
            let refusal = not_authorized(&principal, &action, &resource, &reason);
            let message = refusal.message.unwrap_or_default();
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {reason}"
                ),
                "case {reason:?}: expected IAM's wording, got {message:?}"
            );
            assert_eq!(
                refusal.r#type.as_deref(),
                Some(ACCESS_DENIED_EXCEPTION),
                "case {reason:?}: expected the refusal to name its own class"
            );
        }
    }
}

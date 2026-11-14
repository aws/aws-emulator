// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the CloudWatch action it names — the
//! decision taken in FRONT of the operations whose action is judged over every resource,
//! before any handler reaches the store.
//!
//! A signed call is decided against the calling principal's identity-based policies before
//! the service does anything with it, so a caller holding no grant is told it is not
//! authorized rather than served: a datapoint it may not publish does not land, and a read
//! it may not make answers nothing about what the account holds. The refusal is HTTP 403
//! under the code a client branches on for this class ([`ACCESS_DENIED`]), in the envelope
//! of whichever of this service's protocols the request arrived in.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the SESSION it was
//!    minted for, read from the account's shared session directory
//!    ([`::service_common::management_events::SessionAuditDirectory`], where the minting
//!    service records every session it hands out), else the IAM user that holds that
//!    long-term key, read from IAM's own `access_keys` map (the map `iam:GetUser` resolves a
//!    caller through, so the principal a refusal blames is the one IAM holds the key under),
//!    else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a document `iam:PutRolePolicy` wrote takes effect on
//!    the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default deny.
//!    The SENTENCE a refusal carries is that module's
//!    [`::service_common::iam_authorization::not_authorized_by_identity_policy`] and the
//!    reason clauses beside it, so nothing of IAM's wording is spelled here.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal ARN
//! does. The lookups stay in this crate because the IAM service crate is reached from here
//! and `service-common` cannot reach it without every service that depends on that crate
//! linking IAM.
//!
//! WHICH session a temporary credential belongs to is read from the SHARED directory rather
//! than from the token service's own store, because that store is unreachable from here: the
//! token service depends (through the user-pool provider it verifies web identity tokens
//! against) on this service, so a dependency back on it is a cycle Cargo refuses. The
//! directory is the one place in this tree where the facts about a session live for every
//! service that serves a call made with it, which is exactly the position this gate is in.
//!
//! ## The action, and which operations this decides
//!
//! The action is `cloudwatch:<OperationName>` ("Actions defined by Amazon CloudWatch", IAM
//! Service Authorization Reference) — [`ACTION_PREFIX`], which is NOT this service's signing
//! name (`monitoring`, `smithy_query(query="service_info")`): the API is reached under the
//! endpoint prefix and its actions are published under the service's own name.
//!
//! The operations decided here are the ones that address no single resource of their own:
//! the IAM reference lists their actions under "actions with no resource types", so IAM
//! judges one against every resource ([`EVERY_RESOURCE`]) and the decision is a pure
//! function of the caller's policies and the action name
//! ([`JUDGED_OVER_EVERY_RESOURCE`]). Every OTHER operation of this service is judged by IAM
//! against the ARN of the alarm, dashboard or rule the request addresses, and that is not
//! something this gate can resolve — the request carries a NAME, and which resource an
//! action is authorized on differs per operation. Withholding a grant from such a request
//! here would refuse calls AWS permits (a policy scoped to one alarm's ARN grants the action
//! on that alarm), so they are left to the handler that knows which resource they address
//! and continue to be served as before.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither the
//!   session directory nor IAM holds a record for resolves to the account's own identity,
//!   which is not a principal the IAM store holds policies for — the position AWS account
//!   root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the account's store may hold no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions boundary
//!   the store DOES hold still caps the grant.
//! * **An engine carrying no IAM store**, where no principal and no policy can exist.
//!
//! The decision is taken with no guard of this service's own store held, it writes nothing to
//! either store it reads, and the work it does is bounded by what IAM holds for ONE principal
//! rather than by anything in the request.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Refused, Service};

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The error code this service refuses a caller the IAM policies in force do not permit
/// with, which is what a client branches on for "not permitted". It belongs to the
/// authorization layer in front of the service rather than to any one operation, so no
/// operation of this model declares a shape for it.
pub const ACCESS_DENIED: &str = "AccessDenied";

/// How a policy statement spells one of this service's actions: "Actions defined by Amazon
/// CloudWatch" are published under this prefix (IAM Service Authorization Reference), which
/// differs from the name the API is signed and reached under (see the module docs).
pub const ACTION_PREFIX: &str = "cloudwatch";

/// The resource an action that supports no resource types is judged against, as IAM writes it
/// for such a request: every resource.
pub const EVERY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose credential speaks for nobody this
/// emulator holds a record of — never the key id it presented, which this module does not
/// quote back.
const AMBIENT_PRINCIPAL: fn() -> String = ::service_common::caller_principal::ambient_user_arn;

/// The operations of this service whose action IAM judges over EVERY resource, because the
/// IAM Service Authorization Reference lists it under "actions with no resource types": the
/// metric data plane (publishing a datapoint, the three reads of the series and the catalogue
/// listing, and the alarm listing that is addressed BY A METRIC rather than by an alarm) and
/// the anomaly detectors of those series.
///
/// Source: the "Actions defined by Amazon CloudWatch" table of that reference, whose
/// "Resource types (*required)" column is empty for each of these. Sorted, and read with a
/// binary search, so the set is a fact about the table rather than about the order a call
/// arrives in.
const JUDGED_OVER_EVERY_RESOURCE: &[&str] = &[
    "DeleteAnomalyDetector",
    "DescribeAlarmsForMetric",
    "DescribeAnomalyDetectors",
    "GetMetricData",
    "GetMetricStatistics",
    "GetMetricWidgetImage",
    "ListMetrics",
    "PutAnomalyDetector",
    "PutMetricData",
];

/// Whether IAM judges `operation`'s action over every resource, so this gate can decide it
/// from the caller's policies and the action name alone (see the module docs).
#[must_use]
pub fn judged_over_every_resource(operation: &str) -> bool {
    JUDGED_OVER_EVERY_RESOURCE.binary_search(&operation).is_ok()
}

/// The IAM action `operation` is performed under: `cloudwatch:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// One operation of this service, served only to a caller IAM allows to perform it.
///
/// Wrapping is what makes the decision ONE decision: every request this service serves passes
/// through here, so no handler can forget it and none of them takes it twice. An operation
/// whose action this gate does not decide is handed straight through (see the module docs).
pub struct AuthorizedByIam<Eng, O> {
    /// The shared engine, so the principal's policies are read from the one store every
    /// service of this emulator shares.
    engine: Arc<Eng>,
    /// The operation this one wraps, served for every request IAM allows.
    operation: O,
}

impl<Eng, O> AuthorizedByIam<Eng, O> {
    /// Put `operation`, already constructed from the shared engine, behind the IAM decision.
    pub fn new(engine: Arc<Eng>, operation: O) -> Self {
        Self { engine, operation }
    }
}

impl<Eng, O> ErasedOperation for AuthorizedByIam<Eng, O>
where
    Eng: Send + Sync + 'static,
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = refusal(&*self.engine, svc, req, ctx) {
                return refused;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// This service's [`ACCESS_DENIED`] answer for a request whose caller the IAM policies in
/// force do not permit to perform the action it named, or `None` when they do — and for every
/// operation whose action this gate does not decide ([`judged_over_every_resource`]).
///
/// The action asked about is the operation's own name, taken from the routing key the protocol
/// THIS REQUEST is served in resolves, so it is always a modelled operation's name and never a
/// value the body carried.
///
/// The code and the sentence are stated here; the status is the 403 the framework answers an
/// authorization refusal with, and the envelope is the one this request's own protocol carries
/// an error in — closed by the `<RequestId>` writer this service wraps every answer in — so
/// the refusal is rendered exactly like every other error of this service.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    svc: &dyn Service,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
) -> Option<RtResponse> {
    let protocol = svc.definition().wire_protocol(req);
    let operation = protocol.protocol().routing_key(req).ok()?;
    if !judged_over_every_resource(operation) {
        return None;
    }
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    // Nothing this gate can read bears on the request: there is no document that could deny
    // it, no boundary that could cap it, and the credential is one the gate does not bound.
    if caller.documents.is_empty() && caller.boundary.is_none() && caller.unbounded() {
        return None;
    }
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no such
    // element, so the decision rests on the action and the resource.
    let request = iam::Request::new(&action, EVERY_RESOURCE, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // Taken FIRST, so an explicit `Deny` in a document that CAN be read refuses the request
    // whatever else the principal carries — including a grant this emulator cannot read, which
    // only lifts the DEFAULT deny of a request no readable statement allows.
    let reason = if identity_said == Decision::Denied {
        iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
    } else if !(iam::is_allowed(identity_said, Decision::NotStated) || caller.unbounded()) {
        iam::no_identity_policy_allows(&action)
    } else {
        // "A permissions boundary sets the maximum permissions that an identity-based policy
        // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
        // is not a permission the principal has. The account's own credential carries none.
        match caller
            .boundary
            .as_ref()
            .map(|boundary| iam::identity_decision(::std::slice::from_ref(boundary), &request))
        {
            None | Some(Decision::Allowed) => return None,
            // An explicit deny is a fact about the boundary document, so the refusal names it;
            // a boundary that merely states nothing withheld the grant the policies gave.
            Some(Decision::Denied) => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Some(Decision::NotStated) => iam::no_permissions_boundary_allows(&action),
        }
    };
    let message =
        iam::not_authorized_by_identity_policy(&caller.arn, &action, EVERY_RESOURCE, &reason);
    tracing::info!(
        operation = %operation,
        action = %action,
        "iam does not allow this caller to perform this action"
    );
    Some(svc.definition().refusal_response(
        ProtocolError::Unauthenticated {
            code: ACCESS_DENIED,
            message: ACCESS_DENIED,
        },
        Some(Refused {
            code: ACCESS_DENIED.to_string(),
            message,
        }),
        req,
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN the token service reports, and the ARN
    /// of the IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose document
    /// this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(engine: &Eng, ctx: &OperationContext<'_>) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let mut caller = Caller {
        arn: AMBIENT_PRINCIPAL(),
        principals: vec![AMBIENT_PRINCIPAL()],
        documents: Vec::new(),
        boundary: None,
        account_credential: true,
        unreadable_attachment: false,
    };
    // A SESSION credential: the account's shared session directory holds the session every
    // temporary credential this emulator minted belongs to, so a request signed with one
    // speaks for THAT session — the principal `sts:GetCallerIdentity` reports for it — and is
    // judged on the documents in force on the role that issued it.
    let session = session_directory(engine)
        .and_then(|directory| directory.lock().session(&access_key_id).cloned());
    // No IAM store on this engine: no principal and no policy can exist, so nothing can be
    // judged and the caller is left unbounded, whatever credential it presented.
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    if let Some(session) = session {
        caller.account_credential = false;
        caller.arn = session.arn.clone();
        caller.principals = vec![session.arn];
        // "The permissions of the temporary security credentials are determined by the role
        // that was assumed": the documents are the ISSUER's, under the ARN the account holds
        // the role by — which carries its PATH, where the session ARN does not.
        match session.issuer {
            Some(issuer) => {
                caller.principals.push(issuer.arn.clone());
                caller.read_policies_of(&store, &issuer.arn);
            }
            None => {
                let principal = caller.arn.clone();
                caller.read_policies_of(&store, &principal);
            }
        }
        return caller;
    }
    // IAM's store maps an access key id to the user that owns it (`access_keys`, the map
    // IAM's own `GetUser` resolves a caller through), so a request signed with one speaks for
    // THAT user and is judged on the user's policies rather than served as the account. A key
    // that store does not hold either is the account's own published credential.
    if let Some(user) = store
        .access_keys
        .get(&access_key_id)
        .and_then(|record| store.users.get(&record.user_key))
    {
        caller.account_credential = false;
        caller.arn = user.arn.clone();
        caller.principals = vec![user.arn.clone()];
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The account's SESSION directory in the shared engine: every session this emulator minted,
/// keyed by the access key id it was minted under.
///
/// A SHARED type ([`::service_common::management_events::SessionAuditDirectory`]) rather than
/// a field of any service's own state, because the service that MINTS a session is not the
/// service that serves the calls made with it (see the module docs). Read-only here: the gate
/// writes nothing.
fn session_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::management_events::SessionAuditDirectory,
            >,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one stored document says about `action` over every resource, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, EVERY_RESOURCE, &[]),
        )
    }

    /// RULE: the action this gate asks about is `cloudwatch:<OperationName>`, so a document
    /// scoped to this service's actions decides every operation of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_action_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"cloudwatch:*","Resource":"*"}]}"#;
        for operation in JUDGED_OVER_EVERY_RESOURCE {
            let action = action_for(operation);
            let reached = decision(document, &action);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected cloudwatch:* to allow {action}, got {reached:?}"
            );
        }
        // The endpoint prefix is not the action prefix: a statement spelled with the signing
        // name grants nothing about this service.
        let endpoint_spelling = decision(document, "monitoring:PutMetricData");
        assert_eq!(
            endpoint_spelling,
            Decision::NotStated,
            "case \"the endpoint prefix\": expected no grant, got {endpoint_spelling:?}"
        );
        let outside = decision(document, "logs:PutLogEvents");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected cloudwatch:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: a grant scoped to particular resource ARNs does not cover an action IAM judges
    /// over EVERY resource, which is the reason this gate decides those actions alone.
    #[test]
    fn a_resource_scoped_grant_does_not_cover_an_action_judged_over_every_resource() {
        let scoped = r#"{"Statement":[{"Effect":"Allow","Action":"cloudwatch:PutMetricData",
             "Resource":"arn:aws:cloudwatch:us-west-2:000000000000:alarm:web"}]}"#;
        let reached = decision(scoped, &action_for("PutMetricData"));
        assert_eq!(
            reached,
            Decision::NotStated,
            "case \"a resource-scoped grant\": expected no grant of an action judged over every \
             resource, got {reached:?}"
        );
    }

    /// RULE: the operations decided here are the ones whose action supports no resource types;
    /// every other operation of this service is left to the handler that resolves the resource
    /// it addresses.
    #[test]
    fn only_the_actions_with_no_resource_types_are_decided_here() {
        // (the operation, whether IAM judges its action over every resource)
        let rows: &[(&str, bool)] = &[
            ("PutMetricData", true),
            ("GetMetricStatistics", true),
            ("GetMetricData", true),
            ("ListMetrics", true),
            ("GetMetricWidgetImage", true),
            ("DescribeAlarmsForMetric", true),
            ("PutAnomalyDetector", true),
            ("DeleteAnomalyDetector", true),
            ("DescribeAnomalyDetectors", true),
            // Judged against the ARN of the alarm, the dashboard or the rule the request
            // addresses, which this gate does not resolve.
            ("PutMetricAlarm", false),
            ("PutCompositeAlarm", false),
            ("DescribeAlarms", false),
            ("DeleteAlarms", false),
            ("SetAlarmState", false),
            ("PutDashboard", false),
            ("PutInsightRule", false),
            ("TagResource", false),
            ("PutMetricStream", false),
        ];
        for (operation, expected) in rows {
            let decided = judged_over_every_resource(operation);
            assert_eq!(
                decided, *expected,
                "case {operation:?}: expected judged_over_every_resource {expected}, got \
                 {decided}"
            );
        }
    }

    /// The catalogue is read with a binary search, so it has to be sorted — and a duplicate
    /// entry would mean the same action was listed twice.
    #[test]
    fn the_catalogue_is_sorted_and_holds_each_action_once() {
        for pair in JUDGED_OVER_EVERY_RESOURCE.windows(2) {
            assert!(
                pair[0] < pair[1],
                "case {:?}: the catalogue must be sorted and hold each operation once, got \
                 {:?} after it",
                pair[0],
                pair[1]
            );
        }
    }
}

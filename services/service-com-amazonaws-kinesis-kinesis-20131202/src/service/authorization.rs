// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the CALLER of a request to this service perform the action it asks for —
//! the decision every operation takes BEFORE it reads or writes a stream.
//!
//! Every request to an AWS service is signed, and the credential it is signed with speaks
//! for a principal: an IAM user's long-term access key, or a temporary session the token
//! service minted for a ROLE — which is what an SDK call made from inside a compute
//! environment running under an execution role presents. AWS decides such a request at the
//! IAM layer, ahead of the service looking at the stream the request named: a principal
//! whose policies do not allow the action is told it is not authorized and nothing about
//! the stream, and the call has NO effect — a refused `PutRecord` leaves no record on the
//! shard and a refused `CreateStream` stores nothing.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for ([`sts_store::resolve_caller`], the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    holds the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutRolePolicy` wrote takes effect on the next request and deleting it lifts it
//!    again — beside the RESOURCE POLICY this service holds for the resource the request
//!    addresses, which within one account grants on its own;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, and the SENTENCE a refusal carries,
//!    which is that module's `not_authorized_by_identity_policy` and the reason clauses
//!    beside it, so nothing of IAM's wording is spelled here.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle.
//!
//! ## The action and the resource
//!
//! The action is `kinesis:<OperationName>` ([`action_for`]): `kinesis` is this service's
//! `arnNamespace` and its `sigv4Name` alike (`smithy_query(query="service_info")`), which is
//! the namespace its actions are published under.
//!
//! The resource is what the request's OWN input addresses, by the ARN forms this namespace
//! publishes ([`addressed_resource`]): the stream, the consumer or the channel the request
//! names, built from the identifiers the request carried and the Region it was signed for
//! rather than resolved out of the store — so the decision is the same whether or not that
//! resource exists here, and a refusal tells an unauthorized caller nothing about what the
//! emulator holds. A request that addresses no resource of its own (a listing, an
//! account-level read) is decided on the weaker question the shared evaluator publishes for
//! exactly that case: whether any readable document could allow the action at all.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a permissions boundary the
//!   store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no document, so every action
//! it asks for is refused, and the refusal names that state rather than the key it
//! presented, which this module never quotes back.

use ::std::sync::Arc;
use ::std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::service_common::management_events::{recorded_member_name, recorded_shape, WithheldFromRecord};
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, PreDispatchProtocol, Service,
};

/// The action namespace this service's operations are published under, which is its
/// `arnNamespace` and its SigV4 signing name alike (`smithy_query(query="service_info")`): a
/// policy statement spells one of them `kinesis:<OperationName>`.
pub const ACTION_PREFIX: &str = crate::state::SERVICE_ARN_NAMESPACE;

/// The resource an action that addresses no single resource is decided against: every
/// resource, which is what a statement has to cover to permit such a call.
pub const EVERY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The envelope a pre-dispatch failure of one of this service's operations renders in: this
/// service speaks `awsJson1_1` (`smithy_query(query="service_info")`), which is the same
/// value the generated per-operation shim passes.
const JSON_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/x-amz-json-1.1",
    // The awsJson families convey the error type in the body's `__type`, not in a header.
    errortype_header: false,
};

/// The members a request's identifiers are never read out of, and which are left out of the
/// document the resource is read from: the record payloads a write carries. Nothing in them
/// addresses a resource, and a caller controls their size.
const PAYLOAD_MEMBERS: &[&str] = &["Data", "Records", "Policy"];

/// The IAM action `operation` is performed under: `kinesis:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The refusal a request to `operation` on `resource` gets when the documents in force do
/// not allow it — the sentence IAM states such a refusal with — and `None` when they do.
///
/// `operation` is the operation's own name ([`smithy_server_runtime::Operation::NAME`]) and
/// `resource` the ARN the request ADDRESSES ([`addressed_resource`]).
///
/// Asked BEFORE this service's store is written, and with an ARN built from the identifiers
/// the request carried rather than from a stored record, so the decision does not depend on
/// whether that resource exists.
///
/// Resolving the caller goes through STS's own resolution, which first materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service may
/// take this decision too.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<String> {
    let region = crate::state::request_region(ctx);
    let caller = caller(engine, ctx);
    let policy = resource_policy(engine, &region, resource);
    caller.refusal(&action_for(operation), resource, policy.as_deref())
}

/// The resource-based policy this service holds for the resource `resource` addresses, as it
/// reads at this instant — `None` for a resource that holds none, and for one this account
/// and Region do not hold at all.
///
/// A resource policy grants on its own within one account ("the resource-based policy and
/// the identity-based policy are evaluated together, and access is granted if either allows
/// it"), so the decision below reads it beside the caller's own documents. Read through this
/// service's OWN lookup ([`crate::state::ComAmazonawsKinesisKinesis20131202State::resource_policy_by_arn`]),
/// so the document a gate decides on is the one `PutResourcePolicy` wrote.
fn resource_policy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    resource: &str,
) -> Option<String> {
    if resource == EVERY_RESOURCE {
        return None;
    }
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    store
        .resource_policy_by_arn(SystemTime::now(), region, resource)
        .cloned()
        .flatten()
}

/// The ARN the request `input` ADDRESSES, by the ARN forms this namespace publishes, and
/// [`EVERY_RESOURCE`] for a request that addresses no resource of its own.
///
/// SCHEMA-DRIVEN rather than per operation: the identifiers are read off the built input
/// through the tree's one rendering of a shape as a document ([`recorded_shape`]), so an
/// operation added later is decided against the resource its own input names without a
/// second hand-kept table. The members are this model's own
/// (`smithy_query(query="operation")`), in the order a request narrows what it addresses:
///
/// * `ResourceARN` — the tagging and resource-policy operations, which address a stream or a
///   channel by ARN;
/// * `ChannelARN`, else `ChannelName` — a channel, which a create addresses by name;
/// * `QueueARN` — a queue, which is its own resource type: "the resource identifier in the ARN
///   is generated when the queue is created and is not derived from QueueName"
///   (`QueueDescription$QueueARN`), so a create of one addresses no queue ARN at all and is
///   decided on the weaker question below, the way every other create of a named resource is;
/// * `ConsumerARN` — a registered consumer, which is its own resource type;
/// * `StreamARN`, else `StreamName` — the data stream, "It is recommended that you use the
///   StreamARN input parameter when you invoke this API" (`PutRecord`), so the ARN the
///   request carried wins over the name it also carried;
/// * `ShardIterator` — a read that names its stream only inside the iterator it was handed,
///   which carries that stream's ARN ([`crate::state::ShardIteratorToken`]).
#[must_use]
pub fn addressed_resource(
    input: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    schema: &::aws_smithy_schema::Schema,
    region: &str,
) -> String {
    let document = recorded_shape(
        input,
        schema,
        WithheldFromRecord {
            omitted: PAYLOAD_MEMBERS,
            ..WithheldFromRecord::default()
        },
    );
    let text = |member: &str| {
        document
            .get(recorded_member_name(member))
            .and_then(::serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if let Some(arn) = text("ResourceARN")
        .or_else(|| text("ChannelARN"))
        .or_else(|| text("QueueARN"))
        .or_else(|| text("ConsumerARN"))
        .or_else(|| text("StreamARN"))
    {
        return arn;
    }
    if let Some(name) = text("ChannelName") {
        return crate::state::channel_arn(region, &name);
    }
    if let Some(name) = text("StreamName") {
        return crate::state::stream_arn(region, &name);
    }
    if let Some(token) = text("ShardIterator")
        .as_deref()
        .and_then(crate::state::ShardIteratorToken::decode)
    {
        return token.stream_arn;
    }
    EVERY_RESOURCE.to_string()
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal a resource policy's `Principal` element can name it
    /// by: the session ARN STS reports, the ARN of the IAM role behind an assumed-role
    /// session, and the account the principal belongs to.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
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
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// `Some(message)` when the documents in force do not let this caller perform `action` on
    /// `resource` — the sentence the refusal carries — and `None` when they do.
    ///
    /// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
    /// whatever else the principal carries; otherwise a grant is needed, which either half of
    /// the decision gives (within one account "access is granted if either allows it") and
    /// which a caller this gate does not bound has by definition (see the module docs); and a
    /// grant is capped by the permissions boundary, since "a permissions boundary sets the
    /// maximum permissions that an identity-based policy can grant" (IAM, permissions
    /// boundaries), which takes permissions away and grants none of its own.
    ///
    /// A request that addresses no resource of its own is decided on the shared evaluator's
    /// weaker question ([`iam::identity_may_allow_action`]), which is published for exactly
    /// that case: refusing on `identity_decision` over `*` alone would refuse calls a
    /// resource-scoped statement permits.
    fn refusal(&self, action: &str, resource: &str, policy: Option<&str>) -> Option<String> {
        // An identity-based document states no `Principal` element; a resource-based one is
        // matched against the spellings of the principal that signed.
        let request = iam::Request::new(action, resource, &self.principals);
        let said = iam::identity_decision(&self.documents, &request);
        let resource_said = match policy {
            Some(policy) => iam::resource_decision(policy, &request),
            None => Decision::NotStated,
        };
        let granted = if resource == EVERY_RESOURCE {
            said != Decision::Denied
                && (iam::identity_may_allow_action(
                    &iam::prepare_documents(&self.documents),
                    action,
                ) || self.identity_is_unbounded())
        } else {
            iam::is_allowed(said, resource_said)
                || (said != Decision::Denied
                    && resource_said != Decision::Denied
                    && self.identity_is_unbounded())
        };
        // The ceiling in force on the principal. A call a document explicitly DENIES is
        // already refused on that deny, so no ceiling is read for it.
        let denied = said == Decision::Denied || resource_said == Decision::Denied;
        let capped = match (denied, self.boundary.as_ref()) {
            (false, Some(boundary)) => {
                iam::identity_decision(::std::slice::from_ref(boundary), &request)
            }
            _ => Decision::Allowed,
        };
        if granted && capped == Decision::Allowed {
            return None;
        }
        // WHICH document the deny stands in is a fact the clause names: an explicit `Deny` in
        // one of the principal's own documents is the identity-based clause, and one in the
        // resource's own policy is the resource-based clause, since "an explicit deny in any
        // policy overrides any allow" and the caller is told which policy to go and change.
        let reason = if said == Decision::Denied {
            iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
        } else if resource_said == Decision::Denied {
            iam::RESOURCE_POLICY_EXPLICIT_DENY.to_string()
        } else if !granted {
            iam::no_identity_policy_allows(action)
        } else if capped == Decision::Denied {
            iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string()
        } else {
            iam::no_permissions_boundary_allows(action)
        };
        Some(iam::not_authorized_by_identity_policy(
            &self.arn, action, resource, &reason,
        ))
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
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no document, so every
    // action it asks for is refused — and the refusal names the credential's STATE rather
    // than the key id that was presented, which this module never quotes back.
    let identity = match identity {
        Ok(identity) => identity,
        Err(_) => {
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                ..Caller::default()
            };
        }
    };
    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (the map IAM's own `GetUser` resolves a caller through), so a request
    // signed with one speaks for THAT user and is judged on the user's policies. A key the
    // IAM store does not hold either really is the account's own published credential.
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
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.principals.push(role_arn.clone());
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    // The account the principal belongs to, which is how a resource policy delegates to a
    // whole account rather than to one of its principals.
    caller
        .principals
        .extend(iam::account_principals(&caller.arn));
    caller
}

/// One modelled operation served behind this service's authorization gate: IAM decides the
/// call, and only a call it permits reaches the operation.
///
/// It wraps the operation at the TYPED seam ([`Operation`]), so the gate is handed the
/// deserialized input and reads the resource the request addresses off the built shape rather
/// than parsing a body a second time; and the refusal travels through the operation's own
/// error channel, so it is rendered in this service's envelope like every other failure.
pub struct Authorized<O, Eng> {
    /// The operation that serves a call IAM permits.
    operation: O,
    /// The shared engine: the IAM and STS stores the decision is read from live there.
    engine: Arc<Eng>,
}

impl<O, Eng> Authorized<O, Eng> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: O, engine: Arc<Eng>) -> Self {
        Self { operation, engine }
    }
}

impl<O, E, Eng> Operation for Authorized<O, Eng>
where
    O: Operation<Error = crate::scaffolded::Scaffolded<E>>,
    E: smithy_server_runtime::OperationError + Send + 'static,
    Eng: Send + Sync + 'static,
{
    type Input = O::Input;
    type Output = O::Output;
    type Error = O::Error;

    const NAME: &'static str = O::NAME;
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = O::INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = O::OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = O::SUCCESS_STATUS;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let addressed = addressed_resource(&input, O::INPUT_SCHEMA, &region);
        if let Some(message) = refusal(&*self.engine, ctx, O::NAME, &addressed) {
            return Err(crate::scaffolded::Scaffolded::access_denied(message));
        }
        self.operation.handle(input, ctx).await
    }
}

impl<O, E, Eng> ErasedOperation for Authorized<O, Eng>
where
    O: Operation<Error = crate::scaffolded::Scaffolded<E>> + ErasedOperation,
    E: smithy_server_runtime::OperationError + Send + 'static,
    Eng: Send + Sync + 'static,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The framework's own per-request sequence, driven with THIS wrapper as the
            // operation, so the handler it calls is the gated one.
            smithy_server_runtime::run_operation_http(self, svc, req, ctx, JSON_ENVELOPE).await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's modelled schemes: gating a call must not change which
        // credentials sign it.
        self.operation.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about carries this service's OWN namespace, so a
    /// document scoped to another service's actions states nothing about it.
    ///
    /// The operations are read from the MODEL this crate was generated from, so every
    /// operation of the surface is covered rather than a hand-kept few.
    #[test]
    fn the_action_asked_about_carries_this_services_namespace() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"kinesis:*","Resource":"*"}]}"#;
        for operation in super::super::tests::modelled_operations() {
            let action = action_for(&operation);
            assert_eq!(
                action,
                format!("kinesis:{operation}"),
                "case {operation:?}: expected this service's namespace, got {action:?}"
            );
            let reached = decision(document, &action, EVERY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected kinesis:* to allow {action}, got {reached:?}"
            );
        }
        // Another service's namespace grants nothing here, however close its name.
        for outside in ["kinesisvideo:PutRecord", "firehose:PutRecord"] {
            let reached = decision(document, outside, EVERY_RESOURCE);
            assert_eq!(
                reached,
                Decision::NotStated,
                "case {outside:?}: expected no grant from this service's namespace, got \
                 {reached:?}"
            );
        }
    }

    /// RULE: a grant is read for the resource the request ADDRESSES, so a statement scoped to
    /// one stream covers that stream and no other — not another stream of the same account
    /// and Region, and not the same name in another Region.
    #[test]
    fn a_grant_is_read_for_the_resource_the_request_addressed() {
        let addressed = crate::state::stream_arn("us-west-2", "orders");
        assert_eq!(
            addressed, "arn:aws:kinesis:us-west-2:000000000000:stream/orders",
            "case \"the ARN form\": expected this namespace's stream ARN, got {addressed:?}"
        );
        let action = action_for("PutRecord");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
             "Action":"{action}","Resource":"{addressed}"}}]}}"#
        );
        // (case, the resource the decision is taken against, whether the grant covers it)
        let rows: &[(&str, String, bool)] = &[
            ("the stream the request names", addressed.clone(), true),
            (
                "another stream of the same account and Region",
                crate::state::stream_arn("us-west-2", "archive"),
                false,
            ),
            (
                "the same stream name in another Region",
                crate::state::stream_arn("eu-west-3", "orders"),
                false,
            ),
            (
                "a channel of the same name",
                crate::state::channel_arn("us-west-2", "orders"),
                false,
            ),
        ];
        for (case, resource, covered) in rows {
            let said = decision(&document, &action, resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *covered,
                "case {case:?}: expected the grant to {}cover {resource}, got {said:?}",
                if *covered { "" } else { "not " }
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request addressed
    /// and which of IAM's rules withheld the grant — every part taken from the request and
    /// the stored policies.
    #[test]
    fn the_refusal_states_iams_own_sentence() {
        let principal = "arn:aws:sts::000000000000:assumed-role/writer/forwarder";
        let stream = crate::state::stream_arn("us-east-1", "orders");
        let action = action_for("PutRecord");
        let caller = Caller {
            arn: principal.to_string(),
            principals: vec![principal.to_string()],
            ..Caller::default()
        };
        let stated = caller
            .refusal(&action, &stream, None)
            .expect("a caller with no document at all is refused");
        assert_eq!(
            stated,
            format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {stream} because no identity-based policy allows the {action} action"
            ),
            "case \"no document at all\": expected IAM's wording, got {stated:?}"
        );
        // An explicit deny beside a grant is a fact about the document, so the clause names
        // it rather than the action.
        let denied = Caller {
            documents: vec![format!(
                r#"{{"Statement":[
                 {{"Effect":"Allow","Action":"{action}","Resource":"*"}},
                 {{"Effect":"Deny","Action":"{action}","Resource":"{stream}"}}]}}"#
            )],
            ..caller.clone()
        };
        let stated = denied
            .refusal(&action, &stream, None)
            .expect("an explicit deny refuses");
        assert!(
            stated.ends_with(iam::IDENTITY_POLICY_EXPLICIT_DENY),
            "case \"an explicit deny\": expected the deny clause, got {stated:?}"
        );
        // A grant of exactly this action on exactly this stream is served.
        let allowed = Caller {
            documents: vec![format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"{action}",
                 "Resource":"{stream}"}}]}}"#
            )],
            ..caller.clone()
        };
        assert_eq!(
            allowed.refusal(&action, &stream, None),
            None,
            "case \"a grant of this action on this stream\": expected the call to be served"
        );
        // The resource's OWN policy grants on its own, within one account.
        let policy = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{principal}"}},
             "Action":"{action}","Resource":"{stream}"}}]}}"#
        );
        assert_eq!(
            caller.refusal(&action, &stream, Some(&policy)),
            None,
            "case \"a resource policy naming the caller\": expected the call to be served"
        );
        // An explicit deny in the RESOURCE's policy refuses a caller whose own documents
        // allow the call, and the clause names THAT document rather than the identity's.
        let denies = format!(
            r#"{{"Statement":[{{"Effect":"Deny","Principal":{{"AWS":"{principal}"}},
             "Action":"{action}","Resource":"{stream}"}}]}}"#
        );
        let stated = allowed
            .refusal(&action, &stream, Some(&denies))
            .expect("an explicit deny in the resource policy refuses");
        assert!(
            stated.ends_with(iam::RESOURCE_POLICY_EXPLICIT_DENY),
            "case \"an explicit deny in the resource policy\": expected the resource clause, \
             got {stated:?}"
        );
    }
}

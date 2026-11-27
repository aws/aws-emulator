// SPDX-License-Identifier: CC0-1.0

//! WHEN a cluster-mutating MSK call takes its authorization decision, driven end to
//! end over the same `MultiServiceRouter` the emulator serves from (the shared
//! `kafka_harness`), with the IAM role and the STS session written through those
//! services' own stores — as `iam:CreateRole`, `iam:PutRolePolicy` and
//! `sts:AssumeRole` write them.
//!
//! RULE under test: the decision for a cluster-scoped action is taken from the
//! REQUEST alone — the action, the calling principal and the ARN as submitted —
//! BEFORE the handler resolves the cluster that ARN names. Two consequences, and
//! this suite asserts both:
//!
//! * a caller the decision REFUSES is answered `AccessDeniedException` (HTTP 403)
//!   carrying IAM's own sentence about the principal that signed, the action asked
//!   for and the submitted ARN — the SAME answer whether or not this endpoint holds
//!   a cluster under that ARN, so a refused caller learns nothing about what the
//!   registry holds; and a target in another account, which nothing of this account
//!   can hold a policy over, earns the sentence for a request no resource-based
//!   policy allows, again whatever the registry holds;
//! * only a caller the decision PERMITS is told about the target: a well-formed ARN
//!   of this account that this endpoint's region holds no cluster for is
//!   `NotFoundException` (HTTP 404) in the service's one sentence for a resource it
//!   cannot resolve, and the cluster it DOES hold has the update accepted — except
//!   where the call acts on the surface clients REACH the cluster over, which is the
//!   surface a cluster's own resource-based policy is attached for: only a document
//!   attached to the target cluster can grant such a call, and a cluster carrying
//!   none reaches no grant, so there the permitted caller is refused as unauthorized
//!   too and learns nothing about the registry either
//!   (`connectivity_absent_cluster_refusal.rs` drives that rule for the call that
//!   configures that surface, and `client_vpc_connection_target_grant.rs` for the
//!   calls that manage the connections made over it).
//!
//! The rows differ along what the rule generalises over: the document in force on
//! the calling principal (a deny of exactly this action, a deny of every action of
//! this service, no document at all, an allow of the action), the target the request
//! names (a cluster this endpoint holds, a well-formed ARN it holds nothing for, an
//! ARN of another account), and the OPERATION taking the decision — so the rule is
//! shown to be the service's rather than one handler's.

use serde_json::{json, Value};

use crate::kafka_harness::{
    given_assumed_role_session, given_role_inline_policy, pct, poll_until_active, ClusterRequest,
    Emulator, Reply, ACCOUNT, DEFAULT_ACCESS_KEY_ID, REGION,
};

// The service's own sentence for a resource it cannot resolve, read from the
// service so this suite and the handlers cannot drift apart on what it says.
use service_com_amazonaws_kafka_kafka::state::RESOURCE_NOT_FOUND_MESSAGE;

/// The role the rows sign as, the session's name, and the access key id that
/// session was minted for.
const ROLE: &str = "kafka-mutation-order-caller";
const SESSION: &str = "mutation-order-session";
const SESSION_KEY_ID: &str = "ASIAMUTATIONORDER000";

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// An account that is not the one this emulator serves, so nothing it holds can
/// carry a policy over a resource of that account.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// The status and wire code an authorization refusal carries, spelled out here
/// rather than read from the service so a drift in its own constants fails this
/// suite.
const ACCESS_DENIED_STATUS: u16 = 403;
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The status and wire code the service answers a resource it cannot resolve with.
const NOT_FOUND_STATUS: u16 = 404;
const NOT_FOUND_CODE: &str = "NotFoundException";

/// The principal every request signed with the account's own published credential
/// speaks for — the identity `sts:GetCallerIdentity` reports for it, and the one
/// such a request's refusal names.
fn ambient_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// Which sentence IAM states a decision with, which is what the row expects back.
#[derive(Debug, Clone, Copy)]
enum Sentence {
    /// A readable document of the principal's own explicitly DENIED the request.
    ExplicitDeny,
    /// Nothing readable ALLOWED it, so the request fell to IAM's default deny.
    NoGrant,
}

/// One cluster-mutating call: the operation that answers it, how the model binds
/// it, and the members its body carries beside the cluster version — all present
/// and legal, so a row exercises the decision rather than the body.
struct Mutation {
    operation: &'static str,
    method: &'static str,
    /// What follows the `ClusterArn` path label in the operation's URI.
    suffix: &'static str,
    members: Value,
}

impl Mutation {
    /// The IAM action a call to this operation asks for: `kafka:` plus the API
    /// name, which is how every action of this service's control plane is
    /// published.
    fn action(&self) -> String {
        format!("kafka:{}", self.operation)
    }

    /// The request URI naming `arn`, percent-encoded into the one path segment the
    /// model binds `ClusterArn` to.
    fn uri(&self, arn: &str) -> String {
        format!("/v1/clusters/{}{}", pct(arn), self.suffix)
    }

    /// The request body: this operation's own members, plus the version the caller
    /// is updating from.
    fn body(&self, version: &str) -> Value {
        let mut body = self.members.clone();
        body["currentVersion"] = json!(version);
        body
    }
}

/// The calls the rows are made over: cluster mutations that differ in what they
/// change and in which part of the cluster they change it, so one table shows the
/// decision belongs to the service.
fn mutations() -> Vec<Mutation> {
    vec![
        // `UpdateConnectivity` is deliberately not a row here: it acts on the
        // surface a cluster's own resource-based policy is attached for, so a
        // permitted caller handed an ARN this endpoint holds no cluster under is
        // refused as unauthorized rather than told about the target
        // (`connectivity_absent_cluster_refusal.rs`).
        Mutation {
            operation: "UpdateStorage",
            method: "PUT",
            suffix: "/storage",
            members: json!({ "volumeSizeGB": 2000 }),
        },
        Mutation {
            operation: "UpdateBrokerCount",
            method: "PUT",
            suffix: "/nodes/count",
            members: json!({ "targetNumberOfBrokerNodes": 6 }),
        },
        Mutation {
            operation: "UpdateMonitoring",
            method: "PUT",
            suffix: "/monitoring",
            members: json!({ "enhancedMonitoring": "PER_BROKER" }),
        },
    ]
}

/// The sentence IAM refuses a request one of the principal's own documents
/// explicitly denies with: fixed text around the principal that signed, the action
/// and the resource the request addressed.
fn explicit_deny_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in an identity-based policy"
    )
}

/// The sentence IAM refuses a request no readable document ALLOWS with — its
/// default deny, which closes by naming the action nothing granted.
fn no_grant_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The sentence a request whose target only a RESOURCE-based policy could have
/// allowed is refused with — which is what a resource of another account is, since
/// nothing this account holds carries a policy over it.
fn no_resource_policy_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no resource-based policy allows the {action} action"
    )
}

/// The sentence a row expects for `sentence`.
fn refusal_message(sentence: Sentence, principal: &str, action: &str, resource: &str) -> String {
    match sentence {
        Sentence::ExplicitDeny => explicit_deny_message(principal, action, resource),
        Sentence::NoGrant => no_grant_message(principal, action, resource),
    }
}

/// An identity-based document with one statement of `effect` about `action` on
/// every resource — the shape `iam:PutRolePolicy` takes.
fn document(effect: &str, action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": action, "Resource": "*" }],
    })
}

/// Make `mutation` against `target`, signed with `access_key_id`.
async fn call_as(
    emulator: &Emulator,
    access_key_id: &str,
    mutation: &Mutation,
    target: &str,
    version: &str,
) -> Reply {
    emulator
        .call_as_in(
            access_key_id,
            REGION,
            mutation.method,
            &mutation.uri(target),
            Some(mutation.body(version)),
        )
        .await
}

/// RULE: the decision comes FIRST, so a caller it refuses is told it is not
/// authorized and nothing about the target — the identical refusal for a cluster
/// this endpoint holds and for an ARN it holds nothing for — while a caller it
/// permits reaches the operation's own answer: the not-found sentence for the ARN
/// nothing is held under, and the update accepted for the cluster that is.
#[tokio::test]
async fn an_unauthorized_caller_is_refused_before_the_cluster_is_resolved() {
    let emulator = Emulator::new();
    let principal = given_assumed_role_session(&emulator.engine, ROLE, SESSION, SESSION_KEY_ID);
    // A cluster this endpoint really holds, so a permitted call is answered from
    // state rather than refused for its target.
    let live = emulator
        .create_ok(&ClusterRequest::named("mutation-order"))
        .await;
    let mut version = poll_until_active(&emulator, &live, "the created cluster").await;
    // A well-formed cluster ARN of this account that no registry of this endpoint
    // holds anything under.
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}");

    for mutation in mutations() {
        let action = mutation.action();
        let operation = mutation.operation;

        // Every document that refuses the call, with the sentence IAM states that
        // decision with.
        let refusing: Vec<(&str, Option<Value>, Sentence)> = vec![
            (
                "a deny of exactly this action",
                Some(document("Deny", &action)),
                Sentence::ExplicitDeny,
            ),
            (
                "a deny of every action of this service",
                Some(document("Deny", "kafka:*")),
                Sentence::ExplicitDeny,
            ),
            ("no document at all", None, Sentence::NoGrant),
        ];

        for (case, held, sentence) in refusing {
            given_role_inline_policy(&emulator.engine, ROLE, held.as_ref());
            // The two targets differ ONLY in whether this endpoint holds a cluster
            // under them, and the refusal is the same sentence about the ARN the
            // request itself carried: nothing of what the registry holds is
            // disclosed.
            for (target_case, target) in [
                ("a cluster this endpoint holds", live.as_str()),
                ("an ARN it holds nothing for", absent.as_str()),
            ] {
                let reply = call_as(&emulator, SESSION_KEY_ID, &mutation, target, &version).await;
                assert_eq!(
                    (reply.status, reply.error_code(), reply.error_message()),
                    (
                        ACCESS_DENIED_STATUS,
                        ACCESS_DENIED_CODE,
                        refusal_message(sentence, &principal, &action, target).as_str()
                    ),
                    "case {case:?} / {target_case:?} / {operation}: expected IAM's \
                     {sentence:?} refusal, got {} {}",
                    reply.status,
                    reply.body
                );
            }
        }

        // A caller the decision PERMITS is the one told about the target: every
        // cluster mutation of this service answers a well-formed ARN it holds no
        // cluster for as the absence it is.
        given_role_inline_policy(&emulator.engine, ROLE, Some(&document("Allow", &action)));
        let missed = call_as(&emulator, SESSION_KEY_ID, &mutation, &absent, &version).await;
        assert_eq!(
            (missed.status, missed.error_code(), missed.error_message()),
            (
                NOT_FOUND_STATUS,
                NOT_FOUND_CODE,
                RESOURCE_NOT_FOUND_MESSAGE
            ),
            "case \"an allow of this action\" / \"an ARN it holds nothing for\" / {operation}: \
             expected the absence to be reported, got {} {}",
            missed.status,
            missed.body
        );

        // And the cluster this endpoint DOES hold has the update accepted, so the
        // rows above are refusals of the caller rather than of the request.
        let accepted = call_as(&emulator, SESSION_KEY_ID, &mutation, &live, &version).await;
        assert_eq!(
            accepted.status, 200,
            "case \"an allow of this action\" / \"a cluster this endpoint holds\" / \
             {operation}: expected the update to be accepted, got {} {}",
            accepted.status, accepted.body
        );
        assert!(
            accepted.at("/clusterOperationArn").is_string(),
            "case \"an allow of this action\" / {operation}: an accepted update must answer \
             with the cluster operation it filed, got {}",
            accepted.body
        );
        // The accepted update leaves the cluster UPDATING at a new version once the
        // clock makes it due, so the next operation's rows quote the version the
        // cluster is then at.
        version = poll_until_active(&emulator, &live, operation).await;
    }

    // Nothing the rows stored outlives them.
    given_role_inline_policy(&emulator.engine, ROLE, None);
}

/// RULE (the other half of the decision): a target in an account other than the
/// one this emulator serves is allowed by nothing it holds, so the call is refused
/// with the sentence for a request no resource-based policy allows — whatever the
/// registry holds under that ARN, and before anything of the request is resolved.
#[tokio::test]
async fn a_target_of_another_account_earns_the_resource_policy_refusal() {
    let emulator = Emulator::new();
    let principal = ambient_principal();
    // The version is quoted from a cluster this endpoint really holds, so the row
    // carries a request that would otherwise be accepted.
    let live = emulator
        .create_ok(&ClusterRequest::named("foreign-target"))
        .await;
    let version = poll_until_active(&emulator, &live, "the created cluster").await;

    for mutation in mutations() {
        let action = mutation.action();
        let operation = mutation.operation;
        // Rows: an ARN of another account in this region, in another region, and
        // one naming the same cluster NAME this endpoint holds — none of which any
        // policy of this account covers.
        let targets = [
            (
                "another account, this region",
                format!("arn:aws:kafka:{REGION}:{FOREIGN_ACCOUNT}:cluster/elsewhere/{GHOST_ID}"),
            ),
            (
                "another account, another region",
                format!("arn:aws:kafka:eu-west-1:{FOREIGN_ACCOUNT}:cluster/far-away/{GHOST_ID}"),
            ),
            (
                "another account, this cluster's name",
                format!(
                    "arn:aws:kafka:{REGION}:{FOREIGN_ACCOUNT}:cluster/mutation-order/{GHOST_ID}"
                ),
            ),
        ];
        for (case, target) in &targets {
            let reply = call_as(
                &emulator,
                DEFAULT_ACCESS_KEY_ID,
                &mutation,
                target,
                &version,
            )
            .await;
            assert_eq!(
                (reply.status, reply.error_code(), reply.error_message()),
                (
                    ACCESS_DENIED_STATUS,
                    ACCESS_DENIED_CODE,
                    no_resource_policy_message(&principal, &action, target).as_str()
                ),
                "case {case:?} / {operation}: expected the resource-policy refusal, got {} {}",
                reply.status,
                reply.body
            );
        }
    }
}

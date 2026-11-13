// SPDX-License-Identifier: CC0-1.0

//! Attaching a Kinesis Data Firehose event destination to a configuration set, when the
//! role the destination names has something to say about who may assume it — end to end
//! over HTTP with AWS Identity and Access Management mounted beside this service on one
//! engine.
//!
//! The RULE these cases pin: `KinesisFirehoseDestination$IamRoleArn` is "the ARN of the IAM
//! role that the Amazon SES API v2 uses to send email events to the Amazon Kinesis Data
//! Firehose stream", so this service does not reach the stream as its owner — it ACTS AS
//! that role. A role whose stored trust policy does not admit the assume for this service's
//! own service principal is therefore a destination that could never publish anything, and
//! the request that would establish it — a create of the destination or an update of an
//! existing one — is refused with the operation's modelled input error
//! (`BadRequestException`, HTTP 400) carrying SES's own could-not-assume-role sentence about
//! that role, with nothing stored. Every other answer refuses nothing: a policy that admits
//! the assume however it spells it, and a value this emulator's IAM has no stored trust
//! document for at all.
//!
//! The table spans what the rule generalises over: the effect and the shape of the trust
//! document (the action spelled out, under a service wildcard, under a prefix wildcard and
//! as every action; the principal as one string and as a list; another casing of both; one
//! statement as an object rather than a list; a deny beside the allow; a grant to the
//! account root rather than to a service), and whether the account's IAM holds a role under
//! the ARN at all (a role nothing created, a role of another account, and a value that names
//! no IAM role). The recorded case is one row of it.
//!
//! The last two cases carry the same rule onto the rest of the surface: an update that
//! would repoint a destination at a role this service cannot act as is refused and leaves
//! the stored definition alone, and the refusal is lifted the moment the role's own trust
//! policy admits the assume.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// The Region every call below is signed for — the one `Ses::call` signs for — and so the
/// Region the in-process IAM hop is addressed at.
const REGION: &str = "us-east-1";

/// The service principal this service sends a configuration set's events as, as a role's
/// trust policy names it.
pub(crate) const SES_SERVICE_PRINCIPAL: &str = "ses.amazonaws.com";

/// The delivery stream every row's destination names. The attach decides the ROLE; the
/// stream itself is a resource of a service this suite does not mount, and
/// `KinesisFirehoseDestination$DeliveryStreamArn` is an unpatterned `AmazonResourceName`
/// (`smithy_query(query="shape",shape="KinesisFirehoseDestination")`), so one well-formed
/// stream ARN serves every row.
fn delivery_stream_arn() -> String {
    format!(
        "arn:aws:firehose:{REGION}:{}:deliverystream/ses-events",
        ::engine::emulator_account_id()
    )
}

/// The IAM client the setup creates a row's role through: the SAME in-process hop the
/// handler asks about that role over, so what the suite wrote is what the handler reads.
fn iam(ses: &Ses) -> ::aws_sdk_iam::Client {
    let callback = ses.callback();
    ::service_common::iam_client::iam_client_through(&callback, REGION)
}

/// A trust policy document carrying `statements`.
pub(crate) fn trust(statements: Value) -> Value {
    json!({"Version": "2012-10-17", "Statement": statements})
}

/// One statement granting `service` the assume of the role.
pub(crate) fn allow_assume(service: &str) -> Value {
    json!({
        "Effect": "Allow",
        "Principal": {"Service": service},
        "Action": "sts:AssumeRole",
    })
}

/// The role `name`, created through the IAM API with `document` as its trust policy.
/// Answers the ARN IAM minted for it.
pub(crate) async fn given_role(ses: &Ses, name: &str, document: &Value) -> String {
    let created = iam(ses)
        .create_role()
        .role_name(name)
        .assume_role_policy_document(document.to_string())
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: CreateRole({name}) failed: {error:?}"));
    let arn = created
        .role()
        .map(|role| role.arn().to_string())
        .unwrap_or_default();
    assert!(
        arn.ends_with(&format!(":role/{name}")),
        "setup: CreateRole({name}) must answer the ARN it minted, got {arn:?}"
    );
    arn
}

/// Convenience calls for the path these cases drive, over the shared harness.
pub(crate) trait FirehoseEventDestinations {
    async fn given_set(&self, name: &str) -> Reply;
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn get_destinations(&self, set: &str) -> Reply;
}

impl FirehoseEventDestinations for Ses {
    async fn given_set(&self, name: &str) -> Reply {
        self.create_set(json!({"ConfigurationSetName": name})).await
    }

    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            Some(json!({"EventDestinationName": name, "EventDestination": definition})),
        )
        .await
    }

    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{set}/event-destinations/{name}"),
            Some(json!({"EventDestination": definition})),
        )
        .await
    }

    async fn get_destinations(&self, set: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            None,
        )
        .await
    }
}

/// The definition a Firehose event destination sending its events under `role_arn`
/// carries.
pub(crate) fn firehose_definition(role_arn: &str) -> Value {
    json!({
        "Enabled": true,
        "MatchingEventTypes": ["SEND"],
        "KinesisFirehoseDestination": {
            "IamRoleArn": role_arn,
            "DeliveryStreamArn": delivery_stream_arn(),
        },
    })
}

/// The role ARN the destination `name` of `set` reports, or `None` when the set reports no
/// destination under that name.
pub(crate) async fn reported_role(ses: &Ses, set: &str, name: &str) -> Option<String> {
    let reply = ses.get_destinations(set).await;
    assert_eq!(
        reply.status,
        200,
        "read of {set}'s event destinations: expected 200, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    let destinations = match reply.member("EventDestinations") {
        Value::Array(destinations) => destinations,
        other => panic!("read of {set}'s event destinations: expected an array, got {other:?}"),
    };
    destinations
        .into_iter()
        .find(|destination| destination.get("Name").and_then(Value::as_str) == Some(name))
        .map(|destination| {
            destination
                .pointer("/KinesisFirehoseDestination/IamRoleArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
}

/// The sentence a refused destination carries: SES's own fixed could-not-assume-role text,
/// with the role the definition named taken from the request. Written out here, so the suite
/// pins the wording rather than reading it back from the code under test.
pub(crate) fn refusal(role_arn: &str) -> String {
    format!("Could not assume IAM role <{role_arn}>.")
}

/// Where the role the row's destination names comes from.
enum Role {
    /// A role this account's IAM holds, created with this trust document.
    Held(fn() -> Value),
    /// A value naming no role this account's IAM holds a trust document for, used as the
    /// `IamRoleArn` exactly as written.
    Unheld(&'static str),
}

/// One row of the table: the role the destination names, and whether the attach is refused.
struct RoleCase {
    case: &'static str,
    /// The configuration set and the role name the row uses, which are its own.
    set: &'static str,
    role_name: &'static str,
    role: Role,
    /// Whether this service must refuse the destination.
    refused: bool,
}

/// The rows: the same attach, differing in what the role's trust policy says about it.
const ROLE_CASES: &[RoleCase] = &[
    RoleCase {
        case: "a trust policy granting this service the assume",
        set: "trusted-set",
        role_name: "trusted-role",
        role: Role::Held(|| trust(json!([allow_assume(SES_SERVICE_PRINCIPAL)]))),
        refused: false,
    },
    RoleCase {
        case: "the assume granted under a service wildcard",
        set: "service-wildcard-set",
        role_name: "service-wildcard-role",
        role: Role::Held(|| {
            trust(json!([{
                "Effect": "Allow",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sts:*",
            }]))
        }),
        refused: false,
    },
    RoleCase {
        case: "the assume granted under a prefix wildcard",
        set: "prefix-wildcard-set",
        role_name: "prefix-wildcard-role",
        role: Role::Held(|| {
            trust(json!([{
                "Effect": "Allow",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sts:Assume*",
            }]))
        }),
        refused: false,
    },
    RoleCase {
        case: "the assume granted as every action, to this service among a list of them",
        set: "every-action-set",
        role_name: "every-action-role",
        role: Role::Held(|| {
            trust(json!([{
                "Effect": "Allow",
                "Principal": {"Service": ["events.amazonaws.com", SES_SERVICE_PRINCIPAL]},
                "Action": "*",
            }]))
        }),
        refused: false,
    },
    RoleCase {
        case: "the grant written in another casing, which IAM compares without regard to it",
        set: "casing-set",
        role_name: "casing-role",
        role: Role::Held(|| {
            trust(json!([{
                "Effect": "Allow",
                "Principal": {"Service": "SES.AMAZONAWS.COM"},
                "Action": "STS:ASSUMEROLE",
            }]))
        }),
        refused: false,
    },
    RoleCase {
        case: "the grant as the single statement object the grammar also admits",
        set: "single-statement-set",
        role_name: "single-statement-role",
        role: Role::Held(|| trust(allow_assume(SES_SERVICE_PRINCIPAL))),
        refused: false,
    },
    RoleCase {
        case: "an explicit deny of the assume beside the grant",
        set: "denied-set",
        role_name: "denied-role",
        role: Role::Held(|| {
            trust(json!([
                allow_assume(SES_SERVICE_PRINCIPAL),
                {
                    "Effect": "Deny",
                    "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                    "Action": "sts:AssumeRole",
                },
            ]))
        }),
        refused: true,
    },
    RoleCase {
        case: "a trust policy granting the assume to another service only",
        set: "other-principal-set",
        role_name: "other-principal-role",
        role: Role::Held(|| trust(json!([allow_assume("firehose.amazonaws.com")]))),
        refused: true,
    },
    RoleCase {
        case: "a trust policy granting this service another action than the assume",
        set: "other-action-set",
        role_name: "other-action-role",
        role: Role::Held(|| {
            trust(json!([{
                "Effect": "Allow",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "s3:GetObject",
            }]))
        }),
        refused: true,
    },
    RoleCase {
        case: "a trust policy granting the assume to the account root rather than a service",
        set: "account-root-set",
        role_name: "account-root-role",
        role: Role::Held(|| {
            trust(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:root", ::engine::emulator_account_id())},
                "Action": "sts:AssumeRole",
            }]))
        }),
        refused: true,
    },
    RoleCase {
        // A role record exists in this emulator's IAM only where a client created it HERE,
        // while an event-publishing role is routinely provisioned out of band: its absence
        // is evidence about this store and not about the role the caller named.
        case: "a role of this account that nothing created here",
        set: "absent-role-set",
        role_name: "absent-role",
        role: Role::Unheld("arn:aws:iam::000000000000:role/never-created"),
        refused: false,
    },
    RoleCase {
        case: "a role of another account, whose IAM is not this emulator's to ask",
        set: "other-account-set",
        role_name: "other-account-role",
        role: Role::Unheld("arn:aws:iam::210987654321:role/somebody-elses-role"),
        refused: false,
    },
    RoleCase {
        // `IamRoleArn` targets an unpatterned `AmazonResourceName` of length at least 1, so
        // a value that names no IAM role is a value this emulator holds no trust document
        // for rather than a request this operation refuses.
        case: "a value that names no IAM role",
        set: "not-a-role-set",
        role_name: "not-a-role",
        role: Role::Unheld("ses-event-publishing"),
        refused: false,
    },
];

#[tokio::test]
async fn a_firehose_destination_is_refused_exactly_where_its_role_withholds_the_assume() {
    for case in ROLE_CASES {
        let ses = Ses::with_iam();
        let role_arn = match &case.role {
            Role::Held(document) => given_role(&ses, case.role_name, &document()).await,
            Role::Unheld(written) => (*written).to_string(),
        };

        let created = ses.given_set(case.set).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: setup CreateConfigurationSet expected 200, got {} {} ({})",
            case.case,
            created.status,
            created.error_code(),
            created.message()
        );

        let reply = ses
            .create_destination(case.set, "events", firehose_definition(&role_arn))
            .await;
        if case.refused {
            assert_eq!(
                reply.status,
                400,
                "case {:?}: expected 400, got {} {} ({})",
                case.case,
                reply.status,
                reply.error_code(),
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {:?}: expected BadRequestException, got {} ({})",
                case.case,
                reply.error_code(),
                reply.message()
            );
            assert_eq!(
                reply.message(),
                refusal(&role_arn),
                "case {:?}: expected the authorization failure, got {:?}",
                case.case,
                reply.message()
            );
            assert_eq!(
                reported_role(&ses, case.set, "events").await,
                None,
                "case {:?}: a refused destination must leave nothing behind",
                case.case
            );
        } else {
            assert_eq!(
                reply.status,
                200,
                "case {:?}: expected 200, got {} {} ({})",
                case.case,
                reply.status,
                reply.error_code(),
                reply.message()
            );
            assert!(
                reply.is_empty_body(),
                "case {:?}: the output shape declares no members, so the body must be empty, got \
                 {:?}",
                case.case,
                reply.member("EventDestinations")
            );
            assert_eq!(
                reported_role(&ses, case.set, "events").await.as_deref(),
                Some(role_arn.as_str()),
                "case {:?}: expected the stored destination to send its events under {role_arn:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn an_update_onto_a_role_this_service_cannot_act_as_leaves_the_destination_alone() {
    let case = "an update repointing a destination at a role that withholds the assume";
    let ses = Ses::with_iam();
    let trusted = given_role(
        &ses,
        "update-trusted",
        &trust(json!([allow_assume(SES_SERVICE_PRINCIPAL)])),
    )
    .await;
    let withheld = given_role(
        &ses,
        "update-withheld",
        &trust(json!([allow_assume("firehose.amazonaws.com")])),
    )
    .await;
    let created = ses.given_set("update-set").await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: setup CreateConfigurationSet expected 200, got {} {} ({})",
        created.status,
        created.error_code(),
        created.message()
    );
    let attached = ses
        .create_destination("update-set", "events", firehose_definition(&trusted))
        .await;
    assert_eq!(
        attached.status,
        200,
        "case {case:?}: setup attach expected 200, got {} {} ({})",
        attached.status,
        attached.error_code(),
        attached.message()
    );

    let reply = ses
        .update_destination("update-set", "events", firehose_definition(&withheld))
        .await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "BadRequestException"),
        "case {case:?}: expected a 400 BadRequestException, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    assert_eq!(
        reply.message(),
        refusal(&withheld),
        "case {case:?}: expected the authorization failure, got {:?}",
        reply.message()
    );
    assert_eq!(
        reported_role(&ses, "update-set", "events").await.as_deref(),
        Some(trusted.as_str()),
        "case {case:?}: the refused update must leave the stored destination alone"
    );
}

#[tokio::test]
async fn the_same_attach_succeeds_once_the_role_grants_the_assume() {
    // The decision is the one IAM holds for the role, read for each request rather than
    // settled once: a trust policy that is widened between two identical attaches changes
    // the answer.
    let case = "a trust policy widened between two identical attaches";
    let ses = Ses::with_iam();
    let role_arn = given_role(
        &ses,
        "widened-role",
        &trust(json!([allow_assume("firehose.amazonaws.com")])),
    )
    .await;
    let created = ses.given_set("widened-set").await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: setup CreateConfigurationSet expected 200, got {} {} ({})",
        created.status,
        created.error_code(),
        created.message()
    );

    let refused = ses
        .create_destination("widened-set", "events", firehose_definition(&role_arn))
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "BadRequestException"),
        "case {case:?}: expected a 400 BadRequestException while the role withholds the assume, \
         got {} {} ({})",
        refused.status,
        refused.error_code(),
        refused.message()
    );
    assert_eq!(
        reported_role(&ses, "widened-set", "events").await,
        None,
        "case {case:?}: the refused attach must leave nothing behind"
    );

    iam(&ses)
        .update_assume_role_policy()
        .role_name("widened-role")
        .policy_document(
            trust(json!([
                allow_assume("firehose.amazonaws.com"),
                allow_assume(SES_SERVICE_PRINCIPAL),
            ]))
            .to_string(),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: UpdateAssumeRolePolicy failed: {error:?}"));

    let accepted = ses
        .create_destination("widened-set", "events", firehose_definition(&role_arn))
        .await;
    assert_eq!(
        accepted.status,
        200,
        "case {case:?}: expected 200 once the role grants the assume, got {} {} ({})",
        accepted.status,
        accepted.error_code(),
        accepted.message()
    );
    assert_eq!(
        reported_role(&ses, "widened-set", "events").await.as_deref(),
        Some(role_arn.as_str()),
        "case {case:?}: expected the stored destination to send its events under {role_arn:?}"
    );
}

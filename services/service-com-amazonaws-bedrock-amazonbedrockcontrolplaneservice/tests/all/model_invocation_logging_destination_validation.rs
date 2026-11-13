// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs destination of the account's model invocation logging configuration
//! is RESOLVED against the services that own it before it is installed.
//!
//! # The rule
//!
//! `CloudWatchConfig` names both the log group the invocation records are written INTO
//! (`logGroupName`) and the role they are written UNDER (`roleArn`), both `@required`, and
//! Amazon Bedrock validates the delivery permissions that pair describes before it enables
//! the destination. So a destination either half of which the owning service reports absent —
//! CloudWatch Logs holds no such log group in this Region, or this account's IAM holds no
//! such role — is one whose permissions cannot be validated: it is refused with the modelled
//! `ValidationException` (HTTP 400) whose message states the SUBMITTED pair back, and nothing
//! is installed.
//!
//! That refusal is a different class from the pass-role denial. Whether a role that DOES
//! resolve may be handed to Amazon Bedrock at all is the `iam:PassRole` decision, answered
//! with the modelled `AccessDeniedException` (HTTP 403) and the one fixed sentence that
//! denial is stated with — so a role this account holds that does not trust the service, and
//! a role of another account, keep that answer, while a role nothing holds takes the 400.
//!
//! A refusal has to be GROUNDED in what the owning service answered, which is what
//! [`a_destination_no_owning_service_can_be_asked_about_is_installed`] pins: an emulator
//! assembled without CloudWatch Logs and IAM to ask establishes nothing about either half and
//! installs the configuration.
//!
//! # What the table generalises over
//!
//! [`rows`] holds one row per shape the decision spans, the two halves crossed: which half is
//! absent (the group, the role, both, neither), the FORM of the submitted `logGroupName`
//! (names at both ends of the published 1..512 length, and one carrying every punctuation
//! character CloudWatch Logs' published name rule admits — `. - _ / #`, as
//! `services/common/src/cloudwatch_logs.rs` states that rule), the FORM of the submitted
//! `roleArn` (this account's, the EMPTY account field the member's published pattern
//! `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$` admits, a role under a path, another
//! partition, another account), whether the role that resolves trusts the service, and a
//! configuration that names no CloudWatch Logs destination at all.
//!
//! Every row's expected message is composed from the values THAT row submits, so a message
//! carrying one recorded run's log group or role would fail every other row.
//!
//! The rows are driven in sequence against ONE emulator, and each one reads the
//! configuration back through `GetModelInvocationLoggingConfiguration`, so each get also
//! witnesses that a refused put installed nothing and left the record in force alone.

use serde_json::{json, Value};

use crate::control_plane_authorization::{given_role, trust};
use crate::support::{emulator_with_logs_and_iam, Emulator, Reply, ACCOUNT};

/// The URI `PutModelInvocationLoggingConfiguration` and
/// `GetModelInvocationLoggingConfiguration` are both bound to
/// (`smithy_query(query="operation",operation="PutModelInvocationLoggingConfiguration,GetModelInvocationLoggingConfiguration")`).
const LOGGING_URI: &str = "/logging/modelinvocations";

/// The sentence a denied pass-role is stated with, spelled here so this suite pins the
/// wording a client sees rather than reading it off the service.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// A 12-digit account id that is NOT the calling account.
const OTHER_ACCOUNT: &str = "123456789012";

/// The longest log group name CloudWatch Logs holds, and the shortest — the published bounds
/// `services/common/src/cloudwatch_logs.rs` states, which `logGroupName`'s own modelled
/// `@length(min: 1, max: 512)` matches.
const LONGEST_LOG_GROUP_NAME_CHARS: usize = 512;
const SHORTEST_LOG_GROUP_NAME_CHARS: usize = 1;

/// What Amazon Bedrock answers for a destination whose delivery permissions it could not
/// validate: the fixed text, with the log group and the role the request submitted.
fn unvalidated_destination(log_group: &str, role_arn: &str) -> String {
    format!(
        "Failed to validate permissions for log group: {log_group}, with role: {role_arn}. \
         Verify the IAM role permissions are correct."
    )
}

/// The ARN this account holds `name` under.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// A CloudWatch Logs destination naming `log_group` and `role_arn`.
fn cloud_watch_config(log_group: &str, role_arn: &str) -> Value {
    json!({
        "cloudWatchConfig": { "logGroupName": log_group, "roleArn": role_arn },
        "textDataDeliveryEnabled": true,
    })
}

/// What a row expects the put to answer.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// 200, and the configuration reads back exactly as submitted.
    Installed,
    /// 400 `ValidationException` naming the submitted log group and role.
    Unvalidated,
    /// 403 `AccessDeniedException` with the one pass-role sentence.
    PassRoleDenied,
}

/// One row: the resources the owning services are given first, the configuration the put
/// carries, and the answer it must get.
struct Row {
    case: &'static str,
    /// A log group to create in CloudWatch Logs before the put.
    held_log_group: Option<String>,
    /// A role to record in IAM before the put, and whether its trust policy admits the
    /// Amazon Bedrock service principal.
    held_role: Option<(String, bool)>,
    logging_config: Value,
    expected: Outcome,
}

/// `n` repetitions of `c`, for a name driven at a published bound.
fn repeat(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}

fn rows() -> Vec<Row> {
    let delivery_role = "BedrockInvocationLogging";
    let partition_role = "BedrockInvocationLoggingPartition";
    let untrusted_role = "BedrockInvocationLoggingUntrusted";
    vec![
        Row {
            case: "a held log group and a held role trusted to the service",
            held_log_group: Some("/aws/bedrock/invocation-logs".to_string()),
            held_role: Some((delivery_role.to_string(), true)),
            logging_config: cloud_watch_config(
                "/aws/bedrock/invocation-logs",
                &role_arn(delivery_role),
            ),
            expected: Outcome::Installed,
        },
        Row {
            case: "neither the log group nor the role exists",
            held_log_group: None,
            held_role: None,
            logging_config: cloud_watch_config(
                "/aws/bedrock/nonexistent-probe-log-group",
                &role_arn("nonexistent-role"),
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            case: "a role that resolves under a log group that does not",
            held_log_group: None,
            held_role: Some((delivery_role.to_string(), true)),
            logging_config: cloud_watch_config(
                "/aws/bedrock/absent-group",
                &role_arn(delivery_role),
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            case: "a log group that resolves under a role that does not",
            held_log_group: Some("/aws/bedrock/held-group-four".to_string()),
            held_role: None,
            logging_config: cloud_watch_config(
                "/aws/bedrock/held-group-four",
                &role_arn("never-created-role"),
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            case: "an absent log group name at the published length ceiling",
            held_log_group: None,
            held_role: Some((delivery_role.to_string(), true)),
            logging_config: cloud_watch_config(
                &repeat('g', LONGEST_LOG_GROUP_NAME_CHARS),
                &role_arn(delivery_role),
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            case: "an absent log group name at the published length floor",
            held_log_group: None,
            held_role: Some((delivery_role.to_string(), true)),
            logging_config: cloud_watch_config(
                &repeat('g', SHORTEST_LOG_GROUP_NAME_CHARS),
                &role_arn(delivery_role),
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            // Every punctuation character the published name rule admits.
            case: "an absent log group name carrying the whole punctuation set",
            held_log_group: None,
            held_role: Some((delivery_role.to_string(), true)),
            logging_config: cloud_watch_config(
                "/aws/bedrock/probe.0-9_#",
                &role_arn(delivery_role),
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            // The member's published pattern admits an EMPTY account field, which names no
            // OTHER account: this one's role, and this one holds nothing under that name.
            case: "a role ARN carrying no account at all, naming a role nothing holds",
            held_log_group: Some("/aws/bedrock/held-group-empty-account".to_string()),
            held_role: None,
            logging_config: cloud_watch_config(
                "/aws/bedrock/held-group-empty-account",
                "arn:aws:iam:::role/no-such-logging-role",
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            case: "a role under a path that nothing holds",
            held_log_group: Some("/aws/bedrock/held-group-path".to_string()),
            held_role: None,
            logging_config: cloud_watch_config(
                "/aws/bedrock/held-group-path",
                &format!("arn:aws:iam::{ACCOUNT}:role/service-role/no-such-logging-role"),
            ),
            expected: Outcome::Unvalidated,
        },
        Row {
            // An IAM ARN is partition-global and the account is this one, so the role it
            // names is the role IAM holds.
            case: "a held role of this account named in another partition",
            held_log_group: Some("/aws/bedrock/held-group-partition".to_string()),
            held_role: Some((partition_role.to_string(), true)),
            logging_config: cloud_watch_config(
                "/aws/bedrock/held-group-partition",
                &format!("arn:aws-cn:iam::{ACCOUNT}:role/{partition_role}"),
            ),
            expected: Outcome::Installed,
        },
        Row {
            // The destination RESOLVES, so what is left is the pass: a role whose trust
            // document admits nobody is one the service cannot be given.
            case: "a held role this account holds no trust policy for",
            held_log_group: Some("/aws/bedrock/held-group-untrusted".to_string()),
            held_role: Some((untrusted_role.to_string(), false)),
            logging_config: cloud_watch_config(
                "/aws/bedrock/held-group-untrusted",
                &role_arn(untrusted_role),
            ),
            expected: Outcome::PassRoleDenied,
        },
        Row {
            // `iam:PassRole` is granted by the account that HOLDS the role, so another
            // account's role is one no principal of this account can pass.
            case: "a role of another account under a log group that resolves",
            held_log_group: Some("/aws/bedrock/held-group-foreign".to_string()),
            held_role: None,
            logging_config: cloud_watch_config(
                "/aws/bedrock/held-group-foreign",
                &format!("arn:aws:iam::{OTHER_ACCOUNT}:role/{delivery_role}"),
            ),
            expected: Outcome::PassRoleDenied,
        },
        Row {
            // `S3Config` names no log group and no role, so there is no such destination to
            // resolve and no pass to decide.
            case: "a configuration naming only the object destination",
            held_log_group: None,
            held_role: None,
            logging_config: json!({
                "s3Config": { "bucketName": "bedrock-invocation-logs", "keyPrefix": "prod/" },
                "imageDataDeliveryEnabled": true,
            }),
            expected: Outcome::Installed,
        },
    ]
}

/// Give CloudWatch Logs the log group a row names, through its own `CreateLogGroup`, so the
/// group this control plane resolves is one that service really holds.
async fn given_log_group(emu: &Emulator, name: &str) {
    let reply = emu
        .call_awsjson(
            "logs",
            "Logs_20140328.CreateLogGroup",
            json!({ "logGroupName": name }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "setup: CreateLogGroup({name:?}) must succeed, got {} {:?}",
        reply.status, reply.body
    );
}

/// What `GetModelInvocationLoggingConfiguration` reports, which is the record in force.
async fn installed(emu: &Emulator, case: &str) -> Value {
    let reply: Reply = emu.call("GET", LOGGING_URI, None).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: GetModelInvocationLoggingConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.get("loggingConfig")
}

/// RULE: a CloudWatch Logs destination is installed only where both halves of it resolve
/// where the services that own them hold them; a half the owning service reports absent is a
/// 400 `ValidationException` naming the submitted log group and role, while a role that
/// resolves and may not be passed keeps the 403 `AccessDeniedException` — and no refusal
/// disturbs the record in force.
#[tokio::test]
async fn an_invocation_logging_destination_is_installed_only_where_both_halves_resolve() {
    let emu = emulator_with_logs_and_iam();

    let mut in_force = Value::Null;
    assert_eq!(
        installed(&emu, "before any put").await,
        in_force,
        "case \"before any put\": nothing is configured until a put installs something"
    );

    for row in rows() {
        if let Some(group) = &row.held_log_group {
            given_log_group(&emu, group).await;
        }
        if let Some((role, trusted)) = &row.held_role {
            let document = trusted.then(|| trust(None));
            given_role(&emu.engine, role, document.as_ref());
        }

        let reply = emu
            .call(
                "PUT",
                LOGGING_URI,
                Some(json!({ "loggingConfig": row.logging_config.clone() })),
            )
            .await;
        let status = reply.status;
        let error = reply.error.clone().unwrap_or_default();
        let body = &reply.body;
        let message = reply
            .get("message")
            .as_str()
            .unwrap_or_default()
            .to_string();

        match row.expected {
            Outcome::Installed => {
                // `PutModelInvocationLoggingConfigurationOutput` declares no members, so an
                // admitted put answers the 200 and nothing else.
                let members = body.as_object().map(serde_json::Map::len).unwrap_or(0);
                assert_eq!(
                    (status, error.as_str(), members),
                    (200, "", 0),
                    "case {:?}: an installable destination must answer 200 with no members, \
                     got {status} {error} ({body})",
                    row.case
                );
                in_force = row.logging_config.clone();
            }
            Outcome::Unvalidated => {
                let destination = &row.logging_config["cloudWatchConfig"];
                let submitted_group = destination["logGroupName"].as_str().unwrap_or_default();
                let submitted_role = destination["roleArn"].as_str().unwrap_or_default();
                let expected = unvalidated_destination(submitted_group, submitted_role);
                assert_eq!(
                    (status, error.as_str(), message.as_str()),
                    (400, "ValidationException", expected.as_str()),
                    "case {:?}: a destination whose permissions cannot be validated must be \
                     refused 400 ValidationException naming the submitted pair, got {status} \
                     {error} ({body})",
                    row.case
                );
            }
            Outcome::PassRoleDenied => {
                assert_eq!(
                    (status, error.as_str(), message.as_str()),
                    (403, "AccessDeniedException", PASS_ROLE_REFUSAL),
                    "case {:?}: a role that resolves and may not be passed must be refused \
                     403 AccessDeniedException, got {status} {error} ({body})",
                    row.case
                );
            }
        }

        let reported = installed(&emu, row.case).await;
        assert_eq!(
            reported, in_force,
            "case {:?}: the configuration in force must be the last installed one, got \
             {reported:?}",
            row.case
        );
    }
}

/// RULE (the grounding half): a refusal rests on what the owning service ANSWERED, so an
/// emulator assembled with no CloudWatch Logs and no IAM to ask establishes nothing about
/// either half of the destination and installs the very configuration the table above
/// refuses.
#[tokio::test]
async fn a_destination_no_owning_service_can_be_asked_about_is_installed() {
    let emu = crate::support::emulator();
    let config = cloud_watch_config(
        "/aws/bedrock/nonexistent-probe-log-group",
        &role_arn("nonexistent-role"),
    );
    let reply = emu
        .call(
            "PUT",
            LOGGING_URI,
            Some(json!({ "loggingConfig": config.clone() })),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (200, None),
        "case \"no owning service to ask\": an ungrounded refusal must not be made, got {} \
         {:?} {:?}",
        reply.status,
        reply.error,
        reply.body
    );
    let reported = emu.call("GET", LOGGING_URI, None).await.get("loggingConfig");
    assert_eq!(
        reported, config,
        "case \"no owning service to ask\": the configuration must read back as submitted, \
         got {reported:?}"
    );
}

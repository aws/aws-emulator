// SPDX-License-Identifier: CC0-1.0

//! Attaching an SNS event destination to a configuration set when the topic's own access
//! policy has something to say about it, end to end over HTTP with Amazon SNS mounted
//! beside this service on one engine.
//!
//! The RULE these cases pin: the topic an event destination publishes to is a resource of
//! another service, so this service reaches it as its own service principal and the
//! topic's resource policy decides the publish — an EXPLICIT deny of that publish for that
//! principal refuses the write that would establish the destination, with the modelled
//! input error (`BadRequestException`, HTTP 400) naming the topic and the permission its
//! policy has to grant, and nothing is stored; every other answer a policy gives — an allow, a deny
//! about another principal, another action, another resource or another account, the
//! access policy SNS generates for a topic whose owner set none, or a topic this emulator
//! holds no policy for at all — refuses nothing, and the destination is stored and read
//! back with the topic the definition named.
//!
//! The table below spans what the rule generalises over: the effect and the shape of the
//! statements the topic's policy carries (the action spelled out, spelled in another letter
//! case, spelled with a wildcard, listed beside another action, spelled as every action and
//! spelled as every action but a named one; the principal named as this service, listed
//! beside another service, named as every principal but a named one, named as another
//! service, as every principal and as the account root; the resource named as this topic,
//! listed beside another, matched by a wildcard and named as another; a deny beside an
//! allow), the condition keys a statement scopes itself by and the operator it compares them
//! with (the account the events come from, the owner of the resource they come from, the
//! configuration set they come from — by equality and by ARN pattern — another configuration
//! set and another account), and whether the topic carries a policy of its own at all. The
//! recorded case is one row of it.
//!
//! The last cases carry the same rule onto the rest of the surface: the refusal is lifted
//! the moment the policy stops denying the publish, an update that would make an existing
//! destination publish to a denied topic is refused and leaves the stored definition
//! alone, and a topic this emulator does not hold leaves the destination served.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// The Region every call below is signed for — the one `Ses::call` signs for — and so the
/// Region of the topics and of the configuration-set ARN a policy condition may name.
const REGION: &str = "us-east-1";

/// The service principal this service publishes a configuration set's events as, as a
/// topic policy names it.
const SES_SERVICE_PRINCIPAL: &str = "ses.amazonaws.com";

/// The account every resource below belongs to: the one account this emulator answers for.
fn account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The ARN of the topic `name`, as SNS mints it for this emulator's account.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{}:{name}", account_id())
}

/// The ARN of the configuration set `name`, as this service addresses one.
fn configuration_set_arn(name: &str) -> String {
    format!(
        "arn:aws:ses:{REGION}:{}:configuration-set/{name}",
        account_id()
    )
}

/// What a row's policy is written about: the topic it is attached to, the account the
/// events would come from and the configuration set they would come from.
struct Target {
    topic_arn: String,
    account_id: String,
    configuration_set_arn: String,
}

/// An SNS client over the in-process callback into this emulator — the same seam the
/// handler reads the topic's policy through, so the setup reaches SNS the way it does.
fn sns(ses: &Ses) -> ::aws_sdk_sns::Client {
    let callback = ses.callback();
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://sns.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

/// The topic `name`, created through the SNS API, carrying `policy` as its access policy
/// when the row wrote one. Answers the ARN SNS minted for it.
async fn given_topic(ses: &Ses, name: &str, policy: Option<Value>) -> String {
    let client = sns(ses);
    let created = client
        .create_topic()
        .name(name)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: CreateTopic({name}) failed: {error:?}"));
    let arn = created.topic_arn().unwrap_or_default().to_string();
    assert_eq!(
        arn,
        topic_arn(name),
        "setup: CreateTopic({name}) answered an ARN the cases do not name"
    );
    if let Some(policy) = policy {
        set_topic_policy(ses, &arn, &policy).await;
    }
    arn
}

/// Make `policy` the access policy of the topic `arn`.
async fn set_topic_policy(ses: &Ses, arn: &str, policy: &Value) {
    sns(ses)
        .set_topic_attributes()
        .topic_arn(arn)
        .attribute_name("Policy")
        .attribute_value(policy.to_string())
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: SetTopicAttributes({arn}, Policy) failed: {error:?}"));
}

/// A policy document carrying `statements`.
fn policy(statements: Value) -> Value {
    json!({"Version": "2008-10-17", "Id": "case-policy", "Statement": statements})
}

/// One statement denying this service the publish on `topic_arn`.
fn deny_ses_publish(topic_arn: &str) -> Value {
    json!({
        "Sid": "deny-ses",
        "Effect": "Deny",
        "Principal": {"Service": SES_SERVICE_PRINCIPAL},
        "Action": "sns:Publish",
        "Resource": topic_arn,
    })
}

/// One statement allowing this service the publish on `topic_arn`.
fn allow_ses_publish(topic_arn: &str) -> Value {
    json!({
        "Sid": "allow-ses",
        "Effect": "Allow",
        "Principal": {"Service": SES_SERVICE_PRINCIPAL},
        "Action": "sns:Publish",
        "Resource": topic_arn,
    })
}

/// Convenience calls for the path these cases drive, over the shared harness.
trait EventDestinations {
    async fn given_set(&self, name: &str) -> Reply;
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn get_destinations(&self, set: &str) -> Reply;
}

impl EventDestinations for Ses {
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

/// The definition an SNS event destination publishing to `topic_arn` carries.
fn sns_definition(topic_arn: &str) -> Value {
    json!({
        "Enabled": true,
        "MatchingEventTypes": ["SEND"],
        "SnsDestination": {"TopicArn": topic_arn},
    })
}

/// The topic ARN the destination `name` of `set` reports, or `None` when the set reports
/// no destination under that name.
async fn reported_topic(ses: &Ses, set: &str, name: &str) -> Option<String> {
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
                .pointer("/SnsDestination/TopicArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
}

/// The sentence a refused destination carries: the topic the definition named, and the
/// permission that topic's policy has to grant this service for the destination to work.
/// Written out here, so the suite pins the wording rather than reading it back from the
/// code under test.
fn refusal(topic_arn: &str) -> String {
    format!(
        "Access denied to SNS topic <{topic_arn}>. Verify the SNS topic policy grants Amazon SES \
         the sns:Publish permission."
    )
}

/// One row of the table: the access policy the destination's topic carries, and whether
/// the attach is refused.
struct PolicyCase {
    case: &'static str,
    /// The configuration set and the topic the row uses, which are its own.
    set: &'static str,
    topic: &'static str,
    /// The topic's access policy, or `None` to leave the one SNS generates in place.
    policy: Option<fn(&Target) -> Value>,
    /// Whether this service must refuse the destination.
    refused: bool,
}

/// The rows: the same attach, differing in what the topic's policy says about it.
const POLICY_CASES: &[PolicyCase] = &[
    PolicyCase {
        case: "a policy denying this service the publish on the topic",
        set: "denied-set",
        topic: "denied-topic",
        policy: Some(|target| policy(json!([deny_ses_publish(&target.topic_arn)]))),
        refused: true,
    },
    PolicyCase {
        case: "a deny written with a wildcard action",
        set: "wildcard-action-set",
        topic: "wildcard-action-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "SNS:*",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny of every action for every principal, on every resource",
        set: "deny-all-set",
        topic: "deny-all-topic",
        policy: Some(|_| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "*",
                "Resource": "*",
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny scoped to the account the events come from",
        set: "source-account-set",
        topic: "source-account-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
                "Condition": {"StringEquals": {"aws:SourceAccount": target.account_id}},
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny scoped to the configuration set the events come from",
        set: "source-arn-set",
        topic: "source-arn-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
                "Condition": {
                    "StringEquals": {"aws:SourceArn": target.configuration_set_arn},
                },
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "an allow and a deny of the same publish, where the deny wins",
        set: "allow-and-deny-set",
        topic: "allow-and-deny-topic",
        policy: Some(|target| {
            policy(json!([
                allow_ses_publish(&target.topic_arn),
                deny_ses_publish(&target.topic_arn),
            ]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny written as the single statement the grammar also admits",
        set: "single-statement-set",
        topic: "single-statement-topic",
        policy: Some(|target| policy(deny_ses_publish(&target.topic_arn))),
        refused: true,
    },
    PolicyCase {
        case: "a deny whose action element lists the publish among other actions",
        set: "action-list-set",
        topic: "action-list-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": ["sns:Subscribe", "sns:Publish"],
                "Resource": target.topic_arn,
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        // An action name is compared case-insensitively, so the letter case a policy
        // spells the publish in decides nothing.
        case: "a deny spelling the publish in another letter case",
        set: "action-case-set",
        topic: "action-case-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "SNS:PUBLISH",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny of every action but another one, which the publish is not",
        set: "not-action-set",
        topic: "not-action-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "NotAction": "sns:Subscribe",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny whose principal element lists this service among others",
        set: "principal-list-set",
        topic: "principal-list-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": ["events.amazonaws.com", SES_SERVICE_PRINCIPAL]},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny of every principal but another service, which this one is not",
        set: "not-principal-set",
        topic: "not-principal-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "NotPrincipal": {"Service": "events.amazonaws.com"},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny whose resource element lists this topic among others",
        set: "resource-list-set",
        topic: "resource-list-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": [
                    format!("arn:aws:sns:{REGION}:210987654321:somebody-elses-topic"),
                    target.topic_arn.clone(),
                ],
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny naming the account's topics by a wildcard",
        set: "resource-wildcard-set",
        topic: "resource-wildcard-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": format!("arn:aws:sns:{REGION}:{}:*", target.account_id),
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny scoped to the owner of the resource the events come from",
        set: "source-owner-set",
        topic: "source-owner-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
                "Condition": {"StringEquals": {"aws:SourceOwner": target.account_id}},
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a deny scoped to this account's configuration sets by a pattern",
        set: "source-arn-pattern-set",
        topic: "source-arn-pattern-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
                "Condition": {
                    "ArnLike": {
                        "aws:SourceArn": format!(
                            "arn:aws:ses:*:{}:configuration-set/*",
                            target.account_id,
                        ),
                    },
                },
            }]))
        }),
        refused: true,
    },
    PolicyCase {
        case: "a policy allowing this service the publish and nothing else",
        set: "allowed-set",
        topic: "allowed-topic",
        policy: Some(|target| policy(json!([allow_ses_publish(&target.topic_arn)]))),
        refused: false,
    },
    PolicyCase {
        case: "a deny of the publish for another service principal",
        set: "other-principal-set",
        topic: "other-principal-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": "events.amazonaws.com"},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "a deny of the publish for the account root rather than this service",
        set: "account-root-set",
        topic: "account-root-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"AWS": format!("arn:aws:iam::{}:root", target.account_id)},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "a deny of another action for this service",
        set: "other-action-set",
        topic: "other-action-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Subscribe",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "a deny of the publish on another topic",
        set: "other-resource-set",
        topic: "other-resource-topic",
        policy: Some(|_| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": format!("arn:aws:sns:{REGION}:210987654321:somebody-elses-topic"),
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "a deny scoped to another account than the events come from",
        set: "other-account-set",
        topic: "other-account-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
                "Condition": {"StringEquals": {"aws:SourceAccount": "210987654321"}},
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "a deny scoped to another configuration set than the events come from",
        set: "other-source-arn-set",
        topic: "other-source-arn-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
                "Condition": {
                    "StringEquals": {
                        "aws:SourceArn": configuration_set_arn("a-set-nothing-publishes-from"),
                    },
                },
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "a deny of every action but the publish, which is the one action asked for",
        set: "not-action-covering-set",
        topic: "not-action-covering-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "Principal": {"Service": SES_SERVICE_PRINCIPAL},
                "NotAction": "sns:Publish",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "a deny of every principal but this service, which is the one asking",
        set: "not-principal-covering-set",
        topic: "not-principal-covering-topic",
        policy: Some(|target| {
            policy(json!([{
                "Effect": "Deny",
                "NotPrincipal": {"Service": SES_SERVICE_PRINCIPAL},
                "Action": "sns:Publish",
                "Resource": target.topic_arn,
            }]))
        }),
        refused: false,
    },
    PolicyCase {
        case: "the access policy SNS generates for a topic whose owner set none",
        set: "generated-policy-set",
        topic: "generated-policy-topic",
        policy: None,
        refused: false,
    },
];

#[tokio::test]
async fn an_sns_destination_is_refused_exactly_where_the_topics_policy_denies_the_publish() {
    for case in POLICY_CASES {
        let ses = Ses::with_sns();
        let arn = topic_arn(case.topic);
        let target = Target {
            topic_arn: arn.clone(),
            account_id: account_id(),
            configuration_set_arn: configuration_set_arn(case.set),
        };
        let written = case.policy.map(|write| write(&target));
        given_topic(&ses, case.topic, written).await;

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
            .create_destination(case.set, "events", sns_definition(&arn))
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
                refusal(&arn),
                "case {:?}: expected the authorization failure, got {:?}",
                case.case,
                reply.message()
            );
            assert_eq!(
                reported_topic(&ses, case.set, "events").await,
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
                reported_topic(&ses, case.set, "events").await.as_deref(),
                Some(arn.as_str()),
                "case {:?}: expected the stored destination to publish to {arn:?}, got {:?}",
                case.case,
                reported_topic(&ses, case.set, "events").await
            );
        }
    }
}

#[tokio::test]
async fn the_same_attach_succeeds_once_the_topic_stops_denying_the_publish() {
    let case = "a deny lifted between two identical attaches";
    let ses = Ses::with_sns();
    let arn = topic_arn("probe-topic").to_string();
    given_topic(
        &ses,
        "probe-topic",
        Some(policy(json!([deny_ses_publish(&arn)]))),
    )
    .await;
    let created = ses.given_set("probe-set").await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: setup CreateConfigurationSet expected 200, got {} {} ({})",
        created.status,
        created.error_code(),
        created.message()
    );

    let refused = ses
        .create_destination("probe-set", "probe", sns_definition(&arn))
        .await;
    assert_eq!(
        refused.status,
        400,
        "case {case:?}: expected 400 while the publish is denied, got {} {} ({})",
        refused.status,
        refused.error_code(),
        refused.message()
    );
    assert_eq!(
        reported_topic(&ses, "probe-set", "probe").await,
        None,
        "case {case:?}: the refused attach must leave nothing behind"
    );

    set_topic_policy(&ses, &arn, &policy(json!([allow_ses_publish(&arn)]))).await;
    let accepted = ses
        .create_destination("probe-set", "probe", sns_definition(&arn))
        .await;
    assert_eq!(
        accepted.status,
        200,
        "case {case:?}: expected 200 once the deny is lifted, got {} {} ({})",
        accepted.status,
        accepted.error_code(),
        accepted.message()
    );
    assert_eq!(
        reported_topic(&ses, "probe-set", "probe").await.as_deref(),
        Some(arn.as_str()),
        "case {case:?}: expected the stored destination to publish to {arn:?}, got {:?}",
        reported_topic(&ses, "probe-set", "probe").await
    );
}

#[tokio::test]
async fn an_update_onto_a_denied_topic_is_refused_and_leaves_the_destination_alone() {
    let case = "an update repointing a destination at a denied topic";
    let ses = Ses::with_sns();
    let allowed = topic_arn("update-allowed");
    let denied = topic_arn("update-denied");
    given_topic(
        &ses,
        "update-allowed",
        Some(policy(json!([allow_ses_publish(&allowed)]))),
    )
    .await;
    given_topic(
        &ses,
        "update-denied",
        Some(policy(json!([deny_ses_publish(&denied)]))),
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
        .create_destination("update-set", "events", sns_definition(&allowed))
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
        .update_destination("update-set", "events", sns_definition(&denied))
        .await;
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected 400, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    assert_eq!(
        reply.error_code(),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {} ({})",
        reply.error_code(),
        reply.message()
    );
    assert_eq!(
        reply.message(),
        refusal(&denied),
        "case {case:?}: expected the authorization failure, got {:?}",
        reply.message()
    );
    assert_eq!(
        reported_topic(&ses, "update-set", "events").await.as_deref(),
        Some(allowed.as_str()),
        "case {case:?}: the refused update must leave the stored destination alone, got {:?}",
        reported_topic(&ses, "update-set", "events").await
    );
}

#[tokio::test]
async fn a_topic_this_emulator_holds_no_policy_for_refuses_nothing() {
    // A topic of another account and another Region: nothing here holds a policy for it,
    // so there is no decision to make about the destination and it is stored as any other.
    let case = "a topic of another account, which this emulator holds no policy for";
    let ses = Ses::with_sns();
    let arn = "arn:aws:sns:eu-west-2:210987654321:somebody-elses-topic";
    let created = ses.given_set("unknown-topic-set").await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: setup CreateConfigurationSet expected 200, got {} {} ({})",
        created.status,
        created.error_code(),
        created.message()
    );

    let reply = ses
        .create_destination("unknown-topic-set", "events", sns_definition(arn))
        .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected 200, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    assert_eq!(
        reported_topic(&ses, "unknown-topic-set", "events")
            .await
            .as_deref(),
        Some(arn),
        "case {case:?}: expected the stored destination to publish to {arn:?}, got {:?}",
        reported_topic(&ses, "unknown-topic-set", "events").await
    );
}

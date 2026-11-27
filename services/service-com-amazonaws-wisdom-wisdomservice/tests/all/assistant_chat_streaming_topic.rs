// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon SNS topic an assistant of this model reports as its
//! chat-message stream.
//!
//! The rule under test: an assistant's `integrationConfiguration.topicIntegrationArn` is the
//! ARN Amazon SNS — the service that OWNS topics — answered for the topic this service
//! provisioned WITH that assistant, and that one ARN is what the create and every later read
//! of the assistant (and of the sessions opened under it) report; when the assistant is
//! deleted the topic goes with it, and no other topic of the account is touched.
//!
//! Amazon SNS is mounted beside this model on ONE engine, the way the assembled router mounts
//! them, and every assertion about a topic is resolved THERE through its own API: a topic that
//! service does not answer for is a stream a client could never subscribe to.
//!
//! The tests are TABLES: each row is one create input along an axis the rule generalises over
//! (name form and length, description, tag count, encryption configuration, signed Region),
//! because none of those is what decides which topic the assistant reports.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region a row signs with unless it is exercising the Region axis itself.
const REGION: &str = "us-west-2";

/// The routing key this model answers under (`src/service.rs`'s `OWN_REGISTRATION_KEY`):
/// the shared `wisdom` endpoint identity is served by the Amazon Q in Connect crate, so this
/// model is addressed by its own key.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// The `TopicArn` attribute `GetTopicAttributes` reports for a topic Amazon SNS holds.
const TOPIC_ARN_ATTRIBUTE: &str = "TopicArn";

/// Where an assistant reports its chat-message stream in a create or a read.
const ASSISTANT_TOPIC: &str = "/assistant/integrationConfiguration/topicIntegrationArn";

/// One wire answer, reduced to what a client reads.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn string(&self, pointer: &str) -> Option<String> {
        self.body
            .pointer(pointer)
            .filter(|value| !value.is_null())
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// Amazon Connect Wisdom and Amazon SNS on ONE engine, with an Amazon SNS client over the
/// router's own in-process callback — the very seam the topic provisioning under test reaches
/// that service through.
struct Account {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Account {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the requests still carry the SigV4
        // credential scope, which is what routes them and names the Region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// An Amazon SNS client addressed in `region`, over the in-process callback.
    fn sns(&self, region: &str) -> ::aws_sdk_sns::Client {
        let config = ::aws_sdk_sns::Config::builder()
            .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
            .region(::aws_sdk_sns::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_sns::Client::from_conf(config)
    }

    async fn send(&self, method: &str, path: &str, region: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{ROUTE_KEY}/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("https://wisdom.{region}.amazonaws.com{path}"))
            .header("host", format!("wisdom.{region}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    async fn create_assistant(&self, region: &str, body: Value) -> Reply {
        self.send("POST", "/assistants", region, Some(body)).await
    }

    async fn get_assistant(&self, region: &str, assistant_id: &str) -> Reply {
        self.send("GET", &format!("/assistants/{assistant_id}"), region, None)
            .await
    }

    async fn list_assistants(&self, region: &str) -> Reply {
        self.send("GET", "/assistants", region, None).await
    }

    async fn delete_assistant(&self, region: &str, assistant_id: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/assistants/{assistant_id}"),
            region,
            None,
        )
        .await
    }

    async fn create_session(&self, region: &str, assistant_id: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{assistant_id}/sessions"),
            region,
            Some(body),
        )
        .await
    }

    async fn get_session(&self, region: &str, assistant_id: &str, session_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{assistant_id}/sessions/{session_id}"),
            region,
            None,
        )
        .await
    }

    /// The `TopicArn` Amazon SNS answers for `topic_arn`, or `None` when it holds no topic
    /// under it. This is what makes an assertion about a reported ARN an assertion about a
    /// topic that EXISTS.
    async fn resolved_topic(&self, region: &str, topic_arn: &str) -> Option<String> {
        self.sns(region)
            .get_topic_attributes()
            .topic_arn(topic_arn)
            .send()
            .await
            .ok()?
            .attributes
            .and_then(|attributes| attributes.get(TOPIC_ARN_ATTRIBUTE).cloned())
    }

    /// Create a topic through Amazon SNS's own API, reported by its ARN — a topic of the
    /// account that no assistant owns.
    async fn create_topic(&self, region: &str, name: &str) -> String {
        self.sns(region)
            .create_topic()
            .name(name)
            .send()
            .await
            .expect("Amazon SNS must create the topic")
            .topic_arn
            .expect("a created topic reports its ARN")
    }

    /// Every topic ARN Amazon SNS holds in `region`.
    async fn topics(&self, region: &str) -> BTreeSet<String> {
        self.sns(region)
            .list_topics()
            .send()
            .await
            .expect("Amazon SNS must list its topics")
            .topics
            .unwrap_or_default()
            .into_iter()
            .filter_map(|topic| topic.topic_arn)
            .collect()
    }
}

/// One row of the create table: what differs between rows is the create's input and the
/// Region it is signed for, never which topic the assistant reports.
#[derive(Debug, Clone)]
struct Row {
    label: &'static str,
    region: &'static str,
    body: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "minimal create",
            region: REGION,
            body: json!({ "name": "stream-minimal", "type": "AGENT" }),
        },
        Row {
            label: "described, three tags",
            region: REGION,
            body: json!({
                "name": "stream.described_1",
                "type": "AGENT",
                "description": "An assistant, with punctuation. -",
                "tags": { "team": "alpha", "a.b": "beta", "cost-centre": "42" },
            }),
        },
        Row {
            label: "shortest legal name, another Region",
            region: "eu-west-3",
            body: json!({ "name": "a", "type": "AGENT" }),
        },
        Row {
            label: "longest legal name, customer managed key, third Region",
            region: "ap-southeast-2",
            body: json!({
                "name": "n".repeat(255),
                "type": "AGENT",
                "serverSideEncryptionConfiguration": { "kmsKeyId": "alias/aws/wisdom" },
            }),
        },
    ]
}

/// The fields of a reported ARN: the modelled `GenericArn` form
/// (`^arn:[a-z-]+?:[a-z-]+?:[a-z0-9-]*?:([0-9]{12})?:[a-zA-Z0-9-:/]+$`,
/// `smithy_query(query="shape",shape="AssistantIntegrationConfiguration")`) laid out as
/// partition, service, Region, account and resource.
fn arn_fields(arn: &str) -> Option<(&str, &str, &str, &str, &str)> {
    let mut fields = arn.splitn(6, ':');
    let prefix = fields.next()?;
    (prefix == "arn").then_some(())?;
    Some((
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    ))
}

/// For every row, the ARN the assistant reports is an Amazon SNS topic ARN of the Region the
/// assistant was created in and of this account, it names a topic Amazon SNS really holds,
/// and the create, both reads, the list and a session of the assistant all report that same
/// ARN.
#[tokio::test]
async fn the_reported_streaming_topic_is_a_topic_amazon_sns_holds() {
    for row in rows() {
        // One emulator per row, so the list below reports exactly this row's assistant and
        // Amazon SNS starts out holding no topic at all.
        let account = Account::new();
        let case = row.label;
        let created = account.create_assistant(row.region, row.body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let assistant_id = created
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {case:?}: no assistantId in {}", created.body));
        let reported = created
            .string(ASSISTANT_TOPIC)
            .unwrap_or_else(|| panic!("case {case:?}: no topicIntegrationArn in {}", created.body));

        // The FORM of the value: an Amazon SNS topic ARN in the Region the create was signed
        // for, of the account this emulator serves, with a topic name in it.
        let fields = arn_fields(&reported)
            .unwrap_or_else(|| panic!("case {case:?}: {reported:?} is not an ARN"));
        assert_eq!(
            (fields.0, fields.1, fields.2, fields.3),
            ("aws", "sns", row.region, engine::EMULATOR_ACCOUNT_ID),
            "case {case:?}: expected an Amazon SNS topic ARN of {} in {}, got {reported:?}",
            engine::EMULATOR_ACCOUNT_ID,
            row.region
        );
        assert!(
            !fields.4.is_empty(),
            "case {case:?}: expected a topic name in {reported:?}, got an empty resource"
        );

        // And its PROVENANCE: the ARN is the one Amazon SNS answered for the topic of THIS
        // assistant, so the service that owns topics holds it, under the ARN the assistant
        // advertises and nothing beside it.
        let resolved = account.resolved_topic(row.region, &reported).await;
        assert_eq!(
            resolved.as_ref(),
            Some(&reported),
            "case {case:?}: expected Amazon SNS to hold the reported topic {reported:?}, \
             got {resolved:?}"
        );
        let held = account.topics(row.region).await;
        assert_eq!(
            held,
            BTreeSet::from([reported.clone()]),
            "case {case:?}: expected Amazon SNS to hold exactly the assistant's topic \
             {reported:?}, got {held:?}"
        );

        // Every read of the assistant reports the create's own value, and so does a session
        // opened under it: one assistant, one stream.
        let session = account
            .create_session(row.region, &assistant_id, json!({ "name": "stream" }))
            .await;
        let session_id = session
            .string("/session/sessionId")
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {}", session.body));
        for (reader, reply, pointer) in [
            (
                "GetAssistant",
                account.get_assistant(row.region, &assistant_id).await,
                ASSISTANT_TOPIC,
            ),
            (
                "ListAssistants",
                account.list_assistants(row.region).await,
                "/assistantSummaries/0/integrationConfiguration/topicIntegrationArn",
            ),
            (
                "CreateSession",
                session,
                "/session/integrationConfiguration/topicIntegrationArn",
            ),
            (
                "GetSession",
                account
                    .get_session(row.region, &assistant_id, &session_id)
                    .await,
                "/session/integrationConfiguration/topicIntegrationArn",
            ),
        ] {
            let read = reply.string(pointer);
            assert_eq!(
                read.as_ref(),
                Some(&reported),
                "case {case:?}/{reader}: expected the same topic {reported:?}, got {read:?}"
            );
        }
    }
}

/// Every assistant gets a topic of its own, and each of them exists: three assistants in two
/// Regions leave Amazon SNS holding three distinct topics, one per assistant, each resolving
/// under the ARN that assistant reports.
#[tokio::test]
async fn each_assistant_gets_its_own_topic_and_every_one_of_them_exists() {
    let account = Account::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();

    for (case, region) in [
        ("first assistant", REGION),
        ("second assistant", REGION),
        ("third assistant, another Region", "eu-west-3"),
    ] {
        let created = account
            .create_assistant(
                region,
                json!({ "name": format!("own-{case}").replace(' ', "-"), "type": "AGENT" }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let topic = created
            .string(ASSISTANT_TOPIC)
            .unwrap_or_else(|| panic!("case {case:?}: no topicIntegrationArn in {}", created.body));
        assert!(
            reported.insert(topic.clone()),
            "case {case:?}: topic {topic} was already another assistant's"
        );
        let resolved = account.resolved_topic(region, &topic).await;
        assert_eq!(
            resolved.as_ref(),
            Some(&topic),
            "case {case:?}: expected Amazon SNS to hold {topic:?}, got {resolved:?}"
        );
    }

    // Every topic reported lives in the Region its assistant was created in, so the two
    // Regions hold two and one.
    let first_region = account.topics(REGION).await;
    let second_region = account.topics("eu-west-3").await;
    assert_eq!(
        first_region.len(),
        2,
        "expected the two assistants of {REGION} to leave two topics there, got {first_region:?}"
    );
    assert_eq!(
        second_region.len(),
        1,
        "expected the assistant of eu-west-3 to leave one topic there, got {second_region:?}"
    );
    let all: BTreeSet<String> = first_region.union(&second_region).cloned().collect();
    assert_eq!(
        all, reported,
        "expected Amazon SNS to hold exactly the topics the assistants reported \
         ({reported:?}), got {all:?}"
    );
}

/// The topic is the assistant's: deleting the assistant releases it in Amazon SNS, and
/// nothing else the account holds — another assistant's topic, a topic no assistant owns —
/// is touched.
#[tokio::test]
async fn deleting_an_assistant_releases_only_its_own_topic() {
    let account = Account::new();
    // A topic of the account that belongs to no assistant, created through Amazon SNS's own
    // API: the delete below must leave it alone.
    let unrelated = account.create_topic(REGION, "some-other-topic").await;

    let mut topics: Vec<(String, String)> = Vec::new();
    for case in ["doomed assistant", "surviving assistant"] {
        let created = account
            .create_assistant(
                REGION,
                json!({ "name": case.replace(' ', "-"), "type": "AGENT" }),
            )
            .await;
        let assistant_id = created
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {case:?}: no assistantId in {}", created.body));
        let topic = created
            .string(ASSISTANT_TOPIC)
            .unwrap_or_else(|| panic!("case {case:?}: no topicIntegrationArn in {}", created.body));
        topics.push((assistant_id, topic));
    }
    let (doomed_id, doomed_topic) = topics[0].clone();
    let (_, surviving_topic) = topics[1].clone();

    let deleted = account.delete_assistant(REGION, &doomed_id).await;
    assert_eq!(
        deleted.status, 204,
        "expected HTTP 204 deleting the assistant, got {} with body {}",
        deleted.status, deleted.body
    );

    let resolved = account.resolved_topic(REGION, &doomed_topic).await;
    assert_eq!(
        resolved, None,
        "expected Amazon SNS to hold no topic under {doomed_topic:?} once its assistant was \
         deleted, got {resolved:?}"
    );
    let held = account.topics(REGION).await;
    assert_eq!(
        held,
        BTreeSet::from([surviving_topic.clone(), unrelated.clone()]),
        "expected the surviving assistant's topic {surviving_topic:?} and the unrelated \
         topic {unrelated:?} to be all that is left, got {held:?}"
    );
}

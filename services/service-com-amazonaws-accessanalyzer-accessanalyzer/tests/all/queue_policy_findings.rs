// SPDX-License-Identifier: CC0-1.0

//! The external access an Amazon SQS queue's own access policy grants, as `ListFindings`
//! and `GetAnalyzedResource` report it.
//!
//! Every case goes through the real router: an `http::Request` is dispatched to the mounted
//! services, so the wire routing, the boundary constraint validation, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into a `state` module directly — each queue is created and given its `Policy`
//! attribute through Amazon SQS's OWN API (`CreateQueue`, `SetQueueAttributes`), its ARN is
//! the one `GetQueueAttributes` reports, and the findings are only believed as the analyzer
//! reports them.
//!
//! RULE under test: an external access analyzer reports, for every resource of its own
//! account IN ITS OWN REGION whose resource-based policy this emulator holds — a queue's
//! access policy among them — one finding per principal OUTSIDE that account that an
//! `Allow` statement of that policy grants access to, marked `isPublic` when the statement
//! admits every principal with nothing in its `Condition` binding the caller.
//!
//! The table spans what that rule generalises over for a queue: the principal forms the IAM
//! grammar keys (`AWS` as an account id, as an account root ARN and as a role ARN,
//! `Service`, and the `*` wildcard), `Allow` against `Deny`, a wildcard principal bound by a
//! condition against an unbound one, one action against several, one statement against two,
//! a principal inside the account against one outside it, and a queue carrying no policy at
//! all. The Region scoping and the round trip through `GetAnalyzedResource` are their own
//! tests.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::label;

/// The Region every call is signed for unless a case names another; it is the Region the
/// analyzer's ARN carries and the Region whose namespace holds the queues.
const REGION: &str = "us-east-1";

/// A second Region, so a case can hold the same queue policy in a Region the analyzer under
/// test was not created in.
const OTHER_REGION: &str = "eu-west-1";

/// An account that is not this emulator's own — the external one a finding reports.
const EXTERNAL_ACCOUNT: &str = "111111111111";

/// A second external account, so a case can report two findings that differ only in which
/// account they name.
const OTHER_ACCOUNT: &str = "222222222222";

/// The `ResourceType` a queue's finding carries, from the model's `ResourceType` enum
/// (`smithy_query(query="shape", shape="ResourceType")`).
const QUEUE_RESOURCE_TYPE: &str = "AWS::SQS::Queue";

/// Two actions of the queue's own service, which is the only service a queue policy may
/// name actions of.
const SEND_MESSAGE: &str = "sqs:SendMessage";
const RECEIVE_MESSAGE: &str = "sqs:ReceiveMessage";

/// The `FindingStatus` a finding whose access is still granted and which no archive rule has
/// archived carries.
const STATUS_ACTIVE: &str = "ACTIVE";

/// The account every analyzer and every queue here belongs to: the emulator's own, which is
/// the zone of trust an account analyzer monitors.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// An emulator holding the analyzer service beside the services that own the resources it
/// analyzes: a queue's policy is read out of Amazon SQS's own store, and the IAM store is
/// mounted because Amazon SQS asks it whether a principal of this account named by a queue
/// policy exists.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here is signed: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One restJson1 call to the analyzer service, signed for `region`.
    async fn analyzer_call(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Value {
        let payload = match body {
            Some(body) => serde_json::to_vec(&body).expect("the request body serializes"),
            None => Vec::new(),
        };
        let host = format!("access-analyzer.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/access-analyzer/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        json!({"status": status, "body": parsed})
    }

    /// Create an analyzer in `region` and answer the ARN it reports.
    async fn analyzer(&self, case: &str, region: &str, name: &str) -> String {
        let reply = self
            .analyzer_call(
                region,
                "PUT",
                "/analyzer",
                Some(json!({"analyzerName": name, "type": "ACCOUNT"})),
            )
            .await;
        assert_eq!(
            reply["status"], 200,
            "case {case:?}: CreateAnalyzer must succeed, got {reply:?}"
        );
        reply["body"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an arn in {reply:?}"))
            .to_string()
    }

    /// The findings `ListFindings` reports for `analyzer_arn`, with `filter` applied when one
    /// is given.
    async fn findings(
        &self,
        case: &str,
        region: &str,
        analyzer_arn: &str,
        filter: Option<Value>,
    ) -> Vec<Value> {
        let mut body = json!({"analyzerArn": analyzer_arn});
        if let Some(filter) = filter {
            body["filter"] = filter;
        }
        let reply = self
            .analyzer_call(region, "POST", "/finding", Some(body))
            .await;
        assert_eq!(
            reply["status"], 200,
            "case {case:?}: ListFindings must succeed, got {reply:?}"
        );
        reply["body"]["findings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a findings list in {reply:?}"))
            .clone()
    }

    /// What `GetAnalyzedResource` reports about `resource_arn`, or `Value::Null` when it
    /// reports no record at all. `GET /analyzed-resource?analyzerArn=…&resourceArn=…`, both
    /// members `@httpQuery`.
    async fn analyzed_resource(
        &self,
        case: &str,
        region: &str,
        analyzer_arn: &str,
        resource_arn: &str,
    ) -> Value {
        let reply = self
            .analyzer_call(
                region,
                "GET",
                &format!(
                    "/analyzed-resource?analyzerArn={}&resourceArn={}",
                    label(analyzer_arn),
                    label(resource_arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            reply["status"], 200,
            "case {case:?}: GetAnalyzedResource must succeed, got {reply:?}"
        );
        reply["body"]["resource"].clone()
    }

    /// One Amazon SQS call, signed for `region`.
    async fn sqs(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let host = format!("sqs.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", host)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonSQS.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sqs/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Create a queue in `region`, put `policy` on it through `SetQueueAttributes` when the
    /// case carries one, and answer the ARN Amazon SQS reports for it.
    async fn queue(&self, case: &str, region: &str, name: &str, policy: Option<&str>) -> String {
        let (status, created) = self
            .sqs(region, "CreateQueue", json!({"QueueName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateQueue must succeed, got {status} with {created}"
        );
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateQueue answered {created}"))
            .to_string();
        if let Some(policy) = policy {
            let (status, answered) = self
                .sqs(
                    region,
                    "SetQueueAttributes",
                    json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: SetQueueAttributes must succeed, got {status} with {answered}"
            );
        }
        let (status, attributes) = self
            .sqs(
                region,
                "GetQueueAttributes",
                json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetQueueAttributes must succeed, got {status} with {attributes}"
        );
        attributes["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: GetQueueAttributes answered {attributes}"))
            .to_string()
    }
}

/// A one-statement queue policy: `effect` to `principal` for `actions`, with an optional
/// `Condition`.
fn policy(effect: &str, principal: Value, actions: Value, condition: Option<Value>) -> String {
    let mut statement = json!({"Effect": effect, "Principal": principal, "Action": actions});
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
}

/// One grant a finding reports, reduced to what the assertions read.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Grant {
    /// The principal type the statement keyed it by, and the principal itself.
    principal: (String, String),
    is_public: bool,
    actions: Vec<String>,
    /// The condition keys the statement constrains, key-ordered.
    condition: Vec<(String, String)>,
}

impl Grant {
    fn aws(value: &str, is_public: bool, actions: &[&str]) -> Self {
        Self {
            principal: ("AWS".to_string(), value.to_string()),
            is_public,
            actions: actions.iter().map(|action| action.to_string()).collect(),
            condition: Vec::new(),
        }
    }

    fn bound(mut self, key: &str, value: &str) -> Self {
        self.condition.push((key.to_string(), value.to_string()));
        self.condition.sort();
        self
    }
}

/// One finding off the wire, reduced to the same shape.
fn reduce(finding: &Value) -> Grant {
    let principal: Vec<(String, String)> = finding
        .get("principal")
        .and_then(Value::as_object)
        .map(|map| {
            let mut entries: Vec<(String, String)> = map
                .iter()
                .map(|(kind, value)| (kind.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect();
            entries.sort();
            entries
        })
        .unwrap_or_default();
    assert!(
        principal.len() <= 1,
        "a finding reports one principal, got {finding:?}"
    );
    let mut condition: Vec<(String, String)> = finding
        .get("condition")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    condition.sort();
    Grant {
        principal: principal
            .into_iter()
            .next()
            .unwrap_or_else(|| (String::new(), String::new())),
        is_public: finding
            .get("isPublic")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        actions: finding
            .get("action")
            .and_then(Value::as_array)
            .map(|actions| {
                actions
                    .iter()
                    .map(|action| action.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        condition,
    }
}

/// One row of the table: a queue policy, and the external access the analyzer must report
/// for the queue carrying it.
struct Case {
    label: &'static str,
    /// The queue's `Policy` attribute, or `None` for a queue carrying none.
    policy: Option<String>,
    grants: Vec<Grant>,
}

/// Every row, built once so each case's policy is written beside what it must produce.
fn cases() -> Vec<Case> {
    let own = own_account();
    let external_role = format!("arn:aws:iam::{EXTERNAL_ACCOUNT}:role/peer");
    vec![
        Case {
            label: "an external account named by its id",
            policy: Some(policy(
                "Allow",
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!(SEND_MESSAGE),
                None,
            )),
            grants: vec![Grant::aws(EXTERNAL_ACCOUNT, false, &[SEND_MESSAGE])],
        },
        Case {
            label: "an external account named by its root ARN",
            policy: Some(policy(
                "Allow",
                json!({"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")}),
                json!(SEND_MESSAGE),
                None,
            )),
            // The bare id and the `:root` ARN are the two spellings of one account, so the
            // finding names it the one way.
            grants: vec![Grant::aws(OTHER_ACCOUNT, false, &[SEND_MESSAGE])],
        },
        Case {
            label: "a role of an external account",
            policy: Some(policy(
                "Allow",
                json!({"AWS": external_role.clone()}),
                json!([SEND_MESSAGE, RECEIVE_MESSAGE]),
                None,
            )),
            grants: vec![Grant::aws(
                &external_role,
                false,
                &[SEND_MESSAGE, RECEIVE_MESSAGE],
            )],
        },
        Case {
            label: "every principal, with nothing binding the caller",
            policy: Some(policy(
                "Allow",
                json!("*"),
                json!(SEND_MESSAGE),
                None,
            )),
            grants: vec![Grant::aws("*", true, &[SEND_MESSAGE])],
        },
        Case {
            label: "every principal, bound by a source-account condition",
            policy: Some(policy(
                "Allow",
                json!({"AWS": "*"}),
                json!(SEND_MESSAGE),
                Some(json!({"StringEquals": {"aws:SourceAccount": own.clone()}})),
            )),
            // The wildcard still reaches principals outside the account, so the access is
            // reported; the condition binds the caller, so it is not public.
            grants: vec![
                Grant::aws("*", false, &[SEND_MESSAGE]).bound("aws:SourceAccount", &own)
            ],
        },
        Case {
            label: "the account's own root",
            policy: Some(policy(
                "Allow",
                json!({"AWS": own.clone()}),
                json!(SEND_MESSAGE),
                None,
            )),
            grants: Vec::new(),
        },
        Case {
            label: "an Amazon Web Services service principal",
            policy: Some(policy(
                "Allow",
                json!({"Service": "sns.amazonaws.com"}),
                json!(SEND_MESSAGE),
                None,
            )),
            grants: Vec::new(),
        },
        Case {
            label: "an external account denied",
            policy: Some(policy(
                "Deny",
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!(SEND_MESSAGE),
                None,
            )),
            grants: Vec::new(),
        },
        Case {
            label: "two statements naming two external accounts",
            policy: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": {"AWS": EXTERNAL_ACCOUNT},
                            "Action": SEND_MESSAGE,
                        },
                        {
                            "Effect": "Allow",
                            "Principal": {"AWS": OTHER_ACCOUNT},
                            "Action": RECEIVE_MESSAGE,
                        },
                    ],
                })
                .to_string(),
            ),
            grants: vec![
                Grant::aws(EXTERNAL_ACCOUNT, false, &[SEND_MESSAGE]),
                Grant::aws(OTHER_ACCOUNT, false, &[RECEIVE_MESSAGE]),
            ],
        },
        Case {
            label: "a queue carrying no policy at all",
            policy: None,
            grants: Vec::new(),
        },
    ]
}

/// The whole table: each queue policy, on its own emulator, produces exactly the findings
/// the rule says it does — and every finding is about that queue, with the queue's
/// resource type, owner account and `ACTIVE` status.
#[tokio::test]
async fn a_queue_policy_reports_the_external_access_it_grants() {
    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let emulator = Emulator::new();
        let queue_arn = emulator
            .queue(
                label,
                REGION,
                &format!("probe-{index}"),
                case.policy.as_deref(),
            )
            .await;
        let analyzer = emulator.analyzer(label, REGION, &format!("probe-{index}")).await;

        let findings = emulator.findings(label, REGION, &analyzer, None).await;
        let mut observed: Vec<Grant> = findings.iter().map(reduce).collect();
        observed.sort();
        let mut expected = case.grants.clone();
        expected.sort();
        assert_eq!(
            observed, expected,
            "case {label:?}: expected {expected:?}, got {findings:?}"
        );

        for finding in &findings {
            assert_eq!(
                finding.get("resource").and_then(Value::as_str),
                Some(queue_arn.as_str()),
                "case {label:?}: a finding must be about the queue, got {finding:?}"
            );
            assert_eq!(
                finding.get("resourceType").and_then(Value::as_str),
                Some(QUEUE_RESOURCE_TYPE),
                "case {label:?}: expected the queue's resource type, got {finding:?}"
            );
            assert_eq!(
                finding.get("resourceOwnerAccount").and_then(Value::as_str),
                Some(own_account().as_str()),
                "case {label:?}: expected the owning account, got {finding:?}"
            );
            assert_eq!(
                finding.get("status").and_then(Value::as_str),
                Some(STATUS_ACTIVE),
                "case {label:?}: expected an active finding, got {finding:?}"
            );
            // "It is populated for Amazon S3 bucket findings" (`FindingSummary$sources`):
            // a queue's finding carries none.
            assert!(
                finding.get("sources").is_none_or(Value::is_null),
                "case {label:?}: a queue finding carries no sources, got {finding:?}"
            );
            assert!(
                finding
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()),
                "case {label:?}: a finding carries an id, got {finding:?}"
            );
        }
    }
}

/// Two reads of one analyzer report the same findings with the same ids: nothing of a
/// finding is minted per call, so a client that lists twice sees one finding rather than
/// two.
#[tokio::test]
async fn listing_a_queue_s_findings_twice_reports_the_same_ids() {
    let case = "repeated listing";
    let emulator = Emulator::new();
    emulator
        .queue(
            case,
            REGION,
            "stable",
            Some(&policy(
                "Allow",
                json!({"AWS": EXTERNAL_ACCOUNT}),
                json!(SEND_MESSAGE),
                None,
            )),
        )
        .await;
    let analyzer = emulator.analyzer(case, REGION, "stable").await;

    let first = emulator.findings(case, REGION, &analyzer, None).await;
    let second = emulator.findings(case, REGION, &analyzer, None).await;
    let ids = |findings: &[Value]| -> Vec<String> {
        findings
            .iter()
            .map(|finding| {
                finding
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };
    assert_eq!(
        ids(&first),
        ids(&second),
        "case {case:?}: expected the same ids on both reads, got {first:?} then {second:?}"
    );
    assert_eq!(
        first.len(),
        1,
        "case {case:?}: expected one finding, got {first:?}"
    );
}

/// An analyzer is a Regional resource: it reports the queues of its OWN Region, and a queue
/// of another Region is the analyzer of that Region's to report.
#[tokio::test]
async fn an_analyzer_reports_the_queues_of_its_own_region() {
    let case = "regional scoping";
    let emulator = Emulator::new();
    let public = policy("Allow", json!("*"), json!(SEND_MESSAGE), None);
    let home_queue = emulator
        .queue(case, REGION, "home", Some(&public))
        .await;
    let away_queue = emulator
        .queue(case, OTHER_REGION, "away", Some(&public))
        .await;
    assert_ne!(
        home_queue, away_queue,
        "case {case:?}: the two queues are two resources"
    );

    for (region, expected) in [(REGION, &home_queue), (OTHER_REGION, &away_queue)] {
        // An analyzer name is unique to the account, so each Region's analyzer is named for
        // the Region it was created in.
        let analyzer = emulator
            .analyzer(case, region, &format!("regional-{region}"))
            .await;
        let findings = emulator.findings(case, region, &analyzer, None).await;
        let resources: Vec<&str> = findings
            .iter()
            .filter_map(|finding| finding.get("resource").and_then(Value::as_str))
            .collect();
        assert_eq!(
            resources,
            vec![expected.as_str()],
            "case {case:?}: the {region} analyzer reports the {region} queue, got {findings:?}"
        );
    }
}

/// What the analyzer reports ABOUT the queue it analyzed, read back through
/// `GetAnalyzedResource`, and the `filter` that selects a queue's findings out of a listing.
#[tokio::test]
async fn an_analyzed_queue_is_reported_through_its_own_record_and_filter() {
    let case = "analyzed queue";
    let emulator = Emulator::new();
    let queue_arn = emulator
        .queue(
            case,
            REGION,
            "reported",
            Some(&policy(
                "Allow",
                json!("*"),
                json!([SEND_MESSAGE, RECEIVE_MESSAGE]),
                None,
            )),
        )
        .await;
    let analyzer = emulator.analyzer(case, REGION, "reported").await;

    let record = emulator
        .analyzed_resource(case, REGION, &analyzer, &queue_arn)
        .await;
    assert_eq!(
        record.get("resourceArn").and_then(Value::as_str),
        Some(queue_arn.as_str()),
        "case {case:?}: expected the queue's own record, got {record:?}"
    );
    assert_eq!(
        record.get("resourceType").and_then(Value::as_str),
        Some(QUEUE_RESOURCE_TYPE),
        "case {case:?}: expected the queue's resource type, got {record:?}"
    );
    assert_eq!(
        record.get("isPublic").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: the wildcard grant is public, got {record:?}"
    );
    let mut actions: Vec<&str> = record
        .get("actions")
        .and_then(Value::as_array)
        .map(|actions| actions.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    actions.sort_unstable();
    assert_eq!(
        actions,
        vec![RECEIVE_MESSAGE, SEND_MESSAGE],
        "case {case:?}: expected the statement's actions, got {record:?}"
    );

    // A criterion naming the finding's `resourceType` selects the queue's findings, which is
    // what a client filtering a listing by resource type asks for.
    let selected = emulator
        .findings(
            case,
            REGION,
            &analyzer,
            Some(json!({"resourceType": {"eq": [QUEUE_RESOURCE_TYPE]}})),
        )
        .await;
    assert_eq!(
        selected.len(),
        1,
        "case {case:?}: expected the queue's one finding, got {selected:?}"
    );
    let rejected = emulator
        .findings(
            case,
            REGION,
            &analyzer,
            Some(json!({"resourceType": {"neq": [QUEUE_RESOURCE_TYPE]}})),
        )
        .await;
    assert!(
        rejected.is_empty(),
        "case {case:?}: expected no finding of another resource type, got {rejected:?}"
    );
}

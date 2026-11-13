// SPDX-License-Identifier: CC0-1.0

//! The workspace lifecycle, end to end over HTTP: create, describe, list, rename, tag
//! and delete, each asserted against what the service PERSISTED rather than against
//! what the request carried.
//!
//! Every suite drives the real request path — `MultiServiceRouter::dispatch` over this
//! service mounted on one shared engine — so routing, the `restJson1` wire shapes, the
//! modelled response statuses and the modelled error envelopes are all exercised.
//!
//! That harness (`Amp`, `Reply`, [`text`], [`wait_for_due_transition`]) is `pub(crate)`
//! so the other suites in this test target drive the same path instead of each building
//! their own.

use bytes::Bytes;
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::WORKSPACE_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    RESOURCE_NOT_FOUND_CODE, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETING, STATUS_UPDATING,
    TAGS_PER_RESOURCE_LIMIT, TRANSITION_DELAY, WORKSPACE_QUOTA,
};
use service_com_amazonaws_amp_amazonprometheusservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A KMS key ARN satisfying `kmsKeyArn`'s constraint as this service applies it
/// (`state::KMS_KEY_ARN_PATTERN`, `^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+$`).
fn kms_key_arn(region: &str) -> String {
    format!(
        "arn:aws:kms:{region}:{}:key/1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d",
        engine::emulator_account_id()
    )
}

/// One response, as a test reads it: the HTTP status, the response headers and the
/// parsed body.
///
/// `pub(crate)`, with [`Amp`], [`text`] and [`wait_for_due_transition`], because every
/// suite in this crate drives the service over the same request path — a second copy of
/// this harness is a second set of rules about how a test reaches the emulator.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: serde_json::Value,
}

impl Reply {
    /// The error code a modelled failure renders as the JSON envelope's `__type`.
    pub(crate) fn error_code(&self) -> String {
        text(&self.body["__type"])
    }

    /// One response header, as the wire carries it — `None` when the response has none
    /// by that name.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// The emulator under test: this service on its own shared engine. Shared with the
/// other suites in this test target, which drive the same request path.
pub(crate) struct Amp {
    router: MultiServiceRouter,
    /// The SAME engine the mounted service reads and writes, so a suite can seed a
    /// record the operations under test then read it back over HTTP.
    engine: Arc<engine::InMemoryEngine>,
}

/// The host every request this harness sends is addressed to: this service's regional
/// endpoint, which is also the endpoint the emulator is reached on as far as the request
/// is concerned — so it is the host a response that announces an endpoint for a resource
/// must name.
pub(crate) fn endpoint_host(region: &str) -> String {
    format!("aps.{region}.amazonaws.com")
}

/// The URL a workspace's Prometheus API is announced at for a request this harness sent
/// to `region`: the endpoint the request was addressed to, carrying the documented
/// `/workspaces/<workspaceId>/api/v1/` path.
pub(crate) fn announced_prometheus_endpoint(region: &str, workspace_id: &str) -> String {
    format!(
        "https://{}/workspaces/{workspace_id}/api/v1/",
        endpoint_host(region)
    )
}

impl Amp {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// This service's store in that shared engine — the one the handlers reach through
    /// `crate::state::service_state`.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_amp_amazonprometheusservice::ComAmazonawsAmpAmazonprometheusserviceState,
    > {
        self.engine.state()
    }

    /// Dispatch one request, signed (unverified — authentication is off here) for
    /// `region`, which is also the region the handlers read out of the credential
    /// scope when they build an ARN or a Prometheus endpoint.
    pub(crate) async fn call(&self, method: &str, uri: &str, region: &str, body: &str) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", endpoint_host(region))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/aps/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body.to_owned()))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "response body is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Reply {
            status,
            headers,
            body,
        }
    }

    async fn create(&self, region: &str, body: &serde_json::Value) -> Reply {
        self.call("POST", "/workspaces", region, &body.to_string())
            .await
    }

    pub(crate) async fn describe(&self, region: &str, workspace_id: &str) -> Reply {
        self.call("GET", &format!("/workspaces/{workspace_id}"), region, "")
            .await
    }

    pub(crate) async fn list(&self, region: &str, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/workspaces".to_string()
        } else {
            format!("/workspaces?{query}")
        };
        self.call("GET", &uri, region, "").await
    }

    pub(crate) async fn rename(
        &self,
        region: &str,
        workspace_id: &str,
        body: &serde_json::Value,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/alias"),
            region,
            &body.to_string(),
        )
        .await
    }

    pub(crate) async fn delete(
        &self,
        region: &str,
        workspace_id: &str,
        client_token: Option<&str>,
    ) -> Reply {
        let uri = match client_token {
            Some(token) => format!("/workspaces/{workspace_id}?clientToken={token}"),
            None => format!("/workspaces/{workspace_id}"),
        };
        self.call("DELETE", &uri, region, "").await
    }

    pub(crate) async fn tag(
        &self,
        region: &str,
        resource_arn: &str,
        tags: &serde_json::Value,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/tags/{}", encode_label(resource_arn)),
            region,
            &serde_json::json!({ "tags": tags }).to_string(),
        )
        .await
    }

    pub(crate) async fn untag(&self, region: &str, resource_arn: &str, tag_keys: &[&str]) -> Reply {
        let query: Vec<String> = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", encode_label(key)))
            .collect();
        self.call(
            "DELETE",
            &format!("/tags/{}?{}", encode_label(resource_arn), query.join("&")),
            region,
            "",
        )
        .await
    }

    pub(crate) async fn list_tags(&self, region: &str, resource_arn: &str) -> Reply {
        self.call("GET", &format!("/tags/{}", encode_label(resource_arn)), region, "")
            .await
    }

    /// Create a workspace and let the clock make it `ACTIVE` — the starting point for
    /// every operation the model allows only on an active workspace.
    ///
    /// Waits for the CONDITION, by looking again until a reader reports `ACTIVE`: the
    /// settling is owed at an instant the service decides, so a caller that waited a fixed
    /// span would be asserting about how fast the host runs. The deadline is generous
    /// enough that a loaded host still settles inside it, and each look is one call of the
    /// same reader a client would poll.
    pub(crate) async fn active_workspace(&self, region: &str, alias: &str) -> String {
        let created = self.create(region, &serde_json::json!({ "alias": alias })).await;
        assert_eq!(created.status, 202, "create {alias}: {created:?}");
        let workspace_id = text(&created.body["workspaceId"]);
        let deadline = std::time::Instant::now() + SETTLING_DEADLINE;
        loop {
            let described = self.describe(region, &workspace_id).await;
            let status = text(&described.body["workspace"]["status"]["statusCode"]);
            if status == STATUS_ACTIVE {
                return workspace_id;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "create {alias}: the workspace never reported {STATUS_ACTIVE}, last status \
                 {status:?}"
            );
            tokio::time::sleep(SETTLING_LOOK_INTERVAL).await;
        }
    }

    /// Dispatch one request exactly as it was built, and answer with the status, the
    /// headers and the body BYTES.
    ///
    /// What a suite needs when the request or the response is not the control plane's
    /// JSON: a body that is a compressed protobuf document, or a response whose media
    /// type is another API's.
    pub(crate) async fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> (u16, http::HeaderMap, Vec<u8>) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        (status, headers, response.into_body().to_vec())
    }
}

/// Percent-encode one URI path segment or query value, the way an SDK does: an ARN
/// carries `:` and `/`, and `/tags/{resourceArn}` is a single (non-greedy) label.
/// Everything but the unreserved characters is encoded, so the value survives the
/// server's percent-decode unchanged.
pub(crate) fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        })
        .collect()
}

pub(crate) fn text(value: &serde_json::Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

pub(crate) fn tags_of(value: &serde_json::Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), text(value)))
                .collect()
        })
        .unwrap_or_default()
}

fn tags_json(tags: &[(String, String)]) -> serde_json::Value {
    serde_json::Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
            .collect(),
    )
}

fn expected_tags(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// Sleep past the span the emulator gives a pending lifecycle transition. Blocking is
/// fine: each suite drives one request at a time.
pub(crate) fn wait_for_due_transition() {
    std::thread::sleep(TRANSITION_DELAY + std::time::Duration::from_millis(250));
}

/// How long [`Amp::active_workspace`] keeps looking for the settling the clock owes before
/// it concludes the workspace is not settling at all, and how often it looks. The deadline
/// is far past the span the emulator gives a transition, so a loaded host still settles
/// inside it; nothing is asserted about how long it actually took.
const SETTLING_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
const SETTLING_LOOK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// `WorkspaceId`: length 1..64, pattern `[0-9A-Za-z][-.0-9A-Z_a-z]*`, and the shape the
/// service mints — `ws-` followed by a UUID.
fn assert_well_formed_workspace_id(case: &str, workspace_id: &str) {
    assert!(
        !workspace_id.is_empty() && workspace_id.len() <= 64,
        "case {case:?}: workspaceId must be 1..64 characters, got {workspace_id:?}"
    );
    let mut chars = workspace_id.chars();
    let first = chars.next().unwrap_or(' ');
    assert!(
        first.is_ascii_alphanumeric(),
        "case {case:?}: workspaceId must start with [0-9A-Za-z], got {workspace_id:?}"
    );
    assert!(
        chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')),
        "case {case:?}: workspaceId must match [0-9A-Za-z][-.0-9A-Z_a-z]*, got {workspace_id:?}"
    );
    let uuid = workspace_id
        .strip_prefix("ws-")
        .unwrap_or_else(|| panic!("case {case:?}: workspaceId must start with ws-, got {workspace_id:?}"));
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: workspaceId must carry a UUID after ws-, got {workspace_id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit())),
        "case {case:?}: the UUID in a workspaceId must be hexadecimal, got {workspace_id:?}"
    );
}

/// `WorkspaceArn`: `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:workspace/.+$`.
fn assert_well_formed_workspace_arn(case: &str, arn: &str, region: &str, workspace_id: &str) {
    let expected = format!(
        "arn:aws:aps:{region}:{}:workspace/{workspace_id}",
        engine::emulator_account_id()
    );
    assert_eq!(
        arn, expected,
        "case {case:?}: arn must name this region, the emulator account and the workspace"
    );
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: arn must carry six colon-separated segments, got {arn:?}"
    );
    assert_eq!(segments[2], "aps", "case {case:?}: arn service must be aps, got {arn:?}");
    assert!(
        segments[3].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "case {case:?}: arn region must match [-a-z0-9]+, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].chars().all(|c| c.is_ascii_digit()),
        "case {case:?}: arn account must be 12 digits, got {arn:?}"
    );
    assert!(
        segments[5].starts_with("workspace/") && segments[5].len() > "workspace/".len(),
        "case {case:?}: arn resource must be workspace/<id>, got {arn:?}"
    );
}

/// Every status this crate reports is a value the modelled `WorkspaceStatusCode` enum
/// declares — the catalog comes from the model, not from this test.
#[test]
fn reported_statuses_are_modelled_workspace_status_codes() {
    for status in [STATUS_CREATING, STATUS_ACTIVE, STATUS_UPDATING, STATUS_DELETING] {
        assert!(
            WORKSPACE_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled WorkspaceStatusCode ({WORKSPACE_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A create answers from the record it persisted, and a describe reads that same record
/// back: one row per thing the rule generalises over — the alias (absent, padded, at
/// each length bound), the tag count (none, several, the `TagMap` bound of 50), the
/// customer managed key, and the region the request is signed for.
#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    struct Case {
        name: &'static str,
        region: &'static str,
        alias: Option<String>,
        stored_alias: Option<String>,
        tags: Vec<(String, String)>,
        with_kms_key: bool,
    }

    let fifty_tags: Vec<(String, String)> = (0..50)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();

    let cases = vec![
        Case {
            name: "alias and one tag",
            region: "us-west-2",
            alias: Some("my-ws".to_string()),
            stored_alias: Some("my-ws".to_string()),
            tags: vec![("team".to_string(), "obs".to_string())],
            with_kms_key: false,
        },
        Case {
            name: "alias padded with blank space is trimmed",
            region: "eu-west-1",
            alias: Some("   padded ws   ".to_string()),
            stored_alias: Some("padded ws".to_string()),
            tags: Vec::new(),
            with_kms_key: false,
        },
        Case {
            name: "no alias and no tags",
            region: "us-east-1",
            alias: None,
            stored_alias: None,
            tags: Vec::new(),
            with_kms_key: false,
        },
        Case {
            name: "one-character alias (WorkspaceAlias lower bound)",
            region: "ap-southeast-2",
            alias: Some("a".to_string()),
            stored_alias: Some("a".to_string()),
            tags: vec![
                ("env".to_string(), "dev".to_string()),
                ("owner".to_string(), "platform".to_string()),
            ],
            with_kms_key: false,
        },
        Case {
            name: "100-character alias (WorkspaceAlias upper bound)",
            region: "us-west-2",
            alias: Some("x".repeat(100)),
            stored_alias: Some("x".repeat(100)),
            tags: Vec::new(),
            with_kms_key: false,
        },
        Case {
            name: "fifty tags (the TagMap bound)",
            region: "us-west-2",
            alias: Some("fifty-tags".to_string()),
            stored_alias: Some("fifty-tags".to_string()),
            tags: fifty_tags,
            with_kms_key: false,
        },
        Case {
            name: "customer managed KMS key",
            region: "eu-central-1",
            alias: Some("encrypted".to_string()),
            stored_alias: Some("encrypted".to_string()),
            tags: vec![("team".to_string(), "obs".to_string())],
            with_kms_key: true,
        },
    ];

    for case in cases {
        let amp = Amp::new();
        let mut request = serde_json::Map::new();
        if let Some(alias) = &case.alias {
            request.insert("alias".to_string(), serde_json::Value::String(alias.clone()));
        }
        if !case.tags.is_empty() {
            request.insert("tags".to_string(), tags_json(&case.tags));
        }
        let key_arn = kms_key_arn(case.region);
        if case.with_kms_key {
            request.insert(
                "kmsKeyArn".to_string(),
                serde_json::Value::String(key_arn.clone()),
            );
        }

        let created = amp
            .create(case.region, &serde_json::Value::Object(request))
            .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: CreateWorkspace must answer 202, got {created:?}",
            case.name
        );

        let workspace_id = text(&created.body["workspaceId"]);
        assert_well_formed_workspace_id(case.name, &workspace_id);
        assert_well_formed_workspace_arn(
            case.name,
            &text(&created.body["arn"]),
            case.region,
            &workspace_id,
        );
        assert_eq!(
            text(&created.body["status"]["statusCode"]),
            STATUS_CREATING,
            "case {:?}: a new workspace is CREATING, got {created:?}",
            case.name
        );
        assert_eq!(
            tags_of(&created.body["tags"]),
            expected_tags(&case.tags),
            "case {:?}: create must echo the tags it persisted, got {created:?}",
            case.name
        );
        let expected_key = case.with_kms_key.then(|| key_arn.clone());
        assert_eq!(
            created.body["kmsKeyArn"].as_str().map(str::to_string),
            expected_key,
            "case {:?}: create must echo the kmsKeyArn it persisted, got {created:?}",
            case.name
        );

        let described = amp.describe(case.region, &workspace_id).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeWorkspace must answer 200, got {described:?}",
            case.name
        );
        let workspace = &described.body["workspace"];
        assert_eq!(
            text(&workspace["workspaceId"]),
            workspace_id,
            "case {:?}: describe must report the created id, got {described:?}",
            case.name
        );
        assert_eq!(
            workspace["alias"].as_str().map(str::to_string),
            case.stored_alias,
            "case {:?}: describe must report the stored alias, got {described:?}",
            case.name
        );
        assert_well_formed_workspace_arn(
            case.name,
            &text(&workspace["arn"]),
            case.region,
            &workspace_id,
        );
        assert_eq!(
            text(&workspace["prometheusEndpoint"]),
            announced_prometheus_endpoint(case.region, &workspace_id),
            "case {:?}: describe must announce the endpoint this request reached the \
             service on, got {described:?}",
            case.name
        );
        assert_eq!(
            tags_of(&workspace["tags"]),
            expected_tags(&case.tags),
            "case {:?}: describe must report the stored tags, got {described:?}",
            case.name
        );
        assert_eq!(
            workspace["kmsKeyArn"].as_str().map(str::to_string),
            expected_key,
            "case {:?}: describe must report the stored kmsKeyArn, got {described:?}",
            case.name
        );
        let created_at = workspace["createdAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: createdAt must be a timestamp, got {described:?}", case.name));
        assert!(
            created_at > 0.0,
            "case {:?}: createdAt must be populated, got {created_at}",
            case.name
        );

        // A read never moves the record: the second describe reports the same instant.
        let again = amp.describe(case.region, &workspace_id).await;
        assert_eq!(
            again.body["workspace"]["createdAt"].as_f64(),
            Some(created_at),
            "case {:?}: createdAt must be stable across reads, got {again:?}",
            case.name
        );

        // The workspace a create persisted is the one a listing reports.
        let listed = amp.list(case.region, "").await;
        let ids: Vec<String> = listed.body["workspaces"]
            .as_array()
            .map(|items| items.iter().map(|item| text(&item["workspaceId"])).collect())
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![workspace_id.clone()],
            "case {:?}: ListWorkspaces must carry the created workspace, got {listed:?}",
            case.name
        );
    }
}

/// `CREATING` is transient: the clock owes the move to `ACTIVE`, and nothing else
/// changes with it.
#[tokio::test]
async fn a_creating_workspace_becomes_active_once_the_transition_is_due() {
    let amp = Amp::new();
    let created = amp
        .create("us-west-2", &serde_json::json!({ "alias": "lifecycle" }))
        .await;
    let workspace_id = text(&created.body["workspaceId"]);

    let before = amp.describe("us-west-2", &workspace_id).await;
    assert_eq!(
        text(&before.body["workspace"]["status"]["statusCode"]),
        STATUS_CREATING,
        "case {workspace_id:?}: straight after create the workspace is still CREATING, got {before:?}"
    );

    wait_for_due_transition();

    let after = amp.describe("us-west-2", &workspace_id).await;
    assert_eq!(
        text(&after.body["workspace"]["status"]["statusCode"]),
        STATUS_ACTIVE,
        "case {workspace_id:?}: once the transition is due the workspace is ACTIVE, got {after:?}"
    );
    assert_eq!(
        after.body["workspace"]["createdAt"].as_f64(),
        before.body["workspace"]["createdAt"].as_f64(),
        "case {workspace_id:?}: the transition must not move createdAt, got {after:?}"
    );
    assert_eq!(
        text(&after.body["workspace"]["alias"]),
        "lifecycle",
        "case {workspace_id:?}: the transition must not move the alias, got {after:?}"
    );

    let listed = amp.list("us-west-2", "").await;
    assert_eq!(
        text(&listed.body["workspaces"][0]["status"]["statusCode"]),
        STATUS_ACTIVE,
        "case {workspace_id:?}: a listing reports the same status a describe does, got {listed:?}"
    );
}

/// `clientToken` is `@idempotencyToken`: the same token names the same workspace, a
/// different token (or none) a new one.
#[tokio::test]
async fn replaying_a_client_token_returns_the_same_workspace() {
    struct Case {
        name: &'static str,
        first: Option<&'static str>,
        second: Option<&'static str>,
        same_workspace: bool,
    }

    let cases = [
        Case {
            name: "the same token twice",
            first: Some("token-aaa"),
            second: Some("token-aaa"),
            same_workspace: true,
        },
        Case {
            name: "two different tokens",
            first: Some("token-aaa"),
            second: Some("token-bbb"),
            same_workspace: false,
        },
        Case {
            name: "no token at all",
            first: None,
            second: None,
            same_workspace: false,
        },
        Case {
            name: "a token with the printable punctuation the pattern allows",
            first: Some("~token!42"),
            second: Some("~token!42"),
            same_workspace: true,
        },
    ];

    for case in cases {
        let amp = Amp::new();
        let body = |token: Option<&str>| {
            let mut request = serde_json::Map::new();
            request.insert(
                "alias".to_string(),
                serde_json::Value::String("idempotent".to_string()),
            );
            if let Some(token) = token {
                request.insert(
                    "clientToken".to_string(),
                    serde_json::Value::String(token.to_string()),
                );
            }
            serde_json::Value::Object(request)
        };

        let first = amp.create("us-west-2", &body(case.first)).await;
        let second = amp.create("us-west-2", &body(case.second)).await;
        assert_eq!(
            first.status, 202,
            "case {:?}: the first create must answer 202, got {first:?}",
            case.name
        );
        assert_eq!(
            second.status, 202,
            "case {:?}: the replayed create must answer 202, got {second:?}",
            case.name
        );

        let first_id = text(&first.body["workspaceId"]);
        let second_id = text(&second.body["workspaceId"]);
        assert_eq!(
            first_id == second_id,
            case.same_workspace,
            "case {:?}: expected same_workspace={}, got {first_id:?} and {second_id:?}",
            case.name,
            case.same_workspace
        );

        let listed = amp.list("us-west-2", "").await;
        let count = listed.body["workspaces"].as_array().map_or(0, Vec::len);
        let expected = if case.same_workspace { 1 } else { 2 };
        assert_eq!(
            count, expected,
            "case {:?}: expected {expected} stored workspaces, got {listed:?}",
            case.name
        );
    }
}

/// `alias` on a listing "filters the results to only the workspaces with names that
/// start with the value that you specify here", and is trimmed the way a stored alias
/// is.
#[tokio::test]
async fn list_workspaces_filters_by_alias_prefix() {
    let amp = Amp::new();
    for alias in ["prod-metrics", "prod-logs", "dev-metrics"] {
        amp.create("us-west-2", &serde_json::json!({ "alias": alias }))
            .await;
    }
    // One workspace with no alias at all: it matches no prefix, but an unfiltered
    // listing still carries it.
    amp.create("us-west-2", &serde_json::json!({})).await;

    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("", vec!["", "dev-metrics", "prod-logs", "prod-metrics"]),
        ("alias=prod", vec!["prod-logs", "prod-metrics"]),
        ("alias=prod-m", vec!["prod-metrics"]),
        ("alias=dev", vec!["dev-metrics"]),
        ("alias=d", vec!["dev-metrics"]),
        ("alias=nothing-matches-this", vec![]),
        ("alias=%20%20prod%20%20", vec!["prod-logs", "prod-metrics"]),
    ];

    for (query, expected) in cases {
        let listed = amp.list("us-west-2", query).await;
        assert_eq!(
            listed.status, 200,
            "case {query:?}: ListWorkspaces must answer 200, got {listed:?}"
        );
        let mut aliases: Vec<String> = listed.body["workspaces"]
            .as_array()
            .map(|items| items.iter().map(|item| text(&item["alias"])).collect())
            .unwrap_or_default();
        aliases.sort();
        assert_eq!(
            aliases,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {query:?}: unexpected filtered listing, got {listed:?}"
        );
    }
}

/// `@paginated(inputToken: nextToken, outputToken: nextToken, items: workspaces)`:
/// every page carries at most `maxResults` items, the token walks the rest, and the
/// pages together are exactly the stored set — once per page size, including each
/// published bound of `maxResults` and one past it.
#[tokio::test]
async fn list_workspaces_pages_with_max_results_and_next_token() {
    let amp = Amp::new();
    let mut all_ids = Vec::new();
    for index in 0..5 {
        let created = amp
            .create(
                "us-west-2",
                &serde_json::json!({ "alias": format!("paged-{index}") }),
            )
            .await;
        all_ids.push(text(&created.body["workspaceId"]));
    }
    all_ids.sort();

    for page_size in [1usize, 2, 3, 5, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= all_ids.len() + 1,
                "case {page_size}: pagination did not terminate, saw {seen:?}"
            );
            let mut query = format!("maxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&nextToken={}", encode_label(token)));
            }
            let listed = amp.list("us-west-2", &query).await;
            assert_eq!(
                listed.status, 200,
                "case {page_size}: ListWorkspaces must answer 200, got {listed:?}"
            );
            let page: Vec<String> = listed.body["workspaces"]
                .as_array()
                .map(|items| items.iter().map(|item| text(&item["workspaceId"])).collect())
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {page_size}: a page must hold at most maxResults items, got {page:?}"
            );
            assert!(
                page.iter().all(|id| !seen.contains(id)),
                "case {page_size}: pages must not repeat an item, got {page:?} after {seen:?}"
            );
            seen.extend(page);
            match listed.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all_ids,
            "case {page_size}: the pages together must be the stored set"
        );
        let expected_pages = all_ids.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {page_size}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // One past each published bound of `maxResults` (range 1..1000) is refused before
    // the handler ever runs.
    for out_of_range in ["maxResults=0", "maxResults=1001"] {
        let refused = amp.list("us-west-2", out_of_range).await;
        assert_eq!(
            refused.status, 400,
            "case {out_of_range:?}: a maxResults outside 1..1000 must be refused, got {refused:?}"
        );
    }
}

/// `UpdateWorkspaceAlias` replaces the stored alias, so every read taken once the rename
/// has settled reports the new value — one row per form the new alias takes. Each rename
/// is accepted asynchronously and the new alias is published by the owed settling, so the
/// row waits for it before reading the record back and renaming the same workspace again
/// (the status rule itself is pinned by `workspace_rename_is_accepted_asynchronously`,
/// and the publication rule by `workspace_alias_publication`).
#[tokio::test]
async fn update_workspace_alias_changes_what_later_reads_report() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace("us-west-2", "first-name").await;

    let cases: Vec<(&str, serde_json::Value, Option<String>)> = vec![
        (
            "a new alias",
            serde_json::json!({ "alias": "renamed" }),
            Some("renamed".to_string()),
        ),
        (
            "an alias padded with blank space is trimmed",
            serde_json::json!({ "alias": "  spaced out  " }),
            Some("spaced out".to_string()),
        ),
        (
            "a 100-character alias (WorkspaceAlias upper bound)",
            serde_json::json!({ "alias": "y".repeat(100) }),
            Some("y".repeat(100)),
        ),
        (
            "a one-character alias (WorkspaceAlias lower bound)",
            serde_json::json!({ "alias": "z" }),
            Some("z".to_string()),
        ),
        (
            "an omitted alias leaves the workspace with none",
            serde_json::json!({}),
            None,
        ),
        (
            "an alias sent with a client token",
            serde_json::json!({ "alias": "with-token", "clientToken": "rename-1" }),
            Some("with-token".to_string()),
        ),
    ];

    for (case, body, expected) in cases {
        let renamed = amp.rename("us-west-2", &workspace_id, &body).await;
        assert_eq!(
            renamed.status, 204,
            "case {case:?}: UpdateWorkspaceAlias must answer 204, got {renamed:?}"
        );

        // The rename publishes its alias when the owed settling comes due, so the reads
        // below are taken after it.
        wait_for_due_transition();

        let described = amp.describe("us-west-2", &workspace_id).await;
        assert_eq!(
            described.body["workspace"]["alias"].as_str().map(str::to_string),
            expected,
            "case {case:?}: describe must report the new alias, got {described:?}"
        );
        let listed = amp.list("us-west-2", "").await;
        assert_eq!(
            listed.body["workspaces"][0]["alias"].as_str().map(str::to_string),
            expected,
            "case {case:?}: a listing must report the new alias, got {listed:?}"
        );
    }

    let unknown = amp
        .rename(
            "us-west-2",
            "ws-00000000-0000-4000-8000-000000000000",
            &serde_json::json!({ "alias": "nope" }),
        )
        .await;
    assert_eq!(
        unknown.status, 404,
        "case \"unknown workspace\": UpdateWorkspaceAlias must answer 404, got {unknown:?}"
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case \"unknown workspace\": unexpected error code, got {unknown:?}"
    );
}

/// The three `/tags/{resourceArn}` operations read and write the SAME tag map the
/// workspace carries: a create seeds it, `TagResource` appends and replaces,
/// `UntagResource` removes, and both readers report what is stored.
#[tokio::test]
async fn tags_round_trip_through_the_tag_operations() {
    let amp = Amp::new();
    let created = amp
        .create(
            "us-west-2",
            &serde_json::json!({ "alias": "tagged", "tags": { "team": "obs" } }),
        )
        .await;
    let workspace_id = text(&created.body["workspaceId"]);
    let arn = text(&created.body["arn"]);

    // Each row is applied in order; `expected` is the tag map every reader must then
    // report.
    struct TagCase {
        name: &'static str,
        add: serde_json::Value,
        remove: Vec<&'static str>,
        expected: Vec<(&'static str, &'static str)>,
    }

    let cases = vec![
        TagCase {
            name: "the tags CreateWorkspace persisted",
            add: serde_json::json!({}),
            remove: vec![],
            expected: vec![("team", "obs")],
        },
        TagCase {
            name: "a new key is appended",
            add: serde_json::json!({ "env": "dev" }),
            remove: vec![],
            expected: vec![("env", "dev"), ("team", "obs")],
        },
        TagCase {
            name: "an existing key's value is replaced",
            add: serde_json::json!({ "team": "platform" }),
            remove: vec![],
            expected: vec![("env", "dev"), ("team", "platform")],
        },
        TagCase {
            name: "several keys at once",
            add: serde_json::json!({ "cost-centre": "1234", "tier": "gold" }),
            remove: vec![],
            expected: vec![
                ("cost-centre", "1234"),
                ("env", "dev"),
                ("team", "platform"),
                ("tier", "gold"),
            ],
        },
        TagCase {
            name: "one key removed",
            add: serde_json::json!({}),
            remove: vec!["team"],
            expected: vec![("cost-centre", "1234"), ("env", "dev"), ("tier", "gold")],
        },
        TagCase {
            name: "several keys removed, including one the resource never carried",
            add: serde_json::json!({}),
            remove: vec!["tier", "cost-centre", "never-set"],
            expected: vec![("env", "dev")],
        },
    ];

    for TagCase {
        name: case,
        add: to_add,
        remove: to_remove,
        expected,
    } in cases
    {
        if !to_add.as_object().is_none_or(serde_json::Map::is_empty) {
            let tagged = amp.tag("us-west-2", &arn, &to_add).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: TagResource must answer 200, got {tagged:?}"
            );
        }
        if !to_remove.is_empty() {
            let untagged = amp.untag("us-west-2", &arn, &to_remove).await;
            assert_eq!(
                untagged.status, 200,
                "case {case:?}: UntagResource must answer 200, got {untagged:?}"
            );
        }

        let expected: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        let listed = amp.list_tags("us-west-2", &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListTagsForResource must answer 200, got {listed:?}"
        );
        assert_eq!(
            tags_of(&listed.body["tags"]),
            expected,
            "case {case:?}: ListTagsForResource must report the stored tags, got {listed:?}"
        );

        let described = amp.describe("us-west-2", &workspace_id).await;
        assert_eq!(
            tags_of(&described.body["workspace"]["tags"]),
            expected,
            "case {case:?}: describe must report the same tags, got {described:?}"
        );
    }

    // "Keys must not begin with aws:" (`TagResource$tags`).
    for reserved in ["aws:cloudformation:stack-name", "aws:anything"] {
        let refused = amp
            .tag(
                "us-west-2",
                &arn,
                &serde_json::json!({ reserved: "nope" }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {reserved:?}: a reserved tag key must be refused, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {reserved:?}: unexpected error code, got {refused:?}"
        );
    }
    let still_there = amp.list_tags("us-west-2", &arn).await;
    assert_eq!(
        tags_of(&still_there.body["tags"]),
        expected_tags(&[("env".to_string(), "dev".to_string())]),
        "case \"refused tagging\": a refused request must not change the stored tags, got {still_there:?}"
    );
    let refused_at_create = amp
        .create(
            "us-west-2",
            &serde_json::json!({ "alias": "reserved", "tags": { "aws:nope": "x" } }),
        )
        .await;
    assert_eq!(
        refused_at_create.status, 400,
        "case \"reserved tag key at create\": must be refused, got {refused_at_create:?}"
    );

    // An ARN naming no stored resource is a 404 from every one of the three, with the
    // code aps answers on this ARN-addressed path (`state::RESOURCE_NOT_FOUND_CODE`);
    // `tests/all/rejection_codes.rs` holds that rule to a table of ARN types.
    let unknown_arn = format!(
        "arn:aws:aps:us-west-2:{}:workspace/ws-00000000-0000-4000-8000-000000000000",
        engine::emulator_account_id()
    );
    let replies = vec![
        ("ListTagsForResource", amp.list_tags("us-west-2", &unknown_arn).await),
        (
            "TagResource",
            amp.tag("us-west-2", &unknown_arn, &serde_json::json!({ "a": "b" }))
                .await,
        ),
        (
            "UntagResource",
            amp.untag("us-west-2", &unknown_arn, &["a"]).await,
        ),
    ];
    for (case, reply) in replies {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an unknown resourceArn must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            RESOURCE_NOT_FOUND_CODE,
            "case {case:?}: unexpected error code, got {reply:?}"
        );
    }
}

/// `DeleteWorkspace` starts the deletion: the workspace reports `DELETING`, and once
/// the clock owes the removal it is observably gone from every reader.
#[tokio::test]
async fn delete_workspace_makes_the_workspace_observably_gone() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace("us-west-2", "doomed").await;
    let arn = text(&amp.describe("us-west-2", &workspace_id).await.body["workspace"]["arn"]);

    let deleted = amp.delete("us-west-2", &workspace_id, Some("delete-1")).await;
    assert_eq!(
        deleted.status, 202,
        "case {workspace_id:?}: DeleteWorkspace must answer 202, got {deleted:?}"
    );

    let deleting = amp.describe("us-west-2", &workspace_id).await;
    assert_eq!(
        text(&deleting.body["workspace"]["status"]["statusCode"]),
        STATUS_DELETING,
        "case {workspace_id:?}: a workspace being deleted reports DELETING, got {deleting:?}"
    );
    let listed = amp.list("us-west-2", "").await;
    assert_eq!(
        text(&listed.body["workspaces"][0]["status"]["statusCode"]),
        STATUS_DELETING,
        "case {workspace_id:?}: a listing reports DELETING too, got {listed:?}"
    );

    // Replaying the very call that started the deletion is idempotent.
    let replayed = amp.delete("us-west-2", &workspace_id, Some("delete-1")).await;
    assert_eq!(
        replayed.status, 202,
        "case {workspace_id:?}: replaying the delete must answer 202, got {replayed:?}"
    );

    wait_for_due_transition();

    let gone = amp.describe("us-west-2", &workspace_id).await;
    assert_eq!(
        gone.status, 404,
        "case {workspace_id:?}: a deleted workspace must answer 404, got {gone:?}"
    );
    assert_eq!(
        gone.error_code(),
        "ResourceNotFoundException",
        "case {workspace_id:?}: unexpected error code, got {gone:?}"
    );
    let after = amp.list("us-west-2", "").await;
    assert_eq!(
        after.body["workspaces"].as_array().map_or(1, Vec::len),
        0,
        "case {workspace_id:?}: a deleted workspace must leave the listing, got {after:?}"
    );
    let tags_gone = amp.list_tags("us-west-2", &arn).await;
    assert_eq!(
        tags_gone.status, 404,
        "case {workspace_id:?}: the deleted workspace's tags must be gone too, got {tags_gone:?}"
    );

    for unknown in [
        "ws-00000000-0000-4000-8000-000000000000",
        "ws-does.not.exist",
    ] {
        let reply = amp.delete("us-west-2", unknown, None).await;
        assert_eq!(
            reply.status, 404,
            "case {unknown:?}: deleting an unknown workspace must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code, got {reply:?}"
        );
    }
}

/// `WorkspaceStatusCode` documents that updates and deletions "are allowed only when
/// status is ACTIVE", so a workspace in a transient status refuses both with the
/// modelled `ConflictException` (409).
#[tokio::test]
async fn a_workspace_that_is_not_active_refuses_updates_and_deletions() {
    // While CREATING.
    for case in ["UpdateWorkspaceAlias", "DeleteWorkspace"] {
        let amp = Amp::new();
        let created = amp
            .create("us-west-2", &serde_json::json!({ "alias": "still-creating" }))
            .await;
        let workspace_id = text(&created.body["workspaceId"]);
        let reply = if case == "DeleteWorkspace" {
            amp.delete("us-west-2", &workspace_id, None).await
        } else {
            amp.rename(
                "us-west-2",
                &workspace_id,
                &serde_json::json!({ "alias": "too-early" }),
            )
            .await
        };
        assert_eq!(
            reply.status, 409,
            "case {case:?} while CREATING: must answer 409, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {case:?} while CREATING: unexpected error code, got {reply:?}"
        );
    }

    // While DELETING.
    for case in ["UpdateWorkspaceAlias", "DeleteWorkspace with another token"] {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace("us-west-2", "going-away").await;
        let started = amp.delete("us-west-2", &workspace_id, Some("delete-1")).await;
        assert_eq!(
            started.status, 202,
            "case {case:?}: the first delete must answer 202, got {started:?}"
        );
        let reply = if case == "UpdateWorkspaceAlias" {
            amp.rename(
                "us-west-2",
                &workspace_id,
                &serde_json::json!({ "alias": "too-late" }),
            )
            .await
        } else {
            amp.delete("us-west-2", &workspace_id, Some("delete-2")).await
        };
        assert_eq!(
            reply.status, 409,
            "case {case:?} while DELETING: must answer 409, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {case:?} while DELETING: unexpected error code, got {reply:?}"
        );
    }
}

/// The store is BOUNDED: once it holds `WORKSPACE_QUOTA` workspaces a create is refused
/// with the modelled `ServiceQuotaExceededException` (402) rather than growing the state
/// further, and deleting one makes room again.
#[tokio::test]
async fn create_workspace_refuses_once_the_store_is_at_its_quota() {
    let amp = Amp::new();
    let mut first_id = String::new();
    for index in 0..WORKSPACE_QUOTA {
        let created = amp
            .create(
                "us-west-2",
                &serde_json::json!({ "alias": format!("bulk-{index:04}") }),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {index}: filling the store must answer 202, got {created:?}"
        );
        if index == 0 {
            first_id = text(&created.body["workspaceId"]);
        }
    }

    let refused = amp
        .create("us-west-2", &serde_json::json!({ "alias": "one-too-many" }))
        .await;
    assert_eq!(
        refused.status, 402,
        "case \"at quota\": a create past the bound must answer 402, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "case \"at quota\": unexpected error code, got {refused:?}"
    );

    // The refusal stored nothing: the store still holds exactly the quota.
    let listed = amp
        .list("us-west-2", &format!("maxResults={WORKSPACE_QUOTA}"))
        .await;
    assert_eq!(
        listed.body["workspaces"].as_array().map_or(0, Vec::len),
        WORKSPACE_QUOTA,
        "case \"at quota\": the store must still hold exactly the quota"
    );
    assert!(
        listed.body["nextToken"].is_null(),
        "case \"at quota\": a page holding every workspace carries no nextToken, got {listed:?}"
    );

    // Deleting one releases its record, so a create fits again.
    wait_for_due_transition();
    let deleted = amp.delete("us-west-2", &first_id, Some("make-room")).await;
    assert_eq!(
        deleted.status, 202,
        "case \"make room\": DeleteWorkspace must answer 202, got {deleted:?}"
    );
    wait_for_due_transition();
    let created = amp
        .create("us-west-2", &serde_json::json!({ "alias": "fits-again" }))
        .await;
    assert_eq!(
        created.status, 202,
        "case \"make room\": a create must fit once a record is released, got {created:?}"
    );
}

/// A resource holds at most `TAGS_PER_RESOURCE_LIMIT` tags (`TagMap` is modelled
/// `len ..50`, which is also the per-resource limit), so a merge that would carry it past
/// the bound is refused while one that only replaces values is not.
#[tokio::test]
async fn tag_resource_refuses_to_carry_a_resource_past_the_tag_limit() {
    let amp = Amp::new();
    let seeded: Vec<(String, String)> = (0..TAGS_PER_RESOURCE_LIMIT - 2)
        .map(|i| (format!("seed-{i:02}"), format!("value-{i:02}")))
        .collect();
    let created = amp
        .create(
            "us-west-2",
            &serde_json::json!({ "alias": "many-tags", "tags": tags_json(&seeded) }),
        )
        .await;
    assert_eq!(created.status, 202, "seeding the tags must answer 202, got {created:?}");
    let arn = text(&created.body["arn"]);

    // Filling the map up to the bound is accepted.
    let filled = amp
        .tag(
            "us-west-2",
            &arn,
            &serde_json::json!({ "last-but-one": "a", "last": "b" }),
        )
        .await;
    assert_eq!(
        filled.status, 200,
        "case \"up to the bound\": TagResource must answer 200, got {filled:?}"
    );
    assert_eq!(
        amp.list_tags("us-west-2", &arn).await.body["tags"]
            .as_object()
            .map_or(0, serde_json::Map::len),
        TAGS_PER_RESOURCE_LIMIT,
        "case \"up to the bound\": the resource must hold exactly the limit"
    );

    // One key past it is refused, and the refusal changes nothing.
    let refused = amp
        .tag("us-west-2", &arn, &serde_json::json!({ "one-too-many": "c" }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"one past the bound\": TagResource must be refused, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case \"one past the bound\": unexpected error code, got {refused:?}"
    );
    let unchanged = amp.list_tags("us-west-2", &arn).await;
    assert_eq!(
        unchanged.body["tags"].as_object().map_or(0, serde_json::Map::len),
        TAGS_PER_RESOURCE_LIMIT,
        "case \"one past the bound\": a refused merge must not change the stored tags, got {unchanged:?}"
    );

    // Replacing an existing key's value does not grow the map, so it is accepted at the
    // bound.
    let replaced = amp
        .tag("us-west-2", &arn, &serde_json::json!({ "last": "replaced" }))
        .await;
    assert_eq!(
        replaced.status, 200,
        "case \"replace at the bound\": TagResource must answer 200, got {replaced:?}"
    );
    let listed = amp.list_tags("us-west-2", &arn).await;
    assert_eq!(
        text(&listed.body["tags"]["last"]),
        "replaced",
        "case \"replace at the bound\": the value must be replaced, got {listed:?}"
    );

    // Removing one makes room for a new key again.
    let untagged = amp.untag("us-west-2", &arn, &["seed-00"]).await;
    assert_eq!(
        untagged.status, 200,
        "case \"make room\": UntagResource must answer 200, got {untagged:?}"
    );
    let fits = amp
        .tag("us-west-2", &arn, &serde_json::json!({ "one-too-many": "c" }))
        .await;
    assert_eq!(
        fits.status, 200,
        "case \"make room\": the new key must fit once one is removed, got {fits:?}"
    );
}

/// An id naming no stored workspace is a `ResourceNotFoundException` (404), whatever
/// its form — never an empty 200 and never a 501.
#[tokio::test]
async fn describing_an_unknown_workspace_is_a_resource_not_found() {
    let amp = Amp::new();
    // A stored workspace, so the store is not simply empty.
    amp.create("us-west-2", &serde_json::json!({ "alias": "present" }))
        .await;

    for unknown in [
        "ws-does-not-exist",
        "ws-00000000-0000-4000-8000-000000000000",
        "w",
        "WS-UPPER-CASE",
    ] {
        let reply = amp.describe("us-west-2", unknown).await;
        assert_eq!(
            reply.status, 404,
            "case {unknown:?}: DescribeWorkspace must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceId"]),
            unknown,
            "case {unknown:?}: the error must name the resource asked for, got {reply:?}"
        );
    }
}

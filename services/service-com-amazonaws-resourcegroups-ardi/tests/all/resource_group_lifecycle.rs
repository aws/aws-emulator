// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource-group lifecycle and static membership.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses, so these exercise the whole restJson1 path: routing, boundary
//! validation, the handler, and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a created group is persisted, and a read returns what was created;
//! * a group's ARN is `arn:aws:resource-groups:<region>:<account>:group/<name>`,
//!   with the region taken from the caller's SigV4 credential scope;
//! * a group name identifies at most one group IN ONE REGION, and an identifier
//!   that names no group of the region the request addressed is `NotFoundException`
//!   (404) on every operation that addresses one (the per-region isolation rule
//!   itself is the sibling `region_isolation` suite);
//! * a request that violates a modelled rule is `BadRequestException` (400);
//! * a listing is computed from the store, ordered, and pages through every item
//!   exactly once whatever the page size;
//! * a membership's `ResourceType` is derived from its ARN, and is absent when the
//!   ARN names a type this emulator has no name for.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::EmulatedService;

/// The emulator's canonical account id, which every ARN it mints carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// One emulator: a shared engine, one registered service, and the router every
/// request is dispatched through.
///
/// Shared with the sibling suites of this test target (see `tests/all.rs`), so
/// every suite drives the one restJson1 path rather than building its own.
pub(crate) struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    /// The one engine every registered operation was constructed from, kept so a
    /// suite can inspect the store the requests wrote, or move a due instant the
    /// store holds instead of sleeping for the span it names.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // These tests sign nothing, so request authentication is off; the
        // `authorization` header below is only how a request names its service
        // and region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The shared engine this emulator's service was built from.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Dispatch `POST <path>` with `body` as the restJson1 payload, signed (in
    /// scope only) for `region`. Returns the status and the parsed body.
    pub(crate) async fn call_in(&self, region: &str, path: &str, body: &Value) -> (u16, Value) {
        self.request_in(region, "POST", path, Some(body)).await
    }

    /// Dispatch `<method> <path>`, signed (in scope only) for `region`, with
    /// `body` as the restJson1 payload when the operation takes one and no body
    /// at all when it does not. The operations whose members are bound to the URI
    /// are reached this way, since the model gives them their own method and path.
    /// Returns the status and the parsed body.
    pub(crate) async fn request_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let (status, parsed, _) = self.dispatch_in(region, method, path, body).await;
        (status, parsed)
    }

    /// `request_in`, also answering the `x-amzn-errortype` header — the header restJson1
    /// resolves an error SHAPE from, which a client reads before the body.
    pub(crate) async fn dispatch_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Value, Option<String>) {
        let payload = match body {
            Some(value) => serde_json::to_vec(value).expect("serializable body"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20240101/{region}/resource-groups/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|e| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON ({e}): {:?}",
                    String::from_utf8_lossy(response.body()),
                )
            })
        };
        (status, parsed, error_type)
    }

    /// `POST <path>` in `us-east-1`, answering the status, the body and the
    /// `x-amzn-errortype` header.
    pub(crate) async fn call_reporting_error_type(
        &self,
        path: &str,
        body: &Value,
    ) -> (u16, Value, Option<String>) {
        self.dispatch_in("us-east-1", "POST", path, Some(body)).await
    }

    /// `call_in` in `us-east-1`.
    pub(crate) async fn call(&self, path: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", path, body).await
    }

    /// Call `path` signed (in scope only) for `region`, requiring a 200, and return
    /// the body.
    pub(crate) async fn ok_in(&self, region: &str, path: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in(region, path, body).await;
        assert_eq!(
            status, 200,
            "POST {path} {body} in {region}: expected 200, got {status} {parsed}",
        );
        parsed
    }

    /// Call `path`, requiring a 200, and return the body.
    pub(crate) async fn ok(&self, path: &str, body: &Value) -> Value {
        self.ok_in("us-east-1", path, body).await
    }
}

/// A `CreateGroup` request for `name` whose service configuration lets the group
/// hold static memberships.
///
/// `GroupResources` can only be used with a group configured as one of the three
/// types its documentation names (`smithy_query(query="doc",shape="GroupResources")`),
/// so every suite that adds a static membership creates its group this way; the
/// whole catalog, and the refusal for a group outside it, is the sibling
/// `group_resources_supported_configuration` suite.
pub(crate) fn static_group(name: &str) -> Value {
    json!({ "Name": name, "Configuration": [{ "Type": "AWS::EC2::HostManagement" }] })
}

/// A `CreateGroup` request for `name` whose service configuration makes the group
/// an APPLICATION group.
///
/// `AWS::ResourceGroups::ApplicationGroup` is the configuration type that declares
/// one (`smithy_query(query="doc",shape="Group",member="ApplicationTag")`), and it
/// is also one of the three types `GroupResources` documents, so a group created
/// this way both holds static memberships and is the target an operation documented
/// to act on the application group serves. Shared with every suite that drives such
/// an operation.
pub(crate) fn application_group(name: &str) -> Value {
    json!({
        "Name": name,
        "Configuration": [{ "Type": "AWS::ResourceGroups::ApplicationGroup" }]
    })
}

/// The error code (`__type`) a failure response carries.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("<none>")
}

/// Percent-encode the three characters a base64 `NextToken` can carry that a
/// query string reads differently (`+` is a space to a query reader).
///
/// Shared with the sibling suites of this test target, since `MaxResults` and
/// `NextToken` are bound to the query string on every listing of this service.
pub(crate) fn query_encode(token: &str) -> String {
    token
        .replace('%', "%25")
        .replace('+', "%2B")
        .replace('/', "%2F")
        .replace('=', "%3D")
}

/// Whether `token` matches the modelled `NextToken` pattern `^[a-zA-Z0-9+/]*={0,2}$`.
pub(crate) fn is_modelled_token(token: &str) -> bool {
    let body = token.trim_end_matches('=');
    let padding = token.len() - body.len();
    padding <= 2
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// The `GroupIdentifiers` names of a `ListGroups` page.
pub(crate) fn identifier_names(page: &Value) -> Vec<String> {
    page["GroupIdentifiers"]
        .as_array()
        .expect("GroupIdentifiers is a list")
        .iter()
        .map(|id| id["GroupName"].as_str().expect("GroupName is a string").to_string())
        .collect()
}

/// One row of the create/read round-trip table: a whole `CreateGroup` request.
struct GroupCase {
    label: &'static str,
    name: &'static str,
    request: Value,
}

fn round_trip_cases() -> Vec<GroupCase> {
    vec![
        GroupCase {
            label: "only the required Name",
            name: "minimal-group",
            request: json!({ "Name": "minimal-group" }),
        },
        GroupCase {
            label: "every optional member, tag-filter query, most critical rank",
            name: "full.group_1",
            request: json!({
                "Name": "full.group_1",
                "Description": "demo group with every member set",
                "Criticality": 1,
                "Owner": "owner@example.com",
                "DisplayName": "Full Group",
                "Tags": { "Stage": "Test", "Team": "Platform" },
                "ResourceQuery": {
                    "Type": "TAG_FILTERS_1_0",
                    "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
                }
            }),
        },
        GroupCase {
            label: "cloudformation-stack query, least critical rank, no tags",
            name: "stack-group",
            request: json!({
                "Name": "stack-group",
                "Criticality": 10,
                "ResourceQuery": {
                    "Type": "CLOUDFORMATION_STACK_1_0",
                    "Query": "{\"StackIdentifier\":\"arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0\"}"
                }
            }),
        },
        GroupCase {
            label: "empty tag map and an empty description",
            name: "edge-case-group",
            request: json!({
                "Name": "edge-case-group",
                "Description": "",
                "Tags": {}
            }),
        },
        GroupCase {
            label: "name at the one-character minimum",
            name: "g",
            request: json!({ "Name": "g", "Owner": "sre", "DisplayName": "G" }),
        },
    ]
}

/// A create is persisted and every read returns what was created — by name, by
/// the deprecated `GroupName`, and by ARN — until a delete removes it.
#[tokio::test]
async fn a_created_group_round_trips_through_every_reader_then_disappears_on_delete() {
    for case in round_trip_cases() {
        let label = case.label;
        let name = case.name;
        let emulator = Emulator::new();

        let created = emulator.ok("/groups", &case.request).await;
        let group = created["Group"].clone();
        let expected_arn = format!("arn:aws:resource-groups:us-east-1:{ACCOUNT}:group/{name}");
        assert_eq!(
            group["GroupArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {label:?}: expected GroupArn {expected_arn}, got {:?}",
            group["GroupArn"],
        );
        assert_eq!(
            group["Name"].as_str(),
            Some(name),
            "case {label:?}: expected Name {name}, got {:?}",
            group["Name"],
        );
        for member in ["Description", "Criticality", "Owner", "DisplayName"] {
            let sent = case.request.get(member).cloned().unwrap_or(Value::Null);
            let answered = group.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                sent, answered,
                "case {label:?}: expected Group.{member} {sent}, got {answered}",
            );
        }
        let sent_query = case.request.get("ResourceQuery").cloned().unwrap_or(Value::Null);
        let answered_query = created.get("ResourceQuery").cloned().unwrap_or(Value::Null);
        assert_eq!(
            sent_query, answered_query,
            "case {label:?}: expected ResourceQuery {sent_query}, got {answered_query}",
        );
        let sent_tags = case
            .request
            .get("Tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let answered_tags = created.get("Tags").cloned().unwrap_or(Value::Null);
        assert_eq!(
            sent_tags, answered_tags,
            "case {label:?}: expected Tags {sent_tags}, got {answered_tags}",
        );

        // Every identifier form resolves to the one stored record.
        for identifier in [
            json!({ "Group": name }),
            json!({ "GroupName": name }),
            json!({ "Group": expected_arn }),
            // `Group` wins over the deprecated `GroupName` when both are sent.
            json!({ "Group": name, "GroupName": name }),
        ] {
            let read = emulator.ok("/get-group", &identifier).await;
            assert_eq!(
                read["Group"], group,
                "case {label:?}: GetGroup {identifier} returned {}, expected {group}",
                read["Group"],
            );
        }

        // The listing is computed from the store, so it carries this group.
        let listed = emulator.ok("/groups-list", &json!({})).await;
        assert_eq!(
            identifier_names(&listed),
            vec![name.to_string()],
            "case {label:?}: expected ListGroups to report exactly [{name}], got {listed}",
        );
        let identifier = &listed["GroupIdentifiers"][0];
        for (in_identifier, in_group) in [
            ("GroupName", "Name"),
            ("GroupArn", "GroupArn"),
            ("Description", "Description"),
            ("Criticality", "Criticality"),
            ("Owner", "Owner"),
            ("DisplayName", "DisplayName"),
        ] {
            let listed_value = identifier.get(in_identifier).cloned().unwrap_or(Value::Null);
            let group_value = group.get(in_group).cloned().unwrap_or(Value::Null);
            assert_eq!(
                listed_value, group_value,
                "case {label:?}: expected GroupIdentifier.{in_identifier} {group_value}, \
                 got {listed_value}",
            );
        }
        assert_eq!(
            listed["Groups"][0], group,
            "case {label:?}: the deprecated Groups list must report the same group, got {}",
            listed["Groups"][0],
        );

        // Deleting reports the group as it stood, and removes it from every read.
        let deleted = emulator.ok("/delete-group", &json!({ "Group": name })).await;
        assert_eq!(
            deleted["Group"], group,
            "case {label:?}: expected DeleteGroup to return {group}, got {}",
            deleted["Group"],
        );
        let (status, body) = emulator.call("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "NotFoundException"),
            "case {label:?}: expected a deleted group to be 404 NotFoundException, \
             got {status} {body}",
        );
        let listed = emulator.ok("/groups-list", &json!({})).await;
        assert!(
            identifier_names(&listed).is_empty(),
            "case {label:?}: expected an empty listing after the delete, got {listed}",
        );
    }
}

/// A group's ARN carries the region of the caller's SigV4 credential scope, and
/// the emulator's canonical account id. A scope region that is not shaped like an
/// AWS region (`[a-z]{2}(-[a-z]+)+-\d{1}`, which the modelled `GroupArn` pattern
/// requires) falls back to the emulator default.
#[tokio::test]
async fn a_group_arn_carries_the_scope_region_and_the_emulator_account() {
    // (credential-scope region, the region the ARN must carry)
    let cases = [
        ("us-east-1", "us-east-1"),
        ("eu-west-1", "eu-west-1"),
        ("ap-southeast-2", "ap-southeast-2"),
        ("us-gov-east-1", "us-gov-east-1"),
        // Not a region shape, so the ARN keeps the emulator default.
        ("customer-secret-9", "us-east-1"),
        ("nonsense", "us-east-1"),
    ];
    for (scope_region, expected_region) in cases {
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call_in(scope_region, "/groups", &json!({ "Name": "arn-group" }))
            .await;
        assert_eq!(
            status, 200,
            "case {scope_region:?}: expected 200, got {status} {created}",
        );
        let expected =
            format!("arn:aws:resource-groups:{expected_region}:{ACCOUNT}:group/arn-group");
        assert_eq!(
            created["Group"]["GroupArn"].as_str(),
            Some(expected.as_str()),
            "case {scope_region:?}: expected {expected}, got {:?}",
            created["Group"]["GroupArn"],
        );
        // The ARN is stored, not recomputed: a read in the region the group lives
        // in returns it unchanged.
        let (status, read) = emulator
            .call_in(scope_region, "/get-group", &json!({ "Group": "arn-group" }))
            .await;
        assert_eq!(
            (status, read["Group"]["GroupArn"].as_str()),
            (200, Some(expected.as_str())),
            "case {scope_region:?}: a later read must return the stored ARN, got {status} {read}",
        );
    }
}

/// A group name is unique within one region of the account, and a name starting
/// with any capitalization of the reserved `aws` prefix is refused. (That the same
/// name in ANOTHER region is a different group is the sibling `region_isolation`
/// suite's rule.)
#[tokio::test]
async fn names_are_unique_and_the_reserved_prefix_is_refused() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "taken" })).await;

    // (label, name, expected status)
    let cases = [
        ("a second create for a stored name", "taken", 400),
        ("the reserved prefix, lowercase", "aws-group", 400),
        ("the reserved prefix, uppercase", "AWS-group", 400),
        ("the reserved prefix, mixed case", "Aws-group", 400),
        ("exactly the reserved prefix", "aws", 400),
        // The rule is on the prefix `aws`, not on any name starting with `aw`.
        ("a name that merely starts with aw", "awesome-group", 200),
        ("a name containing aws later on", "my-aws-group", 200),
    ];
    for (label, name, expected) in cases {
        let (status, body) = emulator.call("/groups", &json!({ "Name": name })).await;
        assert_eq!(
            status, expected,
            "case {label:?} ({name}): expected {expected}, got {status} {body}",
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {label:?} ({name}): expected BadRequestException, got {body}",
            );
        }
    }
    // The first group was not overwritten by the refused duplicate.
    let read = emulator.ok("/get-group", &json!({ "Group": "taken" })).await;
    assert_eq!(
        read["Group"]["Name"].as_str(),
        Some("taken"),
        "the refused duplicate must leave the stored group intact, got {read}",
    );
}

/// Every operation that addresses one group answers `NotFoundException` (404) for
/// an identifier that names no stored group — including an ARN whose name exists
/// locally but whose account or region does not match the stored ARN.
#[tokio::test]
async fn an_identifier_that_names_no_stored_group_is_not_found() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "present" })).await;
    let member = json!(["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"]);

    // (label, identifier) — each is substituted into every reader below.
    let identifiers = [
        ("a name nothing answers to", json!("absent")),
        (
            "an ARN for a group that was never created",
            json!(format!("arn:aws:resource-groups:us-east-1:{ACCOUNT}:group/absent")),
        ),
        (
            "an ARN naming a stored group in another account",
            json!("arn:aws:resource-groups:us-east-1:111111111111:group/present"),
        ),
        (
            "an ARN naming a stored group in another region",
            json!(format!("arn:aws:resource-groups:eu-west-1:{ACCOUNT}:group/present")),
        ),
        (
            "an ARN naming a stored group under another service",
            json!(format!("arn:aws:ec2:us-east-1:{ACCOUNT}:group/present")),
        ),
        (
            "a malformed identifier that merely contains the group segment",
            json!("anything:group/present"),
        ),
        (
            "an ARN-looking identifier whose account is not twelve digits",
            json!("arn:aws:resource-groups:us-east-1:00000000000:group/present"),
        ),
    ];
    for (label, identifier) in identifiers {
        let calls = [
            ("/get-group", json!({ "Group": identifier })),
            ("/get-group-configuration", json!({ "Group": identifier })),
            ("/delete-group", json!({ "Group": identifier })),
            (
                "/update-group",
                json!({ "Group": identifier, "Description": "Nope" }),
            ),
            (
                "/group-resources",
                json!({ "Group": identifier, "ResourceArns": member }),
            ),
            ("/list-group-resources", json!({ "Group": identifier })),
        ];
        for (path, body) in calls {
            let (status, answered) = emulator.call(path, &body).await;
            assert_eq!(
                (status, error_code(&answered)),
                (404, "NotFoundException"),
                "case {label:?} on {path}: expected 404 NotFoundException, \
                 got {status} {answered}",
            );
            // Every one of them answers the service's own fixed sentence, which
            // repeats nothing the caller sent.
            assert_eq!(
                answered["Message"].as_str(),
                Some("The specified group does not exist"),
                "case {label:?} on {path}: expected the fixed not-found message, \
                 got {answered}",
            );
        }
    }
    // The stored group is still there: a refused read changed nothing.
    let listed = emulator.ok("/groups-list", &json!({})).await;
    assert_eq!(
        identifier_names(&listed),
        vec!["present".to_string()],
        "a not-found read must not change the store, got {listed}",
    );
}

/// An operation that addresses one group refuses a request that names none.
#[tokio::test]
async fn a_request_that_names_no_group_is_a_bad_request() {
    let emulator = Emulator::new();
    for path in [
        "/get-group",
        "/get-group-configuration",
        "/delete-group",
        "/update-group",
        "/list-group-resources",
    ] {
        let (status, body) = emulator.call(path, &json!({})).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {path:?}: expected 400 BadRequestException, got {status} {body}",
        );
    }
}

/// A value outside a modelled rule — a required member absent, a length, a
/// pattern, a range — is refused as `BadRequestException` (400), the error the
/// model documents for "parameters that violate validation rules".
#[tokio::test]
async fn modelled_constraint_violations_are_refused_as_bad_request() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "constraints" })).await;
    let member = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";
    let eleven: Vec<String> = (0..11).map(|i| format!("{member}{i}")).collect();
    let too_long = "n".repeat(301);

    // (label, path, body)
    let cases = vec![
        (
            "CreateGroup without the required Name",
            "/groups",
            json!({ "Description": "no name" }),
        ),
        (
            "a Name outside the modelled pattern",
            "/groups",
            json!({ "Name": "not a valid name" }),
        ),
        (
            "a Name past the modelled length",
            "/groups",
            json!({ "Name": too_long }),
        ),
        (
            "a Description outside the modelled pattern",
            "/groups",
            json!({ "Name": "bad-description", "Description": "no commas, please" }),
        ),
        (
            "Criticality below the modelled range",
            "/groups",
            json!({ "Name": "low-criticality", "Criticality": 0 }),
        ),
        (
            "Criticality above the modelled range",
            "/groups",
            json!({ "Name": "high-criticality", "Criticality": 11 }),
        ),
        (
            "a Name whose third byte is inside a multi-byte character",
            "/groups",
            json!({ "Name": "ab\u{2603}group" }),
        ),
        (
            "a ResourceQuery Type the model does not declare",
            "/groups",
            json!({
                "Name": "bad-query",
                "ResourceQuery": { "Type": "TAG_FILTERS_9_9", "Query": "{}" }
            }),
        ),
        (
            "a ResourceQuery Query that is not JSON",
            "/groups",
            json!({
                "Name": "unparseable-query",
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "not json at all" }
            }),
        ),
        (
            "a ResourceQuery Query that is JSON but not an object",
            "/groups",
            json!({
                "Name": "array-query",
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "[]" }
            }),
        ),
        (
            "GroupResources without the required Group",
            "/group-resources",
            json!({ "ResourceArns": [member] }),
        ),
        (
            "GroupResources without the required ResourceArns",
            "/group-resources",
            json!({ "Group": "constraints" }),
        ),
        (
            "ResourceArns below the modelled length",
            "/group-resources",
            json!({ "Group": "constraints", "ResourceArns": [] }),
        ),
        (
            "ResourceArns past the modelled length",
            "/group-resources",
            json!({ "Group": "constraints", "ResourceArns": eleven }),
        ),
        (
            "a ResourceArns entry that is not an ARN",
            "/group-resources",
            json!({ "Group": "constraints", "ResourceArns": ["i-0123456789abcdef0"] }),
        ),
        (
            "a ResourceArns entry whose partition and service are empty",
            "/group-resources",
            json!({ "Group": "constraints", "ResourceArns": ["arn:::ec2::instance/i-0abc"] }),
        ),
        (
            "a ResourceArns entry whose account is not twelve digits",
            "/group-resources",
            json!({
                "Group": "constraints",
                "ResourceArns": ["arn:aws:ec2:us-east-1:12345:instance/i-0abc"]
            }),
        ),
        (
            "a ResourceArns entry whose region is not a region",
            "/group-resources",
            json!({
                "Group": "constraints",
                "ResourceArns": ["arn:aws:ec2:nowhere:000000000000:instance/i-0abc"]
            }),
        ),
        (
            "a NextToken this service never issued",
            "/groups-list",
            json!({ "NextToken": "bm90LWEtdG9rZW4h" }),
        ),
        (
            "a ListGroupResources NextToken this service never issued",
            "/list-group-resources",
            json!({ "Group": "constraints", "NextToken": "bm90LWEtdG9rZW4h" }),
        ),
        (
            "a ListGroups filter name the model does not declare",
            "/groups-list",
            json!({ "Filters": [{ "Name": "nickname", "Values": ["x"] }] }),
        ),
        (
            "a ListGroupResources filter name the model does not declare",
            "/list-group-resources",
            json!({
                "Group": "constraints",
                "Filters": [{ "Name": "nickname", "Values": ["x"] }]
            }),
        ),
        (
            "a group with both a ResourceQuery and a Configuration",
            "/groups",
            json!({
                "Name": "both-group",
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{}" },
                "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
            }),
        ),
    ];
    for (label, path, body) in cases {
        let (status, answered) = emulator.call(path, &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {answered}",
        );
    }

    // `MaxResults` is bound to the query string on ListGroups, so its range is
    // refused there too.
    for max_results in ["0", "51", "-1"] {
        let path = format!("/groups-list?maxResults={max_results}");
        let (status, answered) = emulator.call(&path, &json!({})).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case maxResults={max_results:?}: expected 400 BadRequestException, \
             got {status} {answered}",
        );
    }
    // Nothing above created a group.
    let listed = emulator.ok("/groups-list", &json!({})).await;
    assert_eq!(
        identifier_names(&listed),
        vec!["constraints".to_string()],
        "a refused request must create nothing, got {listed}",
    );
}

/// `GroupResources` records each ARN as a member, and `ListGroupResources`
/// reports exactly the recorded memberships with the resource type derived from
/// each ARN. An ARN whose type this emulator has no name for is reported without
/// one rather than with a guess.
#[tokio::test]
async fn membership_reports_the_recorded_arns_with_the_resource_type_derived_from_each() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &static_group("members")).await;

    // (ARN, the CloudFormation-style type name it names, if any)
    let cases: Vec<(&str, Option<&str>)> = vec![
        (
            "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
            Some("AWS::EC2::Instance"),
        ),
        (
            "arn:aws:ec2:us-east-1:000000000000:volume/vol-0123456789abcdef0",
            Some("AWS::EC2::Volume"),
        ),
        (
            "arn:aws:lambda:eu-west-1:000000000000:function:report",
            Some("AWS::Lambda::Function"),
        ),
        (
            "arn:aws:dynamodb:us-east-1:000000000000:table/Orders",
            Some("AWS::DynamoDB::Table"),
        ),
        ("arn:aws:s3:::demo-bucket", Some("AWS::S3::Bucket")),
        (
            "arn:aws:sqs:us-east-1:000000000000:demo-queue",
            Some("AWS::SQS::Queue"),
        ),
        (
            "arn:aws:logs:us-east-1:000000000000:log-group:/demo",
            Some("AWS::Logs::LogGroup"),
        ),
        (
            "arn:aws:states:us-east-1:000000000000:stateMachine:demo",
            Some("AWS::StepFunctions::StateMachine"),
        ),
        // No name for these: an object inside a bucket, and a service this
        // emulator holds no type names for.
        ("arn:aws:s3:::demo-bucket/key.txt", None),
        (
            "arn:aws:madeupservice:us-east-1:000000000000:widget/w-1",
            None,
        ),
    ];

    // `ResourceArns` is modelled 1..10, so the ARNs go in two calls — which also
    // shows a second call ADDS to the membership rather than replacing it.
    for chunk in cases.chunks(5) {
        let arns: Vec<&str> = chunk.iter().map(|(arn, _)| *arn).collect();
        let grouped = emulator
            .ok(
                "/group-resources",
                &json!({ "Group": "members", "ResourceArns": arns }),
            )
            .await;
        let mut succeeded: Vec<String> = grouped["Succeeded"]
            .as_array()
            .expect("Succeeded is a list")
            .iter()
            .map(|arn| arn.as_str().expect("an ARN string").to_string())
            .collect();
        succeeded.sort();
        let mut expected: Vec<String> = arns.iter().map(|arn| (*arn).to_string()).collect();
        expected.sort();
        assert_eq!(
            succeeded, expected,
            "case {arns:?}: expected every ARN in Succeeded, got {grouped}",
        );
        assert_eq!(
            grouped["Failed"].as_array().map(Vec::len),
            Some(0),
            "case {arns:?}: expected no Failed entries, got {grouped}",
        );
        assert_eq!(
            grouped["Pending"].as_array().map(Vec::len),
            Some(0),
            "case {arns:?}: expected no Pending entries, got {grouped}",
        );
    }

    let listed = emulator
        .ok("/list-group-resources", &json!({ "Group": "members" }))
        .await;
    let resources = listed["Resources"].as_array().expect("Resources is a list");
    assert_eq!(
        resources.len(),
        cases.len(),
        "expected {} memberships, got {listed}",
        cases.len(),
    );
    for (arn, expected_type) in &cases {
        let found = resources
            .iter()
            .find(|item| item["Identifier"]["ResourceArn"].as_str() == Some(arn))
            .unwrap_or_else(|| panic!("case {arn:?}: not reported at all in {listed}"));
        let answered = found["Identifier"]["ResourceType"].as_str();
        assert_eq!(
            answered, *expected_type,
            "case {arn:?}: expected ResourceType {expected_type:?}, got {answered:?}",
        );
        // A membership this emulator reports as added carries no status: the
        // modelled `PENDING` means the addition is not finished yet.
        assert!(
            found.get("Status").is_none_or(Value::is_null),
            "case {arn:?}: expected no Status on a completed membership, got {found}",
        );
    }
    // The deprecated `ResourceIdentifiers` reports the same memberships.
    let identifiers = listed["ResourceIdentifiers"]
        .as_array()
        .expect("ResourceIdentifiers is a list");
    let from_items: Vec<Value> = resources
        .iter()
        .map(|item| item["Identifier"].clone())
        .collect();
    assert_eq!(
        identifiers, &from_items,
        "expected ResourceIdentifiers to mirror Resources, got {listed}",
    );

    // Re-adding a recorded ARN is idempotent.
    let repeat = cases[0].0;
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "members", "ResourceArns": [repeat] }),
        )
        .await;
    let listed = emulator
        .ok("/list-group-resources", &json!({ "Group": "members" }))
        .await;
    assert_eq!(
        listed["Resources"].as_array().map(Vec::len),
        Some(cases.len()),
        "re-adding {repeat} must not duplicate the membership, got {listed}",
    );

    // Deleting the group takes its memberships with it.
    emulator
        .ok("/delete-group", &json!({ "Group": "members" }))
        .await;
    emulator.ok("/groups", &static_group("members")).await;
    let listed = emulator
        .ok("/list-group-resources", &json!({ "Group": "members" }))
        .await;
    assert_eq!(
        listed["Resources"].as_array().map(Vec::len),
        Some(0),
        "a recreated group starts with no memberships, got {listed}",
    );
}

/// A `resource-type` filter selects the memberships whose derived type it names,
/// and nothing else.
#[tokio::test]
async fn a_resource_type_filter_selects_only_the_memberships_of_that_type() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &static_group("filtered")).await;
    let instance_a = "arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000a";
    let instance_b = "arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000b";
    let volume = "arn:aws:ec2:us-east-1:000000000000:volume/vol-000000000000000a";
    let table = "arn:aws:dynamodb:us-east-1:000000000000:table/Orders";
    emulator
        .ok(
            "/group-resources",
            &json!({
                "Group": "filtered",
                "ResourceArns": [instance_a, instance_b, volume, table]
            }),
        )
        .await;

    // (filter values, the ARNs the filter must select)
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (vec!["AWS::EC2::Instance"], vec![instance_a, instance_b]),
        (vec!["AWS::EC2::Volume"], vec![volume]),
        (
            vec!["AWS::EC2::Volume", "AWS::DynamoDB::Table"],
            vec![table, volume],
        ),
        (vec!["AWS::EC2::Subnet"], vec![]),
    ];
    for (values, expected) in cases {
        let listed = emulator
            .ok(
                "/list-group-resources",
                &json!({
                    "Group": "filtered",
                    "Filters": [{ "Name": "resource-type", "Values": values }]
                }),
            )
            .await;
        let mut answered: Vec<String> = listed["ResourceIdentifiers"]
            .as_array()
            .expect("ResourceIdentifiers is a list")
            .iter()
            .map(|id| id["ResourceArn"].as_str().expect("an ARN").to_string())
            .collect();
        answered.sort();
        let mut want: Vec<String> = expected.iter().map(|arn| (*arn).to_string()).collect();
        want.sort();
        assert_eq!(
            answered, want,
            "case {values:?}: expected {want:?}, got {listed}",
        );
    }
}

/// A `ListGroups` filter selects the groups whose named attribute carries one of
/// its values; a name the model does not declare is refused rather than silently
/// ignored. (The set-valued `configuration-type` and `resource-type` names are the
/// sibling `list_groups_filters` suite.)
#[tokio::test]
async fn a_list_groups_filter_selects_the_groups_whose_attribute_it_names() {
    let emulator = Emulator::new();
    for (name, criticality, owner, display) in [
        ("alpha", 1, "sre@example.com", "Alpha"),
        ("beta", 5, "sre@example.com", "Beta"),
        ("gamma", 5, "dba@example.com", "Gamma"),
    ] {
        emulator
            .ok(
                "/groups",
                &json!({
                    "Name": name,
                    "Criticality": criticality,
                    "Owner": owner,
                    "DisplayName": display
                }),
            )
            .await;
    }
    emulator.ok("/groups", &json!({ "Name": "plain" })).await;

    // (filter name, values, the groups the filter must select)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("criticality", vec!["5"], vec!["beta", "gamma"]),
        ("criticality", vec!["1", "5"], vec!["alpha", "beta", "gamma"]),
        ("criticality", vec!["7"], vec![]),
        ("owner", vec!["dba@example.com"], vec!["gamma"]),
        ("display-name", vec!["Alpha"], vec!["alpha"]),
        ("display-name", vec!["alpha"], vec![]),
    ];
    for (filter_name, values, expected) in cases {
        let listed = emulator
            .ok(
                "/groups-list",
                &json!({ "Filters": [{ "Name": filter_name, "Values": values }] }),
            )
            .await;
        let want: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            identifier_names(&listed),
            want,
            "case {filter_name}={values:?}: expected {want:?}, got {listed}",
        );
    }

    // A name the modelled `GroupFilterName` does not declare is refused, not
    // ignored: it names an attribute no group has.
    for filter_name in ["group-name", "Configuration-Type"] {
        let (status, body) = emulator
            .call(
                "/groups-list",
                &json!({ "Filters": [{ "Name": filter_name, "Values": ["x"] }] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {filter_name:?}: expected 400 BadRequestException for an undeclared \
             filter name, got {status} {body}",
        );
    }
}

/// A listing pages through every item exactly once, in order, whatever the page
/// size, and the token it hands back is the opaque form the model describes.
#[tokio::test]
async fn paging_walks_every_group_exactly_once_for_every_page_size() {
    let emulator = Emulator::new();
    let names: Vec<String> = (1..=7).map(|i| format!("group-{i}")).collect();
    for name in &names {
        emulator.ok("/groups", &json!({ "Name": name })).await;
    }

    for page_size in [1usize, 2, 3, 6, 7, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let path = match &token {
                Some(t) => format!(
                    "/groups-list?maxResults={page_size}&nextToken={}",
                    query_encode(t)
                ),
                None => format!("/groups-list?maxResults={page_size}"),
            };
            let page = emulator.ok(&path, &json!({})).await;
            let page_names = identifier_names(&page);
            assert!(
                page_names.len() <= page_size,
                "case page_size={page_size}: a page carried {} items, got {page}",
                page_names.len(),
            );
            seen.extend(page_names);
            token = page["NextToken"].as_str().map(str::to_string);
            if let Some(t) = &token {
                assert!(
                    is_modelled_token(t),
                    "case page_size={page_size}: NextToken {t:?} is not the modelled form",
                );
            }
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case page_size={page_size}: paging did not terminate, saw {seen:?}",
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case page_size={page_size}: expected every group once in name order, got {seen:?}",
        );
    }

    // A group deleted between two pages is gone from the rest of the walk, and
    // the token still resumes at the right place.
    let first = emulator.ok("/groups-list?maxResults=3", &json!({})).await;
    assert_eq!(
        identifier_names(&first),
        names[..3].to_vec(),
        "expected the first three groups, got {first}",
    );
    emulator
        .ok("/delete-group", &json!({ "Group": "group-5" }))
        .await;
    let token = first["NextToken"].as_str().expect("a NextToken").to_string();
    let rest = emulator
        .ok(
            &format!("/groups-list?nextToken={}", query_encode(&token)),
            &json!({}),
        )
        .await;
    assert_eq!(
        identifier_names(&rest),
        vec![
            "group-4".to_string(),
            "group-6".to_string(),
            "group-7".to_string()
        ],
        "expected the deleted group to be gone from the continuation, got {rest}",
    );
}

/// Membership listing pages the same way, over the group's members.
#[tokio::test]
async fn paging_walks_every_membership_exactly_once_for_every_page_size() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &static_group("paged")).await;
    let arns: Vec<String> = (1..=6)
        .map(|i| format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/i-00000000000000{i:02}"))
        .collect();
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "paged", "ResourceArns": arns }),
        )
        .await;

    for page_size in [1usize, 2, 4, 6, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "Group": "paged", "MaxResults": page_size });
            if let Some(t) = &token {
                body["NextToken"] = json!(t);
            }
            let page = emulator.ok("/list-group-resources", &body).await;
            let page_arns: Vec<String> = page["ResourceIdentifiers"]
                .as_array()
                .expect("ResourceIdentifiers is a list")
                .iter()
                .map(|id| id["ResourceArn"].as_str().expect("an ARN").to_string())
                .collect();
            assert!(
                page_arns.len() <= page_size,
                "case page_size={page_size}: a page carried {} items, got {page}",
                page_arns.len(),
            );
            seen.extend(page_arns);
            token = page["NextToken"].as_str().map(str::to_string);
            if let Some(t) = &token {
                assert!(
                    is_modelled_token(t),
                    "case page_size={page_size}: NextToken {t:?} is not the modelled form",
                );
            }
            pages += 1;
            assert!(
                pages <= arns.len() + 1,
                "case page_size={page_size}: paging did not terminate, saw {seen:?}",
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, arns,
            "case page_size={page_size}: expected every membership once in ARN order, \
             got {seen:?}",
        );
    }
}

/// A pagination token belongs to the listing that issued it: another listing
/// refuses it rather than resuming at an unrelated key.
#[tokio::test]
async fn a_pagination_token_is_only_accepted_by_the_listing_that_issued_it() {
    let emulator = Emulator::new();
    for name in ["token-a", "token-b"] {
        emulator.ok("/groups", &static_group(name)).await;
    }
    let arns = [
        format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/i-000000000000000a"),
        format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/i-000000000000000b"),
    ];
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "token-a", "ResourceArns": arns }),
        )
        .await;

    let groups_page = emulator.ok("/groups-list?maxResults=1", &json!({})).await;
    let groups_token = groups_page["NextToken"]
        .as_str()
        .expect("a ListGroups NextToken")
        .to_string();
    let members_page = emulator
        .ok(
            "/list-group-resources",
            &json!({ "Group": "token-a", "MaxResults": 1 }),
        )
        .await;
    let members_token = members_page["NextToken"]
        .as_str()
        .expect("a ListGroupResources NextToken")
        .to_string();

    // (label, path, body) — each carries the OTHER listing's token.
    let cases = [
        (
            "a ListGroupResources token on ListGroups",
            format!("/groups-list?nextToken={}", query_encode(&members_token)),
            json!({}),
        ),
        (
            "a ListGroups token on ListGroupResources",
            "/list-group-resources".to_string(),
            json!({ "Group": "token-a", "NextToken": groups_token }),
        ),
        (
            "a token issued for another group's membership listing",
            "/list-group-resources".to_string(),
            json!({ "Group": "token-b", "NextToken": members_token }),
        ),
    ];
    for (label, path, body) in cases {
        let (status, answered) = emulator.call(&path, &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {answered}",
        );
    }
}

/// A group `Configuration` is stored, and the configured group is an ordinary
/// group afterwards. The configuration path itself is covered over a table of
/// configurations in `tests/all/group_owner_configuration_and_update.rs`; this row
/// pins that a configured create is persisted rather than refused.
#[tokio::test]
async fn creating_a_group_with_a_service_configuration_stores_the_group() {
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call(
            "/groups",
            &json!({
                "Name": "configured",
                "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected 200 for a configured group, got {status} {body}",
    );
    let listed = emulator.ok("/groups-list", &json!({})).await;
    assert_eq!(
        identifier_names(&listed),
        vec!["configured".to_string()],
        "a stored configured group must appear in the listing, got {listed}",
    );
}

/// With `MaxResults` absent, a listing answers at the modelled maximum page size
/// (50) and hands back a `NextToken`, rather than returning the whole store in one
/// unbounded response.
#[tokio::test]
async fn an_absent_max_results_pages_at_the_modelled_maximum() {
    let emulator = Emulator::new();
    // 52 groups: two more than one default page holds.
    let mut names: Vec<String> = (1..=52).map(|i| format!("default-{i:03}")).collect();
    names.sort();
    for name in &names {
        emulator.ok("/groups", &json!({ "Name": name })).await;
    }

    let first = emulator.ok("/groups-list", &json!({})).await;
    assert_eq!(
        identifier_names(&first).len(),
        50,
        "case ListGroups: expected a 50-item default page, got {} items",
        identifier_names(&first).len(),
    );
    assert_eq!(
        identifier_names(&first),
        names[..50].to_vec(),
        "case ListGroups: expected the first 50 groups in name order, got {first}",
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case ListGroups: expected a NextToken, got {first}"))
        .to_string();
    let second = emulator
        .ok(
            &format!("/groups-list?nextToken={}", query_encode(&token)),
            &json!({}),
        )
        .await;
    assert_eq!(
        identifier_names(&second),
        names[50..].to_vec(),
        "case ListGroups: expected the remaining two groups, got {second}",
    );
    assert!(
        second["NextToken"].is_null(),
        "case ListGroups: the last page carries no NextToken, got {second}",
    );

    // The same default applies to a membership listing: 60 members, added ten at a
    // time because `ResourceArns` is modelled 1..10.
    emulator.ok("/groups", &static_group("many-members")).await;
    let mut arns: Vec<String> = (1..=60)
        .map(|i| format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/i-{i:017}"))
        .collect();
    arns.sort();
    for chunk in arns.chunks(10) {
        emulator
            .ok(
                "/group-resources",
                &json!({ "Group": "many-members", "ResourceArns": chunk }),
            )
            .await;
    }
    let first = emulator
        .ok("/list-group-resources", &json!({ "Group": "many-members" }))
        .await;
    let page: Vec<String> = first["ResourceIdentifiers"]
        .as_array()
        .expect("ResourceIdentifiers is a list")
        .iter()
        .map(|id| id["ResourceArn"].as_str().expect("an ARN").to_string())
        .collect();
    assert_eq!(
        page,
        arns[..50].to_vec(),
        "case ListGroupResources: expected the first 50 memberships, got {first}",
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case ListGroupResources: expected a NextToken, got {first}"))
        .to_string();
    let second = emulator
        .ok(
            "/list-group-resources",
            &json!({ "Group": "many-members", "NextToken": token }),
        )
        .await;
    let rest: Vec<String> = second["ResourceIdentifiers"]
        .as_array()
        .expect("ResourceIdentifiers is a list")
        .iter()
        .map(|id| id["ResourceArn"].as_str().expect("an ARN").to_string())
        .collect();
    assert_eq!(
        rest,
        arns[50..].to_vec(),
        "case ListGroupResources: expected the remaining ten memberships, got {second}",
    );
    assert!(
        second["NextToken"].is_null(),
        "case ListGroupResources: the last page carries no NextToken, got {second}",
    );
}

/// Every state-changing operation emits one structured audit record naming the
/// operation, the caller's presented access key id, the region, the resource and
/// the outcome — and no request content. A reporting operation emits none.
#[tokio::test]
async fn state_changing_operations_emit_one_audit_record_each() {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    /// One captured event: its target and its fields as strings.
    #[derive(Clone, Debug)]
    struct CapturedEvent {
        target: String,
        fields: BTreeMap<String, String>,
    }

    impl CapturedEvent {
        /// A field's value with the quotes a `Debug`-recorded string carries
        /// removed: the handlers record request-derived values `Debug`-escaped so
        /// none can split a record.
        fn field(&self, name: &str) -> Option<&str> {
            self.fields
                .get(name)
                .map(|value| value.trim_matches('"'))
        }
    }

    /// Collects an event's fields into strings, whatever type they were recorded as.
    struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

    impl Visit for FieldCollector<'_> {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_i64(&mut self, field: &Field, value: i64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    /// A `tracing` subscriber that keeps every event it is handed, so the records
    /// are read as FIELDS rather than scraped out of formatted text. Scoped to this
    /// thread, so no global subscriber is installed.
    #[derive(Clone, Default)]
    struct Capture {
        events: Arc<Mutex<Vec<CapturedEvent>>>,
    }

    impl Capture {
        /// Every audit record captured for `operation` on `group`, oldest first. The
        /// group is part of the filter because this capture is installed once for
        /// the whole test binary, so records of the tests running beside this one
        /// land in it too.
        fn records_for(&self, operation: &str, group: &str) -> Vec<CapturedEvent> {
            self.audit_records()
                .into_iter()
                .filter(|event| {
                    event.field("operation") == Some(operation)
                        && event.field("group") == Some(group)
                })
                .collect()
        }

        /// Every audit record captured for `operation`, whatever the resource.
        fn records_of(&self, operation: &str) -> Vec<CapturedEvent> {
            self.audit_records()
                .into_iter()
                .filter(|event| event.field("operation") == Some(operation))
                .collect()
        }

        /// Every record on this service's audit target.
        fn audit_records(&self) -> Vec<CapturedEvent> {
            self.events
                .lock()
                .expect("the capture buffer")
                .iter()
                .filter(|event| {
                    event.target == service_com_amazonaws_resourcegroups_ardi::state::AUDIT_TARGET
                })
                .cloned()
                .collect()
        }
    }

    impl Subscriber for Capture {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            // This test asserts on events, not spans; one id for every span is enough.
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut fields = BTreeMap::new();
            event.record(&mut FieldCollector(&mut fields));
            self.events
                .lock()
                .expect("the capture buffer")
                .push(CapturedEvent {
                    target: event.metadata().target().to_string(),
                    fields,
                });
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    // Installed ONCE for the whole test binary, and global rather than
    // thread-local: tracing caches a callsite's interest the first time it is
    // reached, and a test running beside this one reaches the handlers' callsites
    // with no subscriber of its own, which would cache them as "never" and suppress
    // the records this test is about.
    static CAPTURE: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
    let capture = CAPTURE
        .get_or_init(|| {
            let capture = Capture::default();
            assert!(
                tracing::subscriber::set_global_default(capture.clone()).is_ok(),
                "a subscriber was already installed, so audit records cannot be captured",
            );
            tracing::callsite::rebuild_interest_cache();
            capture
        })
        .clone();

    let emulator = Emulator::new();
    let arn = format!("arn:aws:ec2:eu-west-1:{ACCOUNT}:instance/i-000000000000000a");
    // Each setup call must SUCCEED, or the assertions below would fail as "no record
    // emitted" when the real cause was a refused request.
    for (path, body) in [
        ("/groups", static_group("audited")),
        (
            "/group-resources",
            json!({ "Group": "audited", "ResourceArns": [arn] }),
        ),
        // A read emits no access record, so the negative assertion below cannot pass
        // on one.
        ("/get-group", json!({ "Group": "audited" })),
        ("/delete-group", json!({ "Group": "audited" })),
    ] {
        let (status, answered) = emulator.call_in("eu-west-1", path, &body).await;
        assert_eq!(
            status, 200,
            "setup {path}: expected 200 before asserting on audit records, got {status} {answered}",
        );
    }

    // The account-settings write is a state change too, and it addresses no group, so
    // its record is found by operation and region. The region is this test's own,
    // because the capture is installed for the whole test binary and the sibling
    // account-settings suite writes settings in regions of its own.
    let settings_region = "eu-north-1";
    for (path, body) in [
        (
            "/update-account-settings",
            json!({ "GroupLifecycleEventsDesiredStatus": "ACTIVE" }),
        ),
        // A read emits no access record here either.
        ("/get-account-settings", json!({})),
    ] {
        let (status, answered) = emulator.call_in(settings_region, path, &body).await;
        assert_eq!(
            status, 200,
            "setup {path}: expected 200 before asserting on audit records, got {status} {answered}",
        );
    }

    // (label, the operation that must have recorded, the outcome it must name)
    let cases = [
        ("the create", "CreateGroup", "created"),
        ("the membership add", "GroupResources", "grouped"),
        ("the delete", "DeleteGroup", "deleted"),
    ];
    for (label, operation, outcome) in cases {
        let matching = capture.records_for(operation, "audited");
        assert_eq!(
            matching.len(),
            1,
            "case {label:?}: expected exactly one audit record for {operation}, \
             captured {matching:?}",
        );
        let record = &matching[0];
        // who, where, what, and how it came out.
        for (field, expected) in [
            ("caller", "AKIDEXAMPLE"),
            ("region", "eu-west-1"),
            ("group", "audited"),
            ("outcome", outcome),
        ] {
            assert_eq!(
                record.field(field),
                Some(expected),
                "case {label:?}: expected {field}={expected:?}, got {record:?}",
            );
        }
    }
    // The account-settings write carries the same who/where/outcome fields.
    let settings_records: Vec<_> = capture
        .records_of("UpdateAccountSettings")
        .into_iter()
        .filter(|event| event.field("region") == Some(settings_region))
        .collect();
    assert_eq!(
        settings_records.len(),
        1,
        "case \"the account-settings write\": expected exactly one audit record for \
         UpdateAccountSettings in {settings_region}, captured {settings_records:?}",
    );
    for (field, expected) in [
        ("caller", "AKIDEXAMPLE"),
        ("region", settings_region),
        ("outcome", "requested"),
    ] {
        assert_eq!(
            settings_records[0].field(field),
            Some(expected),
            "case \"the account-settings write\": expected {field}={expected:?}, \
             got {:?}",
            settings_records[0],
        );
    }
    for reporting in [
        "GetGroup",
        "ListGroups",
        "ListGroupResources",
        "GetAccountSettings",
    ] {
        assert!(
            capture.records_of(reporting).is_empty(),
            "case {reporting:?}: a read is not a state change and must emit no access \
             record, captured {:?}",
            capture.records_of(reporting),
        );
    }
}

/// A membership ARN is accepted when its partition matches the production the
/// modelled `ResourceArn` names — `aws(-[a-z]+)*` — and refused otherwise.
#[tokio::test]
async fn the_arn_partition_production_decides_which_resource_arns_are_accepted() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &static_group("partitions")).await;

    // (label, ARN, the status the partition must produce)
    let cases = [
        (
            "the commercial partition",
            format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/i-000000000000000a"),
            200,
        ),
        (
            "a two-segment partition",
            format!("arn:aws-cn:ec2:cn-north-1:{ACCOUNT}:instance/i-000000000000000b"),
            200,
        ),
        (
            "a three-segment partition",
            format!("arn:aws-us-gov:ec2:us-gov-west-1:{ACCOUNT}:instance/i-000000000000000c"),
            200,
        ),
        (
            "a partition that merely starts with aws",
            format!("arn:awsfoo:ec2:us-east-1:{ACCOUNT}:instance/i-000000000000000d"),
            400,
        ),
        (
            "a partition with an empty segment",
            format!("arn:aws-:ec2:us-east-1:{ACCOUNT}:instance/i-000000000000000e"),
            400,
        ),
        (
            "a partition segment that is not lowercase",
            format!("arn:aws-US:ec2:us-east-1:{ACCOUNT}:instance/i-000000000000000f"),
            400,
        ),
        (
            "no partition at all",
            format!("arn::ec2:us-east-1:{ACCOUNT}:instance/i-0000000000000010"),
            400,
        ),
    ];
    for (label, arn, expected) in cases {
        let (status, body) = emulator
            .call(
                "/group-resources",
                &json!({ "Group": "partitions", "ResourceArns": [arn] }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {label:?} ({arn}): expected {expected}, got {status} {body}",
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {label:?} ({arn}): expected BadRequestException, got {body}",
            );
        } else {
            let listed = emulator
                .ok("/list-group-resources", &json!({ "Group": "partitions" }))
                .await;
            assert!(
                listed["ResourceIdentifiers"]
                    .as_array()
                    .expect("ResourceIdentifiers is a list")
                    .iter()
                    .any(|id| id["ResourceArn"].as_str() == Some(arn.as_str())),
                "case {label:?} ({arn}): expected the membership to be recorded, got {listed}",
            );
        }
    }
}

/// Two groups do not share membership: each record holds its own.
#[tokio::test]
async fn memberships_belong_to_the_group_they_were_added_to() {
    let emulator = Emulator::new();
    let one = "arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000a";
    let two = "arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000b";
    for (name, arn) in [("left", one), ("right", two)] {
        emulator.ok("/groups", &static_group(name)).await;
        emulator
            .ok(
                "/group-resources",
                &json!({ "Group": name, "ResourceArns": [arn] }),
            )
            .await;
    }
    for (name, expected) in [("left", one), ("right", two)] {
        let listed = emulator
            .ok("/list-group-resources", &json!({ "GroupName": name }))
            .await;
        let answered: Vec<&str> = listed["ResourceIdentifiers"]
            .as_array()
            .expect("ResourceIdentifiers is a list")
            .iter()
            .map(|id| id["ResourceArn"].as_str().expect("an ARN"))
            .collect();
        assert_eq!(
            answered,
            vec![expected],
            "case {name:?}: expected exactly [{expected}], got {listed}",
        );
    }
}

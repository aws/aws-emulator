// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of thing group MEMBERSHIP and of DYNAMIC thing groups:
//! `AddThingToThingGroup`, the `ListThingsInThingGroup` reader that proves what it
//! persisted, `CreateDynamicThingGroup` with the `DescribeThingGroup` reader, and
//! `DeleteDynamicThingGroup`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * a membership is a relation between records that EXIST, written by naming each
//!   end either by name or by ARN: a missing end is `ResourceNotFoundException` (404),
//!   a group whose membership is its query's is `InvalidRequestException` (400), and a
//!   repeat of the same add leaves the same one membership;
//! * a group's members are what the group's own reader reports, and a RECURSIVE
//!   listing is the union of the group's members and those of every group below it;
//! * a job targeting a thing group runs on the group's MEMBERS: each gets one
//!   execution, and a group the registry does not hold is a 404;
//! * a dynamic thing group IS its query: an unparsable `queryString` is
//!   `InvalidQueryException` (400) with nothing created, and a created one reports the
//!   query that decides its membership.
//!
//! Each table spans what the rule generalises over — which end is named by name and
//! which by ARN, how many members a group has, how deep the hierarchy goes, and the
//! form of a query string — rather than one recorded request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::UNPARSABLE_QUERY_MESSAGE;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region this suite signs for.
const REGION: &str = "us-east-2";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the thing-group request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.dispatch("POST", &format!("/things/{thing_name}"), Some(&json!({})))
            .await
    }

    async fn create_group(&self, group_name: &str, body: &Value) -> Reply {
        self.dispatch("POST", &format!("/thing-groups/{group_name}"), Some(body))
            .await
    }

    async fn create_dynamic_group(&self, group_name: &str, body: &Value) -> Reply {
        self.dispatch(
            "POST",
            &format!("/dynamic-thing-groups/{group_name}"),
            Some(body),
        )
        .await
    }

    async fn delete_dynamic_group(&self, group_name: &str) -> Reply {
        self.dispatch("DELETE", &format!("/dynamic-thing-groups/{group_name}"), None)
            .await
    }

    async fn describe_group(&self, group_name: &str) -> Reply {
        self.dispatch("GET", &format!("/thing-groups/{group_name}"), None)
            .await
    }

    async fn add_thing_to_group(&self, body: &Value) -> Reply {
        self.dispatch("PUT", "/thing-groups/addThingToThingGroup", Some(body))
            .await
    }

    async fn list_things_in_group(&self, group_name: &str, query: &str) -> Reply {
        self.dispatch(
            "GET",
            &format!("/thing-groups/{group_name}/things{query}"),
            None,
        )
        .await
    }

    async fn create_job(&self, job_id: &str, body: &Value) -> Reply {
        self.dispatch("PUT", &format!("/jobs/{job_id}"), Some(body))
            .await
    }

    async fn list_job_executions_for_thing(&self, thing_name: &str) -> Reply {
        self.dispatch("GET", &format!("/things/{thing_name}/jobs"), None)
            .await
    }

    /// The things a group reports, in the order the listing reported them.
    async fn things_in(&self, case: &str, group_name: &str, query: &str) -> Vec<String> {
        let reply = self.list_things_in_group(group_name, query).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected the listing to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.json()["things"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no things in {}", reply.text()))
            .iter()
            .map(|thing| thing.as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }
}

fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

fn thing_group_arn(group_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/{group_name}")
}

/// RULE: an add writes the membership, whichever way each end is named, and the
/// group's reader reports it.
#[tokio::test]
async fn an_added_thing_is_reported_by_the_groups_own_reader() {
    let emulator = Emulator::new();
    for thing_name in ["sensor-a", "sensor-b", "sensor-c"] {
        assert_eq!(
            emulator.create_thing(thing_name).await.status,
            200,
            "the thing a case adds must be registrable"
        );
    }
    assert_eq!(
        emulator.create_group("fleet", &json!({})).await.status,
        200,
        "the group a case adds to must be creatable"
    );

    // The rows span which end is named by NAME and which by ARN: either identifies the
    // resource, so all four combinations write the same one relation.
    let rows: Vec<(&str, Value)> = vec![
        (
            "both ends by name",
            json!({ "thingGroupName": "fleet", "thingName": "sensor-a" }),
        ),
        (
            "the group by ARN, the thing by name",
            json!({ "thingGroupArn": thing_group_arn("fleet"), "thingName": "sensor-b" }),
        ),
        (
            "the group by name, the thing by ARN",
            json!({ "thingGroupName": "fleet", "thingArn": thing_arn("sensor-c") }),
        ),
        (
            "a repeat of the first add",
            json!({ "thingGroupArn": thing_group_arn("fleet"), "thingArn": thing_arn("sensor-a") }),
        ),
    ];
    for (case, body) in &rows {
        let reply = emulator.add_thing_to_group(body).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    let members = emulator.things_in("every add", "fleet", "").await;
    assert_eq!(
        members,
        vec![
            "sensor-a".to_string(),
            "sensor-b".to_string(),
            "sensor-c".to_string()
        ],
        "case every add: expected each thing once, got {members:?}"
    );

    // The listing is paginated over that one order, so a page plus its continuation is
    // the whole membership and no row appears twice.
    let first_page = emulator.list_things_in_group("fleet", "?maxResults=2").await;
    let first = first_page.json();
    assert_eq!(
        first["things"],
        json!(["sensor-a", "sensor-b"]),
        "case a page of two: expected the first two members, got {first}"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case a page of two: expected a continuation, got {first}"));
    let second = emulator
        .things_in(
            "the continuation",
            "fleet",
            &format!("?maxResults=2&nextToken={token}"),
        )
        .await;
    assert_eq!(
        second,
        vec!["sensor-c".to_string()],
        "case the continuation: expected the rest of the membership, got {second:?}"
    );
}

/// RULE: a membership is only written between records that exist, and only for a group
/// whose membership is a caller's to write.
#[tokio::test]
async fn a_membership_between_records_that_do_not_exist_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_thing("sensor-a").await.status, 200);
    assert_eq!(
        emulator.create_group("fleet", &json!({})).await.status,
        200
    );
    assert_eq!(
        emulator
            .create_dynamic_group(
                "derived",
                &json!({ "queryString": "attributes.zone:north" })
            )
            .await
            .status,
        200,
        "the dynamic group a case names must be creatable"
    );

    let rows: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "a thing group the registry does not hold",
            json!({ "thingGroupName": "no-such-fleet", "thingName": "sensor-a" }),
            404,
            "ResourceNotFoundException",
            format!("ThingGroup {ACCOUNT_ID}:no-such-fleet not found."),
        ),
        (
            "a thing group ARN the registry does not hold",
            json!({ "thingGroupArn": thing_group_arn("no-such-fleet"), "thingName": "sensor-a" }),
            404,
            "ResourceNotFoundException",
            format!("ThingGroup {ACCOUNT_ID}:no-such-fleet not found."),
        ),
        (
            "a thing the registry does not hold",
            json!({ "thingGroupName": "fleet", "thingName": "no-such-sensor" }),
            404,
            "ResourceNotFoundException",
            format!("Thing {ACCOUNT_ID}:no-such-sensor not found."),
        ),
        (
            "a group whose membership is its query's",
            json!({ "thingGroupName": "derived", "thingName": "sensor-a" }),
            400,
            "InvalidRequestException",
            "Cannot add a thing to the dynamic thing group derived: a dynamic thing group's \
             membership is defined by its query"
                .to_string(),
        ),
        (
            "neither thingGroupName nor thingGroupArn",
            json!({ "thingName": "sensor-a" }),
            400,
            "InvalidRequestException",
            "either thingGroupName or thingGroupArn must name the thing group to add the thing to"
                .to_string(),
        ),
        (
            "neither thingName nor thingArn",
            json!({ "thingGroupName": "fleet" }),
            400,
            "InvalidRequestException",
            "either thingName or thingArn must name the thing to add to the thing group"
                .to_string(),
        ),
    ];

    for (case, body, status, code, message) in &rows {
        let reply = emulator.add_thing_to_group(body).await;
        assert_eq!(
            reply.status, *status,
            "case {case}: expected {status}, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            &reply.error_code(),
            code,
            "case {case}: got {}",
            reply.text()
        );
        assert_eq!(
            &reply.error_message(),
            message,
            "case {case}: got {}",
            reply.text()
        );
    }

    // Nothing above was written: the group the adds named still holds no thing.
    let members = emulator.things_in("after the refusals", "fleet", "").await;
    assert!(
        members.is_empty(),
        "case after the refusals: expected no membership written, got {members:?}"
    );

    // A listing of a group that does not exist is the same 404, never an empty page.
    let missing = emulator.list_things_in_group("no-such-fleet", "").await;
    assert_eq!(
        missing.status,
        404,
        "case a listing of a group that does not exist: expected 404, got {} body {}",
        missing.status,
        missing.text()
    );
}

/// RULE: a recursive listing is the union of the group's members and those of every
/// group below it in the hierarchy.
#[tokio::test]
async fn a_recursive_listing_covers_the_groups_below_it() {
    let emulator = Emulator::new();
    // A three-level hierarchy, each level holding one thing of its own: root -> middle
    // -> leaf.
    let levels = [("root", None), ("middle", Some("root")), ("leaf", Some("middle"))];
    for (group_name, parent) in levels {
        let body = match parent {
            Some(parent) => json!({ "parentGroupName": parent }),
            None => json!({}),
        };
        assert_eq!(
            emulator.create_group(group_name, &body).await.status,
            200,
            "case {group_name}: the group must be creatable"
        );
        let thing_name = format!("thing-in-{group_name}");
        assert_eq!(emulator.create_thing(&thing_name).await.status, 200);
        assert_eq!(
            emulator
                .add_thing_to_group(
                    &json!({ "thingGroupName": group_name, "thingName": thing_name })
                )
                .await
                .status,
            200,
            "case {group_name}: the membership must be writable"
        );
    }

    // Each row: the group listed, and the things a recursive listing of it reports.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        (
            "root",
            vec!["thing-in-leaf", "thing-in-middle", "thing-in-root"],
        ),
        ("middle", vec!["thing-in-leaf", "thing-in-middle"]),
        ("leaf", vec!["thing-in-leaf"]),
    ];
    for (group_name, expected) in rows {
        let recursive = emulator
            .things_in(group_name, group_name, "?recursive=true")
            .await;
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            recursive, expected,
            "case {group_name} recursively: expected {expected:?}, got {recursive:?}"
        );
        // A non-recursive listing of the same group reports only its OWN members.
        let direct = emulator.things_in(group_name, group_name, "").await;
        assert_eq!(
            direct,
            vec![format!("thing-in-{group_name}")],
            "case {group_name} directly: expected only its own member, got {direct:?}"
        );
    }
}

/// RULE: a job that targets a thing group runs on the group's MEMBERS — one execution
/// each — and a group the registry does not hold is a 404.
#[tokio::test]
async fn a_group_targeted_job_runs_on_the_groups_members() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_group("fleet", &json!({})).await.status,
        200
    );
    for thing_name in ["sensor-a", "sensor-b"] {
        assert_eq!(emulator.create_thing(thing_name).await.status, 200);
        assert_eq!(
            emulator
                .add_thing_to_group(
                    &json!({ "thingGroupName": "fleet", "thingName": thing_name })
                )
                .await
                .status,
            200
        );
    }
    // A thing that is NOT in the group, so the fan-out is the membership and not the
    // registry.
    assert_eq!(emulator.create_thing("outsider").await.status, 200);

    let created = emulator
        .create_job(
            "group-job",
            &json!({
                "targets": [thing_group_arn("fleet")],
                "document": "{\"operation\":\"reboot\"}",
            }),
        )
        .await;
    assert_eq!(
        created.status,
        200,
        "case a group target: expected 200, got {} body {}",
        created.status,
        created.text()
    );

    for thing_name in ["sensor-a", "sensor-b"] {
        let listed = emulator.list_job_executions_for_thing(thing_name).await;
        assert_eq!(
            listed.status,
            200,
            "case {thing_name}: expected the listing to answer 200, got {} body {}",
            listed.status,
            listed.text()
        );
        let summaries = listed.json();
        let jobs: Vec<String> = summaries["executionSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {thing_name}: no executionSummaries in {summaries}"))
            .iter()
            .map(|entry| entry["jobId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            jobs,
            vec!["group-job".to_string()],
            "case {thing_name}: a member of the targeted group gets one execution, got {jobs:?}"
        );
    }
    let outsider = emulator.list_job_executions_for_thing("outsider").await.json();
    assert_eq!(
        outsider["executionSummaries"],
        json!([]),
        "case a thing outside the group: expected no execution, got {outsider}"
    );

    let unknown_group = emulator
        .create_job(
            "unknown-group-job",
            &json!({
                "targets": [thing_group_arn("no-such-fleet")],
                "document": "{\"operation\":\"reboot\"}",
            }),
        )
        .await;
    assert_eq!(
        unknown_group.status,
        404,
        "case a group the registry does not hold: expected 404, got {} body {}",
        unknown_group.status,
        unknown_group.text()
    );

    // A DYNAMIC group's members are the things its query selects, which this emulator
    // does not evaluate — so the devices it names are not known and the job is refused,
    // never answered with a 200 that dispatched to nobody.
    assert_eq!(
        emulator
            .create_dynamic_group("derived", &json!({ "queryString": "attributes.zone:north" }))
            .await
            .status,
        200
    );
    let dynamic_target = emulator
        .create_job(
            "dynamic-group-job",
            &json!({
                "targets": [thing_group_arn("derived")],
                "document": "{\"operation\":\"reboot\"}",
            }),
        )
        .await;
    assert_eq!(
        dynamic_target.status,
        400,
        "case a dynamic group target: expected 400, got {} body {}",
        dynamic_target.status,
        dynamic_target.text()
    );
    assert_eq!(
        dynamic_target.error_code(),
        "InvalidRequestException",
        "case a dynamic group target: got {}",
        dynamic_target.text()
    );
    // The same fact refuses a listing of that group's things, rather than an empty page
    // standing in for a membership nobody computed.
    let dynamic_listing = emulator.list_things_in_group("derived", "").await;
    assert_eq!(
        dynamic_listing.status,
        400,
        "case listing a dynamic group's things: expected 400, got {} body {}",
        dynamic_listing.status,
        dynamic_listing.text()
    );
    assert_eq!(
        dynamic_listing.error_code(),
        "InvalidRequestException",
        "case listing a dynamic group's things: got {}",
        dynamic_listing.text()
    );
}

/// RULE: a dynamic thing group IS its query — an unparsable one is refused and nothing
/// is created, and a created one reports the query that decides its membership.
#[tokio::test]
async fn a_dynamic_thing_group_is_its_query() {
    let emulator = Emulator::new();

    // The rows span the forms a fleet-indexing query takes: a field-qualified term, a
    // wildcard, a range, quoted text, the boolean operators and grouping.
    let accepted: Vec<(&str, &str)> = vec![
        ("a field-qualified term", "attributes.zone:north"),
        ("a wildcard value", "thingName:sensor-*"),
        ("a range", "registry.version:[1 TO 10]"),
        ("a quoted phrase", "attributes.zone:\"north west\""),
        (
            "boolean operators and grouping",
            "(attributes.zone:north OR attributes.zone:south) AND NOT thingName:spare",
        ),
    ];
    for (index, (case, query_string)) in accepted.iter().enumerate() {
        let group_name = format!("derived-{index}");
        let created = emulator
            .create_dynamic_group(&group_name, &json!({ "queryString": query_string }))
            .await;
        assert_eq!(
            created.status,
            200,
            "case {case}: expected 200, got {} body {}",
            created.status,
            created.text()
        );
        let body = created.json();
        assert_eq!(
            body["queryString"],
            json!(query_string),
            "case {case}: the create must report the stored query, got {body}"
        );
        assert_eq!(
            body["indexName"],
            json!("AWS_Things"),
            "case {case}: a dynamic group is over the thing index, got {body}"
        );
        assert_eq!(
            body["thingGroupArn"],
            json!(thing_group_arn(&group_name)),
            "case {case}: expected the minted ARN, got {body}"
        );

        let described = emulator.describe_group(&group_name).await;
        assert_eq!(
            described.status,
            200,
            "case {case}: expected the read to answer 200, got {} body {}",
            described.status,
            described.text()
        );
        let read = described.json();
        assert_eq!(
            read["queryString"],
            json!(query_string),
            "case {case}: the read must report the stored query, got {read}"
        );
        assert_eq!(
            read["status"],
            json!("ACTIVE"),
            "case {case}: a group whose query is recorded is active, got {read}"
        );

        // The delete takes the group with it, and a repeat of it changes nothing.
        for repeat in ["the delete", "a repeat of the delete"] {
            let deleted = emulator.delete_dynamic_group(&group_name).await;
            assert_eq!(
                deleted.status,
                200,
                "case {case}, {repeat}: expected 200, got {} body {}",
                deleted.status,
                deleted.text()
            );
        }
        assert_eq!(
            emulator.describe_group(&group_name).await.status,
            404,
            "case {case}: the deleted group must be gone"
        );
    }

    // A string that is not a query at all: each row breaks the syntax a different way.
    let unparsable = [
        "thingName:",
        ":north",
        "(attributes.zone:north",
        "attributes.zone:north)",
        "registry.version:[1 TO 10",
        "attributes.zone:\"north",
        "AND thingName:sensor",
        "thingName:sensor AND",
        "thingName:a AND OR thingName:b",
        "   ",
    ];
    for query_string in unparsable {
        let refused = emulator
            .create_dynamic_group("refused", &json!({ "queryString": query_string }))
            .await;
        assert_eq!(
            refused.status,
            400,
            "case {query_string:?}: expected 400, got {} body {}",
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.error_code(),
            "InvalidQueryException",
            "case {query_string:?}: got {}",
            refused.text()
        );
        assert_eq!(
            refused.error_message(),
            UNPARSABLE_QUERY_MESSAGE,
            "case {query_string:?}: got {}",
            refused.text()
        );
        assert_eq!(
            emulator.describe_group("refused").await.status,
            404,
            "case {query_string:?}: a refused create must leave no group behind"
        );
    }

    // The two kinds of group share one namespace, so a name a static group holds is a
    // conflict for the dynamic create too.
    assert_eq!(
        emulator.create_group("fleet", &json!({})).await.status,
        200
    );
    let conflict = emulator
        .create_dynamic_group("fleet", &json!({ "queryString": "thingName:*" }))
        .await;
    assert_eq!(
        conflict.status,
        409,
        "case a name a static group holds: expected 409, got {} body {}",
        conflict.status,
        conflict.text()
    );
    // The static group is left as it was: no query was written onto it.
    let still_static = emulator.describe_group("fleet").await.json();
    assert!(
        still_static.get("queryString").is_none_or(Value::is_null),
        "case a name a static group holds: the static group must be untouched, got {still_static}"
    );
}

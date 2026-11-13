// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for which Region's endpoint serves a Quick Setup resource.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: Quick Setup is a regional control plane, so a configuration
//! manager (and every configuration deployed from its definitions) is held by exactly
//! one Region — the one whose endpoint created it, the Region its `ManagerArn` carries —
//! and is reported and mutated only through that Region. A request signed for any other
//! Region lists none of it (its page is empty and its `NextToken` is the empty string
//! that says there is no next page) and finds none of its
//! identifiers, which `ListConfigurations` states for itself: it "returns configurations
//! deployed by Quick Setup in the requesting Amazon Web Services account and Amazon Web
//! Services Region" (`smithy_query(query="doc",shape="ListConfigurations")`). Every
//! such request is refused by the handler, in this service's own modelled shapes, however
//! it named the resource — as a route's path label, as a `ManagerArn` in the body, or as a
//! configuration id ([`cross_region_refusal`]).
//!
//! The Region a configuration REPORTS is a different thing: `ConfigurationSummary$Region`
//! is "The Amazon Web Services Region where the configuration was deployed"
//! (`smithy_query(query="doc",shape="ConfigurationSummary",member="Region")`), i.e. where
//! the deployment went — so a deployment into another Region is still served by the
//! endpoint that created its manager, and not by the Region it landed in.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for the reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// The organizational unit every definition below deploys to. A definition's parameters
/// must name somewhere to deploy to, and no case here is about which accounts that is,
/// so every create names this one and differs only in the Regions it targets.
const TARGET_OU: &str = "ou-1";

/// The tag key every fixture manager carries, so an `UntagResource` has a key to remove
/// and a cross-Region attempt has something it could have destroyed. Alphanumeric, so
/// it travels through a query string unchanged.
const TAG_KEY: &str = "Env";

/// The tag value that key carries.
const TAG_VALUE: &str = "prod";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in `region` — the signal the router routes on
/// and the handlers read the Region from (the SigV4 credential scope).
fn request(
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{region}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, region, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// A query-string value with every byte outside RFC 3986's unreserved set
/// percent-encoded, which is what an SDK writes for an `@httpQuery` member.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `POST /configurationManager` signed for `region`, with ONE definition carrying
/// `parameters` and one tag, answering the `ManagerArn` it minted.
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    parameters: &Value,
) -> String {
    // Every accepted definition's parameters name somewhere to deploy to, so each
    // caller's `parameters` carry [`TARGET_OU`] beside whatever the case is about.
    let body = json!({
        "Name": name,
        "Tags": { TAG_KEY: TAG_VALUE },
        "ConfigurationDefinitions": [{ "Type": TYPE, "Parameters": parameters }],
    });
    let (status, answer) = call(router, "POST", "/configurationManager", region, Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager in {region} must succeed, got {status} / {answer:?}"
    );
    let arn = answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string();
    assert!(
        arn.contains(&format!(":{region}:")),
        "case {case}: a minted ARN carries the signing Region, got {arn:?}"
    );
    arn
}

/// `GET /configurationManager/{ManagerArn}` signed for `region`.
async fn get_manager(router: &MultiServiceRouter, region: &str, manager_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        region,
        None,
    )
    .await
}

/// `POST /listConfigurationManagers` signed for `region`.
async fn list_managers(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/listConfigurationManagers",
        region,
        Some(body),
    )
    .await
}

/// `POST /listConfigurations` signed for `region`.
async fn list_configurations(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call(router, "POST", "/listConfigurations", region, Some(body)).await
}

/// `GET /getConfiguration/{ConfigurationId}` signed for `region`.
async fn get_configuration(
    router: &MultiServiceRouter,
    region: &str,
    configuration_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(configuration_id)),
        region,
        None,
    )
    .await
}

/// A listing that must succeed, answering the array its `key` member carries.
fn rows_of(case: &str, key: &str, status: u16, answer: &Value) -> Vec<Value> {
    assert_eq!(
        status, 200,
        "case {case}: the listing must succeed, got {status} / {answer:?}"
    );
    // Every one of these listings fits on one page, and an exhausted listing still
    // reports the token member: the empty string, which is what says there is no next
    // page to resume from.
    assert_eq!(
        answer.get("NextToken").and_then(Value::as_str),
        Some(""),
        "case {case}: expected an empty NextToken, got {answer:?}"
    );
    answer
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a {key}, got {answer:?}"))
        .clone()
}

/// The `ManagerArn` of every manager `region`'s endpoint lists.
async fn listed_manager_arns(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
) -> BTreeSet<String> {
    let (status, answer) = list_managers(router, region, &json!({})).await;
    rows_of(case, "ConfigurationManagersList", status, &answer)
        .iter()
        .map(|row| {
            row.get("ManagerArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {row:?}"))
                .to_string()
        })
        .collect()
}

/// One row of the listing table: the case, the Region every create in it is signed for,
/// and one (`Name`, `TargetRegions`) pair per manager created through that endpoint. A
/// definition names the Regions it deploys into, so every row names them.
type ListingRow = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

/// A listing reports exactly what the signed Region holds.
///
/// The rows differ along what the rule generalises over: how many managers a Region
/// holds (none, one, several), whether their definitions deploy where the manager lives
/// or into other Regions, and whether a Region is one that only ever appears as a
/// deployment target.
#[tokio::test]
async fn a_listing_reports_only_what_the_signed_region_holds() {
    let router = emulator();

    let rows: Vec<ListingRow> = vec![
        (
            "one manager, deployed where it was created",
            "us-west-2",
            vec![("west", "us-west-2")],
        ),
        (
            "two managers, one of them deployed into other Regions",
            "eu-west-1",
            vec![
                ("euro-a", "eu-west-1"),
                ("euro-b", "eu-central-1,ap-south-1"),
            ],
        ),
        ("a Region no create was signed for", "ap-southeast-2", vec![]),
        // `eu-central-1` is a Region the eu-west-1 row DEPLOYS INTO: it holds no manager
        // of its own, so its endpoint must report nothing at all.
        ("a Region only deployed into", "eu-central-1", vec![]),
    ];

    // What each Region must then list: the ARNs it minted, and one configuration per
    // Region each of its definitions targets.
    let mut expected: Vec<(&str, &str, BTreeSet<String>, usize)> = Vec::new();
    for (case, region, managers) in &rows {
        let mut arns: BTreeSet<String> = BTreeSet::new();
        let mut configurations = 0_usize;
        for (name, target_regions) in managers {
            let parameters = json!({
                "TargetOrganizationalUnits": TARGET_OU,
                "TargetRegions": target_regions,
            });
            let arn = create_ok(&router, case, region, name, &parameters).await;
            configurations += target_regions.split(',').count();
            assert!(
                arns.insert(arn.clone()),
                "case {case}: every create mints a distinct ARN, got {arn:?} twice"
            );
        }
        expected.push((case, region, arns, configurations));
    }

    for (case, region, arns, configurations) in &expected {
        let listed = listed_manager_arns(&router, case, region).await;
        assert_eq!(
            &listed, arns,
            "case {case}: {region} must list exactly the managers created there, got {listed:?}"
        );
        for arn in &listed {
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {case}: {region} must list no ARN of another Region, got {arn:?}"
            );
        }

        let (status, answer) = list_configurations(&router, region, &json!({})).await;
        let listed_configurations = rows_of(case, "ConfigurationsList", status, &answer);
        assert_eq!(
            listed_configurations.len(),
            *configurations,
            "case {case}: {region} must list {configurations} configurations, got {listed_configurations:?}"
        );
        for row in &listed_configurations {
            let owner = row
                .get("ManagerArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {row:?}"));
            assert!(
                arns.contains(owner),
                "case {case}: {region} must list only configurations of its own managers, \
                 got one owned by {owner:?}"
            );
        }
    }
}

/// One row of the deployment-Region table: the case, the Region the manager is created
/// through, the `TargetRegions` its definition names, and the Regions its configurations
/// must then report.
type DeploymentRow = (
    &'static str,
    &'static str,
    &'static str,
    Vec<&'static str>,
);

/// A configuration is served by the Region that created its manager, not by the Region
/// it was deployed into.
///
/// The rows differ along how many Regions a definition deploys into and whether the
/// manager's own Region is among them — including a row that deploys ONLY into Regions
/// other than the manager's, whose endpoints must still serve none of it.
#[tokio::test]
async fn a_configuration_is_served_by_the_region_that_created_its_manager() {
    let rows: Vec<DeploymentRow> = vec![
        (
            "deployed into the Region the manager lives in",
            "us-west-2",
            "us-west-2",
            vec!["us-west-2"],
        ),
        (
            "deployed into one other Region only",
            "eu-west-1",
            "us-east-1",
            vec!["us-east-1"],
        ),
        (
            "deployed into three Regions, one of them the manager's own",
            "ap-south-1",
            "ap-south-1,eu-central-1,us-east-2",
            vec!["ap-south-1", "eu-central-1", "us-east-2"],
        ),
    ];

    for (case, home, targets, deployed_regions) in rows {
        // One store per row, so an empty answer from another Region's endpoint can only
        // mean that endpoint serves none of this row's resources.
        let router = emulator();
        let parameters = json!({
            "TargetOrganizationalUnits": TARGET_OU,
            "TargetRegions": targets,
        });
        let manager_arn = create_ok(&router, case, home, "deployed-elsewhere", &parameters).await;

        let (status, answer) =
            list_configurations(&router, home, &json!({ "ManagerArn": &manager_arn })).await;
        let listed = rows_of(case, "ConfigurationsList", status, &answer);
        let reported: BTreeSet<String> = listed
            .iter()
            .map(|row| {
                row.get("Region")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: expected a Region, got {row:?}"))
                    .to_string()
            })
            .collect();
        let wanted: BTreeSet<String> = deployed_regions.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            reported, wanted,
            "case {case}: the creating Region must report every deployment, got {reported:?}"
        );

        let ids: Vec<String> = listed
            .iter()
            .map(|row| {
                row.get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: expected an Id, got {row:?}"))
                    .to_string()
            })
            .collect();
        for id in &ids {
            let (status, got) = get_configuration(&router, home, id).await;
            assert_eq!(
                status, 200,
                "case {case}: the creating Region must serve configuration {id:?}, \
                 got {status} / {got:?}"
            );
            assert_eq!(
                got.get("ManagerArn").and_then(Value::as_str),
                Some(manager_arn.as_str()),
                "case {case}: a configuration names the manager that owns it, got {got:?}"
            );
        }

        for other in deployed_regions.iter().filter(|region| **region != home) {
            for id in &ids {
                let (status, got) = get_configuration(&router, other, id).await;
                assert_eq!(
                    status, 404,
                    "case {case}: {other} deploys the configuration but does not serve it; \
                     expected 404 for {id:?}, got {status} / {got:?}"
                );
                assert_eq!(
                    error_code(&got),
                    "ResourceNotFoundException",
                    "case {case}: expected ResourceNotFoundException from {other}, got {got:?}"
                );
            }

            let (status, answer) = list_configurations(&router, other, &json!({})).await;
            let listed_there = rows_of(case, "ConfigurationsList", status, &answer);
            assert!(
                listed_there.is_empty(),
                "case {case}: {other} holds no manager, so it lists no configuration, \
                 got {listed_there:?}"
            );

            let (status, got) = get_manager(&router, other, &manager_arn).await;
            assert_eq!(
                status, 404,
                "case {case}: {other} must not serve a manager of {home}, got {status} / {got:?}"
            );
        }
    }
}

/// Everything a fixture manager is addressed by.
struct Fixture {
    /// The Region whose endpoint created it, and the only one that serves it.
    region: &'static str,
    /// The `Name` the create stored.
    name: &'static str,
    manager_arn: String,
    /// The service-assigned id of its one configuration definition.
    definition_id: String,
    /// The service-assigned id of its one deployed configuration.
    configuration_id: String,
}

/// One manager in `region` with one definition and one deployed configuration, deployed
/// into the Region that serves it.
async fn fixture(router: &MultiServiceRouter, region: &'static str, name: &'static str) -> Fixture {
    let manager_arn = create_ok(
        router,
        region,
        region,
        name,
        &json!({ "TargetOrganizationalUnits": TARGET_OU, "TargetRegions": region }),
    )
    .await;
    let (status, manager) = get_manager(router, region, &manager_arn).await;
    assert_eq!(
        status, 200,
        "case {region}: the creating Region must serve the manager, got {status} / {manager:?}"
    );
    let definition_id = manager["ConfigurationDefinitions"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {region}: expected a definition Id, got {manager:?}"))
        .to_string();

    let (status, answer) =
        list_configurations(router, region, &json!({ "ManagerArn": &manager_arn })).await;
    let listed = rows_of(region, "ConfigurationsList", status, &answer);
    assert_eq!(
        listed.len(),
        1,
        "case {region}: a definition naming one Region deploys exactly once, got {listed:?}"
    );
    let configuration_id = listed[0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {region}: expected an Id, got {listed:?}"))
        .to_string();

    Fixture {
        region,
        name,
        manager_arn,
        definition_id,
        configuration_id,
    }
}

/// Every operation that addresses ONE stored resource by its identifier, in an order a
/// caller can run end to end against one manager: the reads, then the mutations, and the
/// delete last.
const PER_RESOURCE_OPERATIONS: &[&str] = &[
    "GetConfigurationManager",
    "ListTagsForResource",
    "ListConfigurations",
    "GetConfiguration",
    "UpdateConfigurationManager",
    "TagResource",
    "UntagResource",
    "UpdateConfigurationDefinition",
    "DeleteConfigurationManager",
];

/// Which refusal one of [`PER_RESOURCE_OPERATIONS`] earns for a resource of another
/// Region: the handler's own modelled 404, however the request named the resource — as a
/// route's path label (the manager's own route, the tag plane's `ResourceArn`, the
/// configuration-definition route's parent `ManagerArn`), as a `ManagerArn` in the body,
/// or as a configuration id in the path. No operation of this service models a 401.
fn cross_region_refusal(_operation: &str) -> (u16, &'static str) {
    (404, "ResourceNotFoundException")
}

/// One of [`PER_RESOURCE_OPERATIONS`], addressing `fixture`'s identifiers but signed for
/// `region`.
async fn per_resource_call(
    router: &MultiServiceRouter,
    operation: &str,
    region: &str,
    fixture: &Fixture,
) -> (u16, Value) {
    let arn = label(&fixture.manager_arn);
    match operation {
        "GetConfigurationManager" => {
            call(
                router,
                "GET",
                &format!("/configurationManager/{arn}"),
                region,
                None,
            )
            .await
        }
        "ListTagsForResource" => call(router, "GET", &format!("/tags/{arn}"), region, None).await,
        "ListConfigurations" => {
            list_configurations(router, region, &json!({ "ManagerArn": &fixture.manager_arn })).await
        }
        "GetConfiguration" => get_configuration(router, region, &fixture.configuration_id).await,
        "UpdateConfigurationManager" => {
            call(
                router,
                "PUT",
                &format!("/configurationManager/{arn}"),
                region,
                Some(&json!({ "Name": "renamed-through-this-endpoint" })),
            )
            .await
        }
        "TagResource" => {
            call(
                router,
                "PUT",
                &format!("/tags/{arn}"),
                region,
                Some(&json!({ "Tags": { "Owner": "this-endpoint" } })),
            )
            .await
        }
        "UntagResource" => {
            call(
                router,
                "DELETE",
                &format!("/tags/{arn}?tagKeys={}", query_value(TAG_KEY)),
                region,
                None,
            )
            .await
        }
        "UpdateConfigurationDefinition" => {
            call(
                router,
                "PUT",
                &format!(
                    "/configurationDefinition/{arn}/{}",
                    label(&fixture.definition_id)
                ),
                region,
                Some(&json!({ "Parameters": {
                    "TargetOrganizationalUnits": TARGET_OU,
                    "TargetRegions": "us-east-1",
                } })),
            )
            .await
        }
        "DeleteConfigurationManager" => {
            call(
                router,
                "DELETE",
                &format!("/configurationManager/{arn}"),
                region,
                None,
            )
            .await
        }
        other => panic!("{other:?} is not one of {PER_RESOURCE_OPERATIONS:?}"),
    }
}

/// An identifier minted in one Region is not found through another Region's endpoint,
/// and nothing a request signed for another Region asks for is read, changed or removed.
///
/// The table spans every operation that addresses one resource by its identifier — read
/// and mutating, manager-addressed and configuration-addressed — against every ordered
/// pair of Regions holding a manager, and then asserts the SAME request succeeds when it
/// is signed for the Region that owns the resource.
#[tokio::test]
async fn an_identifier_of_another_region_is_not_found() {
    let router = emulator();
    let fixtures = vec![
        fixture(&router, "us-west-2", "west").await,
        fixture(&router, "eu-west-1", "euro").await,
        fixture(&router, "ap-south-1", "asia").await,
    ];

    for owner in &fixtures {
        for caller in &fixtures {
            if caller.region == owner.region {
                continue;
            }
            for operation in PER_RESOURCE_OPERATIONS {
                let case = format!(
                    "{operation} on a {} resource from {}",
                    owner.region, caller.region
                );
                let (status, got) =
                    per_resource_call(&router, operation, caller.region, owner).await;
                let (expected_status, expected_code) = cross_region_refusal(operation);
                assert_eq!(
                    status, expected_status,
                    "case {case}: expected {expected_status}, got {status} / {got:?}"
                );
                assert_eq!(
                    error_code(&got),
                    expected_code,
                    "case {case}: expected {expected_code}, got {got:?}"
                );
            }
        }
    }

    // Nothing those cross-Region requests asked for happened: every manager is still
    // there, still named what its create stored, still carrying exactly its create's
    // tag, and still deploying the one configuration whose id the fixture holds.
    for owner in &fixtures {
        let (status, manager) = get_manager(&router, owner.region, &owner.manager_arn).await;
        assert_eq!(
            status, 200,
            "case {}: the owning Region must still serve the manager, got {status} / {manager:?}",
            owner.region
        );
        assert_eq!(
            manager.get("Name").and_then(Value::as_str),
            Some(owner.name),
            "case {}: no cross-Region request renames a manager, got {manager:?}",
            owner.region
        );
        assert_eq!(
            manager.get("Tags"),
            Some(&json!({ TAG_KEY: TAG_VALUE })),
            "case {}: no cross-Region request changes a manager's tags, got {manager:?}",
            owner.region
        );
        let (status, configuration) =
            get_configuration(&router, owner.region, &owner.configuration_id).await;
        assert_eq!(
            status, 200,
            "case {}: the owning Region must still serve the configuration, \
             got {status} / {configuration:?}",
            owner.region
        );
        assert_eq!(
            configuration.get("Region").and_then(Value::as_str),
            Some(owner.region),
            "case {}: no cross-Region request redeploys a configuration, got {configuration:?}",
            owner.region
        );
    }

    // The same requests, signed for the Region that owns the resource, are served: the
    // refusals above are about the Region, not about the request.
    for owner in &fixtures {
        for operation in PER_RESOURCE_OPERATIONS {
            let case = format!(
                "{operation} on a {} resource from {}",
                owner.region, owner.region
            );
            let (status, got) = per_resource_call(&router, operation, owner.region, owner).await;
            assert_eq!(
                status, 200,
                "case {case}: the owning Region must serve it, got {status} / {got:?}"
            );
        }
    }
}

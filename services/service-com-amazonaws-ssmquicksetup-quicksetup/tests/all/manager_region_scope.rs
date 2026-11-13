// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a configuration manager's own Region governs.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: the Region a configuration manager belongs to is the Region the
//! create was signed for and the Region its `ManagerArn` carries, and it governs exactly
//! one thing — which endpoint reports and mutates that manager, since Quick Setup is a
//! regional control plane whose `ManagerArn` is "The ARN of the Quick Setup
//! configuration" minted by that endpoint
//! (`smithy_query(query="doc",shape="CreateConfigurationManagerOutput",member="ManagerArn")`).
//! It is never a Region a configuration definition is deployed INTO: a definition is
//! deployed into exactly the Regions its own parameters name — the parameter table
//! documented for a configuration type marks `TargetRegions` `(Required)`
//! (`smithy_query(query="doc",shape="ConfigurationDefinitionInput",member="Parameters")`)
//! — so the manager's Region is among the Regions deployed into only when the parameters
//! themselves name it, and a configuration reports "The Amazon Web Services Region where
//! the configuration was deployed"
//! (`smithy_query(query="doc",shape="ConfigurationSummary",member="Region")`), not the
//! Region of the manager that owns it.
//!
//! Each row therefore drives both halves through one manager: the endpoint-scoping half
//! (`GetConfigurationManager`, `UpdateConfigurationManager` and `ListConfigurations`
//! answer only through the creating Region) and the deployment half
//! (`ListConfigurations` reports exactly the targeted Regions).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for a reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// The organizational unit every definition below deploys to: a parameter map must name
/// somewhere to deploy to, and no row here is about which accounts that is.
const TARGET_OU: &str = "ou-1";

/// A Region no row creates a manager in and no row deploys into, so an endpoint of it
/// may serve none of what the rows create.
const UNINVOLVED_REGION: &str = "me-south-1";

/// The description every manager is created with, so the update below visibly changes it.
const CREATED_DESCRIPTION: &str = "created through its own Region";

/// The description a cross-Region update must never manage to write.
const REFUSED_DESCRIPTION: &str = "written through another Region";

/// The description the owning Region's update writes.
const UPDATED_DESCRIPTION: &str = "updated through its own Region";

/// The account this emulator serves, which a definition naming no `TargetAccounts` is
/// deployed in.
fn local_account() -> String {
    engine::emulator_account_id().to_string()
}

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

/// The `Message` a refusal carries.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
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

/// The Region field of an ARN (`arn:aws:<service>:<region>:<account>:<resource>`).
fn arn_region(arn: &str) -> String {
    arn.split(':').nth(3).unwrap_or("<no region>").to_string()
}

/// `GetConfigurationManager` through the endpoint of `region`.
async fn get_manager(
    router: &MultiServiceRouter,
    region: &str,
    manager_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        region,
        None,
    )
    .await
}

/// `UpdateConfigurationManager` through the endpoint of `region`, writing `description`.
async fn update_manager(
    router: &MultiServiceRouter,
    region: &str,
    manager_arn: &str,
    description: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/configurationManager/{}", label(manager_arn)),
        region,
        Some(&json!({ "Description": description })),
    )
    .await
}

/// `ListConfigurations` for one manager through the endpoint of `region`.
async fn list_configurations(
    router: &MultiServiceRouter,
    region: &str,
    manager_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/listConfigurations",
        region,
        Some(&json!({ "ManagerArn": manager_arn })),
    )
    .await
}

/// The (`Account`, `Region`) pair of every configuration deployed for one manager, read
/// through the endpoint of `region`.
async fn deployed_pairs(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    manager_arn: &str,
) -> BTreeSet<(String, String)> {
    let (status, page) = list_configurations(router, region, manager_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConfigurations must succeed through {region}, got {status} / {page:?}"
    );
    page["ConfigurationsList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConfigurationsList, got {page:?}"))
        .iter()
        .map(|row| {
            let account = row["Account"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an Account, got {row:?}"));
            let deployed_region = row["Region"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Region, got {row:?}"));
            (account.to_string(), deployed_region.to_string())
        })
        .collect()
}

/// The `Description` a manager reports through the endpoint of `region`.
async fn described_as(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    manager_arn: &str,
) -> String {
    let (status, manager) = get_manager(router, region, manager_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConfigurationManager must succeed through {region}, \
         got {status} / {manager:?}"
    );
    manager["Description"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Description, got {manager:?}"))
        .to_string()
}

/// One row: the Region the create is signed for, the `TargetRegions` value its single
/// definition carries as written, and the Regions that definition is then deployed into.
struct Row {
    case: &'static str,
    home: &'static str,
    target_regions: &'static str,
    deployed: &'static [&'static str],
}

/// The table: rows spanning what the rule generalises over — which Region the manager
/// belongs to, how many Regions its definition targets, how that comma-separated value
/// is written (padding, repeats), and whether the manager's own Region is among the
/// targeted Regions, none of them, or the only one.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one targeted Region, not the manager's own",
            home: "us-west-2",
            target_regions: "eu-west-1",
            deployed: &["eu-west-1"],
        },
        Row {
            case: "the manager's own Region among two targeted ones",
            home: "us-east-1",
            target_regions: "us-east-1,eu-central-1",
            deployed: &["us-east-1", "eu-central-1"],
        },
        Row {
            case: "the manager's own Region the only targeted one",
            home: "ap-south-1",
            target_regions: "ap-south-1",
            deployed: &["ap-south-1"],
        },
        Row {
            case: "three targeted Regions written with padding and a repeat",
            home: "eu-west-3",
            target_regions: " eu-west-1, ap-southeast-2 ,eu-west-1, sa-east-1",
            deployed: &["eu-west-1", "ap-southeast-2", "sa-east-1"],
        },
        Row {
            case: "a manager and a targeted Region of another partition's naming",
            home: "us-gov-west-1",
            target_regions: "us-gov-east-1",
            deployed: &["us-gov-east-1"],
        },
    ]
}

/// The Regions whose endpoints must serve none of a row's manager: every Region its
/// definition deployed into other than the manager's own, plus a Region nothing in the
/// row ever named.
fn foreign_regions(row: &Row) -> Vec<&'static str> {
    let mut regions: Vec<&'static str> = row
        .deployed
        .iter()
        .copied()
        .filter(|region| *region != row.home)
        .collect();
    regions.push(UNINVOLVED_REGION);
    regions
}

/// A manager's Region is the Region that created it and the Region its ARN carries, it
/// is the only endpoint that reports or mutates the manager, and it is a Region the
/// manager's definition is deployed into only when the definition's parameters name it.
#[tokio::test]
async fn a_managers_region_scopes_its_endpoint_and_never_its_deployments() {
    let local = local_account();

    for row in rows() {
        let case = row.case;
        // One emulator per row, so no row observes another row's writes.
        let router = emulator();
        let parameters = json!({
            "TargetOrganizationalUnits": TARGET_OU,
            "TargetRegions": row.target_regions,
        });
        let (status, created) = call(
            &router,
            "POST",
            "/configurationManager",
            row.home,
            Some(&json!({
                "Name": "region-scope",
                "Description": CREATED_DESCRIPTION,
                "ConfigurationDefinitions": [{ "Type": TYPE, "Parameters": parameters }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConfigurationManager must succeed, got {status} / {created:?}"
        );
        let manager_arn = created["ManagerArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {created:?}"))
            .to_string();
        let minted_in = arn_region(&manager_arn);
        assert_eq!(
            minted_in, row.home,
            "case {case:?}: the ARN must carry the Region the create was signed for, \
             expected {:?}, got {minted_in:?}",
            row.home
        );

        // The deployment half: exactly the Regions the parameters name, whether or not
        // the manager's own Region is among them.
        let want: BTreeSet<(String, String)> = row
            .deployed
            .iter()
            .map(|region| (local.clone(), (*region).to_string()))
            .collect();
        let got = deployed_pairs(&router, case, row.home, &manager_arn).await;
        assert_eq!(
            got, want,
            "case {case:?}: the definition must be deployed into exactly the Regions its \
             parameters name, expected {want:?}, got {got:?}"
        );
        let home_is_targeted = row.deployed.contains(&row.home);
        let home_is_deployed_into = got.iter().any(|(_, region)| region == row.home);
        assert_eq!(
            home_is_deployed_into, home_is_targeted,
            "case {case:?}: the manager's own Region {:?} may be deployed into only when the \
             parameters name it (named: {home_is_targeted}), got {got:?}",
            row.home
        );

        // The endpoint-scoping half: only the creating Region reports the manager.
        let (status, manager) = get_manager(&router, row.home, &manager_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetConfigurationManager must succeed through {:?}, \
             got {status} / {manager:?}",
            row.home
        );
        assert_eq!(
            manager["ManagerArn"].as_str(),
            Some(manager_arn.as_str()),
            "case {case:?}: the owning endpoint must report the manager it minted, \
             got {manager:?}"
        );
        assert_eq!(
            manager["ConfigurationDefinitions"][0]["Parameters"],
            parameters,
            "case {case:?}: the stored definition must carry the parameters the create named, \
             got {manager:?}"
        );

        for foreign in foreign_regions(&row) {
            // The manager's own route carries its ARN as a path label, and the handler
            // resolves it against the Region the request was signed for, so another
            // Region's endpoint — which serves no resource under that ARN — answers the
            // modelled not-found (`tests/all/target_authorization.rs`).
            let (status, answer) = get_manager(&router, foreign, &manager_arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: GetConfigurationManager through {foreign} serves no \
                 manager of {:?}, got {status} / {answer:?}",
                row.home
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException through {foreign}, \
                 got {answer:?}"
            );
            assert_eq!(
                error_message(&answer),
                format!("Quick setup manager with arn: {manager_arn} could not be found."),
                "case {case:?}: expected the refusal through {foreign} to name the ARN the \
                 request carried, got {answer:?}"
            );

            // The same ARN carried in the BODY is no route label, and it is resolved the
            // same way: this service's own not-found sentence, naming the ARN the request
            // carried.
            let (status, answer) = list_configurations(&router, foreign, &manager_arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: ListConfigurations through {foreign} must not find a manager \
                 of {:?}, got {status} / {answer:?}",
                row.home
            );
            assert_eq!(
                error_message(&answer),
                format!("Quick setup manager with arn: {manager_arn} could not be found."),
                "case {case:?}: expected the refusal through {foreign} to name the ARN the \
                 request carried, got {answer:?}"
            );

            let (status, answer) =
                update_manager(&router, foreign, &manager_arn, REFUSED_DESCRIPTION).await;
            assert_eq!(
                status, 404,
                "case {case:?}: UpdateConfigurationManager through {foreign} serves no \
                 manager of {:?}, got {status} / {answer:?}",
                row.home
            );
            let description = described_as(&router, case, row.home, &manager_arn).await;
            assert_eq!(
                description, CREATED_DESCRIPTION,
                "case {case:?}: a refused cross-Region update must leave the manager alone, \
                 got {description:?}"
            );
        }

        // The same mutation through the owning Region is served, and it changes the
        // manager without touching where its definition is deployed.
        let (status, answer) =
            update_manager(&router, row.home, &manager_arn, UPDATED_DESCRIPTION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateConfigurationManager must succeed through {:?}, \
             got {status} / {answer:?}",
            row.home
        );
        let description = described_as(&router, case, row.home, &manager_arn).await;
        assert_eq!(
            description, UPDATED_DESCRIPTION,
            "case {case:?}: the owning endpoint's update must be stored, got {description:?}"
        );
        let after = deployed_pairs(&router, case, row.home, &manager_arn).await;
        assert_eq!(
            after, want,
            "case {case:?}: updating the manager must leave its deployments as its parameters \
             name them, expected {want:?}, got {after:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! WHY a private DNS namespace create reports it could not be created: the reason its
//! `CREATE_NAMESPACE` operation carries when the `Vpc` it named resolves to no VPC of the
//! account.
//!
//! RULE: a private DNS namespace IS a private Route 53 hosted zone bound to the VPC its
//! create named, so a `Vpc` that resolves to no VPC of the account leaves that binding
//! impossible and the operation the create answered with lands `FAIL` with the documented
//! `CANNOT_CREATE_HOSTED_ZONE`, carrying as its reason the refusal that binding is made
//! with: the invalid-VPC sentence, whose FIXED text is `The VPC: ` and ` not found in
//! region ` and whose only variable parts are the VPC id the request named and the Region
//! the request was signed for. The reason is coupled to `Status`
//! (`smithy_query(query="doc",shape="Operation",member="ErrorMessage")`: "If the value of
//! `Status` is `FAIL`, the reason that the operation failed"), so an operation that is not
//! `FAIL` carries none at all, and the staged namespace is rolled back with the failure.
//!
//! The rows below span both variable parts, because that is what the rule generalises over:
//! the `Vpc` form (the legacy short form, the long form Amazon EC2 issues today, and the
//! longest a `ResourceId` may be) and the Region the call is signed for (two Regions, so a
//! Region in the sentence cannot be a constant of this service). A VPC the account really
//! holds is the other side of the rule: it carries no reason at all. The fixed text is
//! stated HERE, independently of the service, so a service that reworded it would fail these
//! rows; the values are taken from each row's own request, never from one recorded run.
//!
//! Every Cloud Map call goes through `MultiServiceRouter::dispatch` (`awsJson1_1`, the same
//! front door a client uses); Amazon EC2 is mounted beside this service on the one shared
//! engine, which is the inventory the `Vpc` is resolved against, and the EC2 read the
//! assertions make is driven over the emulator's own in-process transport.

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::async_namespace_operations::{emulator_with_ec2, set_operation_due, settled_operation};
use crate::cloud_map_core::{call_signed_for, REGION};

/// The fixed text the invalid-VPC sentence opens with, stated here rather than read from the
/// service, so these rows pin the wording instead of echoing whatever it words today.
const OPENING: &str = "The VPC: ";

/// The fixed text between the VPC the request named and the Region it was asked in, stated
/// here for the reason [`OPENING`] is.
const REGION_CLAUSE: &str = " not found in region ";

/// A second Region to sign for, beside [`REGION`]: a member of the modelled `VPCRegion` enum
/// of the hosted zone a private namespace is bound to
/// (`smithy_query(query="shape",shape="VPCRegion",model_path="services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json")`),
/// so the row is a Region a VPC can really be created in.
const OTHER_REGION: &str = "eu-west-1";

/// The longest a `CreatePrivateDnsNamespace$Vpc` may be (`ResourceId`, `@length 0..64`, from
/// `smithy_query(query="operation",operation="CreatePrivateDnsNamespace")`), spelt as a VPC
/// id of that length so the row still names a VPC and not a value of another kind.
fn longest_vpc_id() -> String {
    format!("vpc-{}", "0".repeat(64 - "vpc-".len()))
}

/// An Amazon EC2 client over the emulator's own in-process transport, addressing `region`:
/// the inventory the `Vpc` is resolved against, so a row can show that the VPC it names
/// really is one the account does not hold there.
fn ec2(router: &MultiServiceRouter, region: &str) -> ::aws_sdk_ec2::Client {
    let callback = EmbeddedEmulator::from_router(router.clone());
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// Assert the account holds no VPC under `vpc_id` in `region`, as the inventory the create
/// resolves against answers it — so a row's premise is read from that inventory rather than
/// assumed.
async fn assert_no_such_vpc(router: &MultiServiceRouter, case: &str, region: &str, vpc_id: &str) {
    let answered = ec2(router, region)
        .describe_vpcs()
        .vpc_ids(vpc_id)
        .send()
        .await;
    let code = answered.as_ref().err().and_then(|error| {
        ::aws_sdk_ec2::error::ProvideErrorMetadata::code(error).map(str::to_string)
    });
    assert_eq!(
        code.as_deref(),
        Some("InvalidVpcID.NotFound"),
        "case {case:?}: the account must hold no VPC under {vpc_id} in {region}, got {answered:?}"
    );
}

/// Dispatch an operation signed for `region` that must succeed, and answer its output.
async fn ok_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, answered) = call_signed_for(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status}: {answered}"
    );
    answered
}

/// Create a private DNS namespace named `name` inside `vpc`, signed for `region`, and answer
/// `(operation id, namespace id)`.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    vpc: &str,
) -> (String, String) {
    let answered = ok_in(
        router,
        case,
        region,
        "CreatePrivateDnsNamespace",
        json!({ "Name": name, "Vpc": vpc }),
    )
    .await;
    let operation_id = answered["OperationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the synchronous call answers an OperationId, got {answered}")
        })
        .to_string();
    let filed = ok_in(
        router,
        case,
        region,
        "GetOperation",
        json!({ "OperationId": operation_id.clone() }),
    )
    .await;
    // While the transition is still owed, the operation is not FAIL, so it carries no
    // reason: the model couples `ErrorMessage` to `Status`.
    assert!(
        filed["Operation"]["ErrorMessage"].is_null() && filed["Operation"]["ErrorCode"].is_null(),
        "case {case:?}: an operation that is not FAIL carries no failure reason, got {filed}"
    );
    let namespace_id = filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the CREATE_NAMESPACE operation targets its namespace, got {filed}")
        })
        .to_string();
    (operation_id, namespace_id)
}

/// A create whose `Vpc` names no VPC of the account fails its operation with the invalid-VPC
/// sentence of THAT request — its VPC id and its Region — and leaves no namespace behind.
#[tokio::test]
async fn an_absent_vpc_fails_the_create_with_the_invalid_vpc_sentence_of_that_request() {
    let (router, engine) = emulator_with_ec2();

    // (case, the Region the call is signed for, the `Vpc` it names)
    let cases: Vec<(&str, &str, String)> = vec![
        (
            "the legacy short form, in the Region the suite signs for",
            REGION,
            "vpc-0a1b2c3d".to_string(),
        ),
        (
            "the long form Amazon EC2 issues today, in another Region",
            OTHER_REGION,
            "vpc-0123456789abcdef0".to_string(),
        ),
        ("the longest a Vpc may be", REGION, longest_vpc_id()),
        (
            "the same longest form, asked in another Region",
            OTHER_REGION,
            longest_vpc_id(),
        ),
    ];

    for (index, (case, region, vpc)) in cases.into_iter().enumerate() {
        let name = format!("absent-vpc-{index}.example.com");
        assert_no_such_vpc(&router, case, region, &vpc).await;
        let (operation_id, namespace_id) = created(&router, case, region, &name, &vpc).await;

        // The clock, not the wall clock, settles the operation: the service stores when the
        // transition is owed and materialises it on read.
        set_operation_due(&engine, &operation_id, true);
        let settled = settled_operation(&router, &operation_id).await;
        let operation = &settled["Operation"];
        assert_eq!(
            (&operation["Status"], &operation["ErrorCode"]),
            (&json!("FAIL"), &json!("CANNOT_CREATE_HOSTED_ZONE")),
            "case {case:?}: a namespace whose VPC resolves to nothing fails with the documented \
             code, got {settled}"
        );
        // The fixed text of this suite's own, filled with THIS request's two values: a
        // service that reworded the sentence, dropped the Region, or reported another
        // request's values fails here.
        let expected = format!("{OPENING}{vpc}{REGION_CLAUSE}{region}");
        assert_eq!(
            operation["ErrorMessage"],
            json!(expected),
            "case {case:?}: expected the invalid-VPC sentence {expected:?}, got {settled}"
        );

        // The create that failed left nothing behind, and the stored reason reads back
        // unchanged however often the operation is polled.
        let (status, read) =
            call_signed_for(&router, region, "GetNamespace", json!({ "Id": namespace_id })).await;
        let code = read["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default();
        assert_eq!(
            (status, code),
            (400, "NamespaceNotFound"),
            "case {case:?}: a create that failed leaves no namespace to read, got {read}"
        );
        let read_again = ok_in(
            &router,
            case,
            region,
            "GetOperation",
            json!({ "OperationId": operation_id }),
        )
        .await;
        assert_eq!(
            read_again["Operation"]["ErrorMessage"],
            json!(expected),
            "case {case:?}: the stored reason reads back unchanged, got {read_again}"
        );
    }
}

/// The other side of the rule: a create whose `Vpc` the account really holds succeeds,
/// reports no reason at all, and the namespace reads back carrying the zone created for it.
#[tokio::test]
async fn a_vpc_the_account_holds_leaves_the_operation_without_a_reason() {
    let (router, engine) = emulator_with_ec2();
    // The account's own VPC in this Region, read from the EC2 store the resolution asks —
    // never an id this test invented.
    let held_vpc = {
        let ec2_state = engine
            .state::<service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State>();
        let held = ec2_state
            .lock()
            .ensure_default_network(REGION)
            .expect("the account holds a default VPC in this Region");
        held
    };

    let case = "a VPC the account holds";
    let name = "held-vpc.example.com";
    let (operation_id, namespace_id) = created(&router, case, REGION, name, &held_vpc).await;
    set_operation_due(&engine, &operation_id, true);
    let settled = settled_operation(&router, &operation_id).await;
    assert_eq!(
        settled["Operation"]["Status"],
        json!("SUCCESS"),
        "case {case:?}: the VPC is one the account holds, so the create succeeds, got {settled}"
    );
    assert!(
        settled["Operation"]["ErrorMessage"].is_null()
            && settled["Operation"]["ErrorCode"].is_null(),
        "case {case:?}: an operation that is not FAIL carries no reason, got {settled}"
    );
    let read = ok_in(
        &router,
        case,
        REGION,
        "GetNamespace",
        json!({ "Id": namespace_id }),
    )
    .await;
    assert_eq!(
        read["Namespace"]["Name"],
        json!(name),
        "case {case:?}: the created namespace reads back, got {read}"
    );
    assert!(
        read["Namespace"]["Properties"]["DnsProperties"]["HostedZoneId"].is_string(),
        "case {case:?}: the namespace carries the zone that was created for it, got {read}"
    );
}

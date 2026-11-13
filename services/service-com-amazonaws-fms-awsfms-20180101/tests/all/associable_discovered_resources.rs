// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE classification both batch membership operations
//! make of a resource-set item.
//!
//! RULE under test: an item is associable when every field its ARN CARRIES
//! agrees with the resource set — the partition is this emulator's, the Region is
//! the one the request addresses, the account is one of the organization's — and
//! its type, read off the ARN the way the service that owns the resource writes
//! it, is one the set's `ResourceTypeList` names; a field the owning service
//! leaves EMPTY (a global service's ARN carries no Region, an account-less
//! service's no account) names no value to disagree with, so it refuses nothing.
//! That is the reading the discoverable inventory makes too, so every resource
//! `ListDiscoveredResources` reports as "available to be associated with a
//! resource set" is one `BatchAssociateResource` takes and
//! `BatchDisassociateResource` removes again.
//!
//! [`every_discovered_resource_is_associable`] asserts the rule through the
//! owning services: one row per resource type whose ARN form used to disagree
//! with the inventory — CloudFront's (no Region), API Gateway's (no account, and
//! a type keyword written in the plural inside a path), Amazon EC2's Elastic IP
//! (whose ARN spells the type `elastic-ip` where the resource-type name is
//! `AWS::EC2::EIP`) — beside an EC2 instance, whose ARN carries every field.
//!
//! [`an_item_is_classified_by_the_arn_fields_it_carries`] is the same rule as a
//! table over ARNs a client sends directly, so the rows that must still be
//! refused (another partition, another Region, an account outside the
//! organization, a type the set does not name, a value that is no ARN) are pinned
//! beside the ones an empty field must not refuse — and asserted on BOTH batch
//! operations, which share the classifier.

use serde_json::json;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::discovered_resources::{
    api_gateway_created, api_gateway_method_on_root, ec2_ok, element, emulator, every_uri, p,
};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A Region that is not the one the requests here are signed for, so an ARN
/// naming it addresses another Region's resources.
const OTHER_REGION: &str = "us-east-1";

/// An account id outside this organization: nothing this emulator holds belongs
/// to it, and no policy scope map names it.
const FOREIGN_ACCOUNT: &str = "999999999999";

/// CloudFront's API version, which prefixes every route that service binds.
const CLOUDFRONT_API_VERSION: &str = "2020-05-31";

/// Create one CloudFront distribution over the wire — `POST
/// /2020-05-31/distribution`, restXml — and report the ARN that service minted
/// for it, which is the URI a resource set would associate it by.
///
/// CloudFront is global, so that ARN carries NO Region; the point of creating the
/// distribution through CloudFront's own API is that the URI under test is the
/// one the owning service really mints rather than one spelled out here.
///
/// `pub(crate)` so a sibling suite in this test target stands a global resource
/// up through this one writer rather than a second copy of it.
pub(crate) async fn create_distribution(
    router: &MultiServiceRouter,
    caller_reference: &str,
) -> String {
    let body = format!(
        "<DistributionConfig \
          xmlns=\"http://cloudfront.amazonaws.com/doc/{CLOUDFRONT_API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment><Enabled>true</Enabled>\
         </DistributionConfig>"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/{CLOUDFRONT_API_VERSION}/distribution"))
        .header("host", "cloudfront.amazonaws.com")
        .header("content-type", "application/xml")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/cloudfront/aws4_request, \
             SignedHeaders=host, Signature=deadbeef",
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed restXml request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 201,
        "case {caller_reference:?}: expected HTTP 201 from CreateDistribution, got {status} with \
         {xml}"
    );
    element(caller_reference, &xml, "ARN")
}

/// One row of the inventory table: the resource type, and the URI the service
/// that owns it minted for the resource created through that service's own API.
struct DiscoveredRow {
    resource_type: &'static str,
    uri: String,
}

/// RULE: what the inventory reports as available to be associated with a
/// resource set is what both batch operations take, whatever ARN form the owning
/// service writes — including one that leaves the Region or the account field
/// empty, or that spells the resource type its own way.
#[tokio::test]
async fn every_discovered_resource_is_associable() {
    let (router, engine) = emulator();
    associate_admin(&router).await;

    // --- the resources, each created through the service that OWNS it --------
    let distribution_uri = create_distribution(&router, "fms-associable-distribution").await;

    let created_api = api_gateway_created(
        "rest api",
        &router,
        "/restapis",
        json!({ "name": "fms-associable-api" }),
    )
    .await;
    let api = created_api["id"]
        .as_str()
        .expect("a minted REST API id")
        .to_string();
    // A deployment is the snapshot of the API's methods, so the API must carry one.
    api_gateway_method_on_root(&router, &api, &created_api).await;
    api_gateway_created(
        "stage",
        &router,
        &format!("/restapis/{api}/deployments"),
        json!({ "stageName": "prod" }),
    )
    .await;

    let allocation = element(
        "elastic ip",
        &ec2_ok(
            "elastic ip",
            &router,
            "AllocateAddress",
            &[p("Domain", "vpc")],
        )
        .await,
        "allocationId",
    );
    let instance = element(
        "instance",
        &ec2_ok(
            "instance",
            &router,
            "RunInstances",
            &[
                p("ImageId", "ami-0123456789abcdef0"),
                p("MinCount", "1"),
                p("MaxCount", "1"),
            ],
        )
        .await,
        "instanceId",
    );

    // Rows differ in the OWNING service, in which ARN field that service leaves
    // empty (CloudFront's Region, API Gateway's account, none of EC2's), in how
    // the ARN spells the resource type (a leading segment, a plural segment
    // inside a path, a keyword that differs from the type's own name) and in the
    // depth of the ARN's resource path.
    let rows = vec![
        DiscoveredRow {
            resource_type: state::RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION,
            uri: distribution_uri,
        },
        DiscoveredRow {
            resource_type: state::RESOURCE_TYPE_APIGATEWAY_STAGE,
            uri: format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/prod"),
        },
        DiscoveredRow {
            resource_type: state::RESOURCE_TYPE_EC2_EIP,
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:elastic-ip/{allocation}"),
        },
        DiscoveredRow {
            resource_type: state::RESOURCE_TYPE_EC2_INSTANCE,
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/{instance}"),
        },
    ];

    // Each row's resource is reported by the inventory, under the URI its owning
    // service minted — the precondition the association rule is about.
    for row in &rows {
        let discovered = every_uri(&router, ACCOUNT, row.resource_type).await;
        assert!(
            discovered.contains(&row.uri),
            "case {:?}: the inventory must report {:?}, got {discovered:?}",
            row.resource_type,
            row.uri
        );
    }

    let types: Vec<&str> = rows.iter().map(|row| row.resource_type).collect();
    let (_, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": { "Name": "associable-inventory", "ResourceTypeList": types },
        }),
    )
    .await;
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .expect("a minted Id")
        .to_string();

    let items: Vec<String> = rows.iter().map(|row| row.uri.clone()).collect();
    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": items }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchAssociateResource to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["FailedItems"],
        json!([]),
        "every discovered resource must be associable, got {body}"
    );

    // Read back through the modelled reader: the membership is the set of URIs
    // that were associated.
    let mut expected = items.clone();
    expected.sort();
    let (status, listed) = call(
        &router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListResourceSetResources must answer the membership, got {status} with {listed}"
    );
    let reported: Vec<String> = listed["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("the response must carry an Items list, got {listed}"))
        .iter()
        .map(|item| item["URI"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "the reported membership must be every associated resource, got {listed}"
    );

    // The SAME classifier governs the removal, so each of these items is no
    // failure there either and the membership empties.
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": items }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchDisassociateResource to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["FailedItems"],
        json!([]),
        "every discovered resource must be disassociable, got {body}"
    );
    with_store(&engine, |store| {
        let held: Vec<String> = store.resource_sets[&id].resources.iter().cloned().collect();
        assert!(
            held.is_empty(),
            "disassociating every member must empty the membership, got {held:?}"
        );
    });
}

/// One row of the classification table: a name for the assertion messages, the
/// item a client sends, and the `FailedItemReason` it must be reported with — or
/// `None` when the item must be accepted.
struct ItemCase {
    case: &'static str,
    uri: String,
    reason: Option<&'static str>,
}

/// RULE: each field an item's ARN CARRIES is weighed against the resource set,
/// and a field the owning service leaves empty is weighed against nothing — so an
/// empty Region or account excuses no OTHER field that disagrees, and a type the
/// set does not name is still refused.
#[tokio::test]
async fn an_item_is_classified_by_the_arn_fields_it_carries() {
    // The catalog is the model's own `FailedItemReason`, so a reason this suite
    // expects cannot drift from the enum.
    let reasons = service_com_amazonaws_fms_awsfms_20180101::ops::types::FAILED_ITEM_REASON_VALUES;
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let (_, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": "classified-items",
                "ResourceTypeList": [
                    state::RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION,
                    state::RESOURCE_TYPE_APIGATEWAY_STAGE,
                    state::RESOURCE_TYPE_EC2_EIP,
                    state::RESOURCE_TYPE_EC2_INSTANCE,
                ],
            },
        }),
    )
    .await;
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .expect("a minted Id")
        .to_string();

    // Rows differ in which ARN field is empty (the Region, the account, both,
    // neither), in how the resource type is spelled (a leading segment, a plural
    // segment inside a path, a keyword of the owning service's own), in the depth
    // of the resource path, and in which field disagrees.
    let cases = vec![
        ItemCase {
            case: "global-arn-carries-no-region",
            uri: format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E1EXAMPLE1EXAM"),
            reason: None,
        },
        ItemCase {
            case: "global-arn-carries-neither-region-nor-account",
            uri: "arn:aws:cloudfront:::distribution/E2EXAMPLE2EXAM".to_string(),
            reason: None,
        },
        ItemCase {
            case: "account-less-arn-naming-its-type-in-a-path",
            uri: format!("arn:aws:apigateway:{REGION}::/restapis/a1b2c3d4e5/stages/prod"),
            reason: None,
        },
        ItemCase {
            case: "account-less-arn-of-another-stage",
            uri: format!("arn:aws:apigateway:{REGION}::/restapis/zz9pza/stages/pre-release"),
            reason: None,
        },
        ItemCase {
            // Amazon EC2 addresses an Elastic IP by its allocation,
            // `elastic-ip/<AllocationId>`, where the resource type is named
            // `AWS::EC2::EIP`.
            case: "arn-spelling-its-type-the-owning-services-way",
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:elastic-ip/eipalloc-0123456789abcdef0"),
            reason: None,
        },
        ItemCase {
            case: "arn-carrying-every-field",
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef0"),
            reason: None,
        },
        ItemCase {
            case: "an-empty-region-does-not-excuse-a-foreign-account",
            uri: format!("arn:aws:cloudfront::{FOREIGN_ACCOUNT}:distribution/E3EXAMPLE3EXAM"),
            reason: Some("NOT_VALID_ACCOUNT_ID"),
        },
        ItemCase {
            case: "an-empty-account-does-not-excuse-another-region",
            uri: format!("arn:aws:apigateway:{OTHER_REGION}::/restapis/a1b2c3d4e5/stages/prod"),
            reason: Some("NOT_VALID_REGION"),
        },
        ItemCase {
            case: "an-account-less-arn-of-a-type-the-set-does-not-name",
            uri: format!("arn:aws:apigateway:{REGION}::/restapis/a1b2c3d4e5/resources/f6g7h8"),
            reason: Some("NOT_VALID_RESOURCE_TYPE"),
        },
        ItemCase {
            case: "a-type-the-set-does-not-name",
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:natgateway/nat-0123456789abcdef0"),
            reason: Some("NOT_VALID_RESOURCE_TYPE"),
        },
        ItemCase {
            case: "another-partition",
            uri: format!("arn:aws-cn:cloudfront::{ACCOUNT}:distribution/E4EXAMPLE4EXAM"),
            reason: Some("NOT_VALID_PARTITION"),
        },
        ItemCase {
            case: "no-arn-at-all",
            uri: "/restapis/a1b2c3d4e5/stages/prod".to_string(),
            reason: Some("NOT_VALID_ARN"),
        },
    ];
    for case in &cases {
        if let Some(reason) = case.reason {
            assert!(
                reasons.contains(&reason),
                "case {:?}: {reason:?} is not in the model's FailedItemReason catalog {reasons:?}",
                case.case
            );
        }
    }

    let items: Vec<&str> = cases.iter().map(|case| case.uri.as_str()).collect();
    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": items }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchAssociateResource to answer 200, got {status} with {body}"
    );
    for case in &cases {
        let reported = body["FailedItems"]
            .as_array()
            .unwrap_or_else(|| panic!("FailedItems must be a list, got {body}"))
            .iter()
            .find(|item| item["URI"].as_str() == Some(case.uri.as_str()))
            .map(|item| item["Reason"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            reported.as_deref(),
            case.reason,
            "case {:?}: expected reason {:?}, got {reported:?} in {body}",
            case.case,
            case.reason
        );
    }

    // The accepted rows are the membership now: the classification decides what
    // is stored, not only what is reported.
    let mut expected: Vec<String> = cases
        .iter()
        .filter(|case| case.reason.is_none())
        .map(|case| case.uri.clone())
        .collect();
    expected.sort();
    with_store(&engine, |store| {
        let held: Vec<String> = store.resource_sets[&id].resources.iter().cloned().collect();
        assert_eq!(
            held, expected,
            "the accepted items must be the membership, got {held:?}"
        );
    });

    // `BatchDisassociateResource` reads the SAME classifier, so every row is
    // classified identically there, and the accepted ones leave the membership.
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": items }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchDisassociateResource to answer 200, got {status} with {body}"
    );
    for case in &cases {
        let reported = body["FailedItems"]
            .as_array()
            .unwrap_or_else(|| panic!("FailedItems must be a list, got {body}"))
            .iter()
            .find(|item| item["URI"].as_str() == Some(case.uri.as_str()))
            .map(|item| item["Reason"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            reported.as_deref(),
            case.reason,
            "case {:?}: both batch operations must classify an item alike, expected {:?}, got \
             {reported:?} in {body}",
            case.case,
            case.reason
        );
    }
    with_store(&engine, |store| {
        let held: Vec<String> = store.resource_sets[&id].resources.iter().cloned().collect();
        assert!(
            held.is_empty(),
            "every accepted item must have left the membership, got {held:?}"
        );
    });

    // An identifier that names no resource set is still not found, whatever the
    // items are: the classification never decides the operation's own error.
    let (status, body) = call(
        &router,
        "BatchAssociateResource",
        &json!({
            "ResourceSetIdentifier": "0123456789abcdef0123ab",
            "Items": [format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E5EXAMPLE5EXAM")],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "an unknown resource set must be ResourceNotFoundException, got {status} with {body}"
    );
}

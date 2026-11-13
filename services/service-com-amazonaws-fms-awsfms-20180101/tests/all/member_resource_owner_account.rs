// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE owner reading Firewall Manager makes of a
//! resource it knows by nothing but its URI.
//!
//! RULE under test: a resource belongs to the account its own ARN names where
//! the service that owns it writes one into the account field, and to the
//! account this emulator holds the resource in where that service leaves the
//! field EMPTY — a global service's ARN and an account-less service's carry no
//! account — so `ListResourceSetResources`'s `Resource$AccountId` ("the Amazon
//! Web Services account ID that the associated resource belongs to") and
//! `ListDiscoveredResources`'s `DiscoveredResource$AccountId` ("the Amazon Web
//! Services account ID associated with the discovered resource") report the SAME
//! owner for one resource, whatever ARN form its owning service writes.
//!
//! The table in [`the_inventory_and_the_membership_report_one_owner`] is the
//! class that rule ranges over: one row per owning service, the rows differing in
//! which ARN field that service leaves empty (API Gateway's account, CloudFront's
//! Region, none of Amazon EC2's), in the protocol the resource was created over
//! (`restJson1`, `restXml`, `ec2Query`) and in the depth of the ARN's resource
//! path. Every resource is created through the API of the service that OWNS it,
//! so the URI and the owner under test are the ones that service really mints —
//! not values spelled out here.
//!
//! The ARN FORMS the same rule ranges over, including an account field naming an
//! account other than the administrator's, are the table of the membership
//! suite (`resource_set_resources`), which reports the owner of every row.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::associate_admin;
use crate::associable_discovered_resources::create_distribution;
use crate::discovered_resources::{
    api_gateway_created, api_gateway_method_on_root, ec2_ok, element, emulator, p,
};
use crate::policy_lifecycle::{call, ACCOUNT, REGION};

/// One row of the table: the resource type, and the URI the service that owns it
/// minted for a resource created through that service's own API.
struct Row {
    resource_type: &'static str,
    uri: String,
}

/// The owner the inventory reports for `uri`, walking every page of
/// `ListDiscoveredResources` for one account and type so a page bound cannot hide
/// the resource. `None` when the inventory does not report the URI at all.
async fn discovered_owner(
    router: &MultiServiceRouter,
    resource_type: &str,
    uri: &str,
) -> Option<Option<String>> {
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({
            "MemberAccountIds": [ACCOUNT],
            "ResourceType": resource_type,
        });
        if let Some(next) = token.as_deref() {
            request["NextToken"] = json!(next);
        }
        let (status, body) = call(router, "ListDiscoveredResources", &request).await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: ListDiscoveredResources must answer 200, got {status} with \
             {body}"
        );
        let found = body["Items"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {resource_type:?}: the response must carry an Items list, got {body}")
            })
            .iter()
            .find(|item| item["URI"].as_str() == Some(uri))
            .map(|item| {
                item.get("AccountId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        if found.is_some() {
            return found;
        }
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        // Not reported on this page: the walk ends where the pages do.
        token.as_ref()?;
    }
}

/// The owner the membership reader reports for `uri`, walking every page of
/// `ListResourceSetResources`. `None` when the set does not hold the URI.
async fn member_owner(
    router: &MultiServiceRouter,
    identifier: &str,
    uri: &str,
) -> Option<Option<String>> {
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "Identifier": identifier });
        if let Some(next) = token.as_deref() {
            request["NextToken"] = json!(next);
        }
        let (status, body) = call(router, "ListResourceSetResources", &request).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: ListResourceSetResources must answer 200, got {status} with {body}"
        );
        let found = body["Items"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {uri:?}: the response must carry an Items list, got {body}")
            })
            .iter()
            .find(|item| item["URI"].as_str() == Some(uri))
            .map(|item| {
                item.get("AccountId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        if found.is_some() {
            return found;
        }
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        // Not held on this page: the walk ends where the pages do.
        token.as_ref()?;
    }
}

/// RULE: both surfaces of one resource report the one account that resource
/// belongs to — the account this emulator holds it in where its URI names none.
#[tokio::test]
async fn the_inventory_and_the_membership_report_one_owner() {
    let (router, _engine) = emulator();
    associate_admin(&router).await;

    // --- the resources, each created through the service that OWNS it --------
    let distribution_uri = create_distribution(&router, "fms-member-owner-distribution").await;

    let created_api = api_gateway_created(
        "rest api",
        &router,
        "/restapis",
        json!({ "name": "fms-member-owner-api" }),
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

    let rows = vec![
        Row {
            // API Gateway writes NO account into its ARNs.
            resource_type: state::RESOURCE_TYPE_APIGATEWAY_STAGE,
            uri: format!("arn:aws:apigateway:{REGION}::/restapis/{api}/stages/prod"),
        },
        Row {
            // CloudFront is global: its ARN carries no Region.
            resource_type: state::RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION,
            uri: distribution_uri,
        },
        Row {
            // Amazon EC2 writes every field, the account among them.
            resource_type: state::RESOURCE_TYPE_EC2_INSTANCE,
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/{instance}"),
        },
    ];

    // The resource set that takes every row, declaring each row's type because
    // `ResourceSet$ResourceTypeList` "determines the resources that can be
    // associated to the resource set".
    let types: Vec<&str> = rows.iter().map(|row| row.resource_type).collect();
    let (status, created) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": { "Name": "member-owner-account", "ResourceTypeList": types },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the resource set must be stored, got {status} with {created}"
    );
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

    for row in &rows {
        let discovered = discovered_owner(&router, row.resource_type, &row.uri)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the inventory must report {:?}",
                    row.resource_type, row.uri
                )
            });
        let member = member_owner(&router, &id, &row.uri)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the membership must hold {:?}",
                    row.resource_type, row.uri
                )
            });
        assert_eq!(
            member.as_deref(),
            Some(ACCOUNT),
            "case {:?}: expected the member {:?} to be reported as belonging to {ACCOUNT}, got \
             {member:?}",
            row.resource_type,
            row.uri
        );
        assert_eq!(
            member, discovered,
            "case {:?}: the two surfaces of {:?} must report one owner, the inventory said \
             {discovered:?} and the membership said {member:?}",
            row.resource_type, row.uri
        );
    }
}

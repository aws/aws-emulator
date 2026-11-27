// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for WHEN a Studio domain's clock-owed transitions become
//! observable: the lifecycle a client drives with the documented poll-until-settled loop —
//! create, read to `InService`, re-create the held name, delete, read until the id resolves
//! to nothing, create the freed name again.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness (`Emulator`,
//! `error_code`, `message`, `str_field`) is the one `inference_deployment_lifecycle`
//! defines for this target.
//!
//! The RULE under test: a domain's PROVISIONING is work SageMaker does after `CreateDomain`
//! has answered, so the domain is `Pending` for a span of its own
//! (`state::DOMAIN_IN_SERVICE_AFTER`) and the requests that immediately follow the create
//! still read that status, while a client polling for the condition reaches `InService`
//! without waiting a real provisioning out. The transitions the domain's own CALLS decide —
//! the change `UpdateDomain` admits and the teardown `DeleteDomain` admits — are owed from the
//! instant that call answers (`state::transition_owed_at`), so the FIRST request after one of
//! those reports the settled domain. Every move is still the CLOCK's and no reader decides it
//! — a domain whose due instant is ahead is reported in its transient status by every reader.
//!
//! The tables span what the rule generalises over rather than one recorded flow: the Region
//! the request is signed for, the `AuthMode` the domain is created with, the `DomainName`
//! shape, and which optional members the create carries.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    ComAmazonawsSagemakerSagemakerState, DOMAIN_STATUS_DELETING, DOMAIN_STATUS_IN_SERVICE,
    DOMAIN_STATUS_PENDING,
};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::studio_user_profiles::wait_for_domain_in_service;

/// An execution role inside `RoleArn`'s modelled pattern, for the `DefaultUserSettings`
/// every create must carry.
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/service-role/StudioRole";

/// How far ahead a case that needs the transient status puts the stored due instant: long
/// enough that no host passes it between two calls of the same case.
const WELL_AHEAD: Duration = Duration::from_secs(3_600);

/// A `CreateDomain` request: the required members, the VPC and subnet a SageMaker AI
/// domain's networking must name, plus whatever `extra` carries.
fn create_domain(domain_name: &str, auth_mode: &str, extra: &Value) -> Value {
    let mut request = json!({
        "DomainName": domain_name,
        "AuthMode": auth_mode,
        "DefaultUserSettings": { "ExecutionRole": EXECUTION_ROLE },
        "VpcId": "vpc-0a1b2c3d",
        "SubnetIds": ["subnet-0a1b2c3d"]
    });
    for (member, value) in extra.as_object().expect("the extra members form an object") {
        request[member] = value.clone();
    }
    request
}

/// The ARN `DomainArn`'s modelled pattern gives the domain `domain_id` in `region`.
fn domain_arn(region: &str, domain_id: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:domain/{domain_id}")
}

/// The whole sentence `CreateDomain` refuses a `DomainName` this account already holds a
/// domain under with. A literal on purpose: this suite pins the wording rather than asking
/// the implementation what it is; the only variable part is the name the REQUEST sent.
fn already_exists(domain_name: &str) -> String {
    format!("The Domain Name '{domain_name}' already exists in this account in this region.")
}

/// The whole sentence the operations that address the domain itself refuse a `DomainId` this
/// account holds no domain under with. A literal, for the same reason; the variable part is
/// the ARN the requested id resolves to.
fn does_not_exist(region: &str, domain_id: &str) -> String {
    format!("Domain [{}] does not exist.", domain_arn(region, domain_id))
}

/// Every `DomainId` `ListDomains` reports in `region`, walked to the last page.
async fn all_listed(emulator: &Emulator, region: &str) -> Vec<Value> {
    let mut domains: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let request = match &token {
            None => json!({}),
            Some(token) => json!({ "NextToken": token }),
        };
        let (status, page) = emulator.call_in(region, "ListDomains", &request).await;
        assert_eq!(
            status, 200,
            "case list-{region}: expected HTTP 200 from ListDomains, got {status} with {page}"
        );
        domains.extend(
            page.get("Domains")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("expected a Domains list in {page}"))
                .iter()
                .cloned(),
        );
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    domains
}

/// The domains `ListDomains` reports in `region` under `domain_name`.
async fn listed_under(emulator: &Emulator, region: &str, domain_name: &str) -> Vec<Value> {
    all_listed(emulator, region)
        .await
        .into_iter()
        .filter(|details| str_field(details, "DomainName") == domain_name)
        .collect()
}

/// One row of the lifecycle table.
struct Row {
    case: &'static str,
    region: &'static str,
    auth_mode: &'static str,
    domain_name: String,
    extra: Value,
}

/// RULE: a domain is still being provisioned when its create answers, so the read that
/// immediately follows reports `Pending` and a client that polls reads `InService`; the
/// transitions the domain's own later calls decide are owed from the instant each answers, so
/// a client that creates, re-creates, deletes and re-creates a Studio domain reads the settled
/// resource on its FIRST request after the delete — the held name is refused `ResourceInUse`
/// and leaves no second domain, the deleted id resolves to nothing for both the reader and
/// the update, and the freed name is admitted again.
#[tokio::test]
async fn the_first_request_after_each_domain_call_reports_the_settled_domain() {
    // Rows differ along what the rule generalises over: the Region signed for, the
    // `AuthMode` the domain is created with, the `DomainName` shape (one character,
    // hyphenated, digits only, the 63-character maximum the modelled
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` admits) and the optional members the create
    // carries.
    let rows: Vec<Row> = vec![
        Row {
            case: "a minimal IAM domain",
            region: "us-east-1",
            auth_mode: "IAM",
            domain_name: "studio".to_string(),
            extra: json!({}),
        },
        Row {
            case: "a single-character SSO domain in another Region",
            region: "eu-west-1",
            auth_mode: "SSO",
            domain_name: "a".to_string(),
            extra: json!({}),
        },
        Row {
            case: "a digits-only name carrying optional members and tags",
            region: "ap-southeast-2",
            auth_mode: "IAM",
            domain_name: "2024".to_string(),
            extra: json!({
                "AppNetworkAccessType": "VpcOnly",
                "TagPropagation": "ENABLED",
                "KmsKeyId": "alias/studio",
                "Tags": [{ "Key": "team", "Value": "research" }]
            }),
        },
        Row {
            case: "the longest name the pattern admits",
            region: "us-west-2",
            auth_mode: "IAM",
            domain_name: format!("a{}", "b".repeat(62)),
            extra: json!({}),
        },
    ];

    for row in &rows {
        let Row {
            case,
            region,
            auth_mode,
            domain_name,
            extra,
        } = row;
        let emulator = Emulator::new();
        let request = create_domain(domain_name, auth_mode, extra);

        let (status, created) = emulator.call_in(region, "CreateDomain", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateDomain, got {status} with {created}"
        );
        let domain_id = str_field(&created, "DomainId");

        // The read that FOLLOWS the create reports the domain still provisioning: the move
        // out of `Pending` is the clock's, a span after the create.
        let described = emulator
            .call_in(region, "DescribeDomain", &json!({ "DomainId": domain_id }))
            .await
            .1;
        assert_eq!(
            str_field(&described, "Status"),
            DOMAIN_STATUS_PENDING,
            "case {case:?}: the read after the create must report the provisioning domain, \
             got {described}"
        );
        assert_eq!(
            str_field(&described, "DomainArn"),
            domain_arn(region, &domain_id),
            "case {case:?}: the reader replays the domain's own ARN, got {described}"
        );
        // A client that polls for the condition reaches the provisioned domain.
        wait_for_domain_in_service(&emulator, region, &domain_id).await;

        // While the name is held, the identical create is refused as in use, and the
        // refusal leaves exactly one domain under that name.
        let (status, refused) = emulator.call_in(region, "CreateDomain", &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceInUse"),
            "case {case:?}: a second create of a held name must be refused in use, got \
             {status} with {refused}"
        );
        assert_eq!(
            message(&refused),
            already_exists(domain_name),
            "case {case:?}: expected {:?}, got {:?}",
            already_exists(domain_name),
            message(&refused)
        );
        let matches = listed_under(&emulator, region, domain_name).await;
        assert_eq!(
            matches.len(),
            1,
            "case {case:?}: the refused create stored nothing, so exactly one domain is \
             listed under {domain_name:?}, got {matches:?}"
        );

        let (status, deleted) = emulator
            .call_in(
                region,
                "DeleteDomain",
                &json!({
                    "DomainId": domain_id,
                    "RetentionPolicy": { "HomeEfsFileSystem": "Delete" }
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteDomain, got {status} with {deleted}"
        );

        // The FIRST request after the delete finds the id resolving to nothing — for the
        // reader and for the update alike, each with its own modelled refusal.
        for (operation, body) in [
            ("DescribeDomain", json!({ "DomainId": domain_id })),
            (
                "UpdateDomain",
                json!({
                    "DomainId": domain_id,
                    "DefaultUserSettings": { "ExecutionRole": EXECUTION_ROLE }
                }),
            ),
        ] {
            let (status, gone) = emulator.call_in(region, operation, &body).await;
            assert_eq!(
                (status, error_code(&gone).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}/{operation}: the first request after the delete must find the \
                 domain gone, got {status} with {gone}"
            );
            assert_eq!(
                message(&gone),
                does_not_exist(region, &domain_id),
                "case {case:?}/{operation}: expected {:?}, got {:?}",
                does_not_exist(region, &domain_id),
                message(&gone)
            );
        }
        assert!(
            listed_under(&emulator, region, domain_name).await.is_empty(),
            "case {case:?}: a torn-down domain is listed nowhere"
        );

        // The teardown freed the name, so the identical create is admitted again and the
        // domain it yields is its own — settled on the first read, as the create's is.
        let (status, again) = emulator.call_in(region, "CreateDomain", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the freed name must be admitted again, got {status} with {again}"
        );
        let second_id = str_field(&again, "DomainId");
        assert_ne!(
            second_id, domain_id,
            "case {case:?}: the re-created domain carries its own id, got {again}"
        );
        let described = emulator
            .call_in(region, "DescribeDomain", &json!({ "DomainId": second_id }))
            .await
            .1;
        assert_eq!(
            str_field(&described, "Status"),
            DOMAIN_STATUS_PENDING,
            "case {case:?}: the read after the re-create must report the provisioning \
             domain, got {described}"
        );
        wait_for_domain_in_service(&emulator, region, &second_id).await;
        let matches = listed_under(&emulator, region, domain_name).await;
        assert_eq!(
            matches.len(),
            1,
            "case {case:?}: exactly one domain is listed under the re-created name, got \
             {matches:?}"
        );
    }
}

/// RULE: the move out of a domain's transient status is the CLOCK's and no reader decides
/// it — a domain whose stored due instant is still ahead is reported in that transient
/// status by every reader, however often it is read, and only the instant passing settles
/// it.
#[tokio::test]
async fn a_domain_whose_transition_is_not_due_keeps_its_transient_status() {
    // Rows differ along the transient status and how the record reaches it: the status a
    // create stores, and the status a delete stores.
    for (case, transient) in [
        ("the status the create stores", DOMAIN_STATUS_PENDING),
        ("the status the delete stores", DOMAIN_STATUS_DELETING),
    ] {
        let region = "us-east-1";
        let emulator = Emulator::new();
        let created = emulator
            .call_in(
                region,
                "CreateDomain",
                &create_domain("held", "IAM", &json!({})),
            )
            .await
            .1;
        let domain_id = str_field(&created, "DomainId");
        let describe = json!({ "DomainId": domain_id });

        // The record is put in the transient status with the instant that move is owed at
        // held well ahead, so what the readers report is what the clock still owes rather
        // than a race with it.
        {
            let store = emulator
                .engine
                .state::<ComAmazonawsSagemakerSagemakerState>();
            let mut store = store.lock();
            let domain = store
                .domains
                .get_mut(&domain_id)
                .unwrap_or_else(|| panic!("case {case:?}: the created domain must be stored"));
            domain.status = transient.to_string();
            let due = SystemTime::now() + WELL_AHEAD;
            if transient == DOMAIN_STATUS_DELETING {
                domain.removal_due_at = Some(due);
            } else {
                domain.in_service_due_at = due;
            }
        }

        // Reading it again and again reports the same transient status: a read materialises
        // only what the clock had already made due.
        for probe in 0..3 {
            let described = emulator.call_in(region, "DescribeDomain", &describe).await.1;
            assert_eq!(
                str_field(&described, "Status"),
                transient,
                "case {case:?} probe {probe}: a transition that is not due yet leaves the \
                 transient status, got {described}"
            );
            let listed = listed_under(&emulator, region, "held").await;
            assert_eq!(
                listed.len(),
                1,
                "case {case:?} probe {probe}: the domain is still listed, got {listed:?}"
            );
            assert_eq!(
                str_field(&listed[0], "Status"),
                transient,
                "case {case:?} probe {probe}: the listing reports what the reader does, got \
                 {listed:?}"
            );
        }

        // Once the instant has passed the move is materialised, and the name is free again
        // for the row whose teardown was owed.
        {
            let store = emulator
                .engine
                .state::<ComAmazonawsSagemakerSagemakerState>();
            let mut store = store.lock();
            let domain = store
                .domains
                .get_mut(&domain_id)
                .unwrap_or_else(|| panic!("case {case:?}: the created domain must be stored"));
            let due = SystemTime::now();
            if transient == DOMAIN_STATUS_DELETING {
                domain.removal_due_at = Some(due);
            } else {
                domain.in_service_due_at = due;
            }
        }
        let (status, settled) = emulator.call_in(region, "DescribeDomain", &describe).await;
        if transient == DOMAIN_STATUS_DELETING {
            assert_eq!(
                (status, error_code(&settled).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}: a domain whose removal came due is gone for every caller, got \
                 {status} with {settled}"
            );
        } else {
            assert_eq!(
                (status, str_field(&settled, "Status").as_str()),
                (200, DOMAIN_STATUS_IN_SERVICE),
                "case {case:?}: a domain whose provisioning came due reports it, got {status} \
                 with {settled}"
            );
        }
    }
}

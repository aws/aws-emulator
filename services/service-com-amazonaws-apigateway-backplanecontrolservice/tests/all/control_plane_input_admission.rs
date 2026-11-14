// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what this control plane ADMITS on three members whose value has
//! to name something outside the request: a custom domain name's certificate references,
//! a base path mapping's base path, and a usage plan's quota offset.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `CreateDomainName` / `CreateBasePathMapping`
//! helpers are this crate's existing harness.
//!
//! The rules under test, each over a TABLE of values rather than one recorded call:
//!
//! * a member that names an Amazon Web Services-managed certificate takes the ARN of a
//!   Certificate Manager certificate and nothing else, on the create and on the patch, and
//!   a value that is not one is the modelled `BadRequestException` carrying this service's
//!   fixed unreadable-ARN sentence, with nothing stored;
//! * a `basePath` is admitted only inside the character class API Gateway's own refusal
//!   states — letters, numbers and `$-_.+!*'(),` — so a value carrying anything else (a
//!   non-ASCII character, a space, a URL delimiter) is refused, on the create and on the
//!   patch alike;
//! * a quota `offset` falls inside the window its `period` gives it, which is zero for
//!   `DAY`, 0-6 for `WEEK` and 0-27 for `MONTH`; each published bound is admitted and one
//!   past it is refused, in the sentence AWS states for the period.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::QUOTA_PERIOD_TYPE_VALUES;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    invalid_quota_offset, max_quota_offset, BASE_PATH_CHARACTERS, INVALID_ARN, INVALID_BASE_PATH,
    NONE_BASE_PATH,
};

use crate::base_path_mapping::{create_mapping, deployed_api, get_mapping, mapping_of};
use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};
use crate::usage_plan_lifecycle::get_usage_plan;

const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// Assert `answer` is the modelled `BadRequestException` carrying exactly `message`.
fn assert_bad_request(answer: &Answer, message: &str, case: &str) {
    assert_failure(answer, BAD_REQUEST.0, BAD_REQUEST.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// `CreateDomainName` with `body` sent as given.
async fn create_domain(gateway: &Gateway, body: Value) -> Answer {
    gateway.call("POST", "/domainnames", Some(body)).await
}

/// `GetDomainName` for `domain_name`.
async fn get_domain(gateway: &Gateway, domain_name: &str) -> Answer {
    gateway
        .call("GET", &format!("/domainnames/{domain_name}"), None)
        .await
}

/// `UpdateDomainName` for `domain_name` with one `replace` of `path` to `value`.
async fn patch_domain(gateway: &Gateway, domain_name: &str, path: &str, value: &str) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/domainnames/{domain_name}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": path, "value": value }],
            })),
        )
        .await
}

/// `CreateUsagePlan` with `body` sent as given.
async fn create_usage_plan(gateway: &Gateway, body: Value) -> Answer {
    gateway.call("POST", "/usageplans", Some(body)).await
}

/// `UpdateUsagePlan` for `id` with one `replace` of `path` to `value`.
async fn patch_usage_plan(gateway: &Gateway, id: &str, path: &str, value: &str) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/usageplans/{id}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": path, "value": value }],
            })),
        )
        .await
}

/// `UpdateBasePathMapping` for `base_path` of `domain_name`, replacing the base path with
/// `value`.
async fn patch_mapping(
    gateway: &Gateway,
    domain_name: &str,
    base_path: &str,
    value: &str,
) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/domainnames/{domain_name}/basepathmappings/{base_path}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": "/basePath", "value": value }],
            })),
        )
        .await
}

/// The certificate ARNs a create may carry, spanning what the ARN form generalises over:
/// the partition, the Region, the account field and the certificate's own name. Source:
/// `arn:aws:acm:<region>:<account>:certificate/<id>`, the form
/// `service_common::arn::ACM_CERTIFICATE` requires.
fn admitted_certificate_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "a certificate ARN as Certificate Manager writes one",
            "arn:aws:acm:us-east-1:000000000000:certificate/\
             8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"
                .to_string(),
        ),
        (
            "a certificate of another Region",
            "arn:aws:acm:eu-west-1:000000000000:certificate/eu".to_string(),
        ),
        (
            "a certificate of another partition",
            "arn:aws-cn:acm:cn-north-1:000000000000:certificate/cn".to_string(),
        ),
        (
            "a certificate of another account",
            "arn:aws:acm:us-east-1:123456789012:certificate/other".to_string(),
        ),
    ]
}

/// The values that name no Certificate Manager certificate, spanning the ways a value can
/// fail to: not an ARN at all, an ARN of another vendor, an ARN of another ACM resource
/// type, and an ARN missing fields.
fn refused_certificate_arns() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a value that is no ARN at all", "not-a-well-formed-acm-arn"),
        ("an empty value", ""),
        (
            "a value carrying the prefix only",
            "arn:aws:acm:us-east-1:000000000000:",
        ),
        (
            "an ARN missing the resource field",
            "arn:aws:acm:us-east-1:000000000000",
        ),
        (
            "an ARN of another vendor",
            "arn:aws:iam::000000000000:role/certificates",
        ),
        (
            "an ARN of another Certificate Manager resource type",
            "arn:aws:acm:us-east-1:000000000000:certificate-authority/ca-1",
        ),
        (
            "an ARN of the private CA service",
            "arn:aws:acm-pca:us-east-1:000000000000:certificate-authority/ca-2",
        ),
    ]
}

/// The three members of a domain name that name a certificate, with the endpoint
/// configuration each belongs to: the edge-optimized and private endpoints present the
/// `certificate*` pair, the regional endpoint the `regionalCertificate*` pair.
fn certificate_members() -> Vec<(&'static str, Value)> {
    vec![
        ("certificateArn", json!({ "types": ["EDGE"] })),
        (
            "regionalCertificateArn",
            json!({ "types": ["REGIONAL"] }),
        ),
        (
            "ownershipVerificationCertificateArn",
            json!({ "types": ["REGIONAL"] }),
        ),
    ]
}

#[tokio::test]
async fn a_certificate_member_takes_only_a_certificate_manager_certificate_arn() {
    let gateway = Gateway::new();

    for (member, endpoint) in certificate_members() {
        // A value that IS the ARN of a certificate is stored and reported back, whatever
        // the partition, Region, account or certificate name it carries.
        for (index, (case, value)) in admitted_certificate_arns().into_iter().enumerate() {
            let case = format!("{member}: {case}");
            let domain_name = format!("ok-{index}-{}.example.com", member.to_ascii_lowercase());
            let created = create_domain(
                &gateway,
                json!({
                    "domainName": domain_name,
                    "endpointConfiguration": endpoint,
                    member: value,
                }),
            )
            .await;
            assert_eq!(
                created.status, 201,
                "case {case}: expected 201 from CreateDomainName, got {} with {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.body[member].as_str(),
                Some(value.as_str()),
                "case {case}: expected the create to report the certificate it stored, got {:?}",
                created.body[member]
            );

            // Read back through `GetDomainName`: what the caller was told is what the
            // record holds.
            let read = get_domain(&gateway, &domain_name).await;
            assert_eq!(
                read.body[member].as_str(),
                Some(value.as_str()),
                "case {case}: expected the stored certificate, got {:?}",
                read.body[member]
            );
        }

        // A value that is NOT the ARN of a certificate is refused with the service's fixed
        // unreadable-ARN sentence, and nothing is stored under the name it was sent for.
        for (index, (case, value)) in refused_certificate_arns().into_iter().enumerate() {
            let case = format!("{member}: {case}");
            let domain_name = format!("bad-{index}-{}.example.com", member.to_ascii_lowercase());
            let refused = create_domain(
                &gateway,
                json!({
                    "domainName": domain_name,
                    "endpointConfiguration": endpoint,
                    member: value,
                }),
            )
            .await;
            assert_bad_request(&refused, INVALID_ARN, &case);

            let read = get_domain(&gateway, &domain_name).await;
            assert_eq!(
                read.status, 404,
                "case {case}: a refused create must store nothing, got {} with {:?}",
                read.status, read.body
            );
        }
    }
}

#[tokio::test]
async fn a_patched_certificate_member_is_read_by_the_rule_the_create_reads_it_by() {
    let gateway = Gateway::new();
    let domain_name = "patched-certificate.example.com";
    let settled = "arn:aws:acm:us-west-2:000000000000:certificate/settled";
    created_domain(
        &gateway,
        domain_name,
        json!({
            "endpointConfiguration": { "types": ["REGIONAL"] },
            "regionalCertificateArn": settled,
        }),
    )
    .await;

    for (member, _) in certificate_members() {
        let path = format!("/{member}");
        // What the record carries on this member before any refused patch: a refusal must
        // leave exactly that.
        let before = get_domain(&gateway, domain_name).await.body[member].clone();
        for (case, value) in refused_certificate_arns() {
            let case = format!("patch {member}: {case}");
            let refused = patch_domain(&gateway, domain_name, &path, value).await;
            assert_bad_request(&refused, INVALID_ARN, &case);

            let read = get_domain(&gateway, domain_name).await;
            assert_eq!(
                read.body[member], before,
                "case {case}: a refused patch must change nothing, got {:?}",
                read.body[member]
            );
        }

        // A value that IS a certificate ARN is applied and read back.
        let renewed = format!("arn:aws:acm:us-west-2:000000000000:certificate/{member}");
        let patched = patch_domain(&gateway, domain_name, &path, &renewed).await;
        assert_eq!(
            patched.status, 200,
            "case patch {member}: expected 200 from UpdateDomainName, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            patched.body[member].as_str(),
            Some(renewed.as_str()),
            "case patch {member}: expected the patched certificate, got {:?}",
            patched.body[member]
        );
        let read = get_domain(&gateway, domain_name).await;
        assert_eq!(
            read.body[member].as_str(),
            Some(renewed.as_str()),
            "case patch {member}: expected the stored certificate, got {:?}",
            read.body[member]
        );
    }
}

/// One base-path row: the case, the value the request carries, whether a base path
/// mapping may be stored under it, and whether `GetBasePathMapping`'s URI can address the
/// stored record by it (a value carrying a `/` is several path labels, and one carrying a
/// percent escape or a sub-delimiter is read by the URI's own grammar rather than
/// verbatim, so those records are read out of the collection instead).
struct BasePathRow {
    case: &'static str,
    value: &'static str,
    admitted: bool,
    addressed: bool,
}

#[tokio::test]
async fn a_base_path_is_admitted_only_inside_the_character_class_api_gateway_states() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "base-path-admission", "prod").await;

    // The rows differ along what the rule generalises over: which characters the value
    // carries — each one the class admits, and one past it for every ASCII character
    // outside it — and which script it is written in. The class is the one the refusal
    // itself states (`INVALID_BASE_PATH`, re-exported as `BASE_PATH_CHARACTERS`).
    let rows = [
        BasePathRow {
            case: "a plain base path",
            value: "v1",
            admitted: true,
            addressed: true,
        },
        BasePathRow {
            case: "the documented (none) spelling",
            value: NONE_BASE_PATH,
            admitted: true,
            addressed: true,
        },
        BasePathRow {
            case: "letters, numbers and the quiet punctuation",
            value: "Orders-2_beta.v3",
            admitted: true,
            addressed: true,
        },
        BasePathRow {
            case: "every character the class admits beside letters and numbers",
            value: "v1$-_.+!*'(),x",
            admitted: true,
            addressed: false,
        },
        BasePathRow {
            case: "a non-ASCII base path",
            value: "café-日本語",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "a base path of one non-ASCII character",
            value: "ü",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "a base path carrying a space",
            value: "v1 orders",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "a multi-level base path, whose separator the class does not admit",
            value: "v1/orders",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "an unreserved URL character the class does not admit",
            value: "v1~x",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "a base path carrying a query delimiter",
            value: "v1?stage=prod",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "a base path carrying a percent escape",
            value: "v1%2Forders",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "a base path carrying a fragment delimiter",
            value: "v1#orders",
            admitted: false,
            addressed: false,
        },
        BasePathRow {
            case: "a base path carrying a character no URL carries",
            value: "v1<orders>",
            admitted: false,
            addressed: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let domain_name = format!("admission-{index}.example.com");
        created_domain(&gateway, &domain_name, json!({})).await;

        let answer = create_mapping(
            &gateway,
            &domain_name,
            json!({ "basePath": row.value, "restApiId": api, "stage": stage }),
        )
        .await;

        if !row.admitted {
            assert_bad_request(&answer, INVALID_BASE_PATH, row.case);
            // Nothing was stored, so the domain name's collection is still empty.
            let listed = gateway
                .call(
                    "GET",
                    &format!("/domainnames/{domain_name}/basepathmappings"),
                    None,
                )
                .await;
            assert_eq!(
                listed.body.get("item"),
                None,
                "case {}: a refused create must store nothing, so the collection reports \
                 no page member, got {:?}",
                row.case,
                listed.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 201,
            "case {}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            mapping_of(&answer.body, row.case),
            (row.value.to_string(), api.clone(), Some(stage.clone())),
            "case {}: expected the create to report the mapping it stored, got {:?}",
            row.case,
            answer.body
        );
        // The stored record is what the domain name's collection reports.
        let listed = gateway
            .call(
                "GET",
                &format!("/domainnames/{domain_name}/basepathmappings"),
                None,
            )
            .await;
        assert_eq!(
            listed.body["item"][0]["basePath"].as_str(),
            Some(row.value),
            "case {}: expected the stored base path in the collection, got {:?}",
            row.case,
            listed.body
        );
        // And addressed by that base path, for every spelling `GetBasePathMapping`'s own
        // URI can carry.
        if row.addressed {
            let read = get_mapping(&gateway, &domain_name, row.value).await;
            assert_eq!(
                read.status, 200,
                "case {}: expected 200 from GetBasePathMapping, got {} with {:?}",
                row.case, read.status, read.body
            );
            assert_eq!(
                mapping_of(&read.body, row.case),
                (row.value.to_string(), api.clone(), Some(stage.clone())),
                "case {}: expected the stored mapping, got {:?}",
                row.case,
                read.body
            );
        }
    }

    // The table spans the class the refusal states: every character the class admits is
    // carried by a value that was admitted, and the refusal states every one of them.
    for character in BASE_PATH_CHARACTERS.chars() {
        assert!(
            rows.iter()
                .any(|row| row.admitted && row.value.contains(character)),
            "case the character class: {character:?} is admitted, so a row must carry it"
        );
        assert!(
            INVALID_BASE_PATH.contains(character),
            "case the character class: the refusal must state {character:?}, got \
             {INVALID_BASE_PATH:?}"
        );
    }

    // The patch reads a base path by the same rule, so a mapping cannot be moved onto a
    // base path a create would have refused.
    let domain_name = "patched-path.example.com";
    created_domain(&gateway, domain_name, json!({})).await;
    create_mapping(
        &gateway,
        domain_name,
        json!({ "basePath": "v1", "restApiId": api, "stage": stage }),
    )
    .await;
    for row in rows.iter().filter(|row| !row.admitted) {
        let case = format!("patch onto {}", row.case);
        let refused = patch_mapping(&gateway, domain_name, "v1", row.value).await;
        assert_bad_request(&refused, INVALID_BASE_PATH, &case);
        let read = get_mapping(&gateway, domain_name, "v1").await;
        assert_eq!(
            read.status, 200,
            "case {case}: a refused patch must leave the mapping where it was, got {} with {:?}",
            read.status, read.body
        );
    }
}

/// One quota row: the case, the period the quota names (`None` for a quota that names
/// none), the offset it carries, and whether the plan may be stored.
struct QuotaRow {
    case: String,
    period: Option<&'static str>,
    offset: i32,
    admitted: bool,
}

/// The rows the window rule generalises over: every period the model declares, at the
/// published top of its window and one past it, plus the zero every window admits and a
/// quota that names no period at all.
fn quota_rows() -> Vec<QuotaRow> {
    let mut rows = Vec::new();
    for period in QUOTA_PERIOD_TYPE_VALUES {
        let max = max_quota_offset(period).unwrap_or_else(|| {
            panic!("case {period}: every modelled period gives the offset a window")
        });
        rows.push(QuotaRow {
            case: format!("{period}: a zero offset"),
            period: Some(period),
            offset: 0,
            admitted: true,
        });
        rows.push(QuotaRow {
            case: format!("{period}: the top of the window"),
            period: Some(period),
            offset: max,
            admitted: true,
        });
        rows.push(QuotaRow {
            case: format!("{period}: one past the top of the window"),
            period: Some(period),
            offset: max + 1,
            admitted: false,
        });
        rows.push(QuotaRow {
            case: format!("{period}: far past the top of the window"),
            period: Some(period),
            offset: i32::MAX,
            admitted: false,
        });
    }
    rows.push(QuotaRow {
        case: "a quota that names no period".to_string(),
        period: None,
        offset: 5,
        admitted: true,
    });
    rows
}

#[tokio::test]
async fn a_quota_offset_falls_inside_the_window_its_period_gives_it() {
    let gateway = Gateway::new();

    for (index, row) in quota_rows().into_iter().enumerate() {
        let mut quota = json!({ "limit": 500, "offset": row.offset });
        if let Some(period) = row.period {
            quota["period"] = json!(period);
        }
        let answer = create_usage_plan(
            &gateway,
            json!({ "name": format!("plan-{index}"), "quota": quota }),
        )
        .await;

        if !row.admitted {
            let period = row.period.unwrap_or_default();
            let max = max_quota_offset(period).unwrap_or_default();
            assert_bad_request(&answer, &invalid_quota_offset(period, max), &row.case);
            continue;
        }

        assert_eq!(
            answer.status, 201,
            "case {}: expected 201 from CreateUsagePlan, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            answer.body["quota"]["offset"].as_i64(),
            Some(i64::from(row.offset)),
            "case {}: expected the create to report the quota it stored, got {:?}",
            row.case,
            answer.body["quota"]
        );

        // Read back: the admitted quota is what the record holds.
        let id = string_member(&answer.body, "id", &row.case);
        let read = get_usage_plan(&gateway, &id).await;
        assert_eq!(
            read.body["quota"], answer.body["quota"],
            "case {}: expected the stored quota, got {:?}",
            row.case, read.body["quota"]
        );
    }
}

#[tokio::test]
async fn a_patched_quota_is_read_by_the_window_rule_the_create_reads_it_by() {
    let gateway = Gateway::new();
    let created = create_usage_plan(
        &gateway,
        json!({
            "name": "plan-patched-quota",
            "quota": { "limit": 500, "offset": 0, "period": "MONTH" },
        }),
    )
    .await;
    let id = string_member(&created.body, "id", "the plan every row patches");

    // Patching the OFFSET past the window its period gives it is refused, and so is
    // patching the PERIOD to one whose window the stored offset is already outside: the
    // rule is about the pair, so it is read whichever member of it the patch names.
    for (case, path, value, refused) in [
        ("an offset inside the window", "/quota/offset", "27", false),
        (
            "an offset one past the window",
            "/quota/offset",
            "28",
            true,
        ),
        (
            "a period whose window the stored offset is outside",
            "/quota/period",
            "DAY",
            true,
        ),
        ("an offset back to zero", "/quota/offset", "0", false),
        ("the narrowest period, with a zero offset", "/quota/period", "DAY", false),
    ] {
        let answer = patch_usage_plan(&gateway, &id, path, value).await;
        if refused {
            assert_failure(&answer, BAD_REQUEST.0, BAD_REQUEST.1, case);
            continue;
        }
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from UpdateUsagePlan, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let read = get_usage_plan(&gateway, &id).await;
    assert_eq!(
        read.body["quota"],
        json!({ "limit": 500, "offset": 0, "period": "DAY" }),
        "case the plan every row patches: expected the quota the applied patches left, got {:?}",
        read.body["quota"]
    );
}

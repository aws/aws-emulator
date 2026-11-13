// SPDX-License-Identifier: CC0-1.0

//! The `Service` lifecycle, end to end over HTTP: `CreateService` -> `GetService` ->
//! `ListServices` -> `UpdateService` -> `DeleteService`.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch`), so the modelled
//! constraints, the restJson1 (de)serializers and the HTTP status codes are all exercised
//! exactly as a client would exercise them.
//!
//! The suites are TABLES: each row is one input of the class the rule covers (name
//! length, Region count, tag count, page size, filter, patch shape), not one recorded
//! request.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`) and the Region ARNs are minted in. Signature verification is
/// off for these tests, so the value only has to parse.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY00000000/20260101/us-east-1/resiliencehub/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Longer than the (chosen) span a clock-owed status change waits for, so a read after
/// this has materialised it.
const PAST_THE_TRANSIENT_SPAN: Duration = Duration::from_millis(900);

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService::new(engine),
    );
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_signed_for(router, "us-east-1", method, uri, body).await
}

async fn call_signed_for(
    router: &MultiServiceRouter,
    scope_region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let authorization = if scope_region == "us-east-1" {
        SIGNED.to_string()
    } else {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{scope_region}\
             /resiliencehub/aws4_request, SignedHeaders=host;x-amz-date, \
             Signature=0123456789abcdef"
        )
    };
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/v2/create-service", Some(body)).await
}

async fn get(router: &MultiServiceRouter, service_arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v2/get-service?serviceArn={service_arn}"),
        None,
    )
    .await
}

async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        "/v2/list-services".to_string()
    } else {
        format!("/v2/list-services?{query}")
    };
    call(router, "GET", &uri, None).await
}

/// The simplest body that satisfies `CreateService`'s required members.
fn minimal(name: &str) -> Value {
    json!({
        "name": name,
        "regions": ["us-east-1"],
        "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
    })
}

/// Create `name` and return its ARN, failing the test if the call did not succeed.
async fn create_named(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = create(router, minimal(name)).await;
    assert_eq!(status, 200, "creating {name}: got {body:?}");
    service_arn(&body)
}

fn service_arn(body: &Value) -> String {
    body["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no service.serviceArn in {body:?}"))
        .to_string()
}

fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn summary_names(body: &Value) -> Vec<String> {
    body["serviceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no serviceSummaries array in {body:?}"))
        .iter()
        .map(|summary| summary["name"].as_str().unwrap_or("<unnamed>").to_string())
        .collect()
}

/// Whether `candidate` is a UUID in RFC 9562's canonical 8-4-4-4-12 textual form, with a
/// version nibble in 1..=8 and a variant nibble in `[89ab]`.
fn is_canonical_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    if [8usize, 4, 4, 4, 12]
        .iter()
        .zip(&groups)
        .any(|(expected, group)| group.len() != *expected)
    {
        return false;
    }
    if !candidate
        .chars()
        .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return false;
    }
    let version = groups[2].chars().next().unwrap_or('0');
    let variant = groups[3].chars().next().unwrap_or('0');
    ('1'..='8').contains(&version) && "89ab".contains(variant)
}

// ---------------------------------------------------------------------------
// CreateService + GetService
// ---------------------------------------------------------------------------

/// RULE: a create persists the record it minted, and a read of that record returns the
/// same body — for every shape of input the model admits, not one of them.
///
/// The rows differ along what the rule generalises over: name length (the published 2-
/// and 60-character bounds), Region count (1 and the published maximum of 5), description
/// length (the published 615-character maximum), tag count, `associatedSystems` count,
/// and which optional members are present at all.
#[tokio::test]
async fn a_created_service_is_stored_and_reads_back_identically() {
    let sixty_char_name = "a".repeat(60);
    let longest_description = "d".repeat(615);
    let cases: Vec<(&str, Value)> = vec![
        ("required members only, 2-character name", minimal("ab")),
        (
            "60-character name, 5 Regions, 615-character description",
            json!({
                "name": sixty_char_name,
                "regions": ["us-east-1", "us-west-2", "eu-west-1", "ap-south-1", "sa-east-1"],
                "description": longest_description,
                "permissionModel": {"invokerRoleName": "Role-With.Chars_1"},
            }),
        ),
        (
            "tags, policyArn and kmsKeyId",
            json!({
                "name": "tagged-svc",
                "regions": ["eu-central-1"],
                "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
                "policyArn": format!("arn:aws:resiliencehub:eu-central-1:{ACCOUNT}:policy/p-1"),
                "kmsKeyId": "alias/resilience",
                "tags": {"env": "dev", "team": "payments", "tier": "1"},
            }),
        ),
        (
            "two associated systems, one carrying user journeys",
            json!({
                "name": "linked-svc",
                "regions": ["us-west-2", "us-east-2"],
                "permissionModel": {
                    "invokerRoleName": "ResilienceHubRole",
                    "crossAccountRoles": [
                        {"crossAccountRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/peer"),
                         "externalId": "x-1"}
                    ],
                },
                "associatedSystems": [
                    {"systemArn": format!("arn:aws:resiliencehub:us-west-2:{ACCOUNT}:system/s-1"),
                     "systemName": "checkout",
                     "userJourneyIds": ["j-1", "j-2"]},
                    {"systemArn": format!("arn:aws:resiliencehub:us-west-2:{ACCOUNT}:system/s-2")}
                ],
            }),
        ),
        (
            "dependencyDiscovery DISABLED and a report configuration",
            json!({
                "name": "reporting-svc",
                "regions": ["ap-northeast-1"],
                "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
                "dependencyDiscovery": "DISABLED",
                "reportConfiguration": {
                    "reportOutputs": [
                        {"s3": {"bucketPath": "my-bucket/ngrh-reports/", "bucketOwner": ACCOUNT}}
                    ]
                },
            }),
        ),
    ];

    let router = emulator();
    let mut minted = Vec::new();
    for (label, body) in cases {
        let (status, created) = create(&router, body.clone()).await;
        assert_eq!(status, 200, "case {label}: expected 200, got {created:?}");
        let service = &created["service"];
        let arn = service_arn(&created);

        let expected_prefix = format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:service/");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {label}: expected an ARN under {expected_prefix}, got {arn:?}"
        );
        let resource_id = arn.trim_start_matches(&expected_prefix);
        assert!(
            is_canonical_uuid(resource_id),
            "case {label}: expected a canonical UUID resource id, got {resource_id:?}"
        );

        for member in ["name", "regions", "permissionModel"] {
            assert_eq!(
                service[member], body[member],
                "case {label}: {member} must be stored as sent, got {:?}",
                service[member]
            );
        }
        for member in [
            "description",
            "policyArn",
            "kmsKeyId",
            "associatedSystems",
            "reportConfiguration",
        ] {
            if let Some(sent) = body.get(member) {
                assert_eq!(
                    &service[member], sent,
                    "case {label}: {member} must be stored as sent, got {:?}",
                    service[member]
                );
            } else {
                assert!(
                    service[member].is_null(),
                    "case {label}: {member} was not sent, so it must be absent, got {:?}",
                    service[member]
                );
            }
        }
        // A tag map is the SET of tags the service carries, so a service created without
        // tags carries the EMPTY map rather than withholding the member.
        let expected_tags = body.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            service["tags"], expected_tags,
            "case {label}: expected tags {expected_tags:?}, got {:?}",
            service["tags"]
        );
        assert_eq!(
            service["assessmentStatus"], "NOT_STARTED",
            "case {label}: a new service has not been assessed, got {:?}",
            service["assessmentStatus"]
        );
        assert_eq!(
            service["openFindingsCount"], 0,
            "case {label}: expected 0 open findings, got {:?}",
            service["openFindingsCount"]
        );
        assert_eq!(
            service["resolvedFindingsCount"], 0,
            "case {label}: expected 0 resolved findings, got {:?}",
            service["resolvedFindingsCount"]
        );
        assert_eq!(
            service["accountId"], ACCOUNT,
            "case {label}: expected the emulator account, got {:?}",
            service["accountId"]
        );
        assert!(
            service["createdAt"].as_f64().is_some() && service["updatedAt"].as_f64().is_some(),
            "case {label}: createdAt/updatedAt must be set, got {service:?}"
        );

        let (get_status, read) = get(&router, &arn).await;
        assert_eq!(get_status, 200, "case {label}: get got {read:?}");
        assert_eq!(
            read["service"], created["service"],
            "case {label}: GetService must return the stored service unchanged"
        );

        assert!(
            !minted.contains(&arn),
            "case {label}: every create must mint a fresh ARN, but {arn} repeated"
        );
        minted.push(arn);
    }
}

/// RULE: `clientToken` is an `@idempotencyToken` — replaying a create with the same token
/// returns the service the first call created and creates no second one.
#[tokio::test]
async fn replaying_a_client_token_returns_the_first_service() {
    let longest_token = "t".repeat(63);
    let tokens: Vec<&str> = vec!["t", "tok.1-2_3", &longest_token];

    let router = emulator();
    for (index, token) in tokens.iter().enumerate() {
        let name = format!("idem-svc-{index}");
        let mut body = minimal(&name);
        body["clientToken"] = json!(token);

        let (first_status, first) = create(&router, body.clone()).await;
        assert_eq!(
            first_status, 200,
            "case token {token:?}: first create got {first:?}"
        );
        let (replay_status, replay) = create(&router, body).await;
        assert_eq!(
            replay_status, 200,
            "case token {token:?}: replay got {replay:?}"
        );
        assert_eq!(
            replay["service"], first["service"],
            "case token {token:?}: a replay must return the stored service"
        );
    }

    let (_, listed) = list(&router, "").await;
    assert_eq!(
        summary_names(&listed).len(),
        tokens.len(),
        "a replayed token must not create a second service, got {listed:?}"
    );
}

/// RULE: a service name is unique per account — a second create under a taken name is a
/// 409 `ConflictException`, whatever the rest of the request looks like.
#[tokio::test]
async fn a_duplicate_name_conflicts() {
    let router = emulator();
    let cases = ["dup-svc", "Dup_Svc-2", "ab"];
    for name in cases {
        let (status, first) = create(&router, minimal(name)).await;
        assert_eq!(status, 200, "case {name}: first create got {first:?}");

        // Same name, different everything else.
        let mut second = minimal(name);
        second["regions"] = json!(["eu-west-1", "eu-west-2"]);
        second["description"] = json!("a different service that wants the same name");
        let (status, body) = create(&router, second).await;
        assert_eq!(status, 409, "case {name}: expected 409, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {name}: expected ConflictException, got {body:?}"
        );
    }
}

/// RULE: a well-formed ARN that names no stored service is a 404
/// `ResourceNotFoundException` on every operation that resolves one.
#[tokio::test]
async fn an_unknown_service_arn_is_not_found() {
    let router = emulator();
    // A service that DOES exist, so the store is not simply empty.
    create_named(&router, "present-svc").await;

    let unknown = [
        format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:service/11111111-1111-4111-8111-111111111111"),
        format!("arn:aws-us-gov:resiliencehub:us-gov-east-1:{ACCOUNT}:service/other"),
        format!("arn:aws-cn:resiliencehub:cn-north-1:{ACCOUNT}:service/other"),
        format!("arn:aws:someotherservice:eu-west-1:{ACCOUNT}:thing/not-a-service"),
    ];
    for arn in unknown {
        let probes: Vec<(&str, &str, Option<Value>)> = vec![
            ("GetService", "", None),
            (
                "UpdateService",
                "/v2/update-service",
                Some(json!({"serviceArn": arn, "description": "x"})),
            ),
            (
                "DeleteService",
                "/v2/delete-service",
                Some(json!({"serviceArn": arn})),
            ),
        ];
        for (operation, uri, body) in probes {
            let (status, response) = if operation == "GetService" {
                get(&router, &arn).await
            } else {
                call(&router, "POST", uri, body).await
            };
            assert_eq!(
                status, 404,
                "case {operation} {arn}: expected 404, got {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {operation} {arn}: expected ResourceNotFoundException, got {response:?}"
            );
        }
    }
}

/// RULE: a `serviceArn` that is not an ARN at all fails the member's modelled `length`
/// and `pattern`, so it is refused as a 400 `ValidationException` — not reported as a
/// missing resource.
#[tokio::test]
async fn a_malformed_service_arn_is_refused() {
    let router = emulator();
    let malformed = [
        ("shorter than the 31-character minimum", "not-an-arn"),
        (
            "Region segment is not a Region",
            "arn:aws:resiliencehub:not_a_region:000000000000:service/x",
        ),
        (
            "account segment is not 12 digits",
            "arn:aws:resiliencehub:us-east-1:12345:service/x",
        ),
    ];
    for (label, arn) in malformed {
        let (status, body) = get(&router, arn).await;
        assert_eq!(status, 400, "case {label}: expected 400, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected ValidationException, got {body:?}"
        );
    }
}

/// RULE: a minted ARN carries the Region of the request's SigV4 credential scope when
/// that Region has `AwsRegion`'s modelled form (`^[a-z]{2}((-iso[a-z]?)|(-gov))?-[a-z]+-[0-9]$`),
/// and `us-east-1` otherwise — so every ARN this service mints still satisfies the `Arn`
/// pattern its own readers validate their input against.
///
/// The rows cover each shape the pattern admits (plain, `gov`, `iso`, `iso<x>`) and
/// several it does not (a non-Region, too many segments, a too-short prefix).
#[tokio::test]
async fn a_minted_arn_takes_a_well_formed_scope_region() {
    let cases = [
        ("us-east-1", "us-east-1"),
        ("eu-west-3", "eu-west-3"),
        ("us-gov-east-1", "us-gov-east-1"),
        ("us-iso-east-1", "us-iso-east-1"),
        ("us-isob-east-1", "us-isob-east-1"),
        // Not an AwsRegion: fall back rather than mint an unreadable ARN.
        ("not_a_region", "us-east-1"),
        ("us-a-b-c-1", "us-east-1"),
        ("us-iso-foo-bar-1", "us-east-1"),
        ("u-east-1", "us-east-1"),
        ("us-east-12", "us-east-1"),
    ];

    for (index, (scope_region, expected_region)) in cases.iter().enumerate() {
        let router = emulator();
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            "POST",
            "/v2/create-service",
            Some(minimal(&format!("region-svc-{index}"))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case scope {scope_region}: expected 200, got {created:?}"
        );
        let arn = service_arn(&created);
        let expected_prefix = format!("arn:aws:resiliencehub:{expected_region}:{ACCOUNT}:service/");
        assert!(
            arn.starts_with(&expected_prefix),
            "case scope {scope_region}: expected an ARN under {expected_prefix}, got {arn:?}"
        );
        // And the ARN it minted is one its own readers accept.
        let (get_status, read) = call_signed_for(
            &router,
            scope_region,
            "GET",
            &format!("/v2/get-service?serviceArn={arn}"),
            None,
        )
        .await;
        assert_eq!(
            get_status, 200,
            "case scope {scope_region}: a minted ARN must be readable, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListServices
// ---------------------------------------------------------------------------

/// RULE: an empty store lists nothing, and says so with an empty list.
#[tokio::test]
async fn an_empty_store_lists_no_services() {
    let router = emulator();
    let (status, body) = list(&router, "").await;
    assert_eq!(status, 200, "expected 200, got {body:?}");
    assert_eq!(
        body["serviceSummaries"],
        json!([]),
        "an empty store must answer an empty list, got {body:?}"
    );
    assert!(
        body["nextToken"].is_null(),
        "an empty page has no nextToken, got {body:?}"
    );
}

/// RULE: `ListServices` is `@paginated` — successive pages of `maxResults` summaries,
/// resumed by the previous page's `nextToken`, enumerate every stored service exactly
/// once in creation order, and only the last page omits the token.
///
/// The rows exhaust every page size from 1 to one past the number of stored services,
/// plus the published `maxResults` maximum of 100 — the whole class, not a sample.
#[tokio::test]
async fn pagination_walks_every_service_exactly_once() {
    let router = emulator();
    let created: Vec<String> = ["svc-1", "svc-2", "svc-3", "svc-4", "svc-5"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    for name in &created {
        create_named(&router, name).await;
    }

    for page_size in [1usize, 2, 3, 4, 5, 6, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = format!("maxResults={page_size}");
            if let Some(cursor) = &token {
                query.push_str(&format!("&nextToken={cursor}"));
            }
            let (status, body) = list(&router, &query).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: page {pages} got {body:?}"
            );
            let names = summary_names(&body);
            let remaining = created.len() - seen.len();
            assert_eq!(
                names.len(),
                page_size.min(remaining),
                "case maxResults={page_size}: page {pages} must be full while services \
                 remain, got {names:?}"
            );
            for name in &names {
                assert!(
                    !seen.contains(name),
                    "case maxResults={page_size}: {name} appeared on two pages"
                );
            }
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case maxResults={page_size}: pagination did not terminate"
            );
            match body["nextToken"].as_str() {
                Some(cursor) => {
                    assert!(
                        seen.len() < created.len(),
                        "case maxResults={page_size}: a token was issued after the last \
                         service, got {body:?}"
                    );
                    token = Some(cursor.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case maxResults={page_size}: the pages must enumerate every service in \
             creation order"
        );
    }
}

/// RULE: `maxResults` is modelled `range 1..100`, so each published bound is accepted and
/// a value one past either bound is refused as a 400 `ValidationException`.
#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let router = emulator();
    create_named(&router, "bounded-svc").await;

    for accepted in [1, 100] {
        let (status, body) = list(&router, &format!("maxResults={accepted}")).await;
        assert_eq!(
            status, 200,
            "case maxResults={accepted}: expected 200, got {body:?}"
        );
    }
    for refused in [0, 101] {
        let (status, body) = list(&router, &format!("maxResults={refused}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={refused}: expected 400, got {body:?}"
        );
    }
}

/// RULE: a `nextToken` names a POSITION in the creation order, so a page resumes the same
/// scan even when the record the previous page ended on has since been deleted; and a
/// token this service never issued is a 400, not an empty page.
#[tokio::test]
async fn a_page_token_survives_a_delete_and_rejects_a_forged_token() {
    let router = emulator();
    let mut arns = Vec::new();
    for name in ["p-1", "p-2", "p-3", "p-4"] {
        arns.push(create_named(&router, name).await);
    }

    let (_, first_page) = list(&router, "maxResults=2").await;
    assert_eq!(
        summary_names(&first_page),
        vec!["p-1", "p-2"],
        "the first page must be the first two services, got {first_page:?}"
    );
    let token = first_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("no nextToken in {first_page:?}"))
        .to_string();

    // Delete a service from the page already returned, then resume.
    let (status, deleted) = call(
        &router,
        "POST",
        "/v2/delete-service",
        Some(json!({"serviceArn": arns[1]})),
    )
    .await;
    assert_eq!(status, 200, "deleting p-2 got {deleted:?}");

    let (status, resumed) = list(&router, &format!("maxResults=2&nextToken={token}")).await;
    assert_eq!(status, 200, "resuming got {resumed:?}");
    assert_eq!(
        summary_names(&resumed),
        vec!["p-3", "p-4"],
        "a token must resume the same scan after a delete, got {resumed:?}"
    );

    for forged in [
        // Not hex of a decimal position at all.
        "zzzz",
        "0",
        "not-a-token",
        // Well-formed, but naming a position this store never handed out: hex of "0"
        // (positions start at 1) and hex of "999".
        "30",
        "393939",
    ] {
        let (status, body) = list(&router, &format!("nextToken={forged}")).await;
        assert_eq!(
            status, 400,
            "case nextToken={forged}: expected 400, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case nextToken={forged}: expected ValidationException, got {body:?}"
        );
    }
}

/// RULE: every modelled `ListServices` filter is an equality test against the stored
/// record, and several filters apply together; an absent filter constrains nothing.
///
/// The rows cover each of the six filters, including `assessmentStatus` values the issue
/// never cited (taken from `AssessmentStatus`' modelled value list) and a combination.
#[tokio::test]
async fn every_filter_selects_by_the_stored_value() {
    let router = emulator();
    let policy_one = format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:policy/p-one");
    let policy_two = format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:policy/p-two");
    let system_one = format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:system/s-one");
    let system_two = format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:system/s-two");

    let mut first = minimal("filter-a");
    first["policyArn"] = json!(policy_one);
    first["associatedSystems"] = json!([
        {"systemArn": system_one, "userJourneyIds": ["j-one", "j-two"]}
    ]);
    let (status, body) = create(&router, first).await;
    assert_eq!(status, 200, "creating filter-a got {body:?}");

    let mut second = minimal("filter-b");
    second["policyArn"] = json!(policy_two);
    second["associatedSystems"] = json!([{"systemArn": system_two}]);
    let (status, body) = create(&router, second).await;
    assert_eq!(status, 200, "creating filter-b got {body:?}");

    create_named(&router, "filter-c").await;

    let all = vec!["filter-a", "filter-b", "filter-c"];
    let cases: Vec<(String, Vec<&str>)> = vec![
        (String::new(), all.clone()),
        (format!("policyArn={policy_one}"), vec!["filter-a"]),
        (format!("policyArn={policy_two}"), vec!["filter-b"]),
        (
            format!("policyArn=arn:aws:resiliencehub:us-east-1:{ACCOUNT}:policy/absent"),
            vec![],
        ),
        ("assessmentStatus=NOT_STARTED".to_string(), all.clone()),
        ("assessmentStatus=IN_PROGRESS".to_string(), vec![]),
        ("assessmentStatus=SUCCESS".to_string(), vec![]),
        ("assessmentStatus=PENDING".to_string(), vec![]),
        ("assessmentStatus=FAILED".to_string(), vec![]),
        (format!("accountId={ACCOUNT}"), all.clone()),
        ("accountId=111111111111".to_string(), vec![]),
        (format!("systemArn={system_one}"), vec!["filter-a"]),
        (format!("systemArn={system_two}"), vec!["filter-b"]),
        ("userJourneyId=j-two".to_string(), vec!["filter-a"]),
        ("userJourneyId=j-absent".to_string(), vec![]),
        ("ouId=ou-abcd-12345678".to_string(), vec![]),
        (
            format!("policyArn={policy_one}&assessmentStatus=NOT_STARTED"),
            vec!["filter-a"],
        ),
        (
            format!("policyArn={policy_one}&assessmentStatus=SUCCESS"),
            vec![],
        ),
    ];

    for (query, expected) in cases {
        let (status, body) = list(&router, &query).await;
        assert_eq!(status, 200, "case {query:?}: expected 200, got {body:?}");
        assert_eq!(
            summary_names(&body),
            expected,
            "case {query:?}: wrong services selected, got {body:?}"
        );
    }
}

/// RULE: a `ServiceSummary` projects the SAME stored record the full `Service` does.
#[tokio::test]
async fn a_summary_projects_the_stored_record() {
    let router = emulator();
    let mut body = minimal("summary-svc");
    body["policyArn"] = json!(format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:policy/p"));
    body["regions"] = json!(["us-east-1", "us-west-1"]);
    body["associatedSystems"] =
        json!([{"systemArn": format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:system/s")}]);
    let (_, created) = create(&router, body).await;
    let service = &created["service"];

    let (_, listed) = list(&router, "").await;
    let summary = &listed["serviceSummaries"][0];
    for member in [
        "serviceArn",
        "name",
        "regions",
        "policyArn",
        "associatedSystems",
        "assessmentStatus",
        "openFindingsCount",
        "resolvedFindingsCount",
        "accountId",
        "createdAt",
        "updatedAt",
    ] {
        assert_eq!(
            summary[member], service[member],
            "case {member}: the summary must project the stored value, got {:?} vs {:?}",
            summary[member], service[member]
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateService
// ---------------------------------------------------------------------------

/// RULE: `UpdateService` applies each PRESENT member to the stored record, leaves every
/// absent member untouched, bumps `updatedAt`, and the merged record is what every later
/// read returns.
///
/// Each row patches a FRESH fully-populated service, so the rows differ only in which
/// members the patch carries — one member, a different member, a list member, a structure
/// member, and two at once.
#[tokio::test]
async fn an_update_applies_only_the_members_it_carries() {
    let policy_two = format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:policy/updated");
    let system_two = format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:system/updated");
    let patches: Vec<(&str, Value)> = vec![
        ("description only", json!({"description": "updated"})),
        (
            "regions only",
            json!({"regions": ["us-east-1", "us-west-2", "eu-west-1"]}),
        ),
        ("policyArn only", json!({"policyArn": policy_two})),
        (
            "associatedSystems only",
            json!({"associatedSystems": [{"systemArn": system_two, "systemName": "renamed"}]}),
        ),
        (
            "permissionModel only",
            json!({"permissionModel": {"invokerRoleName": "AnotherRole"}}),
        ),
        (
            "reportConfiguration only",
            json!({"reportConfiguration": {"reportOutputs": [
                {"s3": {"bucketPath": "other-bucket/reports/", "bucketOwner": ACCOUNT}}
            ]}}),
        ),
        (
            "description and regions together",
            json!({"description": "both", "regions": ["ap-south-1"]}),
        ),
    ];

    for (index, (label, patch)) in patches.into_iter().enumerate() {
        let router = emulator();
        let mut body = minimal(&format!("patch-svc-{index}"));
        body["description"] = json!("original");
        body["regions"] = json!(["us-east-1", "us-east-2"]);
        body["policyArn"] = json!(format!(
            "arn:aws:resiliencehub:us-east-1:{ACCOUNT}:policy/original"
        ));
        body["tags"] = json!({"env": "prod"});
        body["associatedSystems"] = json!([
            {"systemArn": format!("arn:aws:resiliencehub:us-east-1:{ACCOUNT}:system/original")}
        ]);
        let (status, created) = create(&router, body).await;
        assert_eq!(status, 200, "case {label}: create got {created:?}");
        let original = created["service"].clone();
        let arn = service_arn(&created);

        // A real interval, so a bumped `updatedAt` is observably later.
        tokio::time::sleep(Duration::from_millis(5)).await;

        let mut request = patch.clone();
        request["serviceArn"] = json!(arn);
        let (status, updated) = call(&router, "POST", "/v2/update-service", Some(request)).await;
        assert_eq!(status, 200, "case {label}: update got {updated:?}");
        let merged = &updated["service"];

        let patched = patch.as_object().expect("a patch object");
        for (member, sent) in patched {
            assert_eq!(
                &merged[member], sent,
                "case {label}: {member} must take the sent value, got {:?}",
                merged[member]
            );
        }
        for member in [
            "name",
            "description",
            "regions",
            "policyArn",
            "tags",
            "associatedSystems",
            "permissionModel",
            "reportConfiguration",
            "assessmentStatus",
            "createdAt",
        ] {
            if patched.contains_key(member) {
                continue;
            }
            assert_eq!(
                merged[member], original[member],
                "case {label}: {member} was not sent, so it must be untouched, got {:?}",
                merged[member]
            );
        }
        let created_at = original["createdAt"].as_f64().expect("createdAt");
        let updated_at = merged["updatedAt"].as_f64().expect("updatedAt");
        assert!(
            updated_at > created_at,
            "case {label}: updatedAt must be bumped past createdAt, got {updated_at} vs \
             {created_at}"
        );

        let (_, read) = get(&router, &arn).await;
        assert_eq!(
            read["service"], updated["service"],
            "case {label}: a later read must return the merged record"
        );
        let (_, listed) = list(&router, "").await;
        assert_eq!(
            listed["serviceSummaries"][0]["updatedAt"], merged["updatedAt"],
            "case {label}: the listing must show the merged record, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteService
// ---------------------------------------------------------------------------

/// RULE: a delete removes the record, then echoes its ARN; the service is afterwards
/// absent from every reader, and its name is free again.
#[tokio::test]
async fn a_deleted_service_disappears_from_every_reader() {
    let router = emulator();
    let mut arns = Vec::new();
    for name in ["del-1", "del-2", "del-3"] {
        arns.push(create_named(&router, name).await);
    }

    for (index, name) in ["del-2", "del-1", "del-3"].iter().enumerate() {
        let arn = match *name {
            "del-1" => &arns[0],
            "del-2" => &arns[1],
            _ => &arns[2],
        };
        let (status, deleted) = call(
            &router,
            "POST",
            "/v2/delete-service",
            Some(json!({"serviceArn": arn})),
        )
        .await;
        assert_eq!(status, 200, "case {name}: expected 200, got {deleted:?}");
        assert_eq!(
            deleted["serviceArn"], json!(arn),
            "case {name}: the delete must echo the ARN, got {deleted:?}"
        );

        let (status, read) = get(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {name}: a deleted service must be gone, got {read:?}"
        );

        let (_, listed) = list(&router, "").await;
        assert!(
            !summary_names(&listed).contains(&(*name).to_string()),
            "case {name}: a deleted service must not be listed, got {listed:?}"
        );
        assert_eq!(
            summary_names(&listed).len(),
            arns.len() - index - 1,
            "case {name}: wrong number of services left, got {listed:?}"
        );

        let (status, again) = call(
            &router,
            "POST",
            "/v2/delete-service",
            Some(json!({"serviceArn": arn})),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name}: deleting twice must be a 404, got {again:?}"
        );
    }

    // The names are free again.
    let (status, recreated) = create(&router, minimal("del-2")).await;
    assert_eq!(
        status, 200,
        "a deleted name must be reusable, got {recreated:?}"
    );
    assert!(
        !arns.contains(&service_arn(&recreated)),
        "a recreated service must get a fresh ARN, got {recreated:?}"
    );
}

// ---------------------------------------------------------------------------
// Status the clock owes
// ---------------------------------------------------------------------------

/// RULE: the store is bounded — at the ceiling a further create is refused with the
/// `ServiceQuotaExceededException` the operation models (HTTP 402), and freeing a slot
/// with `DeleteService` makes room again, so no caller can grow the store without limit.
#[tokio::test]
async fn the_store_is_bounded_by_its_ceiling() {
    use service_com_amazonaws_resiliencehubv2_ngrhservicecore::state::MAX_SERVICES;

    let router = emulator();
    for index in 0..MAX_SERVICES {
        let (status, body) = create(&router, minimal(&format!("bound-svc-{index}"))).await;
        assert_eq!(
            status, 200,
            "case service {index}: a create below the ceiling must succeed, got {body:?}"
        );
    }

    // One past the ceiling, with and without a clientToken.
    for (label, token) in [("no clientToken", None), ("with clientToken", Some("tok"))] {
        let mut body = minimal("bound-svc-over");
        if let Some(token) = token {
            body["clientToken"] = json!(token);
        }
        let (status, refused) = create(&router, body).await;
        assert_eq!(
            status, 402,
            "case {label}: a create past the ceiling must be refused, got {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ServiceQuotaExceededException",
            "case {label}: expected ServiceQuotaExceededException, got {refused:?}"
        );
    }

    // Freeing a slot makes room again.
    let (_, listed) = list(&router, "maxResults=1").await;
    let victim = listed["serviceSummaries"][0]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no summary in {listed:?}"))
        .to_string();
    let (status, deleted) = call(
        &router,
        "POST",
        "/v2/delete-service",
        Some(json!({"serviceArn": victim})),
    )
    .await;
    assert_eq!(status, 200, "freeing a slot got {deleted:?}");
    let (status, created) = create(&router, minimal("bound-svc-over")).await;
    assert_eq!(
        status, 200,
        "a freed slot must be reusable, got {created:?}"
    );
}

/// RULE: a status the service itself has to settle is reported in its transient value
/// until the instant it falls due, and in its settled value from then on — the caller
/// never has to ask for the change, and a read never brings it forward.
///
/// Both rows are such a status: `dependencyDiscovery` enters `INITIALIZING`
/// (`DependencyDiscoveryInput` withholds that value from callers because "INITIALIZING is
/// system-managed") before reaching `ENABLED`, and a DevOps Agent link is `CONNECTING`
/// ("the service is being connected to the Agent Space") before `CONNECTED`. A
/// `DISABLED` discovery settles nothing and must not drift.
#[tokio::test]
async fn a_status_the_clock_owes_settles_on_its_own() {
    let router = emulator();
    let agent_space = format!("arn:aws:aidevops:us-west-2:{ACCOUNT}:agentspace/space-one");

    let mut enabled = minimal("discovery-on");
    enabled["dependencyDiscovery"] = json!("ENABLED");
    let (status, created_enabled) = create(&router, enabled).await;
    assert_eq!(status, 200, "creating discovery-on got {created_enabled:?}");
    let enabled_arn = service_arn(&created_enabled);
    let discovery = &created_enabled["service"]["dependencyDiscovery"];
    assert_eq!(
        discovery["status"], "INITIALIZING",
        "enabling discovery must start it INITIALIZING, got {discovery:?}"
    );
    assert!(
        discovery["message"].as_str().is_some(),
        "the initialization state must carry its status message, got {discovery:?}"
    );

    let mut disabled = minimal("discovery-off");
    disabled["dependencyDiscovery"] = json!("DISABLED");
    let (status, created_disabled) = create(&router, disabled).await;
    assert_eq!(status, 200, "creating discovery-off got {created_disabled:?}");
    let disabled_arn = service_arn(&created_disabled);
    assert_eq!(
        created_disabled["service"]["dependencyDiscovery"]["status"], "DISABLED",
        "DISABLED is terminal and must apply at once, got {created_disabled:?}"
    );

    let mut linked = minimal("agent-linked");
    linked["devOpsAgentConfiguration"] = json!({"agentSpaceArn": agent_space});
    let (status, created_linked) = create(&router, linked).await;
    assert_eq!(status, 200, "creating agent-linked got {created_linked:?}");
    let linked_arn = service_arn(&created_linked);
    let view = &created_linked["service"]["devOpsAgentConfiguration"];
    assert_eq!(
        view["status"], "CONNECTING",
        "a fresh link must start CONNECTING, got {view:?}"
    );
    assert_eq!(
        view["agentSpaceId"], "space-one",
        "the Agent Space id must be parsed from its ARN, got {view:?}"
    );
    assert_eq!(
        view["region"], "us-west-2",
        "the Agent Space Region must be parsed from its ARN, got {view:?}"
    );
    assert!(
        view["connectedAt"].is_null(),
        "nothing has connected yet, so connectedAt must be absent, got {view:?}"
    );

    tokio::time::sleep(PAST_THE_TRANSIENT_SPAN).await;

    let (_, settled) = get(&router, &enabled_arn).await;
    let discovery = &settled["service"]["dependencyDiscovery"];
    assert_eq!(
        discovery["status"], "ENABLED",
        "discovery must settle on ENABLED once due, got {discovery:?}"
    );
    assert!(
        discovery["message"].is_null(),
        "the initialization message belongs to the initialization state only, got \
         {discovery:?}"
    );

    let (_, still_off) = get(&router, &disabled_arn).await;
    assert_eq!(
        still_off["service"]["dependencyDiscovery"]["status"], "DISABLED",
        "a DISABLED discovery must not drift, got {still_off:?}"
    );

    let (_, connected) = get(&router, &linked_arn).await;
    let view = &connected["service"]["devOpsAgentConfiguration"];
    assert_eq!(
        view["status"], "CONNECTED",
        "the link must settle on CONNECTED once due, got {view:?}"
    );
    assert!(
        view["connectedAt"].as_f64().is_some(),
        "a connected link records when it connected, got {view:?}"
    );

    // The listing projects the settled values too.
    let (_, listed) = list(&router, "").await;
    let summaries = listed["serviceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no summaries in {listed:?}"));
    let linked_summary = summaries
        .iter()
        .find(|summary| summary["name"] == json!("agent-linked"))
        .unwrap_or_else(|| panic!("agent-linked missing from {listed:?}"));
    assert_eq!(
        linked_summary["devOpsAgentStatus"], "CONNECTED",
        "the summary must project the settled link status, got {linked_summary:?}"
    );
    assert_eq!(
        linked_summary["devOpsAgentSpaceArn"],
        created_linked["service"]["devOpsAgentConfiguration"]["agentSpaceArn"],
        "the summary must project the stored Agent Space ARN, got {linked_summary:?}"
    );
}

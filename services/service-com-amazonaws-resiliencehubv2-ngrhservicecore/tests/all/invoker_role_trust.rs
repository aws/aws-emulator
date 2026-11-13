// SPDX-License-Identifier: CC0-1.0

//! The invoker role a `permissionModel` names has to be one this service can ASSUME.
//!
//! THE RULE: a service's assessments run under the IAM role
//! `PermissionModel$invokerRoleName` names, so an operation that stores a service with a
//! permission model resolves that role against the account's own IAM records and refuses
//! the call — writing nothing — when the records show the role's trust policy withholding
//! `sts:AssumeRole` from `resiliencehub.amazonaws.com`. The refusal is HTTP 400
//! `ValidationException` carrying
//! `Cannot assume invoker role '<invokerRoleName>'. Ensure the role exists and its trust
//! policy allows access.`, whose one variable part is the value the request sent. The
//! verdict is taken per call and never cached, so an IAM `UpdateAssumeRolePolicy` that
//! repairs the role flips an otherwise identical request from refused to served, and one
//! that breaks it again flips it back.
//!
//! A name the records hold no role under is NOT judged: an invoker role is routinely
//! provisioned out of band, so its absence from this emulator's IAM is evidence about the
//! store rather than about the role.
//!
//! Both services run on ONE engine here, because the records this service reads are the
//! very ones IAM's own `CreateRole` and `UpdateAssumeRolePolicy` wrote: every trust policy
//! below is installed through IAM's wire path, never by reaching into state.
//!
//! The rows span what the rule generalises over: which principal the document admits, how
//! the action and the principal are written (one string, a list, a wildcard, a prefix
//! wildcard, another case), which principal KIND names it (a service principal, an account
//! one), how the document carries its statements (a list, a single object), the effect
//! (an `Allow`, an explicit `Deny`, a `Deny` beside an `Allow`), whether the document is
//! readable as a policy at all, the case and the path the request writes the role name
//! with, and a name IAM holds no role under.
//!
//! A refused call writes NOTHING, so it reserves nothing either: the name it carried is
//! still free once the role is repaired.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one ARNs are minted in.
const REGION: &str = "us-east-1";

/// The error code AWS refuses an unassumable invoker role with.
const VALIDATION: &str = "ValidationException";

/// The principal an invoker role has to trust for this service to assume it.
const SERVICE_PRINCIPAL: &str = "resiliencehub.amazonaws.com";

/// Both services on ONE engine: this service reads the records IAM's handlers wrote.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. Signature
/// verification is off for these tests, so the value only has to parse.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY00000000/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    )
}

/// Send one request and return its status with the body as text.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(bytes::Bytes::from(body))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(&response.into_body()).to_string())
}

/// Drive one `awsQuery` IAM action: `POST /` with the parameters in the form body.
///
/// Every value passed below is free of `&`, `=`, `+` and `%`, so it travels as written.
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    let (status, text) = send(
        router,
        "iam",
        "POST",
        "https://iam.amazonaws.com/",
        "application/x-www-form-urlencoded",
        body,
    )
    .await;
    assert_eq!(status, 200, "IAM {action} must answer 200, got {status} {text}");
    text
}

/// Drive one resiliencehubv2 operation over its own restJson1 path space.
async fn v2(router: &MultiServiceRouter, path: &str, body: Option<Value>) -> (u16, Value) {
    let method = if body.is_some() { "POST" } else { "GET" };
    let payload = body
        .map(|value| serde_json::to_string(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let (status, text) = send(
        router,
        "resiliencehub",
        method,
        &format!("https://resiliencehub.{REGION}.amazonaws.com{path}"),
        "application/json",
        payload,
    )
    .await;
    let parsed = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create_service(router: &MultiServiceRouter, name: &str, role: &str) -> (u16, Value) {
    v2(
        router,
        "/v2/create-service",
        Some(json!({
            "name": name,
            "regions": [REGION],
            "permissionModel": {"invokerRoleName": role},
        })),
    )
    .await
}

async fn get_service(router: &MultiServiceRouter, service_arn: &str) -> (u16, Value) {
    v2(
        router,
        &format!("/v2/get-service?serviceArn={service_arn}"),
        None,
    )
    .await
}

async fn update_service(router: &MultiServiceRouter, service_arn: &str, role: &str) -> (u16, Value) {
    v2(
        router,
        "/v2/update-service",
        Some(json!({
            "serviceArn": service_arn,
            "permissionModel": {"invokerRoleName": role},
        })),
    )
    .await
}

/// Every service `ListServices` reports, by name.
async fn service_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = v2(router, "/v2/list-services", None).await;
    assert_eq!(status, 200, "ListServices must answer 200, got {status} {body:?}");
    body["serviceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no serviceSummaries array in {body:?}"))
        .iter()
        .filter_map(|summary| summary["name"].as_str().map(str::to_string))
        .collect()
}

/// The restJson1 error code, without the namespace the envelope may prefix.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The sentence AWS refuses an invoker role it cannot assume with.
fn refusal(role: &str) -> String {
    format!(
        "Cannot assume invoker role '{role}'. Ensure the role exists and its trust policy \
         allows access."
    )
}

/// A trust policy, written compactly and with no `&`, `=`, `+` or `%`, so it travels in an
/// `awsQuery` form body as written.
///
/// `effect`, `action` and `principal` are written in whichever of the forms a policy admits
/// the row asks for: `action` and `principal` are each either a quoted string or a quoted
/// list.
fn trust_policy(effect: &str, action: &str, principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{principal}}},\"Action\":{action}}}]}}"
    )
}

/// The trust policy a role this service can assume carries.
fn admitting() -> String {
    trust_policy("Allow", "\"sts:AssumeRole\"", "\"resiliencehub.amazonaws.com\"")
}

/// One row: a role IAM holds (or does not), and whether a create naming it is served.
struct Row {
    case: &'static str,
    /// The role IAM is asked to create, with the trust document it carries. `None` for a
    /// row whose value names no role at all.
    created: Option<(&'static str, String)>,
    /// The value the request's `permissionModel.invokerRoleName` carries.
    named: String,
    /// Whether the create is served.
    served: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role trusting this service",
            created: Some(("rh-trusted", admitting())),
            named: "rh-trusted".to_string(),
            served: true,
        },
        Row {
            case: "a role trusting this service among a list of principals",
            created: Some((
                "rh-listed",
                trust_policy(
                    "Allow",
                    "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                    "[\"lambda.amazonaws.com\",\"resiliencehub.amazonaws.com\"]",
                ),
            )),
            named: "rh-listed".to_string(),
            served: true,
        },
        Row {
            case: "a role whose grant names the action by wildcard",
            created: Some((
                "rh-wildcard",
                trust_policy("Allow", "\"sts:*\"", "\"resiliencehub.amazonaws.com\""),
            )),
            named: "rh-wildcard".to_string(),
            served: true,
        },
        Row {
            // IAM entity names "are not distinguished by case", so this is the same role.
            case: "the trusting role named in another case",
            created: None,
            named: "RH-TRUSTED".to_string(),
            served: true,
        },
        Row {
            case: "a role trusting another service only",
            created: Some((
                "rh-foreign",
                trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
            )),
            named: "rh-foreign".to_string(),
            served: false,
        },
        Row {
            case: "the same unassumable role named in another case",
            created: None,
            named: "RH-Foreign".to_string(),
            served: false,
        },
        Row {
            case: "a role explicitly denying this service the assume",
            created: Some((
                "rh-denied",
                trust_policy("Deny", "\"sts:AssumeRole\"", "\"resiliencehub.amazonaws.com\""),
            )),
            named: "rh-denied".to_string(),
            served: false,
        },
        Row {
            // IAM compares an action and a service principal case-insensitively, so the
            // same grant written in another case is the same grant.
            case: "a role whose grant names the action and the principal in another case",
            created: Some((
                "rh-case",
                trust_policy("Allow", "\"STS:AssumeRole\"", "\"ResilienceHub.AmazonAWS.com\""),
            )),
            named: "rh-case".to_string(),
            served: true,
        },
        Row {
            case: "a role whose grant names the action by a prefix wildcard",
            created: Some((
                "rh-prefix",
                trust_policy("Allow", "\"sts:Assume*\"", "\"resiliencehub.amazonaws.com\""),
            )),
            named: "rh-prefix".to_string(),
            served: true,
        },
        Row {
            // A policy's `Statement` is "one statement or a list of them", so a document
            // carrying its one statement as an object states the same grant.
            case: "a role whose document carries its one statement as an object",
            created: Some((
                "rh-one-statement",
                "{\"Version\":\"2012-10-17\",\"Statement\":{\"Effect\":\"Allow\",\
                 \"Principal\":{\"Service\":\"resiliencehub.amazonaws.com\"},\
                 \"Action\":\"sts:AssumeRole\"}}"
                    .to_string(),
            )),
            named: "rh-one-statement".to_string(),
            served: true,
        },
        Row {
            // An explicit `Deny` is final: no `Allow` in the same document puts the grant
            // back.
            case: "a role denying this service the assume beside an allow of it",
            created: Some((
                "rh-allow-and-deny",
                "{\"Version\":\"2012-10-17\",\"Statement\":[\
                 {\"Effect\":\"Allow\",\
                 \"Principal\":{\"Service\":\"resiliencehub.amazonaws.com\"},\
                 \"Action\":\"sts:AssumeRole\"},\
                 {\"Effect\":\"Deny\",\
                 \"Principal\":{\"Service\":\"resiliencehub.amazonaws.com\"},\
                 \"Action\":\"sts:AssumeRole\"}]}"
                    .to_string(),
            )),
            named: "rh-allow-and-deny".to_string(),
            served: false,
        },
        Row {
            // The gate asks whether a SERVICE principal may assume the role, so a trust
            // relationship with the account's own identities is not one this service can
            // act on.
            case: "a role trusting an account principal rather than a service",
            created: Some((
                "rh-account-principal",
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\
                 \"Action\":\"sts:AssumeRole\"}]}"
                    .to_string(),
            )),
            named: "rh-account-principal".to_string(),
            served: false,
        },
        Row {
            case: "a role whose grant is for another action",
            created: Some((
                "rh-other-action",
                trust_policy("Allow", "\"s3:GetObject\"", "\"resiliencehub.amazonaws.com\""),
            )),
            named: "rh-other-action".to_string(),
            served: false,
        },
        Row {
            // The member's `@pattern` admits a path; IAM holds the role under its name.
            case: "a trusting role named under a path",
            created: None,
            named: "service-role/rh-trusted".to_string(),
            served: true,
        },
        Row {
            case: "an unassumable role named under a path",
            created: None,
            named: "service-role/rh-foreign".to_string(),
            served: false,
        },
        Row {
            // An invoker role is routinely provisioned out of band, so a name this
            // emulator's IAM holds nothing under is no evidence about the role.
            case: "a name IAM holds no role under",
            created: None,
            named: "rh-never-created".to_string(),
            served: true,
        },
    ]
}

/// Create `role` in IAM with the trust document `document`, and read it back through IAM's
/// own `GetRole` so the document under test is the one the records hold.
async fn install_role(router: &MultiServiceRouter, role: &str, document: &str) {
    iam(
        router,
        "CreateRole",
        &[("RoleName", role), ("AssumeRolePolicyDocument", document)],
    )
    .await;
    let held = iam(router, "GetRole", &[("RoleName", role)]).await;
    assert!(
        held.contains(&format!("<RoleName>{role}</RoleName>")),
        "GetRole {role} must report the created role, got {held}"
    );
}

#[tokio::test]
async fn a_service_is_created_only_for_an_invoker_role_this_service_can_assume() {
    let router = emulator();
    for (index, row) in rows().into_iter().enumerate() {
        if let Some((role, document)) = &row.created {
            install_role(&router, role, document).await;
        }
        // Each row writes (or fails to write) its own service, so the duplicate-name
        // conflict never stands in for the gate under test.
        let name = format!("invoker-case-{index}");
        let (status, body) = create_service(&router, &name, &row.named).await;
        let listed = service_names(&router).await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {:?}: expected the create to stand, got {status} {body:?}",
                row.case
            );
            let arn = body["service"]["serviceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: no serviceArn in {body:?}", row.case))
                .to_string();
            assert!(
                listed.contains(&name),
                "case {:?}: expected the service to be listed, got {listed:?}",
                row.case
            );
            // Round trip: the stored service reports the role name the request sent.
            let (status, read) = get_service(&router, &arn).await;
            assert_eq!(
                (status, read["service"]["permissionModel"]["invokerRoleName"].as_str()),
                (200, Some(row.named.as_str())),
                "case {:?}: expected {arn} to echo the invoker role, got {status} {read:?}",
                row.case
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, VALIDATION),
                "case {:?}: expected 400 {VALIDATION}, got {status} {body:?}",
                row.case
            );
            assert_eq!(
                error_message(&body),
                refusal(&row.named),
                "case {:?}: unexpected refusal wording in {body:?}",
                row.case
            );
            assert!(
                !listed.contains(&name),
                "case {:?}: a refused create must write no service, got {listed:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn repairing_the_trust_policy_flips_the_same_request_from_refused_to_served() {
    let router = emulator();
    let role = "rh-repaired";
    install_role(
        &router,
        role,
        &trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
    )
    .await;

    // The verdict is a property of the IAM records at the instant of the call, so the
    // identical request is answered differently as the trust policy moves. Each step
    // creates its own service name: only the invoker role's assumability differs.
    let steps: [(&str, Option<String>, bool); 5] = [
        ("the role as created, trusting another service", None, false),
        (
            "the same role, its trust policy repaired",
            Some(admitting()),
            true,
        ),
        ("the repaired role, asked again", None, true),
        (
            "the same role, broken again",
            Some(trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"ec2.amazonaws.com\"",
            )),
            false,
        ),
        (
            "the same role, granting every action to every service it names",
            Some(trust_policy(
                "Allow",
                "\"*\"",
                "[\"ec2.amazonaws.com\",\"resiliencehub.amazonaws.com\"]",
            )),
            true,
        ),
    ];
    for (step, (case, document, served)) in steps.into_iter().enumerate() {
        if let Some(document) = document {
            iam(
                &router,
                "UpdateAssumeRolePolicy",
                &[("RoleName", role), ("PolicyDocument", &document)],
            )
            .await;
        }
        let name = format!("repair-step-{step}");
        let (status, body) = create_service(&router, &name, role).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to stand, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str(), error_message(&body)),
                (400, VALIDATION, refusal(role)),
                "case {case:?}: expected the refusal, got {status} {body:?}"
            );
        }
        let listed = service_names(&router).await;
        assert_eq!(
            listed.contains(&name),
            served,
            "case {case:?}: expected the service to be stored == {served}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_update_onto_an_unassumable_invoker_role_changes_nothing() {
    let router = emulator();
    install_role(&router, "upd-trusted", &admitting()).await;
    install_role(
        &router,
        "upd-foreign",
        &trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
    )
    .await;

    let (status, body) = create_service(&router, "updatable-service", "upd-trusted").await;
    assert_eq!(status, 200, "creating the service: got {status} {body:?}");
    let arn = body["service"]["serviceArn"]
        .as_str()
        .expect("a minted serviceArn")
        .to_string();

    // The same gate on the operation that switches a stored service onto another role: an
    // update naming a role this service cannot assume is refused and merges nothing.
    for (case, role, served) in [
        ("a role trusting another service", "upd-foreign", false),
        ("a name IAM holds no role under", "upd-absent", true),
        ("a role trusting this service", "upd-trusted", true),
    ] {
        let (status, body) = update_service(&router, &arn, role).await;
        let (read_status, read) = get_service(&router, &arn).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: GetService must answer 200, got {read_status} {read:?}"
        );
        let stored = read["service"]["permissionModel"]["invokerRoleName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to stand, got {status} {body:?}"
            );
            assert_eq!(
                stored, role,
                "case {case:?}: expected the stored role to be {role:?}, got {stored:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str(), error_message(&body)),
                (400, VALIDATION, refusal(role)),
                "case {case:?}: expected the refusal, got {status} {body:?}"
            );
            assert_ne!(
                stored, role,
                "case {case:?}: a refused update must merge nothing, got {stored:?}"
            );
        }
    }
}

/// A refused call writes NOTHING, so it reserves nothing either: the name it carried is
/// still free, and the very same request — same name, same role — is served once the role's
/// trust policy is repaired, rather than meeting the duplicate-name conflict.
#[tokio::test]
async fn a_refused_create_reserves_neither_the_name_nor_the_token() {
    let router = emulator();
    let role = "rh-unreserved";
    install_role(
        &router,
        role,
        &trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
    )
    .await;

    // Each attempt carries the SAME name and the SAME idempotency token, so what the second
    // one meets is the gate's verdict and nothing a refused first attempt left behind.
    let name = "unreserved-service";
    let request = json!({
        "name": name,
        "regions": [REGION],
        "permissionModel": {"invokerRoleName": role},
        "clientToken": "unreserved-token",
    });
    let (status, body) = v2(&router, "/v2/create-service", Some(request.clone())).await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body)),
        (400, VALIDATION, refusal(role)),
        "the role as created trusts another service: got {status} {body:?}"
    );

    iam(
        &router,
        "UpdateAssumeRolePolicy",
        &[("RoleName", role), ("PolicyDocument", &admitting())],
    )
    .await;

    let (status, body) = v2(&router, "/v2/create-service", Some(request)).await;
    assert_eq!(
        status, 200,
        "the identical request must be served once the role is repaired, got {status} {body:?}"
    );
    let arn = body["service"]["serviceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no serviceArn in {body:?}"))
        .to_string();
    let listed = service_names(&router).await;
    assert_eq!(
        listed.iter().filter(|listed| *listed == name).count(),
        1,
        "exactly one service must stand under {name:?}, got {listed:?}"
    );
    let (read_status, read) = get_service(&router, &arn).await;
    assert_eq!(
        (
            read_status,
            read["service"]["permissionModel"]["invokerRoleName"].as_str()
        ),
        (200, Some(role)),
        "the stored service must echo the invoker role, got {read_status} {read:?}"
    );
}

/// A document the records hold that cannot be read as a policy at all shows no grant this
/// service could act on, so the role it is attached to is refused like one that withholds
/// the assume.
///
/// IAM refuses a malformed document on `CreateRole`, so the only way a role carries one is
/// an `AssumeRolePolicyDocument` that IS a policy and says nothing about this principal —
/// which the table above covers — or a document IAM accepted and this service cannot read.
/// The case kept here is the one IAM itself admits: a policy carrying no statement at all.
#[tokio::test]
async fn a_statementless_trust_policy_grants_nothing() {
    let router = emulator();
    let role = "rh-statementless";
    install_role(
        &router,
        role,
        "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
    )
    .await;
    let (status, body) = create_service(&router, "statementless-service", role).await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body)),
        (400, VALIDATION, refusal(role)),
        "a trust policy carrying no statement grants nothing: got {status} {body:?}"
    );
    assert!(
        !service_names(&router).await.contains(&"statementless-service".to_string()),
        "a refused create must write no service"
    );
}

/// The principal the gate asks about is this service's own, so a role trusting some OTHER
/// service is refused whichever service that is: the rule is not keyed to one recorded
/// principal.
#[tokio::test]
async fn only_this_services_principal_satisfies_the_gate() {
    let router = emulator();
    for (index, principal) in [
        "lambda.amazonaws.com",
        "ec2.amazonaws.com",
        "ssm.amazonaws.com",
        "resiliencehub.amazonaws.com.evil.example",
    ]
    .into_iter()
    .enumerate()
    {
        let role = format!("rh-principal-{index}");
        install_role(
            &router,
            &role,
            &trust_policy("Allow", "\"sts:AssumeRole\"", &format!("\"{principal}\"")),
        )
        .await;
        let (status, body) = create_service(&router, &format!("principal-{index}"), &role).await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (400, VALIDATION, refusal(&role)),
            "case {principal:?}: only {SERVICE_PRINCIPAL} satisfies the gate, got {status} \
             {body:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! A session policy the mint of a role session passed SCOPES what that credential may do,
//! whether it was passed inline or named by the ARN of a managed policy.
//!
//! RULE: the permissions in force on a temporary credential are the intersection of the
//! identity's own policies and the session policies the mint passed, and a session policy
//! grants nothing of its own — so the service deciding a request signed with that
//! credential refuses an action the identity allows and the session policies do not, at
//! HTTP 403 `AccessDenied`, and refuses one only the session policies allow as if they had
//! allowed nothing. An explicit `Deny` in a session policy refuses whatever any grant says.
//! A managed session policy named by ARN stands for its DEFAULT version as it reads at the
//! instant of the request, so it narrows exactly as an inline document does.
//!
//! Two services have to agree for that to hold — STS mints the session and records the
//! scope it was minted with, IAM holds the managed policies the ARNs name and is the gate
//! that decides a call against them — so this is the suite that can pin it end to end.
//!
//! The rows span what the rule generalises over: the FORM the scope was passed in (a
//! managed ARN alone, an inline document alone, several managed ARNs together, a managed
//! ARN beside an inline document, no scope at all), the EFFECT its statements state (an
//! `Allow` naming one action, an `Allow` naming a wildcard over a namespace, an explicit
//! `Deny`), and WHICH of three operations of the deciding service the call asks for.
//! Beside the table: a scope cannot grant past the identity's policies, a managed scope is
//! re-read when its default version moves, a managed ARN whose document this emulator does
//! not hold scopes nothing, and `PackedPolicySize` grows with every managed ARN packed in.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The region every request here signs for.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is the ambient identity's: the
/// account's own administrative credential, which holds no IAM policies.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The `RoleSessionName` every `AssumeRole` here asks for — the last segment of the
/// assumed-role ARN a refusal names.
const SESSION: &str = "scoped";

/// A trust policy granting the account root permission to assume a role.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// IAM and STS over ONE engine, as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key the service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM action, presenting `access_key_id`.
async fn iam(router: &MultiServiceRouter, access_key_id: &str, body: String) -> (u16, String) {
    query(
        router,
        access_key_id,
        "iam",
        format!("Version=2010-05-08&{body}"),
    )
    .await
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The awsQuery error code of a refusal.
fn error_code(body: &str) -> String {
    xml_element(body, "Code").unwrap_or_else(|| "<no Code>".to_string())
}

/// The `Message` of a refusal.
fn message_of(body: &str) -> String {
    xml_element(body, "Message").unwrap_or_else(|| "<no Message>".to_string())
}

/// One policy document stating `effect` over `action` and every resource. The document
/// carries nothing a form body treats specially, so it travels as written.
fn statement(effect: &str, action: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"*"}}]}}"#
    )
}

/// Create a role the account root may assume, as the ambient identity.
async fn create_role(router: &MultiServiceRouter, role_name: &str) {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        format!("Action=CreateRole&RoleName={role_name}&AssumeRolePolicyDocument={TRUST_POLICY}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole expected 200, got {status}: {body}"
    );
}

/// Embed one inline policy in a role, as the ambient identity.
async fn put_role_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        format!(
            "Action=PutRolePolicy&RoleName={role_name}&PolicyName={policy_name}\
             &PolicyDocument={document}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}/{policy_name:?}: PutRolePolicy expected 200, got {status}: {body}"
    );
}

/// Create a managed policy holding `document` and answer its ARN. It is created only and
/// attached to nothing: a managed SESSION policy is named by ARN at mint time, which is a
/// different thing from attaching it to the role.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str, document: &str) -> String {
    let (status, body) = iam(
        router,
        AMBIENT_KEY,
        format!("Action=CreatePolicy&PolicyName={policy_name}&PolicyDocument={document}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy expected 200, got {status}: {body}"
    );
    xml_element(&body, "Arn")
        .unwrap_or_else(|| panic!("case {policy_name:?}: no <Arn> in {body}"))
}

/// What one `AssumeRole` reported: the access key id of the session it minted, and the
/// `PackedPolicySize` it measured the request's session state at.
struct Minted {
    access_key_id: String,
    packed_policy_size: i32,
}

/// Assume `role_name`, passing `inline` as the inline session policy and `managed` as the
/// managed session policy ARNs.
async fn assume_role(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    inline: Option<&str>,
    managed: &[String],
) -> Minted {
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{role_name}");
    let mut body = format!(
        "Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName={SESSION}"
    );
    if let Some(policy) = inline {
        body.push_str(&format!("&Policy={policy}"));
    }
    for (index, arn) in managed.iter().enumerate() {
        body.push_str(&format!("&PolicyArns.member.{}.arn={arn}", index + 1));
    }
    let (status, body) = query(router, AMBIENT_KEY, "sts", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole expected 200, got {status}: {body}"
    );
    Minted {
        access_key_id: xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("case {case:?}: no <AccessKeyId> in {body}")),
        packed_policy_size: xml_element(&body, "PackedPolicySize")
            .unwrap_or_else(|| panic!("case {case:?}: no <PackedPolicySize> in {body}"))
            .parse()
            .unwrap_or_else(|why| panic!("case {case:?}: PackedPolicySize is not an integer: {why}")),
    }
}

/// The session ARN `AssumeRole` reports for `role_name`, which is what a refusal names as
/// the principal.
fn session_arn(role_name: &str) -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{SESSION}")
}

/// The refusal IAM words for a decision taken on the session of `role_name`.
fn not_authorized(role_name: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
        session_arn(role_name)
    )
}

/// One listing of the deciding service, as a row addresses it: the form action, the
/// parameters it takes, the `iam:` action name a policy statement names it by, and the
/// resource ARN the decision is made against.
struct Listing {
    action: &'static str,
    parameters: &'static str,
    iam_action: &'static str,
    resource: String,
}

/// The three listings the rows ask for — three different operations of the deciding
/// service, each addressing the collection of ONE entity type under the path prefix it
/// lists, which is the resource IAM decides such a call against.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            action: "ListUsers",
            parameters: "PathPrefix=%2Fprobe%2F",
            iam_action: "iam:ListUsers",
            resource: format!("arn:aws:iam::{ACCOUNT}:user/probe/"),
        },
        Listing {
            action: "ListGroups",
            parameters: "PathPrefix=%2Fprobe%2F",
            iam_action: "iam:ListGroups",
            resource: format!("arn:aws:iam::{ACCOUNT}:group/probe/"),
        },
        Listing {
            action: "ListRoles",
            parameters: "PathPrefix=%2Fprobe%2F",
            iam_action: "iam:ListRoles",
            resource: format!("arn:aws:iam::{ACCOUNT}:role/probe/"),
        },
    ]
}

/// Call one listing as `access_key_id`.
async fn list(router: &MultiServiceRouter, access_key_id: &str, listing: &Listing) -> (u16, String) {
    iam(
        router,
        access_key_id,
        format!("Action={}&{}", listing.action, listing.parameters),
    )
    .await
}

/// What a row expects of one listing under its scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The scope allows the call, so it is served.
    Served,
    /// Nothing in the scope allows the call, so the grant the identity holds is withheld.
    NotAllowed,
    /// A statement of the scope explicitly denies the call.
    Denied,
}

/// One row: a form the scope can be passed in, and what each listing then answers.
struct Row {
    case: &'static str,
    /// The inline session policy document the mint passes, if any.
    inline: Option<String>,
    /// The documents of the managed policies the mint names by ARN, in order.
    managed: Vec<String>,
    /// What each listing of [`listings`] answers under that scope, in the same order.
    expected: [Expect; 3],
}

/// The rows: every form a session scope can take, over the three operations.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a managed session policy alone, allowing one action",
            inline: None,
            managed: vec![statement("Allow", "iam:ListUsers")],
            expected: [Expect::Served, Expect::NotAllowed, Expect::NotAllowed],
        },
        Row {
            case: "an inline session policy alone, allowing one action",
            inline: Some(statement("Allow", "iam:ListGroups")),
            managed: Vec::new(),
            expected: [Expect::NotAllowed, Expect::Served, Expect::NotAllowed],
        },
        Row {
            case: "two managed session policies, each allowing one action",
            inline: None,
            managed: vec![
                statement("Allow", "iam:ListUsers"),
                statement("Allow", "iam:ListRoles"),
            ],
            expected: [Expect::Served, Expect::NotAllowed, Expect::Served],
        },
        Row {
            case: "a managed session policy beside an inline one",
            inline: Some(statement("Allow", "iam:ListGroups")),
            managed: vec![statement("Allow", "iam:ListUsers")],
            expected: [Expect::Served, Expect::Served, Expect::NotAllowed],
        },
        Row {
            case: "a managed session policy whose Allow is a wildcard over the namespace",
            inline: None,
            managed: vec![statement("Allow", "iam:List*")],
            expected: [Expect::Served, Expect::Served, Expect::Served],
        },
        Row {
            case: "a managed session policy allowing everything but denying one action",
            inline: None,
            managed: vec![statement("Allow", "*"), statement("Deny", "iam:ListRoles")],
            expected: [Expect::Served, Expect::Served, Expect::Denied],
        },
        Row {
            case: "an inline session policy denying one action beside a managed Allow",
            inline: Some(statement("Deny", "iam:ListUsers")),
            managed: vec![statement("Allow", "*")],
            expected: [Expect::Denied, Expect::Served, Expect::Served],
        },
        Row {
            case: "no session policy at all",
            inline: None,
            managed: Vec::new(),
            expected: [Expect::Served, Expect::Served, Expect::Served],
        },
    ]
}

/// RULE: a request signed with a role session's credential is decided against the
/// intersection of the role's identity-based policies and the session policies the mint
/// passed — whichever form they were passed in — so an action the role allows and the scope
/// omits is refused `AccessDenied`/403 naming the session policies as the document that
/// withheld the grant, an action a session policy explicitly denies is refused naming that
/// deny, and an action both allow is served.
#[tokio::test]
async fn a_session_policy_narrows_the_session_whichever_form_it_was_passed_in() {
    for (index, row) in rows().iter().enumerate() {
        let router = build_router();
        let role_name = format!("ScopedRole{index}");
        create_role(&router, &role_name).await;
        // The ROLE allows all three listings, so every refusal below comes from the scope
        // the credential was minted with and from nothing else.
        put_role_policy(
            &router,
            &role_name,
            "allow-the-three-listings",
            &statement("Allow", "iam:List*"),
        )
        .await;

        let mut managed_arns = Vec::new();
        for (position, document) in row.managed.iter().enumerate() {
            managed_arns.push(
                create_policy(
                    &router,
                    &format!("ScopePolicy{index}x{position}"),
                    document,
                )
                .await,
            );
        }
        let minted = assume_role(
            &router,
            row.case,
            &role_name,
            row.inline.as_deref(),
            &managed_arns,
        )
        .await;

        for (listing, expected) in listings().iter().zip(row.expected) {
            let case = format!("{} asking for {}", row.case, listing.action);
            let (status, body) = list(&router, &minted.access_key_id, listing).await;
            match expected {
                Expect::Served => assert_eq!(
                    status, 200,
                    "case {case:?}: the scope allows this action, so it must be served, got \
                     {status}: {body}"
                ),
                Expect::NotAllowed | Expect::Denied => {
                    assert_eq!(
                        status, 403,
                        "case {case:?}: expected HTTP 403, got {status}: {body}"
                    );
                    let code = error_code(&body);
                    assert_eq!(
                        code, "AccessDenied",
                        "case {case:?}: expected AccessDenied, got {code:?}: {body}"
                    );
                    let reason = if expected == Expect::Denied {
                        "with an explicit deny in a session policy".to_string()
                    } else {
                        format!(
                            "because no session policy allows the {} action",
                            listing.iam_action
                        )
                    };
                    let message = message_of(&body);
                    // The sentence this row set up, which is the whole refusal
                    // (`iam_authorization_details_pointer.rs` owns that reader).
                    crate::iam_authorization_details_pointer::assert_identity_refusal(
                        &case,
                        &message,
                        &not_authorized(
                            &role_name,
                            listing.iam_action,
                            &listing.resource,
                            &reason,
                        ),
                    );
                }
            }
        }
    }
}

/// RULE: "you cannot use session policies to grant more permissions than those allowed by
/// the identity-based policy of the role that is being assumed" — a scope that allows an
/// action the role's own policies do not is refused as if the scope had allowed nothing, and
/// the refusal names the identity-based policies as the document that withheld the grant.
#[tokio::test]
async fn a_session_policy_cannot_grant_past_the_roles_own_policies() {
    for (index, case) in ["a managed session policy", "an inline session policy"]
        .iter()
        .enumerate()
    {
        let router = build_router();
        let role_name = format!("NarrowRole{index}");
        create_role(&router, &role_name).await;
        // The role allows exactly ONE of the three listings.
        put_role_policy(
            &router,
            &role_name,
            "allow-one-listing",
            &statement("Allow", "iam:ListUsers"),
        )
        .await;

        // The scope allows every action, which is more than the role does.
        let broad = statement("Allow", "*");
        let (inline, managed) = if index == 0 {
            (
                None,
                vec![create_policy(&router, &format!("BroadScope{index}"), &broad).await],
            )
        } else {
            (Some(broad.clone()), Vec::new())
        };
        let minted = assume_role(&router, case, &role_name, inline.as_deref(), &managed).await;

        for listing in &listings() {
            let case = format!("{case} asking for {}", listing.action);
            let (status, body) = list(&router, &minted.access_key_id, listing).await;
            if listing.iam_action == "iam:ListUsers" {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the action both the role and the scope allow must be \
                     served, got {status}: {body}"
                );
                continue;
            }
            assert_eq!(
                status, 403,
                "case {case:?}: a scope cannot grant past the role, got {status}: {body}"
            );
            let message = message_of(&body);
            // The sentence is the whole refusal here: the scope allows the action WITHOUT
            // spelling it (it allows every one), so the grant the decision read is not about
            // this action and the identity's own documents withheld it — the refusal family
            // this service words without the pointer at the detail of the decision (the
            // identity service's own
            // `tests/all/session_refusal_authorization_detail_pointer.rs` and
            // `tests/all/scope_specificity_refusal_detail_pointer.rs` are the tables for
            // that line).
            crate::iam_authorization_details_pointer::assert_refusal_sentence(
                &case,
                &message,
                &not_authorized(
                    &role_name,
                    listing.iam_action,
                    &listing.resource,
                    &format!(
                        "because no identity-based policy allows the {} action",
                        listing.iam_action
                    ),
                ),
            );
        }
    }
}

/// RULE: a managed session policy stands for the version that is IN FORCE — "the one that is
/// in effect for the IAM users, groups, and roles to which the policy is attached" — read at
/// the instant of the request, so moving the policy's default version changes what the same
/// unchanged session may do.
#[tokio::test]
async fn a_managed_session_policy_is_read_at_the_instant_of_the_request() {
    let router = build_router();
    let role_name = "RereadRole";
    create_role(&router, role_name).await;
    put_role_policy(
        &router,
        role_name,
        "allow-the-three-listings",
        &statement("Allow", "iam:List*"),
    )
    .await;
    let policy_arn = create_policy(
        &router,
        "MovingScope",
        &statement("Allow", "iam:ListUsers"),
    )
    .await;
    let minted = assume_role(
        &router,
        "a managed scope whose default version moves",
        role_name,
        None,
        ::std::slice::from_ref(&policy_arn),
    )
    .await;

    let users = &listings()[0];
    let groups = &listings()[1];
    for (case, listing, expected) in [
        ("the first version allows ListUsers", users, 200),
        ("the first version omits ListGroups", groups, 403),
    ] {
        let (status, body) = list(&router, &minted.access_key_id, listing).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status}: {body}"
        );
    }

    // A SECOND version of the same policy, made the default: what the scope says changes
    // for the session already minted, because the ARN is what the mint recorded.
    let (status, body) = iam(
        &router,
        AMBIENT_KEY,
        format!(
            "Action=CreatePolicyVersion&PolicyArn={policy_arn}&SetAsDefault=true\
             &PolicyDocument={}",
            statement("Allow", "iam:ListGroups")
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePolicyVersion expected 200, got {status}: {body}"
    );

    for (case, listing, expected) in [
        ("the new default version omits ListUsers", users, 403),
        ("the new default version allows ListGroups", groups, 200),
    ] {
        let (status, body) = list(&router, &minted.access_key_id, listing).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status}: {body}"
        );
    }
}

/// RULE: a managed session policy whose document this emulator does not hold — an entry of
/// AWS's own catalog, owned by AWS rather than by the account — is a scope that cannot be
/// read, and refusing on evidence that does not exist would deny access AWS permits, so such
/// a credential is scoped by nothing.
#[tokio::test]
async fn a_managed_session_policy_whose_document_is_not_held_scopes_nothing() {
    let router = build_router();
    let role_name = "UnreadableScopeRole";
    create_role(&router, role_name).await;
    put_role_policy(
        &router,
        role_name,
        "allow-the-three-listings",
        &statement("Allow", "iam:List*"),
    )
    .await;
    // An AWS-managed policy ARN: the account holds no document for it.
    let minted = assume_role(
        &router,
        "an AWS-managed session policy",
        role_name,
        None,
        &["arn:aws:iam::aws:policy/ReadOnlyAccess".to_string()],
    )
    .await;

    for listing in &listings() {
        let (status, body) = list(&router, &minted.access_key_id, listing).await;
        assert_eq!(
            status, 200,
            "case {:?}: a scope this emulator cannot read must not withhold a grant, got \
             {status}: {body}",
            listing.action
        );
    }
}

/// RULE: `PackedPolicySize` "indicates by percentage how close the policies and tags for
/// your request are to the upper size limit", and the managed policy ARNs are among the
/// things packed into that form — so a mint carrying managed session policy ARNs reports a
/// strictly larger percentage than the same mint carrying none, and each further ARN
/// strictly larger again. A mint carrying none still reports a percentage inside the 1..100
/// band, because the packed session state always carries the token's own header.
#[tokio::test]
async fn packed_policy_size_grows_with_every_managed_session_policy_packed_in() {
    let router = build_router();
    let role_name = "PackedRole";
    create_role(&router, role_name).await;
    put_role_policy(
        &router,
        role_name,
        "allow-the-three-listings",
        &statement("Allow", "iam:List*"),
    )
    .await;

    let mut arns = Vec::new();
    for position in 0..3 {
        arns.push(
            create_policy(
                &router,
                &format!("PackedScope{position}"),
                &statement("Allow", "iam:List*"),
            )
            .await,
        );
    }

    let mut previous: Option<i32> = None;
    for count in 0..=arns.len() {
        let case = format!("{count} managed session policy ARNs");
        let minted = assume_role(&router, &case, role_name, None, &arns[..count]).await;
        let packed = minted.packed_policy_size;
        assert!(
            (1..=100).contains(&packed),
            "case {case:?}: PackedPolicySize must be a percentage of the budget, got {packed}"
        );
        if let Some(before) = previous {
            assert!(
                packed > before,
                "case {case:?}: expected a packed size strictly greater than {before}, got \
                 {packed}"
            );
        }
        previous = Some(packed);
    }
}

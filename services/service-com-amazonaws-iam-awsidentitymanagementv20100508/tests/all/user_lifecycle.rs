// SPDX-License-Identifier: CC0-1.0

//! The IAM user lifecycle, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so an assertion here is
//! an assertion about what a client on the socket sees. Each case is a ROW of a
//! table spanning the class the rule generalises over (path, name case and
//! alphabet, tag count, page size, path prefix, document shape), not just the one
//! input the issue cited.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The fixture user set: names spanning the whole `^[\w+=,.@-]+$` alphabet
/// (`smithy_query(query="operation", operation="CreateUser")`) and paths spanning
/// the root, a one-segment path and a nested path, so a rule about paths is
/// exercised on more than one depth.
const USERS: &[(&str, &str)] = &[
    ("alice", "/eng/"),
    ("Bob", "/"),
    ("carol_7", "/eng/tools/"),
    ("dave.x", "/sales/"),
    ("Eve+1", "/eng/"),
    ("frank=2", "/"),
    ("grace@3", "/sales/west/"),
    ("heidi-4", "/eng/tools/"),
];

/// The user the account holds BEFORE any request has been served: the IAM user this
/// emulator's own long-term credential belongs to
/// (`service_common::caller_principal`), which `iam:CreateUser` did not create and no
/// fixture below creates. Every read of the account's users reports it beside the
/// fixture's own, so the suites that enumerate or count users name it here.
const ACCOUNT_CALLER: &str = ::service_common::caller_principal::AMBIENT_USER_NAME;

/// The path that user sits under: the account root.
const ACCOUNT_CALLER_PATH: &str = ::service_common::caller_principal::AMBIENT_USER_PATH;

/// `names` plus [`ACCOUNT_CALLER`], in the order the service enumerates users:
/// ascending by the case-folded name, which is the key its store is ordered on.
fn stored_names(names: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = names
        .iter()
        .copied()
        .chain(::std::iter::once(ACCOUNT_CALLER))
        .map(ToString::to_string)
        .collect();
    all.sort_by_key(|name| name.to_ascii_lowercase());
    all
}

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
///
/// This module's awsQuery harness is shared with the other suites in this crate
/// (`tests/all/service_linked_roles.rs`), so a request a test drives is built and
/// read exactly one way.
pub(crate) fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router serving only this service, over a single shared engine.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite drives unsigned requests, so no authenticator is registered.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A router serving only this service, over an engine the test ALSO holds, for the
/// suites that have to reach the service's own store — to rewind an instant the
/// clock has already made due, or to seed state whose writer is still a scaffolded
/// 501. Every other suite takes [`router`], which keeps the engine to itself.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite drives unsigned requests, so no authenticator is registered.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            engine.clone(),
        ),
    );
    (router, engine)
}

/// One awsQuery call: `Action` plus form parameters, answered with the HTTP status
/// and the XML body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_signed_by(router, None, action, params).await
}

/// One awsQuery call presented as signed by the access key id `akid`, for the
/// operations that resolve the caller's identity from the signing key
/// (`GetUser` and `CreateAccessKey` with no `UserName`).
///
/// The suite registers no authenticator, so the `Authorization` header is not
/// verified here; it is the only place the signing key id appears on the wire, so
/// it is what the service reads the caller's identity from
/// (`emulator_server::extract_akid`). `None` sends no header at all, which is the
/// unsigned request every other suite drives.
///
/// The credential scope names [`HARNESS_REGION`]; a suite that varies the Region a
/// request addressed takes [`call_signed_in_region`] instead.
pub(crate) async fn call_signed_by(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_signed_in_region(router, akid, HARNESS_REGION, action, params).await
}

/// [`call_signed_by`], with the Region the request's SigV4 credential scope names
/// stated per call.
///
/// The scope is the only place a request carries the Region it addressed, so this is
/// the one knob a suite about Region-dependent behaviour turns.
pub(crate) async fn call_signed_in_region(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_signed_from(router, akid, region, None, action, params).await
}

/// [`call_signed_in_region`], with the address a proxy states it forwarded the request
/// for.
///
/// That header is where the address of a forwarded request comes from, and IAM fills
/// `aws:SourceIp` from it, so this is the one knob a suite about network-scoped grants
/// turns. `None` sends no such header, which is the request every other suite drives —
/// made from the local host.
pub(crate) async fn call_signed_from(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    region: &str,
    forwarded_for: Option<&str>,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The route key the framework resolves for an unsigned request is the
        // endpoint host's first label, which is this service's signing name.
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded");
    let request = match akid {
        // The SigV4 credential scope's first component is the access key id, which
        // is all `extract_akid` reads out of the header; its third is the Region.
        Some(akid) => request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        ),
        None => request,
    };
    let request = match forwarded_for {
        Some(address) => request.header("x-forwarded-for", address),
        None => request,
    };
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body))
                .expect("a well-formed request"),
        )
        .await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only
/// RFC 3986's unreserved characters literal. Every other byte of the UTF-8 form
/// is escaped, so a non-ASCII value survives the round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of every `<tag>` element in the body, in document order, with XML
/// entity and character references resolved.
///
/// A scan rather than a document model, as the sibling awsQuery suites do
/// (`services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/tests/all/sts_sessions.rs`):
/// every tag this suite asks for is a LEAF whose content is text, so there is no
/// nesting to track.
pub(crate) fn elements(body: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        let text = &rest[start + open.len()..];
        let end = text
            .find(&close)
            .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"));
        assert!(
            !text[..end].contains('<'),
            "<{tag}> is not a leaf element, so this suite cannot read it as text: {body}"
        );
        found.push(unescape(&text[..end]));
        rest = &text[end + close.len()..];
    }
    found
}

/// The text of the single `<tag>` element the body must carry.
pub(crate) fn element(body: &str, tag: &str) -> String {
    let found = elements(body, tag);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one <{tag}> in the response, got {}: {body}",
        found.len()
    );
    found.into_iter().next().expect("one element")
}

/// The policy document the single `<tag>` element carries, as a client that follows IAM's
/// contract reads one: percent-decoded back to plain JSON text.
///
/// IAM reports a policy document URL-encoded compliant with RFC 3986 and the caller
/// "can use a URL decoding method to convert the policy back to plain JSON text"
/// (`GetRole`, `GetRolePolicy`, `GetPolicyVersion`, …), so every suite that compares a
/// reported document with the text that was submitted reads it through this one decode.
/// The decoder is the RUNTIME's URI decoder, not this crate's encoder, so a document the
/// service encoded wrongly cannot still match.
pub(crate) fn document_element(body: &str, tag: &str) -> String {
    ::smithy_server_runtime::percent_decode(&element(body, tag))
}

/// [`document_element`] over every `<tag>` element in the body, for a listing that
/// reports one document per entity.
pub(crate) fn document_elements(body: &str, tag: &str) -> Vec<String> {
    elements(body, tag)
        .iter()
        .map(|encoded| ::smithy_server_runtime::percent_decode(encoded))
        .collect()
}

/// The Region every request this harness sends addresses unless a suite names another
/// ([`call_signed_in_region`]).
///
/// A signed request names it in its SigV4 credential scope, and an unsigned one names no
/// Region at all, which is served as the Region IAM's global endpoint is in — the same
/// one.
pub(crate) const HARNESS_REGION: &str = "us-east-1";

/// Assert that `actual` is the whole `AccessDenied` message AWS words for a refused IAM
/// call: the sentence naming the principal that signed it, the action it asked for and
/// the resource the decision was made on, closed by `reason` (an explicit deny, or no
/// policy allowing the action) — followed either by nothing, or by the pointer at the
/// authorization details of the evaluation that withheld the grant.
///
/// Which of the two a given refusal carries is what `tests/all/access_denied_message_wording.rs`
/// is the table for; every other suite asserts the part it set up — the principal, the
/// action, the resource and the reason — and that a continuation, where there is one, is
/// exactly that pointer and carries one id in both of its slots. The id is answered back
/// for a caller comparing two refusals, and is empty for a refusal that ends at the
/// reason.
pub(crate) fn assert_access_denied_message(
    case: &str,
    actual: &str,
    principal: &str,
    action: &str,
    named_as: &str,
    reason: &str,
) -> String {
    let expected = format!(
        "User: {principal} is not authorized to perform: {action} on resource: {named_as} \
         {reason}"
    );
    let Some(rest) = actual.strip_prefix(&expected) else {
        panic!(
            "case {case:?}: expected the refusal to name the principal, the action and the \
             resource and to close with the reason {expected:?}, got {actual:?}"
        );
    };
    let pointed_at = authorization_details_id(rest);
    assert!(
        rest.is_empty() || pointed_at.is_some(),
        "case {case:?}: expected the refusal to end at the reason or to continue with the \
         authorization-details pointer, got {rest:?} after it"
    );
    pointed_at.unwrap_or_default().to_string()
}

/// The fixed text of the clause a refusal that withheld a grant continues with, up to the
/// authorization id: the page the details are read on.
pub(crate) const AUTHORIZATION_DETAILS_PREFIX: &str =
    ". Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1\
     #/authorization-details/";

/// The fixed text between the pointer's two slots, both of which carry the SAME
/// authorization id.
pub(crate) const AUTHORIZATION_DETAILS_INFIX: &str =
    " for complete details, or call the GetRequestAuthorizationDetails API with the \
     following authorization id: ";

/// The authorization id `clause` names, when `clause` is exactly the authorization-details
/// pointer and names one id in both of its slots; `None` when it is anything else.
pub(crate) fn authorization_details_id(clause: &str) -> Option<&str> {
    let body = clause.strip_prefix(AUTHORIZATION_DETAILS_PREFIX)?;
    let (page_id, trailing) = body.split_once(AUTHORIZATION_DETAILS_INFIX)?;
    (page_id == trailing
        && !page_id.is_empty()
        && page_id.chars().all(|character| character.is_ascii_alphanumeric()))
    .then_some(page_id)
}

/// Resolve the references the response serializer emits.
///
/// `vendor/smithy-schema-server/src/xml.rs::xml_escape` emits exactly the five
/// entities XML predefines plus numeric references for tab, LF and CR, so those
/// are the forms this has to read; any other reference is a response this suite
/// does not understand and fails the test rather than being passed through.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let reference = &rest[start..];
        let end = reference
            .find(';')
            .unwrap_or_else(|| panic!("an unterminated XML reference in {text:?}"));
        let name = &reference[1..end];
        out.push(match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            numeric => {
                let code = numeric
                    .strip_prefix('#')
                    .and_then(|digits| match digits.strip_prefix('x') {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => digits.parse().ok(),
                    })
                    .and_then(char::from_u32);
                code.unwrap_or_else(|| {
                    panic!("the response carries a reference this suite does not know: &{name};")
                })
            }
        });
        rest = &reference[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The awsQuery error code (`<Code>`) of a failure envelope.
pub(crate) fn error_code(body: &str) -> String {
    element(body, "Code")
}

/// Create every fixture user, answering the router they were created in.
async fn with_fixture_users() -> MultiServiceRouter {
    let router = router();
    for (name, path) in USERS {
        let (status, body) = call(
            &router,
            "CreateUser",
            &[("UserName", name), ("Path", path)],
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: CreateUser must succeed, got {status}: {body}");
    }
    router
}

/// The fixture user names in the order the service enumerates them: ascending by
/// the case-folded name, which is the key the store is ordered on — beside
/// [`ACCOUNT_CALLER`], the user the account already held.
fn users_in_store_order() -> Vec<String> {
    let names: Vec<&str> = USERS.iter().map(|(name, _)| *name).collect();
    stored_names(&names)
}

/// RULE: `CreateUser` mints and persists a user carrying every required member of
/// the `User` shape, and a later read returns the STORED record — identical
/// `UserId`, `Arn`, `Path` and `CreateDate` — rather than a value recomputed from
/// the reading request.
#[tokio::test]
async fn create_user_persists_every_required_member_and_get_user_returns_it() {
    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();

    for (name, path) in USERS {
        let case = format!("{name} at {path}");
        let (status, created) = call(
            &router,
            "CreateUser",
            &[("UserName", name), ("Path", path)],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {created}");

        assert_eq!(
            element(&created, "UserName"),
            *name,
            "case {case:?}: expected the created user to carry the requested name, got {:?}",
            element(&created, "UserName")
        );
        assert_eq!(
            element(&created, "Path"),
            *path,
            "case {case:?}: expected Path {path:?}, got {:?}",
            element(&created, "Path")
        );
        assert_eq!(
            element(&created, "Arn"),
            format!("arn:aws:iam::{}:user{path}{name}", account_id()),
            "case {case:?}: expected the modelled IAM user ARN, got {:?}",
            element(&created, "Arn")
        );

        // `User$UserId` is `idType`: len 16..128, pattern ^[\w]+$.
        let user_id = element(&created, "UserId");
        assert!(
            (16..=128).contains(&user_id.chars().count()),
            "case {case:?}: expected a UserId of 16..128 characters, got {user_id:?}"
        );
        assert!(
            user_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "case {case:?}: expected a UserId matching ^[\\w]+$, got {user_id:?}"
        );
        assert!(
            !minted_ids.contains(&user_id),
            "case {case:?}: expected a UserId unique across users, got the repeat {user_id:?}"
        );

        // `User$CreateDate` is a timestamp, rendered by awsQuery in date-time form.
        let create_date = element(&created, "CreateDate");
        assert!(
            aws_smithy_types::DateTime::from_str(
                &create_date,
                aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: expected an ISO-8601 CreateDate, got {create_date:?}"
        );

        // The read returns the stored record, not a recomputed one.
        let (status, fetched) = call(&router, "GetUser", &[("UserName", name)]).await;
        assert_eq!(status, 200, "case {case:?}: expected GetUser 200, got {status}: {fetched}");
        for member in ["UserName", "Path", "UserId", "Arn", "CreateDate"] {
            assert_eq!(
                element(&fetched, member),
                element(&created, member),
                "case {case:?}: expected GetUser to return the stored {member}, got {:?}",
                element(&fetched, member)
            );
        }

        // Reading again cannot change what the next caller sees.
        let (_, again) = call(&router, "GetUser", &[("UserName", name)]).await;
        assert_eq!(
            element(&again, "CreateDate"),
            create_date,
            "case {case:?}: a second read changed CreateDate, so the record was not stored"
        );

        minted_ids.push(user_id);
    }
}

/// RULE: a user's `Path` defaults to `/` when the request omits it, and the ARN is
/// built from the stored path.
#[tokio::test]
async fn an_omitted_path_defaults_to_the_account_root() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "rootpath")]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        element(&body, "Path"),
        "/",
        "expected the default path /, got {:?}",
        element(&body, "Path")
    );
    assert_eq!(
        element(&body, "Arn"),
        format!("arn:aws:iam::{}:user/rootpath", account_id()),
        "expected the ARN to be built from the default path, got {:?}",
        element(&body, "Arn")
    );
}

/// RULE: the optional members a request supplies are persisted with the user and
/// reported back on a read; a member it does not supply is reported by neither.
#[tokio::test]
async fn optional_members_round_trip_exactly_as_supplied() {
    /// One row: the user name, the tags to attach, and the permissions boundary
    /// ARN to attach (`None` for neither).
    struct Case<'a> {
        name: &'a str,
        tags: &'a [(&'a str, &'a str)],
        permissions_boundary: Option<&'a str>,
    }

    // Rows differ in tag COUNT (0, 1, 2) and in whether a permissions boundary is
    // attached, which is what the rule generalises over.
    let boundary = format!("arn:aws:iam::{}:policy/Boundary", account_id());
    let cases = [
        Case {
            name: "plain",
            tags: &[],
            permissions_boundary: None,
        },
        Case {
            name: "one_tag",
            tags: &[("team", "iam")],
            permissions_boundary: None,
        },
        Case {
            name: "two_tags",
            tags: &[("team", "iam"), ("env", "prod")],
            permissions_boundary: None,
        },
        Case {
            name: "bounded",
            tags: &[],
            permissions_boundary: Some(boundary.as_str()),
        },
        Case {
            name: "bounded_tagged",
            tags: &[("env", "dev")],
            permissions_boundary: Some(boundary.as_str()),
        },
    ];

    let router = router();
    for Case {
        name,
        tags,
        permissions_boundary,
    } in cases
    {
        let mut params: Vec<(String, String)> =
            vec![("UserName".to_string(), name.to_string())];
        for (index, (key, value)) in tags.iter().enumerate() {
            let n = index + 1;
            params.push((format!("Tags.member.{n}.Key"), (*key).to_string()));
            params.push((format!("Tags.member.{n}.Value"), (*value).to_string()));
        }
        if let Some(arn) = permissions_boundary {
            params.push(("PermissionsBoundary".to_string(), arn.to_string()));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let (status, created) = call(&router, "CreateUser", &borrowed).await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {status}: {created}");

        let (status, fetched) = call(&router, "GetUser", &[("UserName", name)]).await;
        assert_eq!(status, 200, "case {name:?}: expected GetUser 200, got {status}: {fetched}");

        let keys = elements(&fetched, "Key");
        let values = elements(&fetched, "Value");
        let expected_keys: Vec<String> = tags.iter().map(|(k, _)| (*k).to_string()).collect();
        let expected_values: Vec<String> = tags.iter().map(|(_, v)| (*v).to_string()).collect();
        assert_eq!(
            keys, expected_keys,
            "case {name:?}: expected the stored tag keys {expected_keys:?}, got {keys:?}"
        );
        assert_eq!(
            values, expected_values,
            "case {name:?}: expected the stored tag values {expected_values:?}, got {values:?}"
        );

        let reported_boundary = elements(&fetched, "PermissionsBoundaryArn");
        match permissions_boundary {
            Some(arn) => {
                assert_eq!(
                    reported_boundary,
                    vec![arn.to_string()],
                    "case {name:?}: expected the stored boundary ARN, got {reported_boundary:?}"
                );
                // The `PermissionsBoundaryType` AWS puts on the wire beside the ARN:
                // `Policy`, not the name the model gives the single
                // `PermissionsBoundaryAttachmentType` entry.
                assert_eq!(
                    elements(&fetched, "PermissionsBoundaryType"),
                    vec!["Policy".to_string()],
                    "case {name:?}: expected AWS's boundary type, got {:?}",
                    elements(&fetched, "PermissionsBoundaryType")
                );
            }
            None => assert!(
                reported_boundary.is_empty(),
                "case {name:?}: expected no permissions boundary, got {reported_boundary:?}"
            ),
        }
    }
}

/// RULE: an IAM entity name is unique within the account and names are not
/// distinguished by case, so a create naming an existing entity is refused with
/// `EntityAlreadyExists` (HTTP 409) and leaves the account unchanged.
#[tokio::test]
async fn a_repeated_name_is_refused_case_insensitively() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateUser",
        &[("UserName", "MixedCase"), ("Path", "/eng/")],
    )
    .await;
    assert_eq!(status, 200, "expected the first create to succeed, got {status}: {body}");
    let original_id = element(&body, "UserId");

    // Rows differ in the CASE of the name, which is what the rule folds over.
    for spelling in ["MixedCase", "mixedcase", "MIXEDCASE", "mIxEdCaSe"] {
        let (status, body) = call(&router, "CreateUser", &[("UserName", spelling)]).await;
        assert_eq!(
            status, 409,
            "case {spelling:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {spelling:?}: expected EntityAlreadyExists, got {:?}",
            error_code(&body)
        );
    }

    // No second user, and the first is untouched: the account holds the one create that
    // succeeded, beside the user it already held.
    let (_, listed) = call(&router, "ListUsers", &[]).await;
    assert_eq!(
        elements(&listed, "UserName"),
        stored_names(&["MixedCase"]),
        "expected the account to hold exactly the first user, got {:?}",
        elements(&listed, "UserName")
    );
    let (_, fetched) = call(&router, "GetUser", &[("UserName", "mixedcase")]).await;
    assert_eq!(
        element(&fetched, "UserId"),
        original_id,
        "expected the refused creates to leave the stored UserId alone, got {:?}",
        element(&fetched, "UserId")
    );
}

/// RULE: a read naming an entity the account does not hold answers
/// `NoSuchEntity` (HTTP 404).
#[tokio::test]
async fn reading_an_absent_entity_is_not_found() {
    let router = with_fixture_users().await;
    // Rows: a name never created, a name that only differs in a character, and a
    // name at the longest length `existingUserNameType` admits (128).
    let long_name = "n".repeat(128);
    for absent in ["nobody", "alic", "alicee", long_name.as_str()] {
        let (status, body) = call(&router, "GetUser", &[("UserName", absent)]).await;
        assert_eq!(status, 404, "case {absent:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );

        let (status, body) = call(
            &router,
            "GetUserPolicy",
            &[("UserName", absent), ("PolicyName", "AnyPolicy")],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected GetUserPolicy 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: `GetUser` with no `UserName` reports the IAM user the request was signed
/// by; credentials that belong to no IAM user resolve to no user, and IAM refuses
/// the call rather than inventing an identity.
#[tokio::test]
async fn get_user_without_a_name_refuses_non_user_credentials() {
    let router = with_fixture_users().await;
    let (status, body) = call(&router, "GetUser", &[]).await;
    assert_eq!(status, 400, "expected 400, got {status}: {body}");
    // The modelled behaviour under test is the REFUSAL: a 400 carrying no user, under
    // the error code IAM answers a request it cannot accept with, because `GetUser`
    // models no error for a request that named no entity (see the handler's comment).
    assert!(
        elements(&body, "UserName").is_empty(),
        "expected no user to be invented for credentials that belong to none, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected IAM's validation refusal code, got {:?}",
        error_code(&body)
    );
}

/// RULE: an inline policy document is stored verbatim under (user, policy name)
/// and read back byte-identical; a repeat put of the same name replaces it.
#[tokio::test]
async fn an_inline_policy_document_round_trips_byte_identically() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "policyholder")]).await;
    assert_eq!(status, 200, "expected the user to be created, got {status}: {body}");

    // Rows differ in the document's FORM: compact, whitespace-bearing (the
    // `policyDocumentType` pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$ admits
    // tab, LF and CR), nested, and non-ASCII inside \u00FF.
    let documents: &[(&str, &str)] = &[
        (
            "compact",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        ),
        (
            "pretty",
            "{\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": []\n}",
        ),
        (
            "nested",
            r#"{"Statement":[{"Effect":"Deny","Action":["s3:*","iam:*"],"Condition":{"StringEquals":{"aws:username":"a&b<c>d"}}}]}"#,
        ),
        ("latin1", "{\"Sid\":\"caf\u{00e9}-\u{00ff}\",\"Statement\":[]}"),
        // The smallest document the IAM policy grammar admits: `Statement` alone,
        // with both optional top-level elements (`Version`, `Id`) absent.
        ("minimal", "{\"Statement\":[]}"),
    ];

    for (case, document) in documents {
        let (status, body) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", "policyholder"),
                ("PolicyName", "RoundTrip"),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");

        let (status, body) = call(
            &router,
            "GetUserPolicy",
            &[("UserName", "policyholder"), ("PolicyName", "RoundTrip")],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected GetUserPolicy 200, got {status}: {body}");
        assert_eq!(
            document_element(&body, "PolicyDocument"),
            *document,
            "case {case:?}: expected the stored document verbatim, got {:?}",
            document_element(&body, "PolicyDocument")
        );
        assert_eq!(
            element(&body, "UserName"),
            "policyholder",
            "case {case:?}: expected the owning user's name, got {:?}",
            element(&body, "UserName")
        );
        assert_eq!(
            element(&body, "PolicyName"),
            "RoundTrip",
            "case {case:?}: expected the policy name, got {:?}",
            element(&body, "PolicyName")
        );
    }

    // Distinct policy names on one user are distinct documents.
    for (case, document) in documents {
        let (status, body) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", "policyholder"),
                ("PolicyName", case),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    }
    for (case, document) in documents {
        let (_, body) = call(
            &router,
            "GetUserPolicy",
            &[("UserName", "policyholder"), ("PolicyName", case)],
        )
        .await;
        assert_eq!(
            document_element(&body, "PolicyDocument"),
            *document,
            "case {case:?}: a later put of another name overwrote this document"
        );
    }
}

/// RULE: a `PolicyDocument` that is not a JSON object carrying the `Statement`
/// element the IAM policy grammar requires is not a policy, and the put is
/// refused with `MalformedPolicyDocument` (HTTP 400) without storing anything.
#[tokio::test]
async fn a_document_that_is_not_a_json_object_is_malformed() {
    let router = router();
    let (status, _) = call(&router, "CreateUser", &[("UserName", "strict")]).await;
    assert_eq!(status, 200, "expected the user to be created, got {status}");

    // Rows differ in JSON TYPE and in well-formedness: unparseable text, a
    // truncated object, each non-object JSON type, and an object that parses but
    // carries no `Statement` (with and without the optional `Version`).
    let malformed: &[(&str, &str)] = &[
        ("prose", "not a policy at all"),
        ("truncated", "{\"Version\":\"2012-10-17\""),
        ("array", "[{\"Effect\":\"Allow\"}]"),
        ("number", "42"),
        ("string", "\"a policy\""),
        ("boolean", "true"),
        ("null", "null"),
        ("empty object", "{}"),
        ("version only", "{\"Version\":\"2012-10-17\"}"),
    ];

    for (case, document) in malformed {
        let (status, body) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", "strict"),
                ("PolicyName", "Rejected"),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "MalformedPolicyDocument",
            "case {case:?}: expected MalformedPolicyDocument, got {:?}",
            error_code(&body)
        );

        let (status, body) = call(
            &router,
            "GetUserPolicy",
            &[("UserName", "strict"), ("PolicyName", "Rejected")],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused put stored the document anyway, got {status}: {body}"
        );
    }
}

/// RULE: a list operation enumerates the persisted entities in one total order,
/// honours `MaxItems` as a real page size, and hands out a `Marker` a follow-up
/// call resumes from without repeating or skipping an entity.
#[tokio::test]
async fn list_users_pages_through_every_user_exactly_once() {
    let router = with_fixture_users().await;
    let expected = users_in_store_order();

    // Rows span the page size: the published minimum, sizes that do and do not
    // divide the set, a size equal to the set, the published maximum, and the
    // absent-parameter default. `maxItemsType` is range 1..1000
    // (smithy_query(query="operation", operation="ListUsers")).
    let page_sizes: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(expected.len() as i32),
        Some(1000),
        None,
    ];

    for page_size in page_sizes {
        let case = format!("MaxItems={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut params: Vec<(String, String)> = Vec::new();
            if let Some(size) = page_size {
                params.push(("MaxItems".to_string(), size.to_string()));
            }
            if let Some(token) = &marker {
                params.push(("Marker".to_string(), token.clone()));
            }
            let borrowed: Vec<(&str, &str)> = params
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let (status, body) = call(&router, "ListUsers", &borrowed).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");

            let page = elements(&body, "UserName");
            if let Some(size) = page_size {
                assert!(
                    page.len() <= *size as usize,
                    "case {case:?}: expected at most {size} users on a page, got {}",
                    page.len()
                );
            }
            seen.extend(page);

            let truncated = element(&body, "IsTruncated");
            let page_marker = elements(&body, "Marker");
            if truncated == "true" {
                assert_eq!(
                    page_marker.len(),
                    1,
                    "case {case:?}: a truncated page must carry a Marker, got {page_marker:?}"
                );
                marker = Some(page_marker.into_iter().next().expect("a marker"));
            } else {
                assert_eq!(
                    truncated, "false",
                    "case {case:?}: expected IsTruncated true or false, got {truncated:?}"
                );
                assert!(
                    page_marker.is_empty(),
                    "case {case:?}: a final page must carry no Marker, got {page_marker:?}"
                );
                break;
            }

            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every user exactly once in store order, got {seen:?}"
        );
    }
}

/// RULE: `MaxItems` outside its published range is refused before the operation
/// runs, so a page size the model forbids never reaches the store.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = with_fixture_users().await;
    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = call(&router, "ListUsers", &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = call(&router, "ListUsers", &[("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: a `Marker` this service did not issue names no position in the store, so
/// the call is refused rather than silently answered from the beginning; a token
/// that does decode names a POSITION, which stays usable even when the entity it
/// was cut at is gone.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_is_refused() {
    let router = with_fixture_users().await;
    for bogus in ["not-a-marker", "zz", "0g"] {
        let (status, body) = call(&router, "ListUsers", &[("Marker", bogus)]).await;
        assert_eq!(status, 400, "case {bogus:?}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {bogus:?}: expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A marker cut at a user that is then deleted still resumes from that
    // position: the rest of the order follows it, minus the deleted user.
    let (status, body) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected the first page, got {status}: {body}");
    let marker = element(&body, "Marker");
    let first = element(&body, "UserName");
    let (status, _) = call(&router, "DeleteUser", &[("UserName", first.as_str())]).await;
    assert_eq!(status, 200, "expected the first user to be deleted, got {status}");

    let (status, body) = call(&router, "ListUsers", &[("Marker", marker.as_str())]).await;
    assert_eq!(status, 200, "expected a stale marker to still resume, got {status}: {body}");
    let expected: Vec<String> = users_in_store_order()
        .into_iter()
        .filter(|name| *name != first)
        .map(|name| name.to_string())
        .collect();
    assert_eq!(
        elements(&body, "UserName"),
        expected,
        "expected the order after the marker's position, got {:?}",
        elements(&body, "UserName")
    );
}

/// RULE: `PathPrefix` selects exactly the entities whose path begins with it.
#[tokio::test]
async fn list_users_filters_by_path_prefix() {
    let router = with_fixture_users().await;

    // Rows span the prefix depth: the root, a one-segment prefix, a nested
    // prefix, a prefix that selects one user, and one that selects none.
    let cases: &[(&str, &[&str])] = &[
        ("/", &["alice", "Bob", "carol_7", "dave.x", "Eve+1", "frank=2", "grace@3", "heidi-4"]),
        ("/eng/", &["alice", "carol_7", "Eve+1", "heidi-4"]),
        ("/eng/tools/", &["carol_7", "heidi-4"]),
        ("/sales/", &["dave.x", "grace@3"]),
        ("/sales/west/", &["grace@3"]),
        ("/nobody/", &[]),
    ];

    for (prefix, expected) in cases {
        let (status, body) = call(&router, "ListUsers", &[("PathPrefix", prefix)]).await;
        assert_eq!(status, 200, "case {prefix:?}: expected 200, got {status}: {body}");
        let listed = elements(&body, "UserName");
        // [`ACCOUNT_CALLER`] sits at the account root, so the root prefix selects it
        // beside the fixture's users and no deeper prefix does.
        let expected: Vec<String> = if *prefix == ACCOUNT_CALLER_PATH {
            stored_names(expected)
        } else {
            expected.iter().map(|n| (*n).to_string()).collect()
        };
        assert_eq!(
            listed, expected,
            "case {prefix:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

/// RULE: `PathPrefix` and `MaxItems` compose — a filtered enumeration pages
/// through exactly the matching entities, once each.
#[tokio::test]
async fn a_filtered_listing_pages_through_only_the_matching_users() {
    let router = with_fixture_users().await;
    let expected = ["alice", "carol_7", "Eve+1", "heidi-4"];

    for page_size in [1, 2, 3] {
        let case = format!("/eng/ with MaxItems={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let size = page_size.to_string();
            let mut params: Vec<(&str, &str)> =
                vec![("PathPrefix", "/eng/"), ("MaxItems", size.as_str())];
            if let Some(token) = &marker {
                params.push(("Marker", token.as_str()));
            }
            let (status, body) = call(&router, "ListUsers", &params).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
            seen.extend(elements(&body, "UserName"));
            if element(&body, "IsTruncated") == "true" {
                marker = Some(element(&body, "Marker"));
            } else {
                break;
            }
        }
        let expected: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }
}

/// RULE: an entity that still has dependents cannot be deleted — the delete is
/// refused with `DeleteConflict` (HTTP 409) and the entity survives; once the
/// dependents are gone the delete removes it, and its absence is what every later
/// read reports.
#[tokio::test]
async fn a_user_is_deleted_only_once_its_inline_policies_are_gone() {
    let router = with_fixture_users().await;

    // Rows differ in how many inline policies hold the user back, so the rule is
    // exercised beyond the single-policy case.
    let cases: &[(&str, &[&str])] = &[
        ("alice", &["S3Read"]),
        ("Bob", &["S3Read", "DynamoWrite"]),
        ("carol_7", &["A", "B", "C"]),
    ];

    for (user, policies) in cases {
        for policy in *policies {
            let (status, body) = call(
                &router,
                "PutUserPolicy",
                &[
                    ("UserName", user),
                    ("PolicyName", policy),
                    ("PolicyDocument", "{\"Statement\":[]}"),
                ],
            )
            .await;
            assert_eq!(status, 200, "case {user:?}: expected 200, got {status}: {body}");
        }

        // Refused while any dependent remains, including after all but one go.
        for remaining in (1..=policies.len()).rev() {
            let (status, body) = call(&router, "DeleteUser", &[("UserName", user)]).await;
            assert_eq!(
                status, 409,
                "case {user:?} with {remaining} policies: expected 409, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "DeleteConflict",
                "case {user:?} with {remaining} policies: expected DeleteConflict, got {:?}",
                error_code(&body)
            );
            let (status, _) = call(&router, "GetUser", &[("UserName", user)]).await;
            assert_eq!(
                status, 200,
                "case {user:?}: a refused delete removed the user anyway"
            );

            let policy = policies[remaining - 1];
            let (status, body) = call(
                &router,
                "DeleteUserPolicy",
                &[("UserName", user), ("PolicyName", policy)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {user:?}/{policy:?}: expected 200, got {status}: {body}"
            );
            let (status, _) = call(
                &router,
                "GetUserPolicy",
                &[("UserName", user), ("PolicyName", policy)],
            )
            .await;
            assert_eq!(
                status, 404,
                "case {user:?}/{policy:?}: the deleted policy is still readable"
            );
        }

        let (status, body) = call(&router, "DeleteUser", &[("UserName", user)]).await;
        assert_eq!(status, 200, "case {user:?}: expected 200, got {status}: {body}");

        let (status, body) = call(&router, "GetUser", &[("UserName", user)]).await;
        assert_eq!(status, 404, "case {user:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {user:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );

        let (_, listed) = call(&router, "ListUsers", &[]).await;
        assert!(
            !elements(&listed, "UserName").contains(&(*user).to_string()),
            "case {user:?}: the deleted user is still listed: {:?}",
            elements(&listed, "UserName")
        );

        // Deleting again is a not-found, not a second success.
        let (status, body) = call(&router, "DeleteUser", &[("UserName", user)]).await;
        assert_eq!(status, 404, "case {user:?}: expected 404, got {status}: {body}");
    }

    // Every user the cases did not touch is still listed, in store order.
    let remaining: Vec<String> = users_in_store_order()
        .into_iter()
        .filter(|name| !cases.iter().any(|(user, _)| user == name))
        .map(|name| name.to_string())
        .collect();
    let (_, listed) = call(&router, "ListUsers", &[]).await;
    assert_eq!(
        elements(&listed, "UserName"),
        remaining,
        "expected the untouched users to survive, got {:?}",
        elements(&listed, "UserName")
    );
}

/// RULE: a delete naming an entity the account does not hold answers
/// `NoSuchEntity` (HTTP 404), and so does one naming a policy the user does not
/// have.
#[tokio::test]
async fn deleting_an_absent_entity_is_not_found() {
    let router = with_fixture_users().await;

    for absent in ["nobody", "alic", "ALICEE"] {
        let (status, body) = call(&router, "DeleteUser", &[("UserName", absent)]).await;
        assert_eq!(status, 404, "case {absent:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }

    // A user that exists, holding one policy: another policy name is absent.
    let (status, _) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "alice"),
            ("PolicyName", "Present"),
            ("PolicyDocument", "{\"Statement\":[]}"),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected the policy to be stored, got {status}");
    for absent in ["Absent", "Presentt", "Presen"] {
        let (status, body) = call(
            &router,
            "DeleteUserPolicy",
            &[("UserName", "alice"), ("PolicyName", absent)],
        )
        .await;
        assert_eq!(status, 404, "case {absent:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: an account holds a bounded number of users, and a create past the quota
/// is refused with `LimitExceeded` (HTTP 409) rather than growing the account.
///
/// This drives one router round-trip per user up to the quota — about a second in
/// total; keep that in mind if more quota rows land here.
#[tokio::test]
async fn creating_past_the_user_quota_is_refused() {
    // The quota the handler enforces, read from the service's own constant so the
    // test cannot drift from it — and pinned to the number AWS publishes, so a
    // wrong constant fails here rather than silently moving the bound.
    const QUOTA: usize =
        service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_USERS_PER_ACCOUNT;
    assert_eq!(
        QUOTA, 5_000,
        "AWS's IAM and STS quotas page gives \"Users in an account: 5,000\", got {QUOTA}"
    );

    let router = router();
    // The account already holds [`ACCOUNT_CALLER`], and the quota is the number of users
    // the account may hold in total — so the creates that fit inside it are one fewer.
    for n in 0..QUOTA - 1 {
        let name = format!("quota_user_{n}");
        let (status, body) = call(&router, "CreateUser", &[("UserName", name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 up to the quota, got {status}: {body}"
        );
    }

    // Rows: the first create past the quota, and a second one, each refused and
    // each leaving the account at the quota.
    for attempt in ["over_the_line", "over_the_line_again"] {
        let (status, body) = call(&router, "CreateUser", &[("UserName", attempt)]).await;
        assert_eq!(status, 409, "case {attempt:?}: expected 409, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {attempt:?}: expected LimitExceeded, got {:?}",
            error_code(&body)
        );
        let (status, body) = call(&router, "GetUser", &[("UserName", attempt)]).await;
        assert_eq!(
            status, 404,
            "case {attempt:?}: the refused create stored the user anyway, got {status}: {body}"
        );
    }

    // Deleting one makes room for exactly one more.
    let (status, _) = call(&router, "DeleteUser", &[("UserName", "quota_user_0")]).await;
    assert_eq!(status, 200, "expected the delete to succeed, got {status}");
    let (status, body) = call(&router, "CreateUser", &[("UserName", "replacement")]).await;
    assert_eq!(status, 200, "expected the freed slot to be usable, got {status}: {body}");
    let (status, _) = call(&router, "CreateUser", &[("UserName", "one_too_many")]).await;
    assert_eq!(status, 409, "expected the account to be full again, got {status}");
}

/// RULE: the inline policies embedded in one user have a bounded aggregate size,
/// and a put that would carry the user past it is refused with `LimitExceeded`
/// (HTTP 409); replacing a policy counts only the replacement.
#[tokio::test]
async fn an_inline_policy_set_cannot_exceed_its_aggregate_size_quota() {
    const QUOTA: usize = service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_USER_INLINE_POLICY_CHARACTERS;
    assert_eq!(
        QUOTA, 2_048,
        "AWS's IAM and STS quotas page gives \"User policy size cannot exceed 2,048 \
         characters\", got {QUOTA}"
    );

    /// A syntactically valid policy document padded to exactly `characters`.
    fn document_of(characters: usize) -> String {
        let prefix = "{\"Sid\":\"";
        let suffix = "\",\"Statement\":[]}";
        let padding = characters - prefix.len() - suffix.len();
        format!("{prefix}{}{suffix}", "p".repeat(padding))
    }

    let router = router();
    let (status, _) = call(&router, "CreateUser", &[("UserName", "bounded")]).await;
    assert_eq!(status, 200, "expected the user to be created, got {status}");

    // One document exactly at the quota is accepted; one character past it is not.
    let at_quota = document_of(QUOTA);
    assert_eq!(at_quota.chars().count(), QUOTA, "the fixture must sit on the bound");
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "bounded"),
            ("PolicyName", "Big"),
            ("PolicyDocument", at_quota.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "case at-the-bound: expected 200, got {status}: {body}");

    // A second policy of any size would carry the user past the aggregate quota —
    // even the smallest document the IAM policy grammar admits (`Statement` alone).
    for (case, extra) in [
        ("smallest_valid_document", "{\"Statement\":[]}".to_string()),
        ("half_the_quota", document_of(QUOTA / 2)),
    ] {
        let (status, body) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", "bounded"),
                ("PolicyName", "Extra"),
                ("PolicyDocument", extra.as_str()),
            ],
        )
        .await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: expected LimitExceeded, got {:?}",
            error_code(&body)
        );
        let (status, _) = call(
            &router,
            "GetUserPolicy",
            &[("UserName", "bounded"), ("PolicyName", "Extra")],
        )
        .await;
        assert_eq!(status, 404, "case {case:?}: the refused put stored the document anyway");
    }

    // A document past the quota on its own is refused too.
    let past_quota = document_of(QUOTA + 1);
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "bounded"),
            ("PolicyName", "Oversize"),
            ("PolicyDocument", past_quota.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 409, "case past-the-bound: expected 409, got {status}: {body}");

    // REPLACING the existing policy counts only the replacement, so a document
    // that fits is accepted even though the user is already at the quota.
    let replacement = document_of(QUOTA);
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "bounded"),
            ("PolicyName", "Big"),
            ("PolicyDocument", replacement.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "case replace-at-the-bound: expected 200, got {status}: {body}");
    let (_, body) = call(
        &router,
        "GetUserPolicy",
        &[("UserName", "bounded"), ("PolicyName", "Big")],
    )
    .await;
    assert_eq!(
        document_element(&body, "PolicyDocument"),
        replacement,
        "case replace-at-the-bound: expected the replacement to be stored"
    );

    // And once the big policy is gone the user's whole budget is free again.
    let (status, _) = call(
        &router,
        "DeleteUserPolicy",
        &[("UserName", "bounded"), ("PolicyName", "Big")],
    )
    .await;
    assert_eq!(status, 200, "expected the policy to be deleted, got {status}");
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "bounded"),
            ("PolicyName", "Extra"),
            ("PolicyDocument", at_quota.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected the freed budget to be usable, got {status}: {body}");
}

/// RULE: an IAM name is not distinguished by case, so one spelling of a policy
/// name reaches the policy any other spelling of it stored, and a read reports
/// the name as the account holds it.
#[tokio::test]
async fn an_inline_policy_name_is_not_distinguished_by_case() {
    let router = router();
    let (status, _) = call(&router, "CreateUser", &[("UserName", "Casey")]).await;
    assert_eq!(status, 200, "expected the user to be created, got {status}");

    let document = "{\"Sid\":\"one\",\"Statement\":[]}";
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "Casey"),
            ("PolicyName", "S3Read"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected the policy to be stored, got {status}: {body}");

    // Rows differ in the CASE of BOTH names, which is what the rule folds over.
    let spellings: &[(&str, &str)] = &[
        ("Casey", "S3Read"),
        ("casey", "s3read"),
        ("CASEY", "S3READ"),
        ("cASEy", "s3ReaD"),
    ];
    for (user, policy) in spellings {
        let (status, body) = call(
            &router,
            "GetUserPolicy",
            &[("UserName", user), ("PolicyName", policy)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case ({user:?}, {policy:?}): expected 200, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "UserName"),
            "Casey",
            "case ({user:?}, {policy:?}): expected the stored user name, got {:?}",
            element(&body, "UserName")
        );
        assert_eq!(
            element(&body, "PolicyName"),
            "S3Read",
            "case ({user:?}, {policy:?}): expected the stored policy name, got {:?}",
            element(&body, "PolicyName")
        );
        assert_eq!(
            document_element(&body, "PolicyDocument"),
            document,
            "case ({user:?}, {policy:?}): expected the stored document, got {:?}",
            document_element(&body, "PolicyDocument")
        );
    }

    // A put under another spelling REPLACES that one policy — it does not add a
    // second — and the stored name becomes the spelling the last put chose.
    let replaced = "{\"Sid\":\"two\",\"Statement\":[]}";
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "casey"),
            ("PolicyName", "s3read"),
            ("PolicyDocument", replaced),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected the replacing put to succeed, got {status}: {body}");
    let (_, body) = call(
        &router,
        "GetUserPolicy",
        &[("UserName", "Casey"), ("PolicyName", "S3Read")],
    )
    .await;
    assert_eq!(
        document_element(&body, "PolicyDocument"),
        replaced,
        "expected the replacing document, got {:?}",
        document_element(&body, "PolicyDocument")
    );
    assert_eq!(
        element(&body, "PolicyName"),
        "s3read",
        "expected the last put's spelling to be the stored one, got {:?}",
        element(&body, "PolicyName")
    );

    // Exactly one policy is embedded, so the user deletes once the one is gone.
    let (status, body) = call(
        &router,
        "DeleteUserPolicy",
        &[("UserName", "CASEY"), ("PolicyName", "S3READ")],
    )
    .await;
    assert_eq!(status, 200, "expected the delete to succeed, got {status}: {body}");
    let (status, body) = call(&router, "DeleteUser", &[("UserName", "casey")]).await;
    assert_eq!(status, 200, "expected the user to delete, got {status}: {body}");
}

/// RULE: a name outside the published constraints of the member is refused before
/// the operation runs, so it never reaches the store.
#[tokio::test]
async fn a_name_outside_the_published_constraints_is_refused() {
    let router = router();
    // `userNameType` is len 1..64, pattern ^[\w+=,.@-]+$: each bound and one past
    // it, plus a character the alphabet excludes.
    let at_max = "u".repeat(64);
    let past_max = "u".repeat(65);
    let accepted: &[&str] = &["u", at_max.as_str()];
    let refused: &[&str] = &["", past_max.as_str(), "has space", "has/slash"];

    for name in accepted {
        let (status, body) = call(&router, "CreateUser", &[("UserName", name)]).await;
        assert_eq!(
            status, 200,
            "case (len {}): expected 200, got {status}: {body}",
            name.len()
        );
    }
    for name in refused {
        let (status, body) = call(&router, "CreateUser", &[("UserName", name)]).await;
        assert_eq!(
            status, 400,
            "case {name:?} (len {}): expected 400, got {status}: {body}",
            name.len()
        );
        let (status, _) = call(&router, "ListUsers", &[]).await;
        assert_eq!(status, 200, "case {name:?}: the store must still be readable");
    }

    let (_, listed) = call(&router, "ListUsers", &[]).await;
    assert_eq!(
        elements(&listed, "UserName"),
        stored_names(accepted),
        "expected only the accepted names to be stored beside the user the account \
         already held, got {:?}",
        elements(&listed, "UserName")
    );
}

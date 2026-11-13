// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `CreatePresignedMlflowAppUrl`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on the
//! shared `Emulator` harness in `inference_deployment_lifecycle`.
//!
//! The suites assert RULES over TABLES of inputs rather than one recorded call: the URL's
//! form for every region and every app shape, the app's own opaque ARN id addressing it
//! whatever the request signed for, the fresh token each call mints, the
//! `ValidationException` refusal for an ARN no app was created under (the code and fixed
//! message the recorded AWS response carries), the two validity members at and past each
//! published bound, and that presigning changes nothing a later `DescribeMlflowApp`
//! reports beyond the transition the clock owed anyway — and retains the minted token in
//! nothing it reports.
//!
//! The published URL addresses the MLflow session on THIS emulator's own endpoint, because
//! no client of the emulator resolves AWS's managed-UI host; the address and the session
//! served at it are `tests/all/presigned_session.rs`'s subject.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
// The MLflow app harness lives with the create/describe suite, so the create body, the
// role ARN, the ARN-parsing rule and the due span have ONE definition in this crate.
use crate::mlflow_app_lifecycle::{minimal, mlflow_app_id, PAST_CREATED_DUE, ROLE};

/// The base this emulator publishes a session address on: in-process the router is served
/// on no port, so `ctx.endpoint()` answers
/// [`smithy_server_runtime::UNSERVED_URL_BASE`], where a served emulator answers its data
/// port's base.
fn session_base() -> &'static str {
    smithy_server_runtime::UNSERVED_URL_BASE
}

/// A loose floor on the authorization token's length: the emulator's credential store
/// mints a session token of several hundred characters, and this suite pins only that the
/// URL carries a substantial opaque token rather than the store's exact length.
const MIN_TOKEN_LENGTH: usize = 40;

/// Create an app in `region` and answer the ARN the operation minted.
async fn create_in(emulator: &Emulator, case: &str, region: &str, body: &Value) -> String {
    let (status, created) = emulator.call_in(region, "CreateMlflowApp", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateMlflowApp must answer HTTP 200, got {status} with {created:?}"
    );
    str_field(&created, "Arn")
}

/// The `authToken` query value of a presigned MLflow URL whose host and path are the ones
/// the rule prescribes for `arn`. Panics — with the case named — when the URL takes any
/// other form, so every caller gets the same structural check.
///
/// `_region` is the app's own Region, which the address does not carry: the session is
/// served at the endpoint the caller is already talking to, and the app's opaque ARN id is
/// what identifies it there.
fn auth_token(case: &str, url: &str, arn: &str, _region: &str) -> String {
    let expected_prefix = format!(
        "{}/_aws/sagemaker/mlflow-app/{}/auth?authToken=",
        session_base(),
        mlflow_app_id(arn)
    );
    let token = url.strip_prefix(&expected_prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a URL starting {expected_prefix:?}, got {url:?}")
    });

    // `MlflowAppUrl` is modelled len 0..2048.
    assert!(
        url.len() <= 2048,
        "case {case:?}: MlflowAppUrl is modelled len 0..2048, got {} bytes in {url:?}",
        url.len()
    );
    // One query parameter, carrying the authorization token: nothing else is appended.
    assert_eq!(
        url.matches('?').count(),
        1,
        "case {case:?}: expected exactly one query string, got {url:?}"
    );
    assert!(
        !token.contains('&'),
        "case {case:?}: expected the authToken parameter alone, got {url:?}"
    );
    // The token is percent-encoded, so the query value carries only RFC 3986 unreserved
    // characters and `%` escapes — never the `+` or `/` the minted alphabet includes.
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"%-._~".contains(&byte)),
        "case {case:?}: expected a percent-encoded token, got {token:?}"
    );
    assert!(
        token.len() >= MIN_TOKEN_LENGTH,
        "case {case:?}: expected a token of at least {MIN_TOKEN_LENGTH} characters, got {}",
        token.len()
    );
    token.to_string()
}

/// RULE: the operation answers a presigned URL for the app the store holds under the
/// requested ARN — the session address on this emulator's own endpoint, naming the app's
/// own opaque ARN id and carrying a freshly minted authorization token — whatever the
/// app's members and whatever region it was created in.
#[tokio::test]
async fn a_presigned_url_names_the_stored_app_its_region_and_a_minted_authorization_token() {
    // Rows differ along the axes the rule generalises over: the signed region, the app's
    // name length and shape, which optional create members it carries, and which of the
    // two validity members the presign request names.
    let at_the_name_upper_bound = format!("a{}", "b".repeat(255));
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "minimal-app-no-validity-members",
            "us-east-1",
            minimal("presign-minimal"),
            json!({}),
        ),
        (
            "name-at-the-lower-bound-with-expiry",
            "eu-west-3",
            // A single character outside the minted id's hex alphabet, so the
            // "name is not in the URL" check below bites on this row too.
            minimal("z"),
            json!({ "ExpiresInSeconds": 60 }),
        ),
        (
            "name-at-the-upper-bound-with-session-window",
            "ap-southeast-2",
            minimal(&at_the_name_upper_bound),
            json!({ "SessionExpirationDurationInSeconds": 3_600 }),
        ),
        (
            "every-optional-create-member-and-both-validity-members",
            "us-gov-west-1",
            json!({
                "Name": "presign-full",
                "ArtifactStoreUri": "s3://mlflow-artifacts/full/",
                "RoleArn": ROLE,
                "KmsKeyId": "alias/mlflow-key",
                "ModelRegistrationMode": "AutoModelRegistrationEnabled",
                "WeeklyMaintenanceWindowStart": "Tue:03:30",
                "AccountDefaultStatus": "ENABLED",
                "DefaultDomainIdList": ["d-abc123456789"],
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
            json!({ "ExpiresInSeconds": 300, "SessionExpirationDurationInSeconds": 43_200 }),
        ),
    ];

    let mut tokens: Vec<String> = Vec::new();
    for (case, region, create_body, validity) in &cases {
        let emulator = Emulator::new();
        let arn = create_in(&emulator, case, region, create_body).await;

        let mut body = validity.clone();
        body["Arn"] = json!(arn.clone());
        let (status, parsed) = emulator
            .call_in(region, "CreatePresignedMlflowAppUrl", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
        );

        let url = str_field(&parsed, "AuthorizedUrl");
        let token = auth_token(case, &url, &arn, region);
        // The app's name is never part of the URL's host or path: the ARN's id identifies
        // the app, and two apps may share one name. Only the authority and path are
        // checked — the token is random text, so a short name can occur inside it by
        // chance — and every row's name carries a character outside the id's hex
        // alphabet, so the check bites whatever the name's length.
        let name = str_field(create_body, "Name");
        let host_and_path = url.split('?').next().unwrap_or(url.as_str());
        assert!(
            !host_and_path.contains(&name),
            "case {case:?}: the app's name must not be part of the URL, got {host_and_path:?}"
        );
        assert!(
            !tokens.contains(&token),
            "case {case:?}: every call must mint a distinct token, got one twice"
        );
        tokens.push(token);
    }
}

/// RULE: the URL names the APP, not the region the presign request signed for — the
/// resource's identity decides which session the address opens, so a caller signing for
/// another region still gets the app's own address.
#[tokio::test]
async fn the_url_names_the_region_of_the_app_not_the_region_the_request_signed_for() {
    let emulator = Emulator::new();

    // Rows: one app created in a single region, presigned while signed for its own region
    // (the control) and for three others.
    let home = "eu-west-3";
    let arn = create_in(&emulator, "cross-region", home, &minimal("cross-region-app")).await;

    for signing_region in [home, "us-east-1", "ap-southeast-2", "us-gov-west-1"] {
        let case = format!("signed-for-{signing_region}");
        let (status, parsed) = emulator
            .call_in(
                signing_region,
                "CreatePresignedMlflowAppUrl",
                &json!({ "Arn": arn.clone() }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {parsed:?}"
        );
        // `auth_token` requires the host to name `home`, whatever the request signed for.
        auth_token(&case, &str_field(&parsed, "AuthorizedUrl"), &arn, home);
    }
}

/// RULE: the presigned URL carries a grant of its own, so every call mints a FRESH
/// authorization token — while the address it opens the session at stays the app's own,
/// because it is derived from the app's ARN rather than from the call.
#[tokio::test]
async fn every_call_for_one_app_mints_a_fresh_token_under_the_same_host() {
    let emulator = Emulator::new();
    let arn = create_in(&emulator, "repeat", "us-east-1", &minimal("presign-repeat")).await;
    let host_and_path = format!(
        "{}/_aws/sagemaker/mlflow-app/{}/auth?authToken=",
        session_base(),
        mlflow_app_id(&arn)
    );

    // Rows: the same app presigned repeatedly, with and without the validity members, so
    // the freshness does not depend on what the request carried.
    let bodies = [
        json!({ "Arn": arn.clone() }),
        json!({ "Arn": arn.clone(), "ExpiresInSeconds": 5 }),
        json!({ "Arn": arn.clone(), "SessionExpirationDurationInSeconds": 1_800 }),
        json!({
            "Arn": arn.clone(),
            "ExpiresInSeconds": 5,
            "SessionExpirationDurationInSeconds": 1_800,
        }),
    ];

    let mut tokens: Vec<String> = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        let case = format!("call-{index}");
        let parsed = emulator.ok("CreatePresignedMlflowAppUrl", body).await;
        let url = str_field(&parsed, "AuthorizedUrl");
        assert!(
            url.starts_with(&host_and_path),
            "case {case:?}: expected the app's own session address {host_and_path:?}, got \
             {url:?}"
        );
        let token = auth_token(&case, &url, &arn, "us-east-1");
        assert!(
            !tokens.contains(&token),
            "case {case:?}: a repeated call must mint a new token, got {} again",
            &token[..8]
        );
        tokens.push(token);
    }
}

/// RULE: `CreatePresignedMlflowAppUrl` refuses an ARN no MLflow App was created under
/// with HTTP 400 `ValidationException` and one FIXED message — the status, code and text
/// the recorded AWS response carries, rather than the modelled `ResourceNotFound` — and
/// leaves the store untouched.
#[tokio::test]
async fn presigning_an_arn_no_app_was_created_under_is_refused_as_a_validation_failure() {
    let emulator = Emulator::new();
    let existing = create_in(&emulator, "existing", "us-east-1", &minimal("exists")).await;
    let existing_id = mlflow_app_id(&existing).to_string();

    // Rows: an unknown id, the known id in ANOTHER region and under another account (the
    // ARN is the app's whole identity), an id of a different length, and a deleted-looking
    // id — every row satisfies `MlflowAppArn`'s modelled pattern.
    let cases: Vec<(&str, String)> = vec![
        (
            "unknown-id",
            format!(
                "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/ffffffffffffffffffffffffffffffff"
            ),
        ),
        (
            "known-id-in-another-region",
            format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:mlflow-app/{existing_id}"),
        ),
        (
            "known-id-under-another-account",
            format!("arn:aws:sagemaker:us-east-1:210987654321:mlflow-app/{existing_id}"),
        ),
        (
            "short-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/a"),
        ),
    ];

    for (case, arn) in &cases {
        let (status, parsed) = emulator
            .call("CreatePresignedMlflowAppUrl", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the recorded refusal's HTTP 400, got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: expected the error code the recorded AWS response carries, got \
             {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            // The text the recorded AWS response carries, spelled out here so the pin is
            // independent of the constant the handler returns.
            "Resource does not exist",
            "case {case:?}: the refusal must carry the fixed message the recorded response \
             carries, got {:?}",
            message(&parsed)
        );
        assert!(
            !message(&parsed).contains(arn.as_str()),
            "case {case:?}: the fixed message repeats nothing from the request, got {:?}",
            message(&parsed)
        );
        assert!(
            parsed.get("AuthorizedUrl").is_none(),
            "case {case:?}: a refused call presigns nothing, got {parsed:?}"
        );
    }

    // The refusals changed nothing: the app that does exist still presigns and still
    // reads back.
    let parsed = emulator
        .ok(
            "CreatePresignedMlflowAppUrl",
            &json!({ "Arn": existing.clone() }),
        )
        .await;
    auth_token(
        "existing-after-refusals",
        &str_field(&parsed, "AuthorizedUrl"),
        &existing,
        "us-east-1",
    );
    let described = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": existing.clone() }))
        .await;
    assert_eq!(
        str_field(&described, "Arn"),
        existing,
        "case existing-after-refusals: the stored app must be untouched, got {described:?}"
    );
}

/// RULE: the two validity members are accepted anywhere inside the ranges the model
/// publishes (`ExpiresInSeconds` 5..300, `SessionExpirationDurationInSeconds`
/// 1800..43200) and refused one step outside them, before the handler runs — and neither
/// changes the answer's shape, which carries the one member the model declares.
#[tokio::test]
async fn the_validity_members_are_accepted_inside_their_published_ranges_and_refused_outside() {
    let emulator = Emulator::new();
    let arn = create_in(&emulator, "validity", "us-east-1", &minimal("presign-validity")).await;

    // Accepted rows: both published bounds of each range and a value between them.
    let accepted: Vec<(&str, Value)> = vec![
        ("expiry-at-the-published-minimum", json!({ "ExpiresInSeconds": 5 })),
        ("expiry-between-the-bounds", json!({ "ExpiresInSeconds": 123 })),
        ("expiry-at-the-published-maximum", json!({ "ExpiresInSeconds": 300 })),
        (
            "session-at-the-published-minimum",
            json!({ "SessionExpirationDurationInSeconds": 1_800 }),
        ),
        (
            "session-between-the-bounds",
            json!({ "SessionExpirationDurationInSeconds": 20_000 }),
        ),
        (
            "session-at-the-published-maximum",
            json!({ "SessionExpirationDurationInSeconds": 43_200 }),
        ),
    ];
    for (case, validity) in &accepted {
        let mut body = validity.clone();
        body["Arn"] = json!(arn.clone());
        let (status, parsed) = emulator
            .call("CreatePresignedMlflowAppUrl", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value inside the published range is accepted, got {status} with \
             {parsed:?}"
        );
        auth_token(case, &str_field(&parsed, "AuthorizedUrl"), &arn, "us-east-1");
        assert_eq!(
            parsed.as_object().map(|members| members.len()),
            Some(1),
            "case {case:?}: the answer carries AuthorizedUrl alone, got {parsed:?}"
        );
    }

    // Refused rows: one step past each published bound, plus zero and a negative.
    let refused: Vec<(&str, Value)> = vec![
        ("expiry-one-below-the-minimum", json!({ "ExpiresInSeconds": 4 })),
        ("expiry-one-above-the-maximum", json!({ "ExpiresInSeconds": 301 })),
        ("expiry-zero", json!({ "ExpiresInSeconds": 0 })),
        ("expiry-negative", json!({ "ExpiresInSeconds": -1 })),
        (
            "session-one-below-the-minimum",
            json!({ "SessionExpirationDurationInSeconds": 1_799 }),
        ),
        (
            "session-one-above-the-maximum",
            json!({ "SessionExpirationDurationInSeconds": 43_201 }),
        ),
        (
            "session-zero",
            json!({ "SessionExpirationDurationInSeconds": 0 }),
        ),
    ];
    for (case, validity) in &refused {
        let mut body = validity.clone();
        body["Arn"] = json!(arn.clone());
        let (status, parsed) = emulator
            .call("CreatePresignedMlflowAppUrl", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the published range is refused, got {status} with \
             {parsed:?}"
        );
        assert!(
            parsed.get("AuthorizedUrl").is_none(),
            "case {case:?}: a refused call presigns nothing, got {parsed:?}"
        );
    }
}

/// RULE: minting a presigned URL is not a change to the app — a later `DescribeMlflowApp`
/// reports exactly what it would have reported anyway, including the `Creating` ->
/// `Created` transition the CLOCK owes rather than one a presign call brought forward.
#[tokio::test]
async fn presigning_changes_nothing_a_later_describe_reports_beyond_what_the_clock_owed() {
    let emulator = Emulator::new();
    let arn = create_in(&emulator, "stability", "us-east-1", &minimal("presign-stable")).await;

    let before = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
        .await;
    assert_eq!(
        str_field(&before, "Status"),
        "Creating",
        "case immediately-after-create: Status, got {before:?}"
    );

    // Three presigns inside the due span: the transient status survives them, so no
    // presign moves the app's status, and the token the caller received is kept nowhere
    // the service reports.
    for index in 0..3 {
        let parsed = emulator
            .ok("CreatePresignedMlflowAppUrl", &json!({ "Arn": arn.clone() }))
            .await;
        let case = format!("presign-{index}-before-the-due-span");
        let token = auth_token(&case, &str_field(&parsed, "AuthorizedUrl"), &arn, "us-east-1");
        let during = emulator
            .ok("DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
            .await;
        assert_eq!(
            during, before,
            "case {case:?}: the app must read back identically, got {during:?}"
        );
        assert!(
            !during.to_string().contains(&token),
            "case {case:?}: the minted token must be retained in no reported state, got \
             {during:?}"
        );
    }

    tokio::time::sleep(PAST_CREATED_DUE).await;

    // Past the due span the presign materialises what the clock already owed — the same
    // transition a describe would have materialised, and nothing else.
    let parsed = emulator
        .ok("CreatePresignedMlflowAppUrl", &json!({ "Arn": arn.clone() }))
        .await;
    auth_token(
        "presign-after-the-due-span",
        &str_field(&parsed, "AuthorizedUrl"),
        &arn,
        "us-east-1",
    );
    let after = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
        .await;
    assert_eq!(
        str_field(&after, "Status"),
        "Created",
        "case after-the-due-span: Status, got {after:?}"
    );
    assert_eq!(
        after.get("CreationTime"),
        before.get("CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {after:?}"
    );
    for member in ["Arn", "Name", "ArtifactStoreUri", "RoleArn", "ModelRegistrationMode"] {
        assert_eq!(
            after.get(member),
            before.get(member),
            "case after-the-due-span: {member} must not change, got {after:?}"
        );
    }

    // And a second presign after the transition leaves the settled app exactly as it was.
    let parsed = emulator
        .ok("CreatePresignedMlflowAppUrl", &json!({ "Arn": arn.clone() }))
        .await;
    auth_token(
        "presign-after-the-transition",
        &str_field(&parsed, "AuthorizedUrl"),
        &arn,
        "us-east-1",
    );
    let again = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        again, after,
        "case presign-after-the-transition: a settled app must read back identically"
    );
}

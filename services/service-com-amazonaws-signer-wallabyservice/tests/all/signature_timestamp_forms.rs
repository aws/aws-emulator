// SPDX-License-Identifier: CC0-1.0

//! How `GetRevocationStatus` reads the `signatureTimestamp` it takes on the query string,
//! and how it states a refusal of a value a request carried, end to end over HTTP.
//!
//! Three rules under test, all of them about how this endpoint READS a request and how it
//! words what it refuses:
//!
//! 1. A timestamp bound to the query string carries an INSTANT, and the renderings a
//!    client puts that instant on the wire in belong to the protocol rather than to one
//!    member — the RFC 3339 `date-time`, with the `Z` zone or with the numeric UTC offset
//!    RFC 3339 equally admits, and the number of seconds since the epoch, whole or
//!    fractional. Every one of them names the same instant, so every one of them binds
//!    it, and a revocation effective at that instant is reported for a signature made AT
//!    it and not for one made a second earlier.
//! 2. A value that names NO instant never becomes a modelled input at all, so it is not a
//!    violation of a constraint the model declares but a value the protocol could not
//!    convert: HTTP 400 with the framework's own error envelope (the capitalized
//!    `Message` member and the protocol errortype), not this service's modelled
//!    `ValidationException`.
//! 3. A constraint clause QUOTES the value that broke the constraint as the request
//!    carried it, however long that value was — up to the one ceiling this service holds
//!    every echo to, so no request parameter can make the error body as large as itself.
//!
//! Every assertion is driven over a TABLE. The rows differ along what each rule
//! generalises over: the FORM of the timestamp (the `date-time` the framework's own reader
//! takes, the epoch-seconds number, the four zone suffixes RFC 3339 admits, with both
//! signs) and where the instant it names falls relative to the revocation's effective
//! time; the kind of unreadable value and the percent-escaping it arrives under; and the
//! LENGTH of a refused value, walked across the framework's own echo ceiling and the
//! modelled maximum of the member that carried it.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents, spelled out so a wrong account in the
/// handlers fails here.
const ACCOUNT_ID: &str = "000000000000";

/// A published platform that supports revocation and requires no signing material
/// (`SIGNING_PLATFORM_CATALOG`), so one PUT creates a profile a revocation can be recorded
/// against.
const PLATFORM: &str = "Notation-OCI-SHA384-ECDSA";

/// The one `certificateHashes` entry these requests carry. This service revokes profiles
/// and signatures rather than certificates, so no hash is ever reported: the value only has
/// to be one the request can carry.
const CERTIFICATE_HASH: &str = "aa";

/// The instant a revocation below takes effect, in epoch seconds.
const EFFECTIVE_EPOCH_SECS: i64 = 1_700_000_000;

/// The framework's own ceiling on the value it renders into a constraint clause
/// (`MAX_ECHOED_VALUE` in `vendor/smithy-schema-server/src/constraints.rs`): past it the
/// walk stores a prefix plus `... [<n> bytes]`, which is what this service restores.
const FRAMEWORK_ECHO_CEILING: usize = 256;

/// The modelled maximum of an `Arn` member (`smithy_query(query="shape", shape="Arn")`:
/// `@length(min: 20, max: 2048)`), so a value past it is refused by the boundary validator
/// and its clause is the one under test.
const ARN_MAX_LENGTH: usize = 2048;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the `x-amzn-errortype` it named (the header
/// restJson1 resolves an error shape from) and the body as JSON.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The modelled `message` member, or the empty string when the body carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The framework envelope's capitalized `Message` member, or the empty string when the
    /// body carries none.
    fn framework_message(&self) -> &str {
        self.body["Message"].as_str().unwrap_or_default()
    }

    /// The `revokedEntities` a 200 answer reports.
    fn revoked_entities(&self) -> Vec<String> {
        self.body["revokedEntities"]
            .as_array()
            .map(|entities| {
                entities
                    .iter()
                    .map(|entity| entity.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", authorization);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body().to_vec();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Create a signing profile and return the ARN of the version the create minted, together
/// with that version.
async fn create_profile(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the profile create must be served, got {} with {:?}",
        answer.status, answer.body
    );
    (
        answer.body["profileVersionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("the create must answer a profileVersionArn: {:?}", answer.body))
            .to_string(),
        answer.body["profileVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("the create must answer a profileVersion: {:?}", answer.body))
            .to_string(),
    )
}

/// Revoke that profile version from `effective_epoch_secs` on.
async fn revoke_profile(
    router: &MultiServiceRouter,
    name: &str,
    version: &str,
    effective_epoch_secs: i64,
) {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": version,
            "reason": "the signing material is no longer trusted",
            "effectiveTime": effective_epoch_secs,
        })),
    )
    .await;
    assert!(
        answer.status == 200 || answer.status == 204,
        "the profile revocation must be served, got {} with {:?}",
        answer.status,
        answer.body
    );
}

/// A signing-job ARN this endpoint holds no job for, so the answer is about the profile
/// version alone.
fn unknown_job_arn() -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/no-such-job")
}

/// `GET /revocations` with the query parameters exactly as the caller gives them: this is
/// the wire, so a row states the wire form it wants.
async fn revocations(router: &MultiServiceRouter, params: &[(&str, String)]) -> Answer {
    let query = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    call(router, "GET", &format!("/revocations?{query}"), None).await
}

/// A complete, valid query asking about `profile_version_arn` at the instant `timestamp`
/// spells.
fn params(profile_version_arn: &str, timestamp: String) -> Vec<(&'static str, String)> {
    vec![
        ("signatureTimestamp", timestamp),
        ("platformId", PLATFORM.to_string()),
        ("profileVersionArn", profile_version_arn.to_string()),
        ("jobArn", unknown_job_arn()),
        ("certificateHashes", CERTIFICATE_HASH.to_string()),
    ]
}

/// The RFC-3339 `Z` rendering of an epoch instant — the form the framework's own
/// query-value reader takes.
fn date_time(epoch_secs: i64) -> String {
    DateTime::from_secs(epoch_secs)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time")
}

/// `epoch_secs` rendered as RFC 3339 in the zone `offset_seconds` names, with `suffix` as
/// its zone: the SAME instant, spelled the way a client in that zone spells it.
fn zoned(epoch_secs: i64, offset_seconds: i64, suffix: &str) -> String {
    let civil = date_time(epoch_secs + offset_seconds);
    let civil = civil
        .strip_suffix('Z')
        .expect("the date-time rendering ends in Z");
    format!("{civil}{suffix}")
}

/// One wire rendering of a `signatureTimestamp`, and the instant it names.
struct Form {
    case: &'static str,
    render: fn(i64) -> String,
}

/// Every form the wire names an instant in. The `Z`-zoned `date-time` is the one the
/// framework's own query-value reader takes; every other row is a form it does not.
fn forms() -> Vec<Form> {
    vec![
        Form {
            case: "date-time with the Z zone",
            render: date_time,
        },
        Form {
            case: "date-time with the Z zone, colons percent-encoded",
            render: |at| date_time(at).replace(':', "%3A"),
        },
        Form {
            case: "date-time with a lowercase z zone",
            render: |at| zoned(at, 0, "z"),
        },
        Form {
            // The `+` is percent-escaped, as a query component must escape it: a raw `+`
            // is a SPACE in a query string.
            case: "date-time with a zero numeric offset",
            render: |at| zoned(at, 0, "%2B00:00"),
        },
        Form {
            case: "date-time with a positive offset in the hh:mm form",
            render: |at| zoned(at, 5 * 3600 + 1800, "%2B05:30"),
        },
        Form {
            case: "date-time with a negative offset in the hh:mm form",
            render: |at| zoned(at, -8 * 3600, "-08:00"),
        },
        Form {
            case: "date-time with a positive offset in the hhmm form",
            render: |at| zoned(at, 5 * 3600 + 1800, "%2B0530"),
        },
        Form {
            case: "date-time with a negative offset in the hh form",
            render: |at| zoned(at, -8 * 3600, "-08"),
        },
        Form {
            case: "epoch seconds",
            render: |at| at.to_string(),
        },
        Form {
            case: "epoch seconds with a fractional part",
            render: |at| format!("{at}.000"),
        },
    ]
}

/// Every form binds the instant it names: the revocation effective at an instant is
/// reported for a signature made AT it and not for one made a second earlier, whichever
/// form the timestamp was sent in.
#[tokio::test]
async fn the_signature_timestamp_binds_in_every_wire_form_of_an_instant() {
    let router = emulator();
    let (profile_version_arn, version) = create_profile(&router, "Timestamp_Forms").await;
    revoke_profile(
        &router,
        "Timestamp_Forms",
        &version,
        EFFECTIVE_EPOCH_SECS,
    )
    .await;

    for form in forms() {
        for (at, when, expected) in [
            (
                EFFECTIVE_EPOCH_SECS,
                "on the effective time",
                vec![profile_version_arn.clone()],
            ),
            (
                EFFECTIVE_EPOCH_SECS - 1,
                "one second before the effective time",
                Vec::new(),
            ),
        ] {
            let case = format!("{} {when}", form.case);
            let answer = revocations(
                &router,
                &params(&profile_version_arn, (form.render)(at)),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.revoked_entities(),
                expected,
                "case {case}: expected {expected:?} for the instant this form names, got {:?}",
                answer.body
            );
        }
    }
}

/// A `signatureTimestamp` that names no instant is refused by the PROTOCOL: HTTP 400, the
/// framework's errortype, and the sentence under the capitalized `Message` member naming
/// the value it could not convert — with no modelled `message`/`code` pair, since the value
/// never became a modelled input.
#[tokio::test]
async fn a_signature_timestamp_that_names_no_instant_is_refused_by_the_protocol() {
    let router = emulator();
    let (profile_version_arn, _) = create_profile(&router, "No_Instant").await;

    // (case, the value as the wire carries it, the value the refusal echoes)
    let rows: [(&str, &str, &str); 8] = [
        ("a word", "yesterday", "yesterday"),
        (
            "a phrase whose spaces are escaped",
            "not%20a%20timestamp",
            "not a timestamp",
        ),
        ("nothing at all", "", ""),
        ("scientific notation", "1.7e9", "1.7e9"),
        (
            "an explicit plus sign on a number",
            "%2B1700000000",
            "+1700000000",
        ),
        (
            "a date-time with no zone at all",
            "2023-11-14T22%3A13%3A20",
            "2023-11-14T22:13:20",
        ),
        (
            "a date-time past the calendar",
            "2023-13-14T22%3A13%3A20Z",
            "2023-13-14T22:13:20Z",
        ),
        (
            "a date-time whose zone offset is out of range",
            "2023-11-14T22%3A13%3A20%2B24%3A00",
            "2023-11-14T22:13:20+24:00",
        ),
    ];

    for (case, sent, echoed) in rows {
        let answer = revocations(
            &router,
            &params(&profile_version_arn, sent.to_string()),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "SerializationException",
            "case {case:?}: expected the protocol's own errortype, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.framework_message(),
            format!("'{echoed}' can not be converted to Date"),
            "case {case:?}: expected the framework envelope's Message naming the value it \
             could not convert, got {:?}",
            answer.body
        );
        assert!(
            answer.message().is_empty() && answer.body["code"].is_null(),
            "case {case:?}: a protocol-level refusal carries none of the modelled error's \
             members, got {:?}",
            answer.body
        );
    }
}

/// The clause rule is not one operation's: a refusal of ANOTHER operation's value quotes
/// it whole too.
///
/// `GetSigningProfile` binds its `profileName` to the URI path and the model bounds that
/// member by `@length(2, 64)` (`smithy_query(query="shape", shape="ProfileName")`), so a
/// name inside the character class the shape admits but longer than 64 is refused by the
/// boundary validator with the value in its clause. The rows walk that length across the
/// framework's own echo ceiling.
#[tokio::test]
async fn a_clause_quotes_the_whole_submitted_value_on_every_operation() {
    let router = emulator();

    for length in [
        FRAMEWORK_ECHO_CEILING - 1,
        FRAMEWORK_ECHO_CEILING,
        FRAMEWORK_ECHO_CEILING + 1,
        FRAMEWORK_ECHO_CEILING * 4,
    ] {
        // Every character is one the `ProfileName` pattern admits, so the ONE constraint
        // the value breaks is the length maximum.
        let name = "a".repeat(length);
        let case = format!("GetSigningProfile with a profileName of {length} characters");
        let answer = call(&router, "GET", &format!("/signing-profiles/{name}"), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.message().contains(&name),
            "case {case}: the clause must quote the whole value the request carried, got a \
             message of {} characters",
            answer.message().len()
        );
        assert!(
            !answer.message().contains("bytes]"),
            "case {case}: the clause must not state a prefix of the value, got {:?}",
            answer.message()
        );
    }
}

/// A constraint clause of `GetRevocationStatus` quotes the value the request carried AS IT
/// CARRIED IT: no prefix of it, whatever its length.
///
/// The rows walk the length of the refused value across the framework's own echo ceiling —
/// under it, exactly on it, one byte past it — and up to one byte past the member's
/// modelled maximum, for each of the two members the model bounds by length.
#[tokio::test]
async fn a_constraint_clause_quotes_the_whole_submitted_value() {
    let router = emulator();
    let (profile_version_arn, _) = create_profile(&router, "Quoted_Clauses").await;

    // A value of exactly `length` characters, built so it is NOT ARN-shaped: what is under
    // test is the clause, so each row has to reach a refusal, and the shortest rows are
    // under the modelled minimum while the longest are past the maximum.
    let value_of_length = |length: usize| -> String { "x".repeat(length) };

    for member in ["jobArn", "profileVersionArn"] {
        for length in [
            19,
            FRAMEWORK_ECHO_CEILING - 1,
            FRAMEWORK_ECHO_CEILING,
            FRAMEWORK_ECHO_CEILING + 1,
            ARN_MAX_LENGTH + 1,
            ARN_MAX_LENGTH * 2,
        ] {
            let case = format!("{member} of {length} characters");
            let value = value_of_length(length);
            let request: Vec<(&str, String)> =
                params(&profile_version_arn, date_time(EFFECTIVE_EPOCH_SECS))
                    .into_iter()
                    .map(|(name, carried)| {
                        if name == member {
                            (name, value.clone())
                        } else {
                            (name, carried)
                        }
                    })
                    .collect();

            let answer = revocations(&router, &request).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.message().contains(&value),
                "case {case}: the clause must quote the whole value the request carried, got \
                 {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains("bytes]"),
                "case {case}: the clause must not state a prefix of the value, got {:?}",
                answer.message()
            );
            assert!(
                answer.message().contains(member),
                "case {case}: the clause must name the member that carried the value, got {:?}",
                answer.message()
            );
        }
    }
}

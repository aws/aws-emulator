// SPDX-License-Identifier: CC0-1.0

//! How this service words a refusal of a request that supplied NOTHING for an
//! input member it binds a list to on the query string, end to end over HTTP.
//!
//! The rule under test: for a member the model binds a LIST to the query string,
//! a request that supplies no entries and one that omits the parameter are the
//! SAME request on the wire — an SDK serializing an empty list emits no
//! parameter at all — so the refusal names the member and states that it must
//! carry something (`<member> must not be null or empty`) rather than claiming
//! the value was null. A SINGLE-VALUE member the request supplied nothing for is
//! refused by this service's own validator too, in the `Invalid <member>: …` voice
//! it refuses its own rules in (`Invalid <member>: value must not be null`). Every
//! OTHER refusal keeps the framework's own constraint wording, which is accurate
//! for it: a value present but outside a declared bound.
//!
//! The cases are tables. They span what the rule generalises over — which member
//! misses, whether it misses by being omitted or by carrying nothing, how many
//! entries a supplied list carries, and which other class of refusal fires — and
//! the members are those the model binds: this suite drives the one
//! `GetRevocationStatus` binds, and `tag_keys_supplied_nothing_wording` drives the
//! one `UntagResource` binds on `DELETE /tags/{resourceArn}`.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_signer_wallabyservice::state::SIGNING_PLATFORM_CATALOG;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here so a wrong account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// The instant the `signatureTimestamp` of every request below names.
const BASE_EPOCH_SECS: i64 = 1_700_000_000;

/// The sentence this service states for a query-bound list member a request
/// supplied nothing for, with the member's own name in it.
fn supplied_nothing(member: &str) -> String {
    format!("{member} must not be null or empty")
}

/// The sentence this service states for a SINGLE-VALUE member a request supplied
/// nothing for, with the member's own name in it.
fn absent_member(member: &str) -> String {
    format!("Invalid {member}: value must not be null")
}

/// The query string of one case: the `@httpQuery` parameters it carries, in the
/// order it carries them.
type Params = Vec<(&'static str, String)>;

/// A SigV4 credential scope naming this service for `REGION`. It is what routes a
/// request to signer.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `message` member, or the empty string when the body carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// `GET /revocations` with exactly the query parameters given, so a case can
/// leave one out or supply it with nothing.
async fn get_revocations(router: &MultiServiceRouter, params: &[(&str, String)]) -> Answer {
    let query = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let uri = if query.is_empty() {
        "/revocations".to_string()
    } else {
        format!("/revocations?{query}")
    };
    let request = ::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION))
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A `signatureTimestamp` query value: the ISO 8601 (`date-time`) rendering a
/// `@httpQuery`-bound timestamp carries by default.
fn timestamp_param() -> String {
    DateTime::from_secs(BASE_EPOCH_SECS)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time")
}

/// A signing-profile VERSION ARN of this account and region, well formed and
/// naming nothing stored — which this operation reports as "not revoked" rather
/// than refusing, so a case about a MISSING member is not answered for some
/// other reason.
fn profile_version_arn() -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/Wording_Profile/abcde12345")
}

/// A signing-job ARN of this account and region, well formed and naming nothing
/// stored.
fn job_arn() -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/1a2b3c4d-5e6f-7890-abcd-ef1234567890")
}

/// A published `platformId`, taken from the catalog this service serves rather
/// than written out.
fn platform_id() -> &'static str {
    SIGNING_PLATFORM_CATALOG
        .first()
        .expect("this service publishes at least one signing platform")
        .platform_id
}

/// A certificate hash of the documented shape: two hex SHA-384 digests, the
/// subject certificate's TBS hash followed by its parent CA's
/// (`smithy_query(query="doc",shape="GetRevocationStatusRequest",
/// member="certificateHashes")`).
fn certificate_hash(child: &str, parent: &str) -> String {
    use sha2::Digest;
    format!(
        "{}{}",
        hex::encode(sha2::Sha384::digest(child.as_bytes())),
        hex::encode(sha2::Sha384::digest(parent.as_bytes()))
    )
}

/// Every required query parameter of `GetRevocationStatus`, in the order the
/// model declares them, each carrying a value this endpoint serves.
fn complete_request() -> Params {
    vec![
        ("signatureTimestamp", timestamp_param()),
        ("platformId", platform_id().to_string()),
        ("profileVersionArn", profile_version_arn()),
        ("jobArn", job_arn()),
        ("certificateHashes", certificate_hash("child", "root")),
    ]
}

/// `complete_request()` without the parameter named `member`.
fn without(member: &str) -> Params {
    complete_request()
        .into_iter()
        .filter(|(name, _)| *name != member)
        .collect()
}

/// A request that supplies nothing for the query-bound LIST member is refused by
/// name: the member omitted, and the member omitted from a request that is
/// otherwise complete in every other way the table varies, are the same request
/// on the wire and are stated identically.
#[tokio::test]
async fn a_list_member_supplied_with_nothing_is_refused_by_name() {
    let router = emulator();

    // The rows differ in what else the request carries: nothing else varies the
    // refusal, because the member that supplied nothing is the whole reason.
    let mut rows: Vec<(&str, Params)> = vec![
        ("certificateHashes omitted", without("certificateHashes")),
        (
            "certificateHashes omitted, the others in reverse order",
            {
                let mut params = without("certificateHashes");
                params.reverse();
                params
            },
        ),
        (
            "certificateHashes omitted, an unmodelled parameter supplied",
            {
                let mut params = without("certificateHashes");
                params.push(("ignoredByTheModel", "anything".to_string()));
                params
            },
        ),
    ];
    // The same member, on a request whose other values name resources of another
    // region and account: the refusal is about the member that is missing.
    rows.push((
        "certificateHashes omitted, the other ARNs naming elsewhere", {
            let mut params = without("certificateHashes");
            for (name, value) in &mut params {
                if *name == "profileVersionArn" {
                    *value =
                        "arn:aws:signer:eu-west-1:210987654321:/signing-profiles/Other_P/abcde12345"
                            .to_string();
                }
                if *name == "jobArn" {
                    *value = "arn:aws:signer:eu-west-1:210987654321:/signing-jobs/other-job"
                        .to_string();
                }
            }
            params
        },
    ));

    for (case, params) in &rows {
        let answer = get_revocations(&router, params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            supplied_nothing("certificateHashes"),
            "case {case:?}: expected {:?}, got {:?}",
            supplied_nothing("certificateHashes"),
            answer.message()
        );
    }
}

/// A request that DOES supply entries is not refused by that check, whatever the
/// entries are and however many of them there are.
#[tokio::test]
async fn a_list_member_that_carries_entries_is_served() {
    let router = emulator();

    let one = vec![certificate_hash("child", "root")];
    let two = vec![
        certificate_hash("child", "root"),
        certificate_hash("intermediate", "root"),
    ];
    let five: Vec<String> = (0..5)
        .map(|index| certificate_hash(&format!("child-{index}"), "root"))
        .collect();
    // An entry that is the empty string is still an ENTRY: the member carries
    // something, so the supplied-nothing check does not fire.
    let empty_entry = vec![String::new()];

    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one hash", one),
        ("two hashes", two),
        ("five hashes", five),
        ("one empty entry", empty_entry),
    ];

    for (case, hashes) in &rows {
        let mut params = without("certificateHashes");
        for hash in hashes {
            params.push(("certificateHashes", hash.clone()));
        }
        let answer = get_revocations(&router, &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let reported = answer.body["revokedEntities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected revokedEntities, got {:?}", answer.body));
        assert!(
            reported.is_empty(),
            "case {case:?}: nothing named is revoked, got {reported:?}"
        );
    }
}

/// A member the request supplied nothing for is refused in this service's own voice, and
/// every OTHER refusal keeps the framework's own constraint wording: a value present but
/// outside its declared bound. Neither of them is stated as the list sentence.
///
/// The rows differ along the class of refusal (a single-value member absent, a value
/// present and outside its bound) and the member at fault, which is what the two wordings
/// are chosen between.
#[tokio::test]
async fn every_other_refusal_keeps_the_constraint_wording() {
    let router = emulator();

    // One row per required SINGLE-VALUE member omitted — refused in the `Invalid
    // <member>: …` voice — plus two rows for a value that is present and outside the
    // declared 20..2048 length of an ARN member, which keeps the framework's clause.
    let mut rows: Vec<Row> = Vec::new();
    for member in ["signatureTimestamp", "platformId", "profileVersionArn", "jobArn"] {
        rows.push(Row {
            case: "a member omitted",
            member,
            params: without(member),
            stated: Stated::AbsentMember,
        });
    }
    for member in ["profileVersionArn", "jobArn"] {
        let mut params = complete_request();
        for (name, value) in &mut params {
            if *name == member {
                *value = "arn:aws:signer".to_string();
            }
        }
        rows.push(Row {
            case: "a value shorter than its bound",
            member,
            params,
            stated: Stated::FrameworkClause,
        });
    }

    for Row {
        case,
        member,
        params,
        stated,
    } in &rows
    {
        let answer = get_revocations(&router, params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({member}): expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains("ValidationException"),
            "case {case:?} ({member}): expected a ValidationException, got {:?}",
            answer.body
        );
        match stated {
            Stated::AbsentMember => assert_eq!(
                answer.message(),
                absent_member(member),
                "case {case:?} ({member}): expected this service's absent-member sentence, \
                 got {:?}",
                answer.message()
            ),
            Stated::FrameworkClause => assert!(
                answer.message().contains("failed to satisfy constraint"),
                "case {case:?} ({member}): expected the framework's constraint wording, got \
                 {:?}",
                answer.message()
            ),
        }
        assert!(
            !answer.message().contains("must not be null or empty"),
            "case {case:?} ({member}): expected the list sentence NOT to be stated, got {:?}",
            answer.message()
        );
        assert!(
            answer.message().contains(member),
            "case {case:?} ({member}): expected the refusal to name the member, got {:?}",
            answer.message()
        );
    }
}

/// Which wording a row expects.
enum Stated {
    /// This service's own sentence for a member the request supplied nothing for.
    AbsentMember,
    /// The framework's constraint clause, which states the bound a present value broke.
    FrameworkClause,
}

/// One row of the table: what the case is, the member it is about, the request that
/// provokes it, and which wording that refusal carries.
struct Row {
    case: &'static str,
    member: &'static str,
    params: Params,
    stated: Stated,
}

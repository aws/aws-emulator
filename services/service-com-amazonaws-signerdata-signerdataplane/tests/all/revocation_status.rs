// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRevocationStatus`, this service's only operation.
//!
//! The rule under test: the response lists the identifier of every entity the request
//! names — the signing-profile version, the signing job, and each certificate hash —
//! that the revocation registry holds a revocation for whose effective time is at or
//! before the signature's timestamp and whose platform scope (when it has one) is the
//! signature's platform; and nothing else, so the list is empty when no candidate
//! matches.
//!
//! Every assertion is driven over a TABLE so nothing here can pass against an answer
//! built for one shape: the rows vary which registry holds the revocation, how many
//! candidates match, how many certificate hashes the request carries, where the
//! signature's timestamp falls relative to the effective time (including on it, and one
//! millisecond either side), and whether the revocation is scoped to a platform.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_signerdata_signerdataplane::state::{
    RevocationRecord, RevokedEntityKind,
};
use service_com_amazonaws_signerdata_signerdataplane::{
    ComAmazonawsSignerdataSignerdataplaneState, EmulatedService,
};

/// The region the test's ARNs are built for.
pub(crate) const REGION: &str = "us-east-1";

/// A platform id that satisfies the model's `^[a-zA-Z0-9_-]+$` pattern. Taken from the
/// `GetRevocationStatus` example embedded in the service's Smithy model.
pub(crate) const PLATFORM: &str = "Notation-OCI-SHA384-ECDSA";

/// A second, different platform id — also pattern-valid — so platform scoping is
/// exercised against a platform the scoped revocation does not name.
const OTHER_PLATFORM: &str = "AWSLambda-SHA384-ECDSA";

/// The instant the tables hang their offsets off: the epoch second the model's own
/// example uses for `signatureTimestamp`.
pub(crate) fn base_instant() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// `base_instant()` shifted by `millis`, which may be negative.
fn at_offset_millis(millis: i64) -> SystemTime {
    let magnitude = Duration::from_millis(millis.unsigned_abs());
    if millis < 0 {
        base_instant()
            .checked_sub(magnitude)
            .expect("an instant before the base is representable")
    } else {
        base_instant()
            .checked_add(magnitude)
            .expect("an instant after the base is representable")
    }
}

/// A signing-profile version ARN (model shape `Arn`, length 20..2048).
pub(crate) fn profile_version_arn(name: &str) -> String {
    engine::emulator_arn("signer", REGION, &format!("/signing-profiles/{name}/v1"))
}

/// A signing-job ARN (model shape `Arn`, length 20..2048).
pub(crate) fn job_arn(name: &str) -> String {
    engine::emulator_arn("signer", REGION, &format!("/signing-jobs/{name}"))
}

/// A certificate hash (model shape `CertificateHash`, length 1..2048): the SHA-256 of
/// `name`, which is the shape of hash the model's own example carries.
pub(crate) fn certificate_hash(name: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(name.as_bytes()))
}

/// The emulator under test: the shared engine (the only store), plus a router with this
/// one service mounted, driven through the same `dispatch` the production front door
/// uses.
pub(crate) struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Record a revocation in the shared engine state — the seam that stands in for the
    /// Signer control plane's `RevokeSignature` / `RevokeSigningProfile`, since the data
    /// plane models no write.
    pub(crate) fn revoke(&self, kind: RevokedEntityKind, key: &str, record: RevocationRecord) {
        self.engine
            .state::<ComAmazonawsSignerdataSignerdataplaneState>()
            .lock()
            .revoke(kind, key, record);
    }

    /// Drop a revocation from the registry.
    fn unrevoke(&self, kind: RevokedEntityKind, key: &str) -> bool {
        self.engine
            .state::<ComAmazonawsSignerdataSignerdataplaneState>()
            .lock()
            .unrevoke(kind, key)
            .is_some()
    }

    /// `GET /revocations?…`, returning the whole response.
    ///
    /// Names and values go onto the query string EXACTLY as the caller gives them: this
    /// is the wire, so a test row states the wire form it wants (every value the tables
    /// below use is query-legal as written — RFC 3986 lets a query carry `:` and `/`
    /// unescaped, which is how an SDK sends an ARN — and the rows that need an escape
    /// write it, `%20`, themselves). No codec here, hand-rolled or otherwise.
    async fn send(&self, params: &[(&str, String)]) -> http::Response<bytes::Bytes> {
        let query = params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let mut uri = format!("https://data-signer.{REGION}.amazonaws.com/revocations");
        if !query.is_empty() {
            uri.push('?');
            uri.push_str(&query);
        }
        let request = http::Request::builder()
            .method("GET")
            .uri(&uri)
            .header("host", format!("data-signer.{REGION}.amazonaws.com"))
            .body(bytes::Bytes::new())
            .expect("build the GetRevocationStatus request");
        self.router.dispatch(request).await
    }

    /// The same read, as the status and the raw body bytes.
    async fn get_raw(&self, params: &[(&str, String)]) -> (u16, bytes::Bytes) {
        let response = self.send(params).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// The same read, as the status, the `x-amzn-errortype` the response names (the
    /// header restJson1 resolves an error shape from) and the body parsed as JSON.
    pub(crate) async fn get_with_error_type(
        &self,
        params: &[(&str, String)],
    ) -> (u16, Option<String>, Value) {
        let response = self.send(params).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        (status, error_type, parse_body(params, &response.into_body()))
    }

    /// The same read, with the body parsed as JSON (restJson1).
    pub(crate) async fn get(&self, params: &[(&str, String)]) -> (u16, Value) {
        let (status, body) = self.get_raw(params).await;
        (status, parse_body(params, &body))
    }
}

/// One response body, as the JSON a restJson1 answer carries (`Null` for an empty body).
fn parse_body(params: &[(&str, String)], body: &[u8]) -> Value {
    if body.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(body).unwrap_or_else(|e| {
        panic!(
            "params {params:?}: response body must be JSON, got {:?} ({e})",
            String::from_utf8_lossy(body)
        )
    })
}


/// A `signatureTimestamp` query value: the ISO 8601 (`date-time`) rendering a
/// `@httpQuery`-bound timestamp carries by default.
pub(crate) fn timestamp_param(at: SystemTime) -> String {
    let since_epoch = at
        .duration_since(UNIX_EPOCH)
        .expect("the test's instants are after the epoch");
    DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).expect("seconds fit in i64"),
        since_epoch.subsec_nanos(),
    )
    .fmt(Format::DateTime)
    .expect("an epoch-based instant renders as date-time")
}

/// The full, valid query of one request.
pub(crate) fn params(
    signature_timestamp: SystemTime,
    platform_id: &str,
    profile: &str,
    job: &str,
    hashes: &[String],
) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("signatureTimestamp", timestamp_param(signature_timestamp)),
        ("platformId", platform_id.to_string()),
        ("profileVersionArn", profile.to_string()),
        ("jobArn", job.to_string()),
    ];
    for hash in hashes {
        out.push(("certificateHashes", hash.clone()));
    }
    out
}

/// The `revokedEntities` of a 200 response.
pub(crate) fn revoked_entities(status: u16, body: &Value, case: &str) -> Vec<String> {
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200, got {status} with {body}"
    );
    let list = body
        .get("revokedEntities")
        .unwrap_or_else(|| {
            panic!("case {case}: the response must carry revokedEntities, got {body}")
        })
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: revokedEntities must be a list, got {body}"));
    list.iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: every revoked entity must be a string, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// The error code (`__type`) and message of a modelled error response.
fn error_of(body: &Value, case: &str) -> (String, String) {
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: error body must carry __type, got {body}"))
        // `__type` may be namespaced (`com.amazonaws.signerdata#Name`).
        .rsplit('#')
        .next()
        .expect("a non-empty __type")
        .to_string();
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: error body must carry a message, got {body}"))
        .to_string();
    (code, message)
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// Every subset of the three candidate kinds: the response lists exactly the
/// identifiers of the candidates the registry holds a revocation for, in the order the
/// request names them (profile version, job, then each certificate hash), and is empty
/// when none is revoked.
///
/// The recorded identifier is deliberately NOT the registry key, so an answer echoing
/// the request's own values cannot pass.
#[tokio::test]
async fn revoked_entities_are_the_revoked_candidates_and_only_those() {
    let emulator = Emulator::new();
    let effective = at_offset_millis(-3_600_000);
    let signed_at = base_instant();

    // Eight rows, one per subset of {profile version, job, certificate}: the rule
    // generalises over WHICH candidates are revoked and HOW MANY, so every combination
    // is a row.
    for mask in 0_u8..8 {
        let profile_revoked = mask & 1 != 0;
        let job_revoked = mask & 2 != 0;
        let certificate_revoked = mask & 4 != 0;

        let profile = profile_version_arn(&format!("profile-{mask}"));
        let job = job_arn(&format!("job-{mask}"));
        // Three hashes; only the middle one is ever revoked, so a response that
        // reported the whole list, or the first entry, cannot pass.
        let hashes: Vec<String> = (0..3)
            .map(|i| certificate_hash(&format!("cert-{mask}-{i}")))
            .collect();

        let profile_entity = format!("revoked-profile-version-{mask}");
        let job_entity = format!("revoked-signing-job-{mask}");
        let certificate_entity = format!("revoked-certificate-{mask}");

        if profile_revoked {
            emulator.revoke(
                RevokedEntityKind::ProfileVersion,
                &profile,
                RevocationRecord::new(&profile_entity, effective).with_reason("key compromise"),
            );
        }
        if job_revoked {
            emulator.revoke(
                RevokedEntityKind::SigningJob,
                &job,
                RevocationRecord::new(&job_entity, effective),
            );
        }
        if certificate_revoked {
            emulator.revoke(
                RevokedEntityKind::Certificate,
                &hashes[1],
                RevocationRecord::new(&certificate_entity, effective),
            );
        }

        let mut expected: Vec<String> = Vec::new();
        if profile_revoked {
            expected.push(profile_entity);
        }
        if job_revoked {
            expected.push(job_entity);
        }
        if certificate_revoked {
            expected.push(certificate_entity);
        }

        let case = format!(
            "profile_revoked={profile_revoked} job_revoked={job_revoked} \
             certificate_revoked={certificate_revoked}"
        );
        let (status, body) = emulator
            .get(&params(signed_at, PLATFORM, &profile, &job, &hashes))
            .await;
        assert_eq!(
            revoked_entities(status, &body, &case),
            expected,
            "case {case}: revokedEntities must be exactly the revoked candidates' \
             recorded identifiers, got {body}"
        );
    }
}

/// `revokedEntities` is always present — an empty list, never an absent member — when
/// nothing is revoked, whatever the request carries. Rows vary the number of
/// certificate hashes, since that is what the list-shaped input generalises over.
#[tokio::test]
async fn nothing_revoked_is_an_empty_list_not_an_absent_member() {
    let emulator = Emulator::new();
    // Revoke something the requests below never name, so the registry is non-empty and
    // "empty answer" cannot come from an empty store.
    emulator.revoke(
        RevokedEntityKind::SigningJob,
        &job_arn("unrelated"),
        RevocationRecord::new("revoked-unrelated-job", at_offset_millis(-1)),
    );

    for hash_count in [1_usize, 2, 5, 25] {
        let case = format!("certificateHashes count={hash_count}");
        let hashes: Vec<String> = (0..hash_count)
            .map(|i| certificate_hash(&format!("clean-{hash_count}-{i}")))
            .collect();
        let (status, body) = emulator
            .get(&params(
                base_instant(),
                PLATFORM,
                &profile_version_arn("clean"),
                &job_arn("clean"),
                &hashes,
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            body,
            json!({ "revokedEntities": [] }),
            "case {case}: the response must carry an empty revokedEntities list"
        );
    }
}

/// A revocation applies to a signature made AT or AFTER its effective time, and to no
/// earlier one. Rows straddle the effective instant, including one millisecond either
/// side of it and the instant itself.
#[tokio::test]
async fn a_revocation_applies_only_from_its_effective_time() {
    let emulator = Emulator::new();
    let effective = base_instant();
    let profile = profile_version_arn("timed");
    let job = job_arn("timed");
    let hash = certificate_hash("timed");

    emulator.revoke(
        RevokedEntityKind::ProfileVersion,
        &profile,
        RevocationRecord::new("revoked-timed-profile-version", effective),
    );
    emulator.revoke(
        RevokedEntityKind::SigningJob,
        &job,
        RevocationRecord::new("revoked-timed-job", effective),
    );
    emulator.revoke(
        RevokedEntityKind::Certificate,
        &hash,
        RevocationRecord::new("revoked-timed-certificate", effective),
    );

    // (description, signature offset from the effective time in ms, revoked?)
    let cases: &[(&str, i64, bool)] = &[
        ("a year before the effective time", -31_536_000_000, false),
        ("an hour before", -3_600_000, false),
        ("a second before", -1_000, false),
        ("a millisecond before", -1, false),
        ("exactly at the effective time", 0, true),
        ("a millisecond after", 1, true),
        ("an hour after", 3_600_000, true),
        ("a year after", 31_536_000_000, true),
    ];

    for (what, offset_millis, expected_revoked) in cases {
        let case = format!("signature {what} (offset {offset_millis}ms)");
        let (status, body) = emulator
            .get(&params(
                at_offset_millis(*offset_millis),
                PLATFORM,
                &profile,
                &job,
                std::slice::from_ref(&hash),
            ))
            .await;
        let expected: Vec<String> = if *expected_revoked {
            vec![
                "revoked-timed-profile-version".to_string(),
                "revoked-timed-job".to_string(),
                "revoked-timed-certificate".to_string(),
            ]
        } else {
            Vec::new()
        };
        assert_eq!(
            revoked_entities(status, &body, &case),
            expected,
            "case {case}: a revocation must apply from its effective time onward, \
             got {body}"
        );
    }
}

/// A revocation scoped to one signing platform applies only to a signature made on
/// that platform; an unscoped one applies on every platform.
#[tokio::test]
async fn a_platform_scoped_revocation_applies_only_on_that_platform() {
    let emulator = Emulator::new();
    let effective = at_offset_millis(-1_000);

    let scoped_job = job_arn("scoped");
    emulator.revoke(
        RevokedEntityKind::SigningJob,
        &scoped_job,
        RevocationRecord::new("revoked-scoped-job", effective).for_platform(PLATFORM),
    );
    let unscoped_job = job_arn("unscoped");
    emulator.revoke(
        RevokedEntityKind::SigningJob,
        &unscoped_job,
        RevocationRecord::new("revoked-unscoped-job", effective),
    );

    // (job named by the request, platform of the signature, expected answer)
    let cases: &[(&str, &str, &[&str])] = &[
        (&scoped_job, PLATFORM, &["revoked-scoped-job"]),
        (&scoped_job, OTHER_PLATFORM, &[]),
        (&unscoped_job, PLATFORM, &["revoked-unscoped-job"]),
        (&unscoped_job, OTHER_PLATFORM, &["revoked-unscoped-job"]),
    ];

    for (job, platform, expected) in cases {
        let case = format!("job={job} platform={platform}");
        let (status, body) = emulator
            .get(&params(
                base_instant(),
                platform,
                &profile_version_arn("scoping"),
                job,
                &[certificate_hash("scoping")],
            ))
            .await;
        assert_eq!(
            revoked_entities(status, &body, &case),
            expected.iter().map(|e| (*e).to_string()).collect::<Vec<_>>(),
            "case {case}: a platform-scoped revocation must apply only on its own \
             platform, got {body}"
        );
    }
}

/// Each candidate is looked up in the registry for ITS OWN kind: a value revoked as a
/// certificate hash is not reported for the job ARN that happens to equal it, and so on
/// for every ordered pair of kinds.
#[tokio::test]
async fn a_candidate_is_looked_up_only_in_its_own_registry() {
    let effective = at_offset_millis(-1_000);

    // One shared value, used as all three candidates of the request, revoked in exactly
    // one registry per row. Only the candidate of that registry's kind may be reported.
    for revoked_in in RevokedEntityKind::ALL {
        let emulator = Emulator::new();
        // 20..2048 characters, so it is a legal `Arn` as well as a legal
        // `CertificateHash` — which is what lets one value stand in for all three.
        let shared = job_arn(&format!("shared-{}", revoked_in.input_member()));
        emulator.revoke(
            revoked_in,
            &shared,
            RevocationRecord::new(format!("revoked-as-{}", revoked_in.input_member()), effective),
        );

        let case = format!("revoked in the {} registry", revoked_in.input_member());
        let (status, body) = emulator
            .get(&params(
                base_instant(),
                PLATFORM,
                &shared,
                &shared,
                std::slice::from_ref(&shared),
            ))
            .await;
        assert_eq!(
            revoked_entities(status, &body, &case),
            vec![format!("revoked-as-{}", revoked_in.input_member())],
            "case {case}: the identifier must be reported exactly once, for the \
             candidate of that registry's own kind, got {body}"
        );
    }
}

/// The read does not mutate: repeating the identical request answers byte-identically
/// and leaves the registry exactly as it was. A revocation added afterwards becomes
/// visible to the next read, and one removed becomes invisible — so the answer is
/// computed from persisted state rather than canned.
#[tokio::test]
async fn reads_are_repeatable_and_follow_the_registry() {
    let emulator = Emulator::new();
    let effective = at_offset_millis(-1_000);
    let profile = profile_version_arn("followed");
    let job = job_arn("followed");
    let hash = certificate_hash("followed");
    let request = params(
        base_instant(),
        PLATFORM,
        &profile,
        &job,
        std::slice::from_ref(&hash),
    );

    emulator.revoke(
        RevokedEntityKind::SigningJob,
        &job,
        RevocationRecord::new("revoked-followed-job", effective),
    );

    // Repeating the read answers identically, three times over.
    let (first_status, first) = emulator.get_raw(&request).await;
    for attempt in 2..=3 {
        let (status, body) = emulator.get_raw(&request).await;
        assert_eq!(
            (status, &body),
            (first_status, &first),
            "case attempt={attempt}: an identical read must answer identically"
        );
    }
    // And the revocation it reported is still in the registry, untouched.
    assert_eq!(
        emulator
            .engine
            .state::<ComAmazonawsSignerdataSignerdataplaneState>()
            .lock()
            .revocation(RevokedEntityKind::SigningJob, &job)
            .map(|record| record.entity_id.clone()),
        Some("revoked-followed-job".to_string()),
        "a read must not consume the revocation it reported"
    );

    // Adding a revocation becomes visible to the next read…
    emulator.revoke(
        RevokedEntityKind::Certificate,
        &hash,
        RevocationRecord::new("revoked-followed-certificate", effective),
    );
    let (status, body) = emulator.get(&request).await;
    assert_eq!(
        revoked_entities(status, &body, "after adding a certificate revocation"),
        vec![
            "revoked-followed-job".to_string(),
            "revoked-followed-certificate".to_string(),
        ],
        "a revocation added to the registry must be reported by the next read, got {body}"
    );

    // … and removing one becomes invisible to it.
    assert!(
        emulator.unrevoke(RevokedEntityKind::Certificate, &hash),
        "the certificate revocation must have been in the registry to remove"
    );
    let (status, body) = emulator.get(&request).await;
    assert_eq!(
        revoked_entities(status, &body, "after removing the certificate revocation"),
        vec!["revoked-followed-job".to_string()],
        "a revocation removed from the registry must not be reported, got {body}"
    );

    assert!(
        emulator.unrevoke(RevokedEntityKind::SigningJob, &job),
        "the job revocation must have been in the registry to remove"
    );
    let (status, body) = emulator.get(&request).await;
    assert_eq!(
        revoked_entities(status, &body, "after removing every revocation"),
        Vec::<String>::new(),
        "an empty registry must answer an empty revokedEntities, got {body}"
    );
}

/// A request that leaves out a required query parameter is refused with 400
/// `ValidationException` naming that parameter — never a 501 (which would claim the
/// operation is unimplemented) and never a 200. One row per required parameter, plus
/// the row that omits all of them.
#[tokio::test]
async fn a_missing_required_query_parameter_is_a_validation_error() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("required");
    let job = job_arn("required");
    let hash = certificate_hash("required");
    let complete = params(
        base_instant(),
        PLATFORM,
        &profile,
        &job,
        std::slice::from_ref(&hash),
    );

    // The model marks all five input members `@required` + `@httpQuery`
    // (smithy_query(query="operation",operation="GetRevocationStatus")).
    let required = [
        "signatureTimestamp",
        "platformId",
        "profileVersionArn",
        "jobArn",
        "certificateHashes",
    ];

    // The complete request is accepted, so each row below differs from an accepted
    // request in exactly the one parameter it drops.
    let (status, body) = emulator.get(&complete).await;
    assert_eq!(
        status, 200,
        "the complete request must be accepted, got {status} with {body}"
    );

    for omitted in required {
        let case = format!("omitted={omitted}");
        let partial: Vec<(&str, String)> = complete
            .iter()
            .filter(|(name, _)| *name != omitted)
            .cloned()
            .collect();
        let (status, body) = emulator.get(&partial).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error_of(&body, &case);
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        assert!(
            message.contains(omitted),
            "case {case}: the message must name the missing parameter, got {message:?}"
        );
    }

    // Nothing at all on the query string: still a 400 naming every parameter.
    let (status, body) = emulator.get(&[]).await;
    assert_eq!(
        status, 400,
        "case omitted=all: expected HTTP 400, got {status} with {body}"
    );
    let (code, message) = error_of(&body, "omitted=all");
    assert_eq!(
        code, "ValidationException",
        "case omitted=all: expected ValidationException, got {body}"
    );
    for name in required {
        assert!(
            message.contains(name),
            "case omitted=all: the message must name {name}, got {message:?}"
        );
    }
}

/// A value outside a constraint the model declares is refused with 400
/// `ValidationException`. The rows walk each published bound — the `platformId` pattern
/// `^[a-zA-Z0-9_-]+$` and the `Arn` length range 20..2048 — on the bound itself and one
/// step past it.
///
/// A `signatureTimestamp` that names no instant is NOT one of these: it never becomes a
/// modelled input, so it is refused by the protocol rather than for a modelled constraint
/// (`tests/all/signature_timestamp_forms.rs`).
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_a_validation_error() {
    let emulator = Emulator::new();
    let good_profile = profile_version_arn("bounds");
    let good_job = job_arn("bounds");
    let good_hash = certificate_hash("bounds");

    // The `Arn` length range is 20..2048 inclusive, so 20 and 2048 are accepted and 19
    // and 2049 are not. The model constrains `Arn` by LENGTH only, so a row on a bound
    // is simply a string of exactly that length: the canonical ARN prefix, truncated or
    // padded in its resource segment (every character is ASCII, so bytes and chars
    // agree).
    let arn_of_length = |length: usize| -> String {
        let base = engine::emulator_arn("signer", REGION, "/signing-jobs/");
        let mut arn: String = base.chars().take(length).collect();
        while arn.len() < length {
            arn.push('a');
        }
        assert_eq!(arn.len(), length, "the padded ARN must be {length} characters");
        arn
    };
    // 19 characters: shorter than the minimum, and too short for a real ARN prefix.
    let too_short = "arn:aws:signer:us-e".to_string();
    assert_eq!(too_short.len(), 19, "the too-short ARN must be 19 characters");

    // (description, parameter, value, expected status)
    let cases: Vec<(&str, &str, String, u16)> = vec![
        // `platformId` pattern rows. The first carries its space as the `%20` the wire
        // spells it with, so it also proves the value is percent-decoded before the
        // pattern is applied.
        ("platformId with a space", "platformId", "bad%20id".to_string(), 400),
        ("platformId with a bang", "platformId", "AWSLambda!".to_string(), 400),
        ("platformId with a dot", "platformId", "AWSLambda.SHA384".to_string(), 400),
        ("platformId empty", "platformId", String::new(), 400),
        // Pattern-valid, including the underscore and digits the pattern admits and the
        // issue did not cite.
        ("platformId with an underscore", "platformId", "Notation_OCI_2".to_string(), 200),
        ("platformId all digits", "platformId", "384".to_string(), 200),
        // `Arn` length rows, on each bound and one past it, for both ARN members.
        ("jobArn one under the minimum length", "jobArn", too_short.clone(), 400),
        ("jobArn at the minimum length", "jobArn", arn_of_length(20), 200),
        ("jobArn at the maximum length", "jobArn", arn_of_length(2048), 200),
        ("jobArn one over the maximum length", "jobArn", arn_of_length(2049), 400),
        (
            "profileVersionArn one under the minimum length",
            "profileVersionArn",
            too_short.clone(),
            400,
        ),
        ("profileVersionArn at the minimum length", "profileVersionArn", arn_of_length(20), 200),
        ("profileVersionArn at the maximum length", "profileVersionArn", arn_of_length(2048), 200),
        (
            "profileVersionArn one over the maximum length",
            "profileVersionArn",
            arn_of_length(2049),
            400,
        ),
    ];

    for (what, parameter, value, expected_status) in cases {
        let case = format!("{what} ({parameter}={value:.40?})");
        let request: Vec<(&str, String)> = params(
            base_instant(),
            PLATFORM,
            &good_profile,
            &good_job,
            std::slice::from_ref(&good_hash),
        )
        .into_iter()
        .map(|(name, original)| {
            if name == parameter {
                (name, value.clone())
            } else {
                (name, original)
            }
        })
        .collect();

        let (status, body) = emulator.get(&request).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_status == 400 {
            let (code, message) = error_of(&body, &case);
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            assert!(
                !message.is_empty(),
                "case {case}: the error must carry a message, got {body}"
            );
        } else {
            assert_eq!(
                revoked_entities(status, &body, &case),
                Vec::<String>::new(),
                "case {case}: an accepted request with nothing revoked must answer an \
                 empty list, got {body}"
            );
        }
    }
}

/// A required member the request supplied nothing for is refused for the
/// constraint it ACTUALLY violated, named together with the member.
///
/// For the LIST member that constraint is "must not be null or empty": a
/// query-bound list that is absent and one that is empty are the SAME request —
/// an SDK serializing an empty list emits no parameter at all — so the refusal
/// cannot assert the value was null. For a single-value member absence IS null,
/// so those keep the framework's own wording, which says exactly that.
///
/// One row per required member (the model marks all five `@required` +
/// `@httpQuery`), plus the row that omits every one of them, plus the row where
/// the member is present and its VALUE is what fails — a different constraint,
/// which keeps its own wording.
#[tokio::test]
async fn a_missing_required_member_is_refused_for_the_constraint_it_violated() {
    let emulator = Emulator::new();
    let profile = profile_version_arn("constraint");
    let job = job_arn("constraint");
    let hash = certificate_hash("constraint");
    let complete = params(
        base_instant(),
        PLATFORM,
        &profile,
        &job,
        std::slice::from_ref(&hash),
    );

    // The one LIST member: `certificateHashes` targets `CertificateHashes`
    // (smithy_query(query="shape",shape="CertificateHashes") — type "list");
    // every other required member targets a single value.
    let list_member = "certificateHashes";
    let required = [
        "signatureTimestamp",
        "platformId",
        "profileVersionArn",
        "jobArn",
        list_member,
    ];

    for omitted in required {
        let case = format!("omitted={omitted}");
        let partial: Vec<(&str, String)> = complete
            .iter()
            .filter(|(name, _)| *name != omitted)
            .cloned()
            .collect();
        let (status, body) = emulator.get(&partial).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error_of(&body, &case);
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        if omitted == list_member {
            assert_eq!(
                message,
                format!("{omitted} must not be null or empty"),
                "case {case}: expected the refusal to name the member and the empty-or-null \
                 constraint it violated, got {message:?}"
            );
        } else {
            assert!(
                message.contains(omitted) && message.contains("must not be null"),
                "case {case}: expected the refusal to name the member and that it must not be \
                 null, got {message:?}"
            );
            assert!(
                !message.contains("or empty"),
                "case {case}: a single-value member has no empty form, so the refusal must not \
                 claim one, got {message:?}"
            );
        }
    }

    // Nothing on the query string: a mixed refusal, which keeps the framework's
    // wording naming every member.
    let (status, body) = emulator.get(&[]).await;
    assert_eq!(
        status, 400,
        "case omitted=all: expected HTTP 400, got {status} with {body}"
    );
    let (_, message) = error_of(&body, "omitted=all");
    for member in required {
        assert!(
            message.contains(member),
            "case omitted=all: expected the message to name {member}, got {message:?}"
        );
    }

    // A member that IS supplied, with a value that fails a bound, is a different
    // refusal: it keeps the wording that echoes the offending value, and must not
    // claim the member was absent. `CertificateHash` is `@length(1, 2048)`, so an
    // empty hash is a value violation rather than an empty list.
    let empty_hash: Vec<(&str, String)> = complete
        .iter()
        .map(|(name, value)| {
            if *name == list_member {
                (*name, String::new())
            } else {
                (*name, value.clone())
            }
        })
        .collect();
    let (status, body) = emulator.get(&empty_hash).await;
    assert_eq!(
        status, 400,
        "case certificateHashes=<empty value>: expected HTTP 400, got {status} with {body}"
    );
    let (code, message) = error_of(&body, "certificateHashes=<empty value>");
    assert_eq!(
        code, "ValidationException",
        "case certificateHashes=<empty value>: expected ValidationException, got {body}"
    );
    assert!(
        message.contains(list_member),
        "case certificateHashes=<empty value>: the refusal must name the member, got {message:?}"
    );
}

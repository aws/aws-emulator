// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of a session registry record, and what every recorded session
//! therefore carries.
//!
//! THE RULE UNDER TEST: a session record is built out of what the mint asked for by this
//! service's own constructor ([`NewSession::into_session`]) and nowhere else, so the facts
//! a record DERIVES are the same however the session was recorded — the credential stops
//! being valid at `created_at + DurationSeconds`, the record is keyed by the access key id
//! the credential carries, and it starts un-revoked — and every fact the mint PASSED
//! (identity, kind, session policies, tags, source identity, external id, role) is carried
//! verbatim.
//!
//! WHY it matters beyond this crate: the identity a request speaks for is resolved from the
//! record under its access key id, so every service that judges such a request reads these
//! facts. A session recorded through the constructor is therefore resolved exactly as a
//! wire mint's is, which the rows below assert through `GetCallerIdentity` — the operation
//! that reports the record — and against a session minted by `AssumeRole` over the wire.
//!
//! The rows span what the rule generalises over: the session KIND (all three this registry
//! holds), the DURATION across the modelled bounds (`AssumeRole$DurationSeconds` is
//! `range 900..43200`, `GetSessionToken$DurationSeconds` `range 900..129600`), what the mint
//! passed beside the identity (nothing at all, an inline session policy, managed session
//! policies, tags and transitive tag keys, a source identity, an external id, a role), and
//! the side of the clock the derived expiry falls on.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use sts::state::{
    ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState, Identity, NewSession,
    Session, SessionKind, SessionPolicies,
};

use super::sts_sessions::{
    call, element, emulator, emulator_account, member, now_secs, seed_demo_role, timestamp_secs,
    LONG_TERM_KEY,
};

/// One row: a session as a mint would ask for it, under the access key id its credential
/// carries.
struct Row {
    case: &'static str,
    access_key_id: &'static str,
    /// What the mint asked for, which the constructor turns into the record.
    request: NewSession,
    /// The instant this row's record must stop being valid when the mint is recorded at
    /// [`RECORDED_AT_UNIX`], written out as a Unix second rather than computed, so the
    /// derivation is pinned independently of the code that performs it.
    expires_at_unix: u64,
}

/// The instant the first test records every row's mint at: a fixed Unix second, so each
/// row can name the expiry it must reach as a literal.
const RECORDED_AT_UNIX: u64 = 1_767_225_600;

/// The identity a row's session speaks for, as `GetCallerIdentity` reports it.
fn assumed_role_identity(role_name: &str, session_name: &str) -> Identity {
    let account = emulator_account();
    Identity {
        user_id: format!("AROASEEDEDDEMOROLE001:{session_name}"),
        account: account.clone(),
        arn: format!("arn:aws:sts::{account}:assumed-role/{role_name}/{session_name}"),
    }
}

/// The identity a `GetSessionToken` session speaks for: the IAM user that asked for it.
fn user_identity(user_name: &str) -> Identity {
    let account = emulator_account();
    Identity {
        user_id: format!("AIDASEEDEDUSER{user_name}"),
        account: account.clone(),
        arn: format!("arn:aws:iam::{account}:user/{user_name}"),
    }
}

/// The identity a `GetFederationToken` session speaks for: the federated user it named.
fn federated_identity(name: &str) -> Identity {
    let account = emulator_account();
    Identity {
        user_id: format!("{account}:{name}"),
        account: account.clone(),
        arn: format!("arn:aws:sts::{account}:federated-user/{name}"),
    }
}

/// A session policy document, as a mint passes one.
fn scoping_document(action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The rows every test below drives: the mints this registry holds records for, spanning
/// the kinds, the modelled duration bounds and everything a mint can pass.
fn rows() -> Vec<Row> {
    let demo_role = format!("arn:aws:iam::{}:role/demo", emulator_account());
    vec![
        Row {
            case: "an assumed role at the modelled minimum duration, nothing else passed",
            access_key_id: "ASIARECORDEDROW00001",
            expires_at_unix: 1_767_226_500,
            request: NewSession {
                role_arn: Some(demo_role.clone()),
                ..NewSession::new(
                    assumed_role_identity("demo", "minimum"),
                    SessionKind::AssumedRole,
                    900,
                )
            },
        },
        Row {
            case: "an assumed role at the modelled maximum duration, scoped and tagged",
            access_key_id: "ASIARECORDEDROW00002",
            expires_at_unix: 1_767_268_800,
            request: NewSession {
                role_arn: Some(demo_role.clone()),
                source_identity: Some("alice@example.com".to_string()),
                external_id: Some("tenant-42".to_string()),
                tags: vec![
                    ("project".to_string(), "blue".to_string()),
                    ("cost-centre".to_string(), "7".to_string()),
                ],
                transitive_tag_keys: vec!["project".to_string()],
                packed_policy_size: Some(37),
                policies: SessionPolicies {
                    inline: Some(scoping_document("s3:GetObject")),
                    managed_arns: vec![
                        format!("arn:aws:iam::{}:policy/read-only", emulator_account()),
                        "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess".to_string(),
                    ],
                    ..SessionPolicies::default()
                },
                ..NewSession::new(
                    assumed_role_identity("demo", "maximum"),
                    SessionKind::AssumedRole,
                    43_200,
                )
            },
        },
        Row {
            case: "a session token at the modelled maximum duration",
            access_key_id: "ASIARECORDEDROW00003",
            expires_at_unix: 1_767_355_200,
            request: NewSession::new(user_identity("alice"), SessionKind::SessionToken, 129_600),
        },
        Row {
            case: "a federation token carrying one inline session policy",
            access_key_id: "ASIARECORDEDROW00004",
            expires_at_unix: 1_767_229_200,
            request: NewSession {
                policies: SessionPolicies {
                    inline: Some(scoping_document("sqs:SendMessage")),
                    managed_arns: Vec::new(),
                    ..SessionPolicies::default()
                },
                packed_policy_size: Some(6),
                ..NewSession::new(
                    federated_identity("mobile-app"),
                    SessionKind::FederationToken,
                    3_600,
                )
            },
        },
    ]
}

/// The record the constructor builds carries every fact the mint passed, and derives the
/// three it owns: the registry key, the expiry (`created_at + DurationSeconds`) and that
/// the record starts un-revoked.
#[test]
fn a_session_record_derives_its_expiry_and_carries_every_fact_the_mint_passed() {
    let recorded_at = SystemTime::UNIX_EPOCH + Duration::from_secs(RECORDED_AT_UNIX);
    for row in rows() {
        let Row {
            case,
            access_key_id,
            request,
            expires_at_unix,
        } = row;
        let asked = request.clone();
        let record = request.into_session(access_key_id, recorded_at);

        assert_eq!(
            record.access_key_id, access_key_id,
            "case {case:?}: the record is keyed by the credential's access key id, got {:?}",
            record.access_key_id
        );
        assert_eq!(
            record.created_at, recorded_at,
            "case {case:?}: the record is created at the instant of the mint, got {:?}",
            record.created_at
        );
        let expected_expiry = SystemTime::UNIX_EPOCH + Duration::from_secs(expires_at_unix);
        assert_eq!(
            record.expires_at, expected_expiry,
            "case {case:?}: expected an expiry at the Unix second {expires_at_unix} \
             (recorded at {RECORDED_AT_UNIX}, lasting {} seconds), got {:?}",
            asked.duration_seconds, record.expires_at
        );
        assert!(
            !record.is_expired(record.expires_at - Duration::from_secs(1))
                && record.is_expired(record.expires_at),
            "case {case:?}: the session is valid up to its expiry and not a moment longer, got \
             an expiry at {:?}",
            record.expires_at
        );
        assert!(
            !record.revoked,
            "case {case:?}: a freshly recorded session is not revoked, got revoked={}",
            record.revoked
        );

        // Everything the mint passed, carried verbatim.
        assert_eq!(
            (
                record.duration_seconds,
                &record.identity,
                record.kind,
                &record.source_identity,
                &record.tags,
                &record.transitive_tag_keys,
                record.packed_policy_size,
                &record.policies,
                &record.external_id,
                &record.role_arn,
            ),
            (
                asked.duration_seconds,
                &asked.identity,
                asked.kind,
                &asked.source_identity,
                &asked.tags,
                &asked.transitive_tag_keys,
                asked.packed_policy_size,
                &asked.policies,
                &asked.external_id,
                &asked.role_arn,
            ),
            "case {case:?}: the record must carry what the mint asked for, got {record:?}"
        );
    }
}

/// A session recorded through the constructor is resolved by the operation that reports
/// it: `GetCallerIdentity` answers that record's identity for a request signed with its
/// access key id, and the derived expiry alone decides whether the credential is still
/// valid.
#[tokio::test]
async fn get_caller_identity_reports_a_recorded_session_until_its_derived_expiry() {
    let (router, engine) = emulator();
    for row in rows() {
        let Row {
            case,
            access_key_id,
            request,
            expires_at_unix: _,
        } = row;
        let identity = request.identity.clone();
        let duration = u64::try_from(request.duration_seconds).unwrap();
        // The same mint recorded twice: once at this instant, so the derived expiry is
        // still ahead of the clock, and once a whole duration plus a second ago, so the
        // clock has passed it. Nothing but the recording instant differs.
        let live_key = access_key_id.to_string();
        let expired_key = format!("{access_key_id}EXP");
        let now = SystemTime::now();
        {
            let state = engine.state::<StsState>();
            let mut held = state.lock();
            let live = request.clone().into_session(&live_key, now);
            held.insert(live).unwrap_or_else(|e| {
                panic!("case {case:?}: recording the live session: {}", e.message())
            });
            let gone_at = now - Duration::from_secs(duration + 1);
            let expired = request.into_session(&expired_key, gone_at);
            held.insert(expired).unwrap_or_else(|e| {
                panic!(
                    "case {case:?}: recording the expired session: {}",
                    e.message()
                )
            });
        }

        let (status, body) = call(&router, &live_key, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a live recorded session is served, got {status} {body}"
        );
        assert_eq!(
            (
                member(case, &body, "UserId"),
                member(case, &body, "Account"),
                member(case, &body, "Arn"),
            ),
            (
                identity.user_id.clone(),
                identity.account.clone(),
                identity.arn.clone()
            ),
            "case {case:?}: expected the recorded session's own identity {identity:?}, got {body}"
        );

        let (status, body) = call(&router, &expired_key, "GetCallerIdentity", &[]).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("ExpiredTokenException")),
            "case {case:?}: a session whose derived expiry has passed is refused, got \
             {status} {body}"
        );
    }
}

/// The wire mint goes through the SAME constructor: the session `AssumeRole` records for
/// the credential it hands back derives its expiry from the duration the call asked for,
/// is keyed by the access key id in the response, and starts un-revoked — for every
/// duration across the modelled range.
#[tokio::test]
async fn a_wire_mint_records_the_session_the_constructor_builds() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    // (case, the DurationSeconds the call asks for, the seconds the record must last)
    // `AssumeRole$DurationSeconds` is `range 900..43200`; a call that names none lasts the
    // documented hour.
    let cases: [(&str, Option<i32>, i32); 4] = [
        ("no duration named: the documented default", None, 3_600),
        ("the modelled minimum", Some(900), 900),
        ("an hour", Some(3_600), 3_600),
        ("the modelled maximum", Some(43_200), 43_200),
    ];

    for (index, (case, asked, expected_seconds)) in cases.into_iter().enumerate() {
        let mut params = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", format!("minted-{index}")),
        ];
        if let Some(asked) = asked {
            params.push(("DurationSeconds", asked.to_string()));
        }
        let requested_at = now_secs();
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");

        let access_key_id = member(case, &body, "AccessKeyId");
        let record: Session = engine
            .state::<StsState>()
            .lock()
            .session(&access_key_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the mint recorded no session under {access_key_id:?}")
            });

        assert_eq!(
            record.access_key_id, access_key_id,
            "case {case:?}: the record is keyed by the credential handed back, got {:?}",
            record.access_key_id
        );
        assert_eq!(
            record.duration_seconds, expected_seconds,
            "case {case:?}: expected a session lasting {expected_seconds} seconds, got {}",
            record.duration_seconds
        );
        assert_eq!(
            record.expires_at,
            record.created_at + Duration::from_secs(u64::try_from(expected_seconds).unwrap()),
            "case {case:?}: the mint's record must expire at created_at + DurationSeconds, got \
             created_at {:?} and expires_at {:?}",
            record.created_at,
            record.expires_at
        );
        assert!(
            !record.revoked,
            "case {case:?}: a freshly minted session is not revoked"
        );
        // And the expiry the caller was handed is the record's own.
        let handed = timestamp_secs(case, &member(case, &body, "Expiration"));
        let recorded = i64::try_from(
            record
                .expires_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("an expiry after the Unix epoch")
                .as_secs(),
        )
        .expect("an expiry that fits in an i64");
        assert_eq!(
            handed, recorded,
            "case {case:?}: the Expiration handed to the caller must be the record's own, got \
             {handed} against {recorded}"
        );
        assert!(
            handed >= requested_at + i64::from(expected_seconds),
            "case {case:?}: the expiry must be at least the asked duration past the call, got \
             {handed} against {requested_at} + {expected_seconds}"
        );
    }
}

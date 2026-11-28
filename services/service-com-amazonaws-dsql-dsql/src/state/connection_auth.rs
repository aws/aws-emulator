// SPDX-License-Identifier: CC0-1.0

//! The CREDENTIAL a session on a cluster's PostgreSQL endpoint presents, and
//! whether the cluster accepts it.
//!
//! Aurora DSQL authenticates a connection with an AUTH TOKEN rather than with a
//! password the service keeps: the client presigns a request for the cluster's
//! own host and sends the result as the PostgreSQL password
//! (`GenerateDbConnectAdminAuthToken` / `GenerateDbConnectAuthToken` are
//! client-side presigners, so no call of this service mints one). The token is
//! therefore the only thing that can be judged, and what it carries is what
//! decides the login:
//!
//! * the ACTION it was presigned for, which is bound to the database user it logs
//!   on as — the administrator action for the admin role, the connect action for
//!   every other role;
//! * the HOST it was presigned against, which is the cluster's own endpoint, so a
//!   token minted for another cluster's address opens no session here;
//! * its LIFETIME, which the token carries as the instant it was signed at plus
//!   the seconds it was signed for. The documentation of the token
//!   (<https://docs.aws.amazon.com/aurora-dsql/latest/userguide/SECTION_authentication-token.html>)
//!   is explicit that it authenticates the CONNECTION: the lifetime is spent at
//!   connect time and a session already open is not touched when it runs out,
//!   which is why this is asked once, by the login, and never again by a
//!   statement.
//!
//! Nothing here holds a secret, and nothing is compared with one: a presigned
//! credential is judged by its own contents, which is exactly the login the
//! shared data plane hands to the owning service
//! ([`::service_common::sql_wire_planes::ServedDatabase::verified_login`]). The
//! query fields are read with the tree's own reading of a presigned request's
//! query ([`::service_common::query_parameter`]).
//!
//! What a presigned credential's own contents are is therefore the whole of the
//! decision, and every field of it is held to what AWS put there:
//!
//! * the ALGORITHM, which is the one a presigned AWS request is signed under;
//! * the CREDENTIAL SCOPE, which names the access key the token was signed with,
//! the date it was signed on, a Region, this service's own signing name and the
//! `aws4_request` terminator — a scope naming another service's signing name is
//! not a credential for a cluster, whatever it is otherwise;
//! * the SIGNATURE, which is the hex of a SHA-256 HMAC and nothing else;
//! * the signing INSTANT, which may not be further ahead of this clock than the
//! skew AWS allows a signed request, and the LIFETIME above.
//!
//! What it does NOT do is recompute the signature, for the same reason the Amazon
//! RDS endpoint's token does not
//! (`services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/iam_auth_token.rs`):
//! the signing key belongs to the credential the token was presigned with, and
//! the listener the session arrived on resolves none. The other half of the
//! decision is the DATABASE's — the role the token names has to be one the
//! cluster holds ([`role_does_not_exist`]) — so a token alone admits nobody to a
//! role that does not exist.
//!
//! Amazon RDS reads a token of its OWN beside this one
//! (`services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/iam_auth_token.rs`),
//! and what the two decide is not the same question: an RDS token names the
//! database user in a field of its own and the endpoint's PORT in its authority,
//! is scoped to that service's own signing name, and is held to the 15-minute
//! lifetime that service documents, while a cluster's token carries the role in
//! the ACTION it was presigned for, no port, and the lifetime its client asked
//! for. The shared half — the query of a presigned request and the instant its
//! `X-Amz-Date` names — is read through `service_common` and `chrono` here
//! rather than spelled a second time.

use std::time::{Duration, SystemTime};

use ::service_common::sql_wire_planes::StatedRefusal;

/// The database user the administrator action logs on as: Aurora DSQL's
/// administrator role, which is the one a cluster has from the moment it is
/// created.
pub const ADMIN_USER: &str = "admin";

/// The one database a cluster holds, which is the database a session names (or
/// is served when it names none).
pub const CLUSTER_DATABASE: &str = "postgres";

/// The action a token for the administrator role is presigned for.
const ADMIN_ACTION: &str = "DbConnectAdmin";

/// The action a token for every other database role is presigned for.
const CONNECT_ACTION: &str = "DbConnect";

/// The query fields a presigned request carries, which is the form a token is
/// written in: `<host>/?Action=<action>&X-Amz-Algorithm=…&X-Amz-Credential=…
/// &X-Amz-Date=…&X-Amz-Expires=…&X-Amz-SignedHeaders=…&X-Amz-Signature=…`.
const ACTION_FIELD: &str = "Action";
const ALGORITHM_FIELD: &str = "X-Amz-Algorithm";
const CREDENTIAL_FIELD: &str = "X-Amz-Credential";
const DATE_FIELD: &str = "X-Amz-Date";
const EXPIRES_FIELD: &str = "X-Amz-Expires";
const SIGNATURE_FIELD: &str = "X-Amz-Signature";

/// The algorithm a presigned AWS request names, and the only one a token is
/// signed under.
const SIGNING_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The signing name a cluster's auth token is presigned for, which is the
/// service its credential scope names (`<akid>/<date>/<region>/dsql/aws4_request`).
const TOKEN_SIGNING_SERVICE: &str = "dsql";

/// The terminator a SigV4 credential scope ends with.
const SCOPE_TERMINATOR: &str = "aws4_request";

/// How many characters a SigV4 signature is: the lower-case hex of a SHA-256
/// HMAC.
const SIGNATURE_CHARS: usize = 64;

/// How far ahead of this clock a token's signing instant may be: the skew AWS
/// allows a signed request, so a client whose clock runs slightly fast still
/// authenticates, and one dated far in the future does not.
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(15 * 60);

/// The form of the date a credential scope carries, which is the day of the
/// instant `X-Amz-Date` names.
const SCOPE_DATE_FORMAT: &str = "%Y%m%d";

/// How a connection that is not accepted is refused: the `SQLSTATE` for a
/// credential that does not authorize the session it asked for
/// (`invalid_authorization_specification`, PostgreSQL's "Appendix A. Error
/// Codes"), under which Aurora DSQL words the refusal itself.
const ACCESS_DENIED_STATE: &str = "28000";

/// The sentence a connection the cluster does not accept is refused with.
const ACCESS_DENIED_MESSAGE: &str = "unable to accept connection, access denied";

/// The format the signing instant of a presigned request is written in: ISO 8601
/// basic, in UTC, which is what `X-Amz-Date` carries.
const AMZ_DATE_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// Whether the cluster reached at `endpoint_host` accepts the `password` a
/// session presented for `user` at `now`.
///
/// `Ok` admits the login; `Err` is the refusal, worded as Aurora DSQL words it,
/// for the wire to report and close the connection with.
pub fn admits(
    password: &str,
    user: &str,
    endpoint_host: &str,
    now: SystemTime,
) -> Result<(), StatedRefusal> {
    // A password that is not an auth token at all names no action, no host and no
    // lifetime, so there is nothing to accept it on.
    let Some(token) = AuthToken::read(password) else {
        return Err(access_denied(None));
    };
    // The action a token is presigned for is bound to the role it logs on as, so
    // an administrator token presented for another role — and a connect token
    // presented for the administrator — is not this login's credential.
    let expected = action_for(user);
    if token.action != expected {
        return Err(access_denied(Some(format!(
            "Wrong user to action mapping. user: {user}, action: {}",
            token.action
        ))));
    }
    // The token is presigned against ONE host: the cluster's own endpoint. A
    // token minted for another address proves nothing about this cluster.
    if !token.names_host(endpoint_host) {
        return Err(access_denied(None));
    }
    // The lifetime, spent at connect: a token presented after the instant it was
    // signed for has run out opens no session, while one already open is left
    // alone, because nothing asks this again.
    let expires_at = token.expires_at();
    if expires_at <= now {
        return Err(access_denied(Some(format!(
            "Signature expired: {} is now earlier than {}",
            amz_date(expires_at),
            amz_date(now)
        ))));
    }
    // ... and a token signed further ahead of this clock than the skew AWS
    // allows a signed request is no credential either: a lifetime that has not
    // begun authenticates nothing.
    if token.signed_at > now + MAX_CLOCK_SKEW {
        return Err(access_denied(Some(format!(
            "Signature not yet current: {} is still later than {}",
            amz_date(token.signed_at),
            amz_date(now)
        ))));
    }
    Ok(())
}

/// The refusal of a login as a role the cluster does not hold, which is what
/// PostgreSQL answers a login naming one: `role "<name>" does not exist`, under
/// the same class a credential that authorizes nothing is refused under.
///
/// A cluster holds the administrator role from the moment it is created
/// ([`ADMIN_USER`]) and no other until a session creates one, so a token minted
/// for the connect action proves only that its holder may connect AS some role —
/// which role is then a question for the database.
#[must_use]
pub fn role_does_not_exist(user: &str) -> StatedRefusal {
    StatedRefusal {
        sqlstate: ACCESS_DENIED_STATE,
        message: format!("role \"{user}\" does not exist"),
        hint: None,
    }
}

/// The refusal of a credential that says nothing about this cluster at all — one
/// presented in a form no auth token is written in.
///
/// The same sentence [`admits`] refuses a password that is not a token with:
/// what the cluster tells a client is that the connection was not accepted, and
/// nothing about the credential it was offered.
#[must_use]
pub fn not_this_clusters_credential() -> StatedRefusal {
    access_denied(None)
}

/// The action a token for the database user `user` has to be presigned for: the
/// administrator action for the administrator role, the connect action for every
/// other role.
fn action_for(user: &str) -> &'static str {
    if user == ADMIN_USER {
        ADMIN_ACTION
    } else {
        CONNECT_ACTION
    }
}

/// The refusal of a connection the cluster does not accept, with the hint that
/// says why where there is one to say.
fn access_denied(hint: Option<String>) -> StatedRefusal {
    StatedRefusal {
        sqlstate: ACCESS_DENIED_STATE,
        message: ACCESS_DENIED_MESSAGE.to_string(),
        hint,
    }
}

/// One auth token, as the password a session presented carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthToken {
    /// The action it was presigned for.
    action: String,
    /// The host it was presigned against, lower-cased: a host is compared
    /// case-insensitively.
    host: String,
    /// The instant it was signed at.
    signed_at: SystemTime,
    /// How long after that instant it authenticates a connection.
    lifetime: Duration,
}

impl AuthToken {
    /// The token `password` carries, or `None` when it is not one: a token is a
    /// presigned request, so a value that does not carry the fields a presigned
    /// request is made of is not a token at all.
    fn read(password: &str) -> Option<Self> {
        // The token is written as the presigned request's host, path and query,
        // with no scheme; a client that kept one is read the same way.
        let without_scheme = password
            .split_once("://")
            .map_or(password, |(_scheme, rest)| rest);
        let (address, query) = without_scheme.split_once('?')?;
        let host = address
            .split('/')
            .next()
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host.is_empty() {
            return None;
        }
        let field = |name: &str| ::service_common::query_parameter(query, name);
        // Signed under AWS's own algorithm, by a credential naming a scope of
        // this service's own, with a signature over it: the three a presigned
        // request always carries, each held to the form AWS gives it.
        if field(ALGORITHM_FIELD) != Some(SIGNING_ALGORITHM) {
            return None;
        }
        let signed_at = amz_instant(field(DATE_FIELD)?)?;
        if !names_this_services_scope(field(CREDENTIAL_FIELD)?, signed_at) {
            return None;
        }
        if !is_a_sigv4_signature(field(SIGNATURE_FIELD)?) {
            return None;
        }
        let lifetime = Duration::from_secs(field(EXPIRES_FIELD)?.parse::<u64>().ok()?);
        Some(Self {
            action: field(ACTION_FIELD)?.to_string(),
            host,
            signed_at,
            lifetime,
        })
    }

    /// Whether this token was presigned against `host`.
    fn names_host(&self, host: &str) -> bool {
        self.host == host.trim_end_matches('.').to_ascii_lowercase()
    }

    /// The instant after which this token authenticates no further connection.
    fn expires_at(&self) -> SystemTime {
        self.signed_at
            .checked_add(self.lifetime)
            .unwrap_or(self.signed_at)
    }
}

/// Whether `credential` is a SigV4 credential scope of THIS service's, for a
/// request signed at `signed_at`: an access key id, the day that instant falls
/// on, a Region, this service's signing name and the `aws4_request` terminator.
///
/// A scope naming another service's signing name is a credential for that
/// service and not for a cluster, and one naming another day is not the scope
/// the request's own `X-Amz-Date` is signed under — neither is a token this
/// endpoint was presented with.
///
/// The value is read as the query carries it, which is how the tree reads a
/// presigned query ([`::service_common::query_parameter`]), so the scope's own
/// separators arrive percent-escaped from a client that escaped them: they are
/// decoded by the framework's own decoder
/// ([`::smithy_server_runtime::percent_decode`]) rather than by a substitution
/// written here.
fn names_this_services_scope(credential: &str, signed_at: SystemTime) -> bool {
    let unescaped = ::smithy_server_runtime::percent_decode(credential);
    let parts: Vec<&str> = unescaped.split('/').collect();
    let [key_id, date, region, service, terminator] = parts.as_slice() else {
        return false;
    };
    !key_id.is_empty()
        && *date == scope_date(signed_at)
        && !region.is_empty()
        && *service == TOKEN_SIGNING_SERVICE
        && *terminator == SCOPE_TERMINATOR
}

/// Whether `signature` is a SigV4 signature: the lower-case hex of a SHA-256
/// HMAC, and nothing else.
fn is_a_sigv4_signature(signature: &str) -> bool {
    signature.len() == SIGNATURE_CHARS
        && signature
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
}

/// The day `instant` falls on, as a credential scope writes it.
fn scope_date(instant: SystemTime) -> String {
    let seconds = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    ::chrono::DateTime::from_timestamp(i64::try_from(seconds).unwrap_or(i64::MAX), 0)
        .map(|at| at.format(SCOPE_DATE_FORMAT).to_string())
        .unwrap_or_default()
}

/// The instant an `X-Amz-Date` names, or `None` when the value is not one.
fn amz_instant(value: &str) -> Option<SystemTime> {
    let naive = ::chrono::NaiveDateTime::parse_from_str(value, AMZ_DATE_FORMAT).ok()?;
    let seconds = naive.and_utc().timestamp();
    let seconds = u64::try_from(seconds).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

/// `instant` written the way a presigned request writes one.
fn amz_date(instant: SystemTime) -> String {
    let seconds = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    ::chrono::DateTime::from_timestamp(i64::try_from(seconds).unwrap_or(i64::MAX), 0)
        .map(|at| at.format(AMZ_DATE_FORMAT).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host every token below is presigned against.
    const HOST: &str = "127.0.0.1";

    /// One instant a token is signed at, and the clock a login arrives on.
    fn signed_at() -> SystemTime {
        amz_instant("20260101T000000Z").expect("the signing instant reads")
    }

    /// A token presigned for `action` against `host`, signed at `at` for
    /// `lifetime` seconds — the form the client-side presigner answers, with the
    /// credential scope of the day it was signed on and a signature of the shape
    /// a SigV4 signature has.
    fn token(action: &str, host: &str, at: SystemTime, lifetime: u64) -> String {
        format!(
            "{host}/?Action={action}&X-Amz-Algorithm={SIGNING_ALGORITHM}\
             &X-Amz-Credential=AKIAEXAMPLE%2F{}%2Fus-east-1%2F{TOKEN_SIGNING_SERVICE}\
             %2F{SCOPE_TERMINATOR}\
             &X-Amz-Date={}&X-Amz-Expires={lifetime}&X-Amz-SignedHeaders=host\
             &X-Amz-Signature={}",
            scope_date(at),
            amz_date(at),
            "0".repeat(SIGNATURE_CHARS)
        )
    }

    /// RULE: the TOKEN is accepted for the user its action is bound to, against
    /// the host it was presigned for, until the instant its lifetime runs out.
    /// Whether the cluster HOLDS that role is a separate question, answered by
    /// [`role_does_not_exist`] where it does not.
    ///
    /// The rows span what the rule generalises over: both actions and the roles
    /// they are bound to, lifetimes from a few seconds to the presigner's
    /// default, and a login before, exactly at and after the instant the token
    /// runs out.
    #[test]
    fn a_token_authenticates_its_own_action_host_and_lifetime() {
        let at = signed_at();
        // (case, action, user, lifetime, the login's instant, admitted)
        let rows: &[(&str, &str, &str, u64, Duration, bool)] = &[
            (
                "an admin token well inside its lifetime",
                ADMIN_ACTION,
                ADMIN_USER,
                900,
                Duration::from_secs(1),
                true,
            ),
            (
                "an admin token with one second left",
                ADMIN_ACTION,
                ADMIN_USER,
                20,
                Duration::from_secs(19),
                true,
            ),
            (
                "an admin token at the instant it runs out",
                ADMIN_ACTION,
                ADMIN_USER,
                20,
                Duration::from_secs(20),
                false,
            ),
            (
                "an admin token past its lifetime",
                ADMIN_ACTION,
                ADMIN_USER,
                20,
                Duration::from_secs(35),
                false,
            ),
            (
                "a connect token for another role",
                CONNECT_ACTION,
                "appuser",
                900,
                Duration::from_secs(1),
                true,
            ),            (
                "a connect token presented for the admin role",
                CONNECT_ACTION,
                ADMIN_USER,
                900,
                Duration::from_secs(1),
                false,
            ),
            (
                "an admin token presented for another role",
                ADMIN_ACTION,
                "appuser",
                900,
                Duration::from_secs(1),
                false,
            ),
        ];
        for (case, action, user, lifetime, elapsed, admitted) in rows {
            let presented = token(action, HOST, at, *lifetime);
            let verdict = admits(&presented, user, HOST, at + *elapsed);
            assert_eq!(
                verdict.is_ok(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {verdict:?}"
            );
        }
    }

    /// RULE: the refusal is Aurora DSQL's own sentence, and the hint says which
    /// of the token's facts failed — the action-to-user mapping, the lifetime
    /// that has run out, or a signing instant further ahead of this clock than a
    /// signed request's skew.
    #[test]
    fn a_refusal_carries_the_fixed_sentence_and_the_hint_for_its_cause() {
        let at = signed_at();
        let expired = admits(
            &token(ADMIN_ACTION, HOST, at, 20),
            ADMIN_USER,
            HOST,
            at + Duration::from_secs(35),
        )
        .expect_err("a token past its lifetime is refused");
        assert_eq!(
            expired.message, ACCESS_DENIED_MESSAGE,
            "case expired: expected {ACCESS_DENIED_MESSAGE:?}, got {:?}",
            expired.message
        );
        assert_eq!(
            expired.hint.as_deref(),
            Some("Signature expired: 20260101T000020Z is now earlier than 20260101T000035Z"),
            "case expired: the hint must name the instant the token ran out, got {:?}",
            expired.hint
        );

        let mismapped = admits(&token(CONNECT_ACTION, HOST, at, 900), ADMIN_USER, HOST, at)
            .expect_err("a connect token is refused for the admin role");
        assert_eq!(
            mismapped.hint.as_deref(),
            Some("Wrong user to action mapping. user: admin, action: DbConnect"),
            "case mismapped: the hint must name the user and the action, got {:?}",
            mismapped.hint
        );
        assert_eq!(
            mismapped.message, ACCESS_DENIED_MESSAGE,
            "case mismapped: expected {ACCESS_DENIED_MESSAGE:?}, got {:?}",
            mismapped.message
        );

        // A token signed further ahead of this clock than the skew a signed
        // request is allowed: its lifetime has not begun, so it authenticates
        // nothing.
        let ahead = at + MAX_CLOCK_SKEW + Duration::from_secs(60);
        let not_yet = admits(&token(ADMIN_ACTION, HOST, ahead, 900), ADMIN_USER, HOST, at)
            .expect_err("a token signed past the allowed skew is refused");
        assert_eq!(
            not_yet.hint.as_deref(),
            Some(
                format!(
                    "Signature not yet current: {} is still later than {}",
                    amz_date(ahead),
                    amz_date(at)
                )
                .as_str()
            ),
            "case not yet current: the hint must name the instant it was signed at, got {:?}",
            not_yet.hint
        );
        assert_eq!(
            not_yet.message, ACCESS_DENIED_MESSAGE,
            "case not yet current: expected {ACCESS_DENIED_MESSAGE:?}, got {:?}",
            not_yet.message
        );
        // ... while one inside that skew still authenticates, which is what the
        // skew is for.
        let inside = admits(
            &token(ADMIN_ACTION, HOST, at + Duration::from_secs(60), 900),
            ADMIN_USER,
            HOST,
            at,
        );
        assert!(
            inside.is_ok(),
            "case inside the skew: a clock slightly ahead must still authenticate, got {inside:?}"
        );
    }

    /// RULE: a password that is not a token, and a token presigned against
    /// another host, are refused with the same sentence and no hint — neither
    /// says anything about this cluster.
    #[test]
    fn a_password_that_is_not_this_clusters_token_is_refused() {
        let at = signed_at();
        let full = token(ADMIN_ACTION, HOST, at, 900);
        // (case, the password presented)
        let rows: &[(&str, String)] = &[
            ("an ordinary password", "Pw9#xZq!notatoken".to_string()),
            ("nothing at all", String::new()),
            (
                "a token presigned against another host",
                token(ADMIN_ACTION, "other-cluster.example", at, 900),
            ),
            (
                "the query of a token without its host",
                full.split_once('?')
                    .map(|(_, query)| format!("?{query}"))
                    .unwrap_or_default(),
            ),
            (
                "a token carrying no signature",
                full.replace(&format!("&X-Amz-Signature={}", "0".repeat(SIGNATURE_CHARS)), ""),
            ),
            (
                "a token whose signature is not a SigV4 signature",
                full.replace(&"0".repeat(SIGNATURE_CHARS), "0123456789abcdef"),
            ),
            (
                "a token whose signature carries what hex does not",
                full.replace(&"0".repeat(SIGNATURE_CHARS), &"Z".repeat(SIGNATURE_CHARS)),
            ),
            (
                "a token signed under another algorithm",
                full.replace(SIGNING_ALGORITHM, "AWS4-ECDSA-P256-SHA256"),
            ),
            (
                "a token whose scope names another service",
                full.replace(
                    &format!("%2F{TOKEN_SIGNING_SERVICE}%2F"),
                    "%2Frds-db%2F",
                ),
            ),
            (
                "a token whose scope names another day than its own signing instant",
                full.replace(&format!("%2F{}%2F", scope_date(at)), "%2F20200101%2F"),
            ),
            (
                "a token whose scope is not a credential scope at all",
                full.replace(
                    &format!(
                        "AKIAEXAMPLE%2F{}%2Fus-east-1%2F{TOKEN_SIGNING_SERVICE}%2F{SCOPE_TERMINATOR}",
                        scope_date(at)
                    ),
                    "AKIAEXAMPLE",
                ),
            ),
            (
                "a token carrying no lifetime",
                full.replace("&X-Amz-Expires=900", ""),
            ),
            (
                "a token whose signing instant is not a date",
                full.replace(&amz_date(at), "not-a-date"),
            ),
        ];
        for (case, presented) in rows {
            let refusal = admits(presented, ADMIN_USER, HOST, at)
                .expect_err(&format!("case {case:?}: the login must be refused"));
            assert_eq!(
                refusal.message, ACCESS_DENIED_MESSAGE,
                "case {case:?}: expected {ACCESS_DENIED_MESSAGE:?}, got {:?}",
                refusal.message
            );
            assert_eq!(
                refusal.hint, None,
                "case {case:?}: a credential that says nothing about this cluster carries no \
                 hint, got {:?}",
                refusal.hint
            );
        }
    }

    /// RULE: the host a token names is compared as a host is — the case it was
    /// written in does not matter, and a trailing root dot names the same host.
    #[test]
    fn the_host_is_compared_as_a_host() {
        let at = signed_at();
        // (case, the host the token was presigned against, the endpoint's host)
        let rows: &[(&str, &str, &str)] = &[
            ("the same spelling", "cluster.localhost", "cluster.localhost"),
            ("another case", "Cluster.LOCALHOST", "cluster.localhost"),
            ("a trailing root dot", "cluster.localhost.", "cluster.localhost"),
            ("a loopback address", "127.0.0.1", "127.0.0.1"),
        ];
        for (case, signed_for, endpoint) in rows {
            let verdict = admits(
                &token(ADMIN_ACTION, signed_for, at, 900),
                ADMIN_USER,
                endpoint,
                at,
            );
            assert!(
                verdict.is_ok(),
                "case {case:?}: a token presigned for {signed_for:?} must be accepted at \
                 {endpoint:?}, got {verdict:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! WHAT the temporary credential `GetDataAccess` vends IS: a session of the registered
//! location's IAM role, CAPPED by a session policy derived from the matched grant.
//!
//! "Returns a temporary access credential from S3 Access Grants to the grantee or client
//! application. The temporary credential is an Amazon Web Services STS token that grants them
//! access to the S3 data" (`GetDataAccess`), and the role that token is a session of is the
//! one the location was registered with: "the IAM role that S3 Access Grants assumes must
//! have the following permissions specified in the trust policy when registering the
//! location: sts:AssumeRole" (same operation). So what the holder of the credential may then
//! do is what that role's own policies allow — "the temporary security credentials … have the
//! permissions of the role" (`sts:AssumeRole`) — NARROWED to the data the grant covers, which
//! is what makes the credential a least-privilege one rather than the role itself.
//!
//! The narrowing is a SESSION POLICY, the one mechanism AWS caps a vended session's
//! permissions with: "the resulting session's permissions are the intersection of the role's
//! identity-based policies and the session policies" (`AssumeRoleRequest$Policy`). It is
//! recorded on the session the token service holds, through the shared role-session mint
//! ([`sts_store::mint_role_session`]), because the service that MINTS a session is not the
//! service that serves the calls made with it: Amazon S3's own authorizer reads the scope off
//! the session the presented access key id names, so a read of a key the grant does not cover
//! is refused there, by S3, in the sentence IAM words for a request no session policy allows
//! — and a read inside the grant is left to the role's own policies.
//!
//! Minting a bare credential instead would hand out a session that speaks for nobody, which
//! every service serves as the ambient identity: account-wide authority, which is the
//! opposite of what this operation is for.
//!
//! LOCK ORDER: every function here reaches the token service's and IAM's stores, and holds no
//! guard of THIS service's store while it does, so it must be called with this service's
//! guard given up — the same discipline as this service's other reaches into another store
//! ([`crate::state::account_public_access_block`]).

use ::std::time::SystemTime;

use ::emulator_server::CredentialStore;
use ::serde_json::json;
use ::service_com_amazonaws_s3_amazons3::state::{bucket_arn, object_arn};
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

use crate::state::{PERMISSION_READ, PERMISSION_READWRITE, PERMISSION_WRITE};

/// The policy language version every document this module writes declares, which is the
/// current one IAM publishes ("2012-10-17 … the current version of the policy language",
/// IAM User Guide, *IAM JSON policy elements: Version*).
const POLICY_VERSION: &str = "2012-10-17";

/// The scheme every S3 Access Grants scope is written in: an `S3Prefix` is "the S3 URI path
/// of the data" (`GetDataAccessRequest$Target`).
const S3_URI_SCHEME: &str = "s3://";

/// The condition key a listing's `prefix` parameter answers, which the bucket-level half of a
/// scoped session policy is written over so that a credential scoped to one prefix can
/// enumerate THAT prefix and no other (Amazon S3 User Guide, *Actions, resources, and
/// condition keys for Amazon S3*: `s3:prefix`).
const PREFIX_CONDITION_KEY: &str = "s3:prefix";

/// The OBJECT-level actions a `READ` grant's session policy allows on the data under its
/// scope.
///
/// The set S3 Access Grants documents for the READ half of a location's role policy (Amazon
/// S3 User Guide, *Register a location*): reading an object, reading a named version of one,
/// reading either one's ACL, and listing the parts a multipart upload holds.
pub const READ_OBJECT_ACTIONS: &[&str] = &[
    "s3:GetObject",
    "s3:GetObjectVersion",
    "s3:GetObjectAcl",
    "s3:GetObjectVersionAcl",
    "s3:ListMultipartUploadParts",
];

/// The BUCKET-level actions a `READ` grant's session policy allows, each confined to the key
/// space the scope covers by [`PREFIX_CONDITION_KEY`].
///
/// The bucket half of the same documented role policy: enumerating a bucket's keys, its
/// versions and the multipart uploads it holds. A bucket-level action is authorized over the
/// BUCKET's ARN rather than an object's, which is why it is a statement of its own.
pub const READ_BUCKET_ACTIONS: &[&str] = &[
    "s3:ListBucket",
    "s3:ListBucketVersions",
    "s3:ListBucketMultipartUploads",
];

/// The OBJECT-level actions a `WRITE` grant's session policy allows on the data under its
/// scope.
///
/// The WRITE half of the same documented role policy: writing an object, writing either an
/// object's or a version's ACL, deleting an object or a named version of one, and abandoning
/// a multipart upload. A write grant confers no bucket-level action: enumerating a bucket's
/// contents is reading it.
pub const WRITE_OBJECT_ACTIONS: &[&str] = &[
    "s3:PutObject",
    "s3:PutObjectAcl",
    "s3:PutObjectVersionAcl",
    "s3:DeleteObject",
    "s3:DeleteObjectVersion",
    "s3:AbortMultipartUpload",
];

/// What one vend is asked for: the role the token is a session of, the scope and permission
/// the token carries, how long it lasts and the name of the session itself.
#[derive(Debug, Clone)]
pub struct DataAccessSessionPlan {
    /// The `IAMRoleArn` of the registered location the matched grant narrows — the role S3
    /// Access Grants assumes for that location.
    pub role_arn: String,
    /// The S3 URI path the token is scoped to: the matched grant's scope, or the requested
    /// target for a vend asking for the `Minimal` privilege — the very value the response
    /// reports as `MatchedGrantTarget`.
    pub scope: String,
    /// The `Permission` the vend asked for, which the matched grant satisfies. The token
    /// carries THAT permission and not the grant's, so a `READ` vend against a `READWRITE`
    /// grant is a read-only credential.
    pub permission: String,
    /// The session duration the vend settled on, in seconds.
    pub duration_seconds: i32,
    /// The name of the session, which its own ARN carries. Minted per vend
    /// ([`crate::state::ComAmazonawsS3controlAwss3controlservicev20180820State::new_data_access_session_name`]),
    /// because S3 Access Grants mints a new session per `GetDataAccess` call.
    pub session_name: String,
}

/// One vended session: the credential to hand the caller, and the identity it speaks for.
#[derive(Debug, Clone)]
pub struct VendedDataAccess {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    /// The instant the credential stops being accepted.
    pub expires_at: SystemTime,
    /// `arn:aws:sts::<account>:assumed-role/<role name>/<session name>` — the ARN
    /// `sts:GetCallerIdentity` reports for these very credentials, and the principal a refusal
    /// of a request they signed names.
    pub session_arn: String,
}

/// Mint the session `plan` describes, and record it everywhere a request signed with it is
/// resolved from.
///
/// `None` when no session of that role can be recorded at all — a `role_arn` that is not an
/// IAM role ARN, an engine carrying no token-service store, or a bounded session registry
/// with no room left. The caller then vends NOTHING: a credential this emulator cannot scope
/// is a credential that would act as the whole account.
pub fn vend<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    now: SystemTime,
    plan: &DataAccessSessionPlan,
) -> Option<VendedDataAccess> {
    let role = sts_store::parse_role_arn(&plan.role_arn)?;
    // The role's own id when the account's IAM holds the role, and the id STS derives from
    // the ARN otherwise: a location may be registered with a role provisioned out of band.
    let role_id = held_role_id(engine, &plan.role_arn);
    let session_arn = sts_store::sts_arn(
        &role.account,
        &format!("assumed-role/{}/{}", role.name, plan.session_name),
    );
    let identity = sts_store::Identity {
        user_id: format!("{role_id}:{}", plan.session_name),
        account: role.account,
        arn: session_arn.clone(),
    };
    let registry = sts_store::service_state(engine)?;
    let minted = sts_store::mint_role_session(
        engine,
        store,
        &registry,
        now,
        sts_store::NewSession {
            // The grant-derived cap, which is the whole point of the vend.
            policies: sts_store::SessionPolicies {
                inline: Some(session_policy(&plan.scope, &plan.permission)),
                managed_arns: Vec::new(),
            },
            role_arn: Some(plan.role_arn.clone()),
            ..sts_store::NewSession::new(
                identity,
                sts_store::SessionKind::AssumedRole,
                plan.duration_seconds,
            )
        },
    )
    .ok()?;
    Some(VendedDataAccess {
        access_key_id: minted.credentials.access_key_id,
        secret_access_key: minted.credentials.secret_access_key,
        session_token: minted.credentials.session_token,
        expires_at: minted.session.expires_at,
        session_arn,
    })
}

/// Give up a session this module vended, and the credential material with it — for a vend
/// this service decided, after minting, that it cannot honour.
pub fn forget<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    access_key_id: &str,
) {
    match sts_store::service_state(engine) {
        Some(registry) => registry.lock().discard(access_key_id, store),
        None => {
            let _ = store.delete(access_key_id);
        }
    }
}

/// The id the account's IAM holds for the role `role_arn`, and the id STS derives from the
/// ARN when that store holds no such role.
fn held_role_id<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: &str) -> String {
    let held = sts_store::iam_state(engine).and_then(|iam| {
        let store = iam.lock();
        store
            .roles
            .values()
            .find(|role| role.arn == role_arn)
            .map(|role| role.role_id.clone())
    });
    held.unwrap_or_else(|| sts_store::role_id(role_arn))
}

/// The session policy one vend caps its session with: an `Allow` of exactly the actions
/// `permission` covers, on exactly the S3 resources `scope` covers, and nothing else.
///
/// A session policy GRANTS nothing — it caps what the role's own policies grant — so the
/// document needs no `Deny`: an action or a resource it says nothing about is withheld,
/// which is what refuses a read of a key outside the scope while leaving a read inside it to
/// the role.
///
/// The resources are read off the scope exactly as coverage is ([`crate::state::
/// scope_coverage`]): a scope ending in `*` covers every key starting with the text before
/// it, a scope ending in `/` covers every key beneath it, and any other scope names one
/// object and the keys under it as a prefix boundary.
#[must_use]
pub fn session_policy(scope: &str, permission: &str) -> String {
    let path = ScopedPath::of(scope);
    let mut statements = Vec::new();
    let object_actions = object_actions(permission);
    if !object_actions.is_empty() {
        statements.push(json!({
            "Effect": "Allow",
            "Action": object_actions,
            "Resource": path.object_arns(),
        }));
    }
    let bucket_actions = bucket_actions(permission);
    if !bucket_actions.is_empty() {
        let mut statement = json!({
            "Effect": "Allow",
            "Action": bucket_actions,
            "Resource": [bucket_arn(&path.bucket)],
        });
        // A bucket-level action is authorized over the BUCKET, which carries no key — so a
        // scope that covers only PART of a bucket confines the enumeration by the one key
        // the request answers about its key space ([`PREFIX_CONDITION_KEY`]), and a scope
        // that covers every key of the bucket confines it by nothing.
        if path.confines_keys() {
            statement["Condition"] = json!({"StringLike": {PREFIX_CONDITION_KEY: path.keys}});
        }
        statements.push(statement);
    }
    json!({"Version": POLICY_VERSION, "Statement": statements}).to_string()
}

/// The object-level actions `permission` covers: the read set, the write set, or both —
/// "READWRITE – Grant both read and write access to the S3 data" (`Permission`).
fn object_actions(permission: &str) -> Vec<&'static str> {
    match permission {
        PERMISSION_READ => READ_OBJECT_ACTIONS.to_vec(),
        PERMISSION_WRITE => WRITE_OBJECT_ACTIONS.to_vec(),
        PERMISSION_READWRITE => READ_OBJECT_ACTIONS
            .iter()
            .chain(WRITE_OBJECT_ACTIONS)
            .copied()
            .collect(),
        // A permission outside the modelled enum reaches no handler — `Permission` is an
        // enum member and a value outside it is refused before this runs — and covers
        // nothing rather than everything.
        _ => Vec::new(),
    }
}

/// The bucket-level actions `permission` covers: the read set for a permission that includes
/// reading, and none for a write-only one (see [`READ_BUCKET_ACTIONS`]).
fn bucket_actions(permission: &str) -> Vec<&'static str> {
    match permission {
        PERMISSION_READ | PERMISSION_READWRITE => READ_BUCKET_ACTIONS.to_vec(),
        _ => Vec::new(),
    }
}

/// What one S3 Access Grants scope covers, as a policy names it: the bucket, and the key
/// patterns inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopedPath {
    /// The bucket the scope is in, as a policy's resource names it.
    bucket: String,
    /// The key patterns the scope covers, in IAM's wildcard spelling.
    keys: Vec<String>,
}

impl ScopedPath {
    /// Read `scope` — an S3 URI path — into the bucket and key patterns it covers.
    ///
    /// Total over any text: a value carrying no `s3://` scheme and a scope naming no bucket
    /// at all (the default location `s3://`, which stands for every bucket) both yield a
    /// path, so no caller has to decide what a scope it cannot read covers.
    fn of(scope: &str) -> Self {
        let path = scope.strip_prefix(S3_URI_SCHEME).unwrap_or(scope);
        let (bucket, rest) = match path.split_once('/') {
            Some((bucket, rest)) => (bucket, rest),
            None => (path, ""),
        };
        // The default location names no bucket, and covers every one.
        let bucket = match bucket.is_empty() {
            true => ANY.to_string(),
            false => bucket.to_string(),
        };
        let keys = if rest.is_empty() || rest.ends_with('/') {
            // A bucket, or a prefix boundary: every key beneath it.
            vec![format!("{rest}{ANY}")]
        } else if rest.ends_with(ANY) {
            // A wildcard the scope wrote itself: every key starting with the text before it.
            vec![rest.to_string()]
        } else {
            // One object key, and the keys under it as a prefix boundary.
            vec![rest.to_string(), format!("{rest}/{ANY}")]
        };
        Self { bucket, keys }
    }

    /// The ARNs of the objects this path covers, one per key pattern.
    fn object_arns(&self) -> Vec<String> {
        self.keys
            .iter()
            .map(|key| object_arn(&self.bucket, key))
            .collect()
    }

    /// Whether this path covers only PART of its bucket's key space — which is what a
    /// bucket-level action has to be confined by, there being no key on a bucket's own ARN
    /// to confine it with.
    fn confines_keys(&self) -> bool {
        self.keys != [ANY]
    }
}

/// IAM's wildcard: "any combination of characters" (IAM User Guide, *IAM JSON policy
/// elements: Resource*), which is also how an S3 Access Grants scope writes one.
const ANY: &str = "*";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row is one form an S3 Access Grants scope takes, and what a policy over it
    /// names: the bucket, and the key patterns inside it.
    #[test]
    fn a_scope_names_the_keys_it_covers() {
        let rows = [
            ("a bucket wildcard", "s3://ledger/*", "ledger", vec!["*"]),
            (
                "a prefix wildcard",
                "s3://ledger/granted/*",
                "ledger",
                vec!["granted/*"],
            ),
            (
                "a prefix boundary",
                "s3://ledger/granted/",
                "ledger",
                vec!["granted/*"],
            ),
            ("a bare bucket", "s3://ledger", "ledger", vec!["*"]),
            ("a bucket path", "s3://ledger/", "ledger", vec!["*"]),
            (
                "one object key",
                "s3://ledger/granted/inside.txt",
                "ledger",
                vec!["granted/inside.txt", "granted/inside.txt/*"],
            ),
            ("the default location", "s3://", "*", vec!["*"]),
        ];
        for (case, scope, bucket, keys) in rows {
            let path = ScopedPath::of(scope);
            let expected = ScopedPath {
                bucket: bucket.to_string(),
                keys: keys.iter().map(|key| (*key).to_string()).collect(),
            };
            assert_eq!(
                path, expected,
                "case {case:?}: expected the scope to cover {expected:?}, got {path:?}"
            );
        }
    }

    /// The document is a policy whose statements carry the actions the permission covers:
    /// read-only, write-only, or both, and the bucket-level statement only where reading is
    /// covered.
    #[test]
    fn the_permission_decides_the_actions_the_policy_allows() {
        let rows = [
            (PERMISSION_READ, true, false, true),
            (PERMISSION_WRITE, false, true, false),
            (PERMISSION_READWRITE, true, true, true),
        ];
        for (permission, reads, writes, lists) in rows {
            let document = session_policy("s3://ledger/granted/*", permission);
            for (action, expected) in [
                ("s3:GetObject", reads),
                ("s3:PutObject", writes),
                ("s3:ListBucket", lists),
            ] {
                let carried = document.contains(&format!("\"{action}\""));
                assert_eq!(
                    carried, expected,
                    "case {permission:?}: expected the policy to carry {action} = {expected}, \
                     got {carried} in {document}"
                );
            }
        }
    }
}

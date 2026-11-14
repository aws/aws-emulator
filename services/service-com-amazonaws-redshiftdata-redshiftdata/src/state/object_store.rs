// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 side of `COPY`, `UNLOAD` and Spectrum: the objects a statement
//! reads or writes, and whether the IAM role it names may do it.
//!
//! A load reads objects and an export writes them, both THROUGH Amazon S3's own
//! store on this engine ([`super::data_plane`]) rather than from a copy of the
//! data kept here: an object a caller staged with `PutObject` is what a `COPY`
//! loads, and an object an `UNLOAD` wrote is what `GetObject` then serves.
//!
//! Every refusal this module answers is one Amazon S3 itself would answer about
//! that object — the read is not permitted, there is no such key, there is no
//! such bucket — and each is reported as the cluster reports a failure of its
//! own call to Amazon S3: the `S3ServiceException` line
//! ([`S3Failure::message`]), carrying the status and the error code of that
//! answer. That is what a client sees when its `COPY` names a role whose
//! policies do not allow the read: the failure is S3's, raised inside the
//! cluster's own request, and not a parse error and not a load error.
//!
//! WHETHER the role may read or write is decided where the answer lives: the
//! bucket's own policy, held by the object store, and the role's identity-based
//! policies, held by IAM — evaluated by the tree's ONE evaluation of a policy
//! document ([`::service_common::iam_authorization`]), so this service and
//! every other one decide the same documents alike. Nothing is refused on an
//! answer this emulator could not get: no IAM beside this service, a value that
//! names no role of this account, or a role IAM holds nothing under each leave
//! the question undecided, and the read is made.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::data_plane::{DataPlane, IdentityStore, ObjectStore};

/// How many bytes of Amazon S3 objects ONE statement may move.
///
/// A caller chooses the objects a load reads and the rows an export writes, so
/// what one statement can make this emulator hold is bounded here, as every
/// other thing a request can multiply is. Counted before a body is copied, so a
/// prefix naming more than this is refused rather than buffered and then
/// refused.
pub const MAX_TRANSFER_BYTES: usize = 32 * 1024 * 1024;

/// How many objects ONE statement may read or write, bounded for the same
/// reason [`MAX_TRANSFER_BYTES`] is.
pub const MAX_TRANSFER_OBJECTS: usize = 256;

/// The IAM action a load's read of one object performs: "s3:GetObject" is the
/// permission the role a `COPY` runs under has to carry ("the IAM role must
/// have permission to … s3:GetObject", *Authorization parameters*).
pub const OBJECT_READ_ACTION: &str = "s3:GetObject";

/// The IAM action an export's write of one object performs: "s3:PutObject",
/// which `UNLOAD` documents as one of the permissions its role has to carry.
pub const OBJECT_WRITE_ACTION: &str = "s3:PutObject";

/// The IAM action a read of a bucket's object LIST performs, which a load from a
/// key prefix makes before it reads the objects it found ("s3:ListBucket").
pub const BUCKET_LIST_ACTION: &str = "s3:ListBucket";

/// What a statement's own call into Amazon S3 answered, when it did not answer
/// with the objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3Failure {
    /// The read or the write is not permitted, carrying the message Amazon S3
    /// answers it with — which names the principal the cluster made the call
    /// as, the action it was refused and the resource it named.
    AccessDenied(String),
    /// The address names no object the store holds.
    NoSuchKey,
    /// The address names no bucket the store holds.
    NoSuchBucket,
    /// There is no object store on this engine to call at all, so nothing was
    /// read or written and nothing is claimed. A failure of this emulator, not
    /// of the statement.
    Unreachable,
    /// The objects named are more than one statement may move
    /// ([`MAX_TRANSFER_BYTES`], [`MAX_TRANSFER_OBJECTS`]).
    TooMuch(String),
}

impl S3Failure {
    /// The line the cluster reports for a failure of its own Amazon S3 call.
    ///
    /// The form is the engine's: `S3ServiceException`, the message and the
    /// status Amazon S3 answered with, the error code it named, and the request
    /// ids of that call. The ids are the ones of THIS call, minted per call as
    /// every S3 request id is.
    pub fn message(&self) -> String {
        let (text, status, code) = match self {
            // The message Amazon S3 states for each of these codes, as its own
            // error responses carry it (Amazon S3 API Reference, *Error
            // responses*).
            Self::AccessDenied(refusal) => (refusal.as_str(), 403, "AccessDenied"),
            Self::NoSuchKey => ("The specified key does not exist.", 404, "NoSuchKey"),
            Self::NoSuchBucket => (
                "The specified bucket does not exist",
                404,
                "NoSuchBucket",
            ),
            Self::Unreachable => {
                return "ERROR: the Amazon S3 service this cluster reads and writes objects \
                        through is not reachable from this emulator"
                    .to_string()
            }
            Self::TooMuch(detail) => return format!("ERROR: {detail}"),
        };
        format!(
            "ERROR: S3ServiceException:{text},Status {status},Error {code},Rid {},ExtRid {},\
             CanRetry 1",
            request_id(),
            extended_request_id()
        )
    }
}

/// The request id of one call into the object store, in the form Amazon S3
/// mints one: 16 upper-case hexadecimal digits.
fn request_id() -> String {
    let minted = ::uuid::Uuid::new_v4().simple().to_string();
    minted[..16].to_ascii_uppercase()
}

/// The extended request id of one call, in the form Amazon S3 mints one: a
/// longer opaque token.
fn extended_request_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// One object address: the bucket it lives in and its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectAddress {
    /// The bucket.
    pub bucket: String,
    /// The key, or the key PREFIX for an address that names a set of objects.
    pub key: String,
}

/// The bucket and key an `s3://bucket/key` address names, however the caller
/// cased the scheme. `None` for a value that is not such an address.
pub fn split_uri(uri: &str) -> Option<ObjectAddress> {
    let rest = uri
        .strip_prefix("s3://")
        .or_else(|| uri.strip_prefix("S3://"))?;
    let (bucket, key) = rest.split_once('/')?;
    if bucket.is_empty() {
        return None;
    }
    Some(ObjectAddress {
        bucket: bucket.to_string(),
        key: key.to_string(),
    })
}

/// Read ONE object out of the store, as the engine's own `GetObject` would.
pub fn read_object(
    plane: &DataPlane,
    role_arn: Option<&str>,
    address: &ObjectAddress,
) -> Result<Vec<u8>, S3Failure> {
    let store = plane.objects.as_ref().ok_or(S3Failure::Unreachable)?;
    authorize(plane, role_arn, OBJECT_READ_ACTION, address)?;
    let held = store.lock();
    let bucket = held.bucket(&address.bucket).ok_or(S3Failure::NoSuchBucket)?;
    let object = bucket.object(&address.key).ok_or(S3Failure::NoSuchKey)?;
    if object.body.len() > MAX_TRANSFER_BYTES {
        return Err(too_much(format!(
            "the object s3://{}/{} is larger than the {MAX_TRANSFER_BYTES} bytes one statement \
             of this emulator moves",
            address.bucket, address.key
        )));
    }
    Ok(object.body.clone())
}

/// Read every object whose key starts with `address.key`, in key order, as the
/// engine's own list-and-read would.
///
/// An EMPTY answer is `NoSuchKey`: a prefix the store holds nothing under names
/// no object, which is what the engine's call reports.
pub fn read_objects_under(
    plane: &DataPlane,
    role_arn: Option<&str>,
    address: &ObjectAddress,
) -> Result<Vec<(String, Vec<u8>)>, S3Failure> {
    let store = plane.objects.as_ref().ok_or(S3Failure::Unreachable)?;
    // Listing a bucket is a permission of its own: a role that may not list it
    // cannot find the objects under a prefix at all.
    authorize(
        plane,
        role_arn,
        BUCKET_LIST_ACTION,
        &ObjectAddress {
            bucket: address.bucket.clone(),
            key: String::new(),
        },
    )?;
    let taken = {
        let held = store.lock();
        let bucket = held.bucket(&address.bucket).ok_or(S3Failure::NoSuchBucket)?;
        let mut taken: Vec<(String, Vec<u8>)> = Vec::new();
        let mut bytes = 0_usize;
        // Taken one object and one byte at a time, so a prefix naming more than
        // this statement may move is refused rather than copied whole first.
        for (key, object) in bucket
            .objects()
            .filter(|(key, _)| key.starts_with(address.key.as_str()))
        {
            bytes = bytes.saturating_add(object.body.len());
            if bytes > MAX_TRANSFER_BYTES {
                return Err(too_much(format!(
                    "the objects under s3://{}/{} are larger than the {MAX_TRANSFER_BYTES} bytes \
                     one statement of this emulator moves",
                    address.bucket, address.key
                )));
            }
            if taken.len() >= MAX_TRANSFER_OBJECTS {
                return Err(too_much(format!(
                    "s3://{}/{} names more than the {MAX_TRANSFER_OBJECTS} objects one statement \
                     of this emulator moves",
                    address.bucket, address.key
                )));
            }
            taken.push((key.to_string(), object.body.clone()));
        }
        taken
    };
    if taken.is_empty() {
        return Err(S3Failure::NoSuchKey);
    }
    // And reading each object the listing found is that object's own read, so it
    // is authorized against that object and not against the prefix.
    for (key, _) in &taken {
        authorize(
            plane,
            role_arn,
            OBJECT_READ_ACTION,
            &ObjectAddress {
                bucket: address.bucket.clone(),
                key: key.clone(),
            },
        )?;
    }
    let mut taken = taken;
    taken.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(taken)
}

/// Whether the store holds any object whose key starts with `address.key`.
///
/// What `UNLOAD` asks before it writes: "by default, UNLOAD fails if it finds
/// files that it would possibly overwrite".
pub fn holds_objects_under(plane: &DataPlane, address: &ObjectAddress) -> bool {
    let Some(store) = plane.objects.as_ref() else {
        return false;
    };
    let held = store.lock();
    held.bucket(&address.bucket).is_some_and(|bucket| {
        bucket
            .objects()
            .any(|(key, _)| key.starts_with(address.key.as_str()))
    })
}

/// Write `objects` — each a key and its bytes — into `bucket` through the object
/// store's own write path, so what this writes is what that service then serves.
pub fn write_objects(
    plane: &DataPlane,
    role_arn: Option<&str>,
    bucket: &str,
    objects: &[(String, Vec<u8>)],
) -> Result<(), S3Failure> {
    let store = plane.objects.as_ref().ok_or(S3Failure::Unreachable)?;
    if objects.len() > MAX_TRANSFER_OBJECTS {
        return Err(too_much(format!(
            "this statement would write more than the {MAX_TRANSFER_OBJECTS} objects one \
             statement of this emulator writes"
        )));
    }
    let bytes: usize = objects.iter().map(|(_, body)| body.len()).sum();
    if bytes > MAX_TRANSFER_BYTES {
        return Err(too_much(format!(
            "this statement would write more than the {MAX_TRANSFER_BYTES} bytes one statement \
             of this emulator writes"
        )));
    }
    for (key, _) in objects {
        authorize(
            plane,
            role_arn,
            OBJECT_WRITE_ACTION,
            &ObjectAddress {
                bucket: bucket.to_string(),
                key: key.clone(),
            },
        )?;
    }
    let mut held = store.lock();
    if held.bucket(bucket).is_none() {
        return Err(S3Failure::NoSuchBucket);
    }
    for (key, body) in objects {
        let stored = ::service_com_amazonaws_s3_amazons3::state::Object::stored(
            body.clone(),
            // The objects an export writes are text, which is what the engine
            // writes them as.
            Some(EXPORTED_CONTENT_TYPE.to_string()),
        );
        let written = held.put_object(bucket, key, stored);
        if !matches!(
            written,
            Some(
                ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Created
                    | ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Replaced
            )
        ) {
            // The store refused the write — its own object or byte ceiling — so
            // the export did NOT write what it was asked to, and says so rather
            // than reporting objects nothing holds.
            return Err(too_much(format!(
                "the object store would not take the object {key} this statement writes"
            )));
        }
    }
    Ok(())
}

/// The media type the objects an export writes carry: they are the delimited
/// text an `UNLOAD` renders.
const EXPORTED_CONTENT_TYPE: &str = "text/plain";

/// Report that a statement names more data than one statement of this emulator
/// moves.
fn too_much(detail: String) -> S3Failure {
    S3Failure::TooMuch(detail)
}

/// Whether the role a statement names may perform `action` on the object
/// `address` names, decided against the documents in force.
///
/// `Ok(())` wherever nothing REFUSES it, which includes everything this emulator
/// could not establish: a statement that named no role, no IAM mounted beside
/// this service, a value that is no role ARN of this account, and a role IAM
/// holds nothing under — a role is routinely provisioned out of band, so its
/// absence from this store says nothing about its permissions. A role this
/// store DOES hold is the authority on what it may do, so a call its documents
/// do not allow is refused.
fn authorize(
    plane: &DataPlane,
    role_arn: Option<&str>,
    action: &str,
    address: &ObjectAddress,
) -> Result<(), S3Failure> {
    let Some(role_arn) = role_arn else {
        return Ok(());
    };
    let resource = if address.key.is_empty() {
        ::service_com_amazonaws_s3_amazons3::state::bucket_arn(&address.bucket)
    } else {
        ::service_com_amazonaws_s3_amazons3::state::object_arn(&address.bucket, &address.key)
    };
    let principals = principals(role_arn);
    let context = [(PRINCIPAL_ARN_KEY.to_string(), role_arn.to_string())];
    let principal = assumed_role_session(role_arn);
    let request = iam::Request::new(action, &resource, &principals).with_context(&context);
    // "An explicit deny in any policy overrides any allow": the bucket's own
    // policy is read first, so a bucket that denies this principal refuses the
    // call whatever the role carries.
    if let Some(policy) = bucket_policy(plane.objects.as_ref(), &address.bucket) {
        if iam::resource_decision(&policy, &request) == Decision::Denied {
            return Err(S3Failure::AccessDenied(refusal(
                &principal,
                action,
                &resource,
                Withheld::ResourceDeny,
            )));
        }
    }
    let Some(identities) = plane.identities.as_ref() else {
        return Ok(());
    };
    if let Some(withheld) = role_withholds(identities, role_arn, &request) {
        return Err(S3Failure::AccessDenied(refusal(
            &principal,
            action,
            &resource,
            withheld,
        )));
    }
    Ok(())
}

/// The session name a Redshift cluster assumes a statement's `IAM_ROLE` under.
///
/// A cluster does not read an object as the caller: it ASSUMES the role the
/// statement named and calls Amazon S3 as that role's session, which is the
/// principal S3's own refusal names. The session name is the one Amazon Redshift
/// uses for the role it assumes for a `COPY` or an `UNLOAD`.
const ROLE_SESSION_NAME: &str = "RedshiftIamRoleSession";

/// The principal the cluster makes the call as: the assumed-role session of the
/// role the statement named,
/// `arn:aws:sts::<account>:assumed-role/<role>/<session>`.
fn assumed_role_session(role_arn: &str) -> String {
    let role = ::service_common::iam_role::role_name(role_arn).unwrap_or_default();
    let account = role_arn
        .split(':')
        .nth(4)
        .filter(|account| !account.is_empty())
        .map_or_else(
            || ::engine::emulator_account_id().to_string(),
            str::to_string,
        );
    format!("arn:aws:sts::{account}:assumed-role/{role}/{ROLE_SESSION_NAME}")
}

/// WHY a call was not permitted, which is the clause the refusal ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Withheld {
    /// No identity-based policy allows the action.
    NoIdentityAllow,
    /// An identity-based policy denies it explicitly.
    IdentityDeny,
    /// The resource's own policy denies it explicitly.
    ResourceDeny,
    /// A permissions boundary does not allow it.
    BoundaryWithholds,
}

/// The message Amazon S3 refuses an unauthorized request with: the principal
/// that made it, the action it was refused, the resource it named and the reason
/// the documents in force gave.
///
/// The wording is AWS's own for an authorization failure ("User: … is not
/// authorized to perform: … on resource: … because …"), with the values taken
/// from the request and the documents this call was decided against.
fn refusal(principal: &str, action: &str, resource: &str, withheld: Withheld) -> String {
    let because = match withheld {
        Withheld::NoIdentityAllow => {
            format!("because no identity-based policy allows the {action} action")
        }
        Withheld::IdentityDeny => {
            "with an explicit deny in an identity-based policy".to_string()
        }
        Withheld::ResourceDeny => "with an explicit deny in a resource-based policy".to_string(),
        Withheld::BoundaryWithholds => {
            format!("because no permissions boundary allows the {action} action")
        }
    };
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: \"{resource}\" \
         {because}"
    )
}

/// The IAM global condition key naming the identity whose policies are in
/// force: for a call a service makes under an assumed role, that role's ARN.
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";

/// Every spelling of the role a resource-based policy may name: the role's own
/// ARN, and the account spellings a deny of the account refuses each of its
/// principals under.
fn principals(role_arn: &str) -> Vec<String> {
    let mut principals = vec![role_arn.to_string()];
    principals.extend(iam::account_principals(role_arn));
    principals
}

/// The policy the object store holds on `bucket`, if any.
fn bucket_policy(store: Option<&ObjectStore>, bucket: &str) -> Option<String> {
    let held = store?.lock();
    held.bucket(bucket).and_then(|bucket| bucket.policy.clone())
}

/// Whether the documents IAM holds for `role_arn` WITHHOLD the request, and
/// why.
///
/// The documents in force on the role, decided in IAM's own order: an explicit
/// deny refuses whatever any allow says, an allow grants, and a boundary caps
/// what those documents grant. `None` — withhold nothing — for a role this store
/// holds nothing under, and for an attachment whose document this emulator does
/// not hold, which is a grant nothing here can read.
fn role_withholds(
    identities: &IdentityStore,
    role_arn: &str,
    request: &iam::Request<'_>,
) -> Option<Withheld> {
    let store = identities.lock();
    if !store.roles.values().any(|role| role.arn == role_arn) {
        return None;
    }
    let policies = store.principal_policies(role_arn)?;
    let stated = iam::identity_decision(&policies.documents, request);
    let granted = iam::is_allowed(stated, Decision::NotStated)
        || (stated != Decision::Denied && !policies.uncarried.is_empty());
    // "A permissions boundary sets the maximum permissions that an
    // identity-based policy can grant": a call the ceiling does not allow is
    // refused even where the role's own documents allow it. A call an
    // identity-based policy explicitly denies is already refused.
    let capped = if stated == Decision::Denied {
        Decision::Allowed
    } else {
        store
            .permissions_boundary_document(role_arn)
            .map_or(Decision::Allowed, |document| {
                iam::identity_decision(::std::slice::from_ref(&document), request)
            })
    };
    if granted && capped == Decision::Allowed {
        return None;
    }
    Some(if stated == Decision::Denied {
        Withheld::IdentityDeny
    } else if capped != Decision::Allowed {
        Withheld::BoundaryWithholds
    } else {
        Withheld::NoIdentityAllow
    })
}

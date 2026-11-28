// SPDX-License-Identifier: CC0-1.0

//! WHAT the temporary credentials this service hands a caller for a build upload ARE: a
//! session SCOPED to that one upload, so the credential can write the build's files at the
//! location it was issued for and can do nothing else with it.
//!
//! "This operation creates a new build resource and also returns an Amazon S3 location with
//! temporary access credentials. Use the credentials to manually upload your build files to
//! the specified Amazon S3 location" (`CreateBuild`), and `RequestUploadCredentials`
//! "retrieves a fresh set of credentials for use when uploading a new set of game build
//! files": what the credential is FOR is the upload, and the location it is issued with is
//! the one object it may write. A credential that could also read that location back would
//! hand the caller access to the service's own build bucket, which is not what either
//! operation vends.
//!
//! The narrowing is a SESSION POLICY, the one mechanism AWS caps a vended session's
//! permissions with: "the resulting session's permissions are the intersection of the role's
//! identity-based policies and the session policies" (`AssumeRoleRequest$Policy`, the STS
//! model). It is recorded on the session the token service holds, because the service that
//! MINTS a session is not the service that serves the calls made with it: Amazon S3's own
//! authorizer reads the scope off the session the presented access key id names, so a call
//! the upload scope does not allow — a read of the very key just written — is refused THERE,
//! by S3, in the sentence IAM words for a request no session policy allows, while the upload
//! itself is served.
//!
//! The session stands on the account's own published credential, the identity every
//! credential this emulator did not mint for an IAM identity resolves to: that identity
//! carries no identity-based document, so what a request signed with the vended credential
//! may do is exactly what the scope above allows
//! (`sts_store::identity_is_account_credential`). Minting a bare credential instead — one
//! the token service holds no session for — hands out exactly that unbounded identity with
//! no scope at all, which is the opposite of what an upload credential is for.
//!
//! LOCK ORDER: every function here reaches the token service's store, and holds no guard of
//! THIS service's store while it does, so it must be called with this service's guard given
//! up — the same discipline as this service's other reaches into another store
//! ([`crate::state::provision_build_upload_bucket`]).

use ::std::time::SystemTime;

use ::emulator_server::CredentialStore;
use ::serde_json::json;
use ::service_com_amazonaws_s3_amazons3::state::object_arn;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

use crate::ops::types::S3Location;

/// The policy language version the document below declares, which is the current one IAM
/// publishes ("2012-10-17 … the current version of the policy language", IAM User Guide,
/// *IAM JSON policy elements: Version*).
const POLICY_VERSION: &str = "2012-10-17";

/// The actions an upload of ONE object needs, and the only ones an upload credential is
/// scoped to.
///
/// Writing the object is `s3:PutObject`; an upload large enough to be sent in parts also
/// needs the two actions S3 documents for the multipart form of the same write — "you must
/// have the `s3:PutObject` permission … to abort a multipart upload, you must have the
/// `s3:AbortMultipartUpload` permission … you must have the `s3:ListMultipartUploadParts`
/// permission to list the parts" (Amazon S3 User Guide, *Multipart upload and permissions*).
/// Reading the object back is NOT among them: a credential vended for an upload uploads.
pub const UPLOAD_OBJECT_ACTIONS: &[&str] = &[
    "s3:PutObject",
    "s3:AbortMultipartUpload",
    "s3:ListMultipartUploadParts",
];

/// One vended upload credential: the three values a caller signs the upload with.
///
/// The material comes from the emulator's credential store, the only thing that creates
/// credentials, so every value here is one a later signed request verifies against.
#[derive(Clone)]
pub struct VendedUploadCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
}

/// The two secrets of a vended credential are written nowhere but the response that hands
/// them to the caller, so this reports the access key id — which names the session a later
/// request is judged as — and says only that the rest are held.
impl ::std::fmt::Debug for VendedUploadCredentials {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("VendedUploadCredentials")
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

/// Vend a credential scoped to the upload of `location`, for the resource `resource_id`
/// holds the files of.
///
/// `None` when no such session can be recorded at all — an engine carrying no token-service
/// store, a bounded session registry with no room left, or a location naming no bucket and
/// key to scope the session to. The caller then vends NOTHING: a credential this emulator
/// cannot scope is a credential that would act as the whole account.
///
/// Call it with this service's guard given up (see the module docs).
pub fn vend<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    now: SystemTime,
    resource_id: &str,
    location: &S3Location,
) -> Option<VendedUploadCredentials> {
    let (bucket, key) = (location.bucket.as_deref()?, location.key.as_deref()?);
    let account = ::engine::emulator_account_id().to_string();
    // The principal the credential speaks for, which is what `sts:GetCallerIdentity` reports
    // for it and what a refusal of a request it signs names: a session of the account that
    // owns the resource, named by the resource whose upload it is for. It is no IAM identity
    // of the account and carries no document of its own, which is why the scope below is the
    // whole of what it may do.
    let session_arn = sts_store::sts_arn(&account, &format!("federated-user/{resource_id}"));
    let identity = sts_store::Identity {
        user_id: format!("{account}:{resource_id}"),
        account,
        arn: session_arn,
    };
    let registry = sts_store::service_state(engine)?;
    let minted = {
        let mut held = registry.lock();
        sts_store::mint_session(
            engine,
            store,
            &mut held,
            now,
            sts_store::NewSession {
                // The upload-only cap, which is the whole point of the vend.
                policies: sts_store::SessionPolicies {
                    inline: Some(session_policy(bucket, key)),
                    managed_arns: Vec::new(),
                    // The credential's permissions come from the scope and from nothing
                    // else: the identity it stands on bounds it with no document, so an
                    // empty scope would have to grant nothing rather than withhold nothing
                    // ([`sts_store::SessionPolicies::none_means_no_permissions`]).
                    none_means_no_permissions: true,
                    ..sts_store::SessionPolicies::default()
                },
                // The identity whose own policies the session's permissions are the
                // intersection with: the account's own published credential, which this
                // service vends the upload on the authority of.
                federating_user_arn: Some(sts_store::ambient_identity().arn),
                ..sts_store::NewSession::new(
                    identity,
                    sts_store::SessionKind::FederationToken,
                    // "These credentials have a limited lifespan"
                    // (`RequestUploadCredentialsOutput$UploadCredentials`), whose length
                    // Amazon GameLift Servers publishes nowhere, so the session lasts as
                    // long as a vended session of this kind does by default.
                    sts_store::VENDED_SESSION_DEFAULT_DURATION_SECONDS,
                )
            },
        )
        .ok()?
    };
    // Which CLASS of credential this vend minted, for the service that decides a request
    // signed with it: a federated-user credential, which may not call IAM at all
    // (`sts_store::publish_minted_credential`). The session registry's guard is given up
    // first, so no caller holds the IAM lock and the session lock at once.
    sts_store::publish_minted_credential(sts_store::iam_state(engine).as_ref(), &minted, now);
    Some(VendedUploadCredentials {
        access_key_id: minted.credentials.access_key_id,
        secret_access_key: minted.credentials.secret_access_key,
        session_token: minted.credentials.session_token,
    })
}

/// Give up an upload credential this module vended, and the session it belongs to: the set a
/// fresher one replaces, and one a call decided after minting that it cannot honour.
///
/// Both halves go together, so an access key id this emulator no longer honours neither
/// verifies nor resolves a principal. Call it with this service's guard given up (see the
/// module docs).
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
    // The account's audit view of its sessions names only sessions a caller holds, so a
    // credential that is withdrawn leaves no record of a session nobody can sign with.
    sts_store::forget_audited_session(engine, access_key_id);
}

/// The session policy one vend caps its credential with: an `Allow` of exactly the upload
/// actions ([`UPLOAD_OBJECT_ACTIONS`]) on exactly the one object the upload writes, and
/// nothing else.
///
/// A session policy GRANTS nothing — it caps what the identity behind the credential may do
/// — so the document needs no `Deny`: an action or a resource it says nothing about is
/// withheld, which is what refuses a read of the uploaded object while leaving the upload
/// itself served.
#[must_use]
pub fn session_policy(bucket: &str, key: &str) -> String {
    json!({
        "Version": POLICY_VERSION,
        "Statement": [{
            "Effect": "Allow",
            "Action": UPLOAD_OBJECT_ACTIONS,
            "Resource": [object_arn(bucket, key)],
        }],
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document names the upload actions on the one object it was written for, and
    /// nothing else: the read of that same object is not among them.
    #[test]
    fn the_scope_allows_the_upload_actions_on_the_one_object() {
        let document = session_policy("prod-gamescale-builds-us-west-2", "000000000000/build-1");
        for action in UPLOAD_OBJECT_ACTIONS {
            assert!(
                document.contains(&format!("\"{action}\"")),
                "case {action:?}: expected the scope to allow the action, got {document}"
            );
        }
        for withheld in ["s3:GetObject", "s3:ListBucket", "s3:DeleteObject"] {
            assert!(
                !document.contains(&format!("\"{withheld}\"")),
                "case {withheld:?}: expected the scope to withhold the action, got {document}"
            );
        }
        assert!(
            document.contains("\"arn:aws:s3:::prod-gamescale-builds-us-west-2/000000000000/build-1\""),
            "case the resource: expected the one object the upload writes, got {document}"
        );
    }
}

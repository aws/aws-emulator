// SPDX-License-Identifier: CC0-1.0

//! Grants on a KMS key: the record `CreateGrant` writes, what a grant permits, and the
//! identifiers, tokens and refusal wordings the grant surface shares.
//!
//! One concern of [`super`]'s store, in its own file: the records and helpers here are
//! reached as `crate::state::…` exactly as they were before, because the parent
//! re-exports them.

use super::*;
use crate::ops::types::{GrantConstraints, GrantListEntry};

/// The largest number of grants this emulator's store holds for ONE KMS key.
///
/// A bound the EMULATOR imposes, not the quota KMS publishes: `CreateGrant` is
/// reachable by any caller and a grant without a `Name` is a new record every time,
/// so without a cap a loop of valid creates grows the process without end. Past it
/// `CreateGrant` refuses with the modelled `LimitExceededException` — "a length
/// constraint or quota was exceeded" — and persists nothing.
pub const MAX_GRANTS_PER_KEY: usize = 500;

/// The order a grant's permitted operations are reported in: the order the model
/// DECLARES the `GrantOperation` enum's values in
/// (`smithy_query(query="shape",shape="GrantOperation")`, whose `values` are listed in
/// declaration order).
///
/// `GrantOperationList` is a plain list of that enum, so the model fixes no order for
/// it, and the recorded AWS wire reports a grant's operations in this one whatever
/// order the creating request listed them in. Projecting through it is what makes two
/// readings of one grant, and two grants permitting the same set, render identically.
pub const GRANT_OPERATION_ORDER: &[&str] = &[
    "Decrypt",
    "Encrypt",
    "GenerateDataKey",
    "GenerateDataKeyWithoutPlaintext",
    "ReEncryptFrom",
    "ReEncryptTo",
    "Sign",
    "Verify",
    "GetPublicKey",
    "CreateGrant",
    "RetireGrant",
    "DescribeKey",
    "GenerateDataKeyPair",
    "GenerateDataKeyPairWithoutPlaintext",
    "GenerateMac",
    "VerifyMac",
    "DeriveSharedSecret",
    "Encapsulate",
    "Decapsulate",
];

/// Where `operation` comes in [`GRANT_OPERATION_ORDER`] — past the end for a value the
/// enum does not declare, which the modelled enum constraint refuses before a handler
/// runs, so such a value keeps the place the request gave it rather than being dropped.
pub fn grant_operation_rank(operation: &str) -> usize {
    GRANT_OPERATION_ORDER
        .iter()
        .position(|declared| *declared == operation)
        .unwrap_or(GRANT_OPERATION_ORDER.len())
}

/// One grant on a KMS key: "a policy instrument that allows Amazon Web Services
/// principals to use KMS keys in cryptographic operations"
/// (`smithy_query(query="doc",shape="CreateGrant")`).
#[derive(Debug, Clone)]
pub struct Grant {
    /// The minted grant id, which is also this record's map key.
    pub grant_id: String,
    /// The key id of the KMS key the grant applies to. A grant records the key it
    /// RESOLVED to, never the alias the request named, so repointing an alias
    /// cannot move an existing grant to another key.
    pub key_id: String,
    /// That key's ARN, which is what a grant reader reports as the entry's `KeyId`:
    /// `KeyIdType` carries either form, and KMS reports the grant's key as the ARN.
    pub key_arn: String,
    /// The Region whose endpoint the grant was created through. A KMS key is a
    /// regional resource, so its grants are too.
    pub region: String,
    /// "The Amazon Web Services account under which the grant was issued"
    /// (`smithy_query(shape="GrantListEntry")` -> `IssuingAccount`).
    pub issuing_account: String,
    /// The `Name` the request gave, if any: "If a name was provided in the
    /// CreateGrant request, that name is returned. Otherwise this value is null"
    /// (`smithy_query(shape="GrantListEntry")` -> `Name`).
    pub name: Option<String>,
    pub creation_date: SystemTime,
    pub grantee_principal: Option<String>,
    pub grantee_service_principal: Option<String>,
    pub retiring_principal: Option<String>,
    pub retiring_service_principal: Option<String>,
    /// "The list of operations permitted by the grant", in the order the request
    /// listed them.
    pub operations: Vec<String>,
    pub constraints: Option<GrantConstraints>,
}

impl Grant {
    /// This grant's `GrantListEntry`, computed from the record — the one place a
    /// grant is projected onto the wire, so two readers cannot disagree.
    ///
    /// `Name` is ALWAYS reported: the name the grant was created with, or the empty
    /// string for a grant created without one, so a client reading it never has to
    /// tell a missing member from a grant that has no name. The member's own
    /// documentation says the value "is null" for a grant created without a name
    /// (`smithy_query(query="doc",shape="GrantListEntry",member="Name")`), but the
    /// recorded AWS wire (`simparity-wire-aws-v1`, the `kms-grant-*` fixtures)
    /// reports the empty string, so that is what a reader answers.
    ///
    /// `GranteePrincipal` is the grant's grantee IDENTITY rather than the member the
    /// creating request happened to name it in: "if the grantee principal is an Amazon Web
    /// Services service, the GranteePrincipal field contains an Amazon Web Services service
    /// principal"
    /// (`smithy_query(query="doc",shape="GrantListEntry",member="GranteePrincipal")`), so a
    /// grant created for a service reports that service principal there — and under
    /// `GranteeServicePrincipal` as well, which is what says the grantee IS a service. A
    /// client reading who holds a key's grants therefore reads one member for all of them
    /// ([`Self::grantee_identity`]).
    ///
    /// The permitted operations are reported in the order the `GrantOperation` enum
    /// declares its values ([`GRANT_OPERATION_ORDER`]) rather than in the order the
    /// creating request listed them: they are a SET of permissions (which is also how
    /// a retry of `CreateGrant` compares them, [`Self::fingerprint`]), so the
    /// projection puts them in one canonical order and a client comparing two readings
    /// of the same grant sees no difference.
    pub fn list_entry(&self) -> GrantListEntry {
        let mut operations = self.operations.clone();
        operations.sort_by_key(|operation| grant_operation_rank(operation));
        GrantListEntry {
            key_id: Some(self.key_arn.clone()),
            grant_id: Some(self.grant_id.clone()),
            name: Some(self.name.clone().unwrap_or_default()),
            creation_date: Some(date_time(self.creation_date)),
            grantee_principal: self.grantee_identity().map(str::to_string),
            retiring_principal: self.retiring_principal.clone(),
            issuing_account: Some(self.issuing_account.clone()),
            operations: Some(operations),
            constraints: self.constraints.clone(),
            grantee_service_principal: self.grantee_service_principal.clone(),
            retiring_service_principal: self.retiring_service_principal.clone(),
        }
    }

    /// The identity that got the permissions in this grant: "the identity that gets the
    /// permissions in the grant"
    /// (`smithy_query(query="doc",shape="GrantListEntry",member="GranteePrincipal")`) — the
    /// principal the creating request named, or the SERVICE principal for a grant created
    /// for a service, since that member's documentation says the field carries a service
    /// principal when the grantee is a service.
    ///
    /// The ONE reading of a grant's grantee, so what a reader REPORTS as the grantee and
    /// what it FILTERS on cannot disagree: a client that filters by the grantee it just
    /// read is answered that grant.
    pub fn grantee_identity(&self) -> Option<&str> {
        self.grantee_principal
            .as_deref()
            .or(self.grantee_service_principal.as_deref())
    }

    /// Everything about this grant that came from the request that created it,
    /// digested into 64 hex characters.
    ///
    /// "When this value is present, you can retry a `CreateGrant` request with
    /// identical parameters; if the grant already exists, the original `GrantId` is
    /// returned without creating a new grant"
    /// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Name")`), so a
    /// retry has to be recognisable from the request alone — this is what "identical
    /// parameters" is compared as.
    ///
    /// The permitted operations are sorted and de-duplicated first: they are a SET
    /// of permissions, so a request listing the same ones in another order is the
    /// same request. The encryption-context constraints are maps, and are
    /// canonicalized the same way for the same reason. Every part is
    /// length-prefixed, so no two different requests can digest to the same bytes.
    /// The grant id, the creation date and the grant tokens are NOT part of it: the
    /// first two are what a retry is allowed to reuse, and the last is "unique with
    /// every `CreateGrant` request".
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"kms-grant\0");
        for part in [
            self.key_id.as_str(),
            self.name.as_deref().unwrap_or_default(),
            self.grantee_principal.as_deref().unwrap_or_default(),
            self.grantee_service_principal.as_deref().unwrap_or_default(),
            self.retiring_principal.as_deref().unwrap_or_default(),
            self.retiring_service_principal.as_deref().unwrap_or_default(),
        ] {
            feed(&mut hasher, part);
        }

        let mut operations: Vec<&str> = self.operations.iter().map(String::as_str).collect();
        operations.sort_unstable();
        operations.dedup();
        for operation in operations {
            feed(&mut hasher, operation);
        }

        let constraints = self.constraints.as_ref();
        feed(
            &mut hasher,
            constraints
                .and_then(|constraints| constraints.source_arn.as_deref())
                .unwrap_or_default(),
        );
        feed_context(
            &mut hasher,
            "EncryptionContextEquals",
            constraints.and_then(|constraints| constraints.encryption_context_equals.as_ref()),
        );
        feed_context(
            &mut hasher,
            "EncryptionContextSubset",
            constraints.and_then(|constraints| constraints.encryption_context_subset.as_ref()),
        );

        hex::encode(hasher.finalize())
    }

    /// The `GrantId` a NAMED grant has: DERIVED from the grant's parameters rather than
    /// minted, so one named grant has one id however many times it is created.
    ///
    /// "When this value is present, you can retry a `CreateGrant` request with identical
    /// parameters; if the grant already exists, the original `GrantId` is returned
    /// without creating a new grant"
    /// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Name")`): a named
    /// grant's identity is its parameters, so nothing about its id may come from the CALL
    /// that created it — which is what makes recreating a named grant that was revoked
    /// answer the id that grant had, and what lets a client key its bookkeeping on the
    /// id it got back. A grant with no `Name` has no such identity and its id is minted
    /// ([`mint_grant_id`]), because "when this value is absent, all CreateGrant requests
    /// result in a new grant with a unique GrantId even if all the supplied parameters
    /// are identical".
    ///
    /// It is the 64 lowercase hex characters a grant id is ([`names_a_grant_id`]),
    /// digested from exactly the parameters a retry is recognised by
    /// ([`Self::fingerprint`]) — so two requests that are the same named grant derive the
    /// same id and two that are not cannot — together with the Region the grant belongs
    /// to, since a grant lives at one Region's endpoint and the fingerprint names only
    /// the key.
    pub fn derived_grant_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"kms-grant-id\0");
        feed(&mut hasher, &self.region);
        feed(&mut hasher, &self.fingerprint());
        hex::encode(hasher.finalize())
    }

    /// Whether this grant permits `operation` on the key `key_id` under the request's
    /// encryption context `context`.
    ///
    /// A grant covers ONE key, so a grant on another key permits nothing here: "a
    /// grant on one KMS key does not authorize use of another". It covers exactly the
    /// operations it lists. And its encryption-context constraints "allow the
    /// permissions in the grant only when the encryption context in the request
    /// matches (EncryptionContextEquals) or includes (EncryptionContextSubset) the
    /// encryption context specified in the constraint"
    /// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Constraints")`),
    /// and they "apply only to cryptographic operations that support an encryption
    /// context" — so an operation that takes none is judged without them.
    ///
    /// A `SourceArn` constraint restricts "an Amazon Web Services resource on behalf
    /// of which the request is made"; nothing in a direct request names such a
    /// resource, so it narrows nothing here.
    pub fn permits(
        &self,
        key_id: &str,
        operation: &str,
        context: Option<&HashMap<String, String>>,
    ) -> bool {
        if self.key_id != key_id {
            return false;
        }
        if !self
            .operations
            .iter()
            .any(|permitted| permitted == operation)
        {
            return false;
        }
        let takes_context = GRANT_OPERATION_PROFILES
            .iter()
            .find(|profile| profile.operation == operation)
            .is_some_and(|profile| profile.encryption_context);
        if !takes_context {
            return true;
        }
        self.constraints
            .as_ref()
            .is_none_or(|constraints| encryption_context_allows(constraints, context))
    }
}

/// Why a request's grant tokens do not authorize it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantRefusal {
    /// A token is not one this service minted, so it names no grant at all — which is
    /// what makes the GRANT TOKEN the thing that "is not valid".
    MalformedToken,
    /// The tokens are well formed, but no grant they name authorizes the request: the
    /// grant is gone, is on another key, does not permit the operation, or its
    /// encryption-context constraint is not met. Permission that would have come from
    /// a grant is absent, which is an authorization failure rather than a bad request.
    NotAuthorized,
}

/// Whether a grant's encryption-context constraints allow a request carrying
/// `context`.
///
/// "In a grant constraint, the key in each key-value pair is not case sensitive, but
/// the value is case sensitive"
/// (`smithy_query(query="doc",shape="GrantConstraints")`), so the keys are folded and
/// the values are not. An absent context is an empty one: `EncryptionContextEquals`
/// then allows only an empty constraint, and `EncryptionContextSubset` only one with
/// no pairs to include.
fn encryption_context_allows(
    constraints: &GrantConstraints,
    context: Option<&HashMap<String, String>>,
) -> bool {
    let empty = HashMap::new();
    let request = folded_keys(context.unwrap_or(&empty));

    if let Some(equals) = constraints.encryption_context_equals.as_ref() {
        if folded_keys(equals) != request {
            return false;
        }
    }
    if let Some(subset) = constraints.encryption_context_subset.as_ref() {
        if !folded_keys(subset)
            .into_iter()
            .all(|(key, value)| request.get(&key) == Some(&value))
        {
            return false;
        }
    }
    true
}

/// An encryption context with its keys folded to lower case, so two contexts that
/// differ only in the case of their KEYS compare equal while their values stay
/// case-sensitive.
fn folded_keys(context: &HashMap<String, String>) -> BTreeMap<String, String> {
    context
        .iter()
        .map(|(key, value)| (key.to_lowercase(), value.clone()))
        .collect()
}

/// Add one length-prefixed part to a digest, so two different sequences of parts
/// cannot run together into the same bytes.
fn feed(hasher: &mut Sha256, part: &str) {
    let len = u32::try_from(part.len()).unwrap_or(u32::MAX);
    hasher.update(len.to_be_bytes());
    hasher.update(part.as_bytes());
}

/// [`feed`] for an encryption-context map, under `label` so the two constraint maps
/// cannot be swapped without changing the digest. The entries are sorted first: the
/// map is unordered, so only its content may decide the digest.
fn feed_context(hasher: &mut Sha256, label: &str, context: Option<&HashMap<String, String>>) {
    feed(hasher, label);
    let mut entries: Vec<(&str, &str)> = context
        .map(|context| {
            context
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_unstable();
    for (key, value) in entries {
        feed(hasher, key);
        feed(hasher, value);
    }
}

/// The account ARN KMS reports as a grant's `IssuingAccount`: the emulator's own
/// account, which is the account that owns every key here.
pub fn issuing_account_arn() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// How KMS words `DryRunOperationException`, "the request was rejected because the
/// DryRun parameter was specified"
/// (`smithy_query(query="doc",shape="DryRunOperationException")`).
///
/// One constant for the whole service, so no two operations word the same refusal
/// differently.
pub const DRY_RUN_MESSAGE: &str =
    "The request would have succeeded, but the DryRun option is set.";

/// How a `GrantId` that cannot name any grant is refused: `InvalidGrantIdException`'s
/// message names the value the request gave, since that string is the whole of what
/// is wrong with it.
///
/// One function for the whole service, so no two operations word the same malformed
/// grant id differently.
pub fn invalid_grant_id_message(requested: &str) -> String {
    format!("The specified GrantId is not valid: {requested}.")
}

/// How a grant that is not held is refused: the grant id the request gave, since a
/// grant that was never created, one already retired and one already revoked are the
/// same absence.
///
/// One function for the whole service, so no two operations word it differently.
pub fn grant_not_found_message(grant_id: &str) -> String {
    format!("Grant ID {grant_id} not found")
}

/// How a `RetireGrant` is refused when the grant it identified is not held.
///
/// KMS will not confirm whether a grant exists to a caller that cannot retire it:
/// "permission to retire a grant is determined primarily by the grant"
/// (`smithy_query(query="doc",shape="RetireGrant")`), so with no such grant there is no
/// permission to retire one — and the refusal is the authorization failure that reports
/// nothing about the resource, naming the caller and the action alone
/// ([`unauthorized_resource_message`]).
pub fn retire_grant_access_denied_message(principal: &str, operation: &str) -> String {
    unauthorized_resource_message(principal, operation)
}

/// A freshly minted grant id: the 64 lowercase hex characters KMS gives a grant,
/// drawn from the OS CSPRNG so two grants never share one.
///
/// `None` means the platform refused to produce entropy; the caller answers a
/// modelled server error rather than minting a predictable id.
pub fn mint_grant_id() -> Option<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(hex::encode(bytes))
}

/// How many characters a grant id has: [`mint_grant_id`] draws 32 bytes, and hex
/// doubles them.
const GRANT_ID_HEX_LEN: usize = 64;

/// Whether `grant_id` has the form a grant id takes.
///
/// `GrantIdType`'s modelled 1..128 length accepts far more than a grant id can be,
/// so the form is what identifies a value that cannot name any grant — as opposed to
/// a well-formed id that simply names none.
pub fn names_a_grant_id(grant_id: &str) -> bool {
    grant_id.len() == GRANT_ID_HEX_LEN
        && grant_id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Tags a grant token as one this service minted, and distinguishes it from the
/// pagination tokens, which share the encoding.
const GRANT_TOKEN_PREFIX: &str = "grant-token/1/";

/// A freshly minted grant token for `grant_id`.
///
/// "The returned grant token is unique with every `CreateGrant` request, even when a
/// duplicate `GrantId` is returned. All grant tokens for the same grant ID can be
/// used interchangeably"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Name")`): the token
/// therefore carries the grant id it belongs to plus a fresh nonce, so a new one is
/// minted per call and any of them names the same grant. It is self-describing
/// rather than persisted, so a grant does not accumulate a token per retry.
///
/// `None` means the platform refused to produce entropy.
pub fn mint_grant_token(grant_id: &str) -> Option<String> {
    let mut nonce = [0u8; 16];
    SystemRandom::new().fill(&mut nonce).ok()?;
    Some(hex::encode(format!(
        "{GRANT_TOKEN_PREFIX}{grant_id}/{}",
        hex::encode(nonce)
    )))
}

/// The grant id a grant token names, or `None` when the token is not one this
/// service minted — which is what makes a foreign token an
/// `InvalidGrantTokenException` instead of being silently ignored.
pub fn decode_grant_token(token: &str) -> Option<String> {
    let decoded = hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let rest = text.strip_prefix(GRANT_TOKEN_PREFIX)?;
    let (grant_id, nonce) = rest.split_once('/')?;
    (!grant_id.is_empty() && !nonce.is_empty()).then(|| grant_id.to_string())
}

/// Whether `principal` is given the way KMS requires a grant principal to be: "use
/// the Amazon Resource Name (ARN) of an Amazon Web Services principal"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="GranteePrincipal")`),
/// and the same for the retiring principal `ListRetirableGrants` and `CreateGrant` take:
/// "valid principals include Amazon Web Services accounts, IAM users, IAM roles,
/// federated users, and assumed role users"
/// (`smithy_query(query="doc",shape="ListRetirableGrantsRequest",member="RetiringPrincipal")`).
///
/// `PrincipalIdType`'s modelled pattern accepts any word-ish string, so the IDENTITY form
/// is what is left to check — and it is the same union of forms every other service reads
/// a policy principal against, so it is the tree's one reading of it
/// ([`::service_common::arn::is_iam_principal_arn`]) rather than a second grammar here: an
/// ARN whose resource field names no IAM or STS identity (no resource type at all, or one
/// that is not an identity's) denotes no principal, which is what the operations model
/// `InvalidArnException` for.
pub fn names_a_principal_arn(principal: &str) -> bool {
    ::service_common::arn::is_iam_principal_arn(principal)
}

/// How a value that names no AWS principal is refused: `InvalidArnException`'s message
/// names the value the request gave, since that string is the whole of what is wrong with
/// it.
///
/// KMS words this refusal of a PRINCIPAL differently from the one it words for a key
/// identifier ([`invalid_arn_message`]) — the recorded wording names the ARN with no
/// sentence around it — so the two live side by side here rather than one being used for
/// both. One function for every member that carries a principal, so no two operations
/// word the same refusal differently.
pub fn invalid_principal_arn_message(requested: &str) -> String {
    format!("ARN is not valid: {requested}")
}

/// What one `GrantOperation` needs of the KMS key it is granted on.
///
/// "This list must include only operations that are permitted in a grant. Also, the
/// operation must be supported on the KMS key. For example, you cannot create a
/// grant for a symmetric encryption KMS key that allows the `Sign` operation, or a
/// grant for an asymmetric KMS key that allows the `GenerateDataKey` operation"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Operations")`) —
/// the first example is a `KeyUsage` mismatch and the second a key-TYPE mismatch, so
/// a profile records both.
#[derive(Debug, Clone, Copy)]
pub struct GrantOperationProfile {
    /// The `GrantOperation` value this profile describes
    /// (`crate::ops::types::GRANT_OPERATION_VALUES`).
    pub operation: &'static str,
    /// The `KeyUsage` values a key must carry for this operation. EMPTY means the
    /// operation is key management, which every usage permits.
    pub usages: &'static [&'static str],
    /// The key types ([`KeySpecProfile::family`]) that support this operation. EMPTY
    /// means every type does.
    pub families: &'static [&'static str],
    /// Whether the operation this grants takes an `EncryptionContext`. The
    /// encryption-context grant constraints "apply only to cryptographic operations
    /// that support an encryption context … not … operations that do not support an
    /// encryption context, such as cryptographic operations with asymmetric KMS keys
    /// and management operations"
    /// (`smithy_query(query="doc",shape="GrantConstraints")`), so this is what decides
    /// whether a constraint is applied at all.
    pub encryption_context: bool,
}

/// What each `GrantOperation` needs of the key, from the `KeyUsage` matrix
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`) and the
/// operation each one grants: a cryptographic operation needs the usage that
/// operation runs under, and the key type whose material can perform it, while
/// `CreateGrant`, `DescribeKey` and `RetireGrant` are management operations that
/// every key supports.
pub const GRANT_OPERATION_PROFILES: &[GrantOperationProfile] = &[
    GrantOperationProfile {
        operation: "CreateGrant",
        usages: &[],
        families: &[],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "DescribeKey",
        usages: &[],
        families: &[],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "RetireGrant",
        usages: &[],
        families: &[],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "Encrypt",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "Decrypt",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "ReEncryptFrom",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "ReEncryptTo",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    // A data key is generated under a SYMMETRIC encryption key, which is what makes
    // `GenerateDataKey` on an asymmetric key the documented example of an operation
    // the key does not support.
    GrantOperationProfile {
        operation: "GenerateDataKey",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "GenerateDataKeyWithoutPlaintext",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "GenerateDataKeyPair",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "GenerateDataKeyPairWithoutPlaintext",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "Sign",
        usages: &[KEY_USAGE_SIGN_VERIFY],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "Verify",
        usages: &[KEY_USAGE_SIGN_VERIFY],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "GenerateMac",
        usages: &[KEY_USAGE_GENERATE_VERIFY_MAC],
        families: &[FAMILY_HMAC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "VerifyMac",
        usages: &[KEY_USAGE_GENERATE_VERIFY_MAC],
        families: &[FAMILY_HMAC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "DeriveSharedSecret",
        usages: &[KEY_USAGE_KEY_AGREEMENT],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    // Both directions of the key-encapsulation mechanism run under the one usage an
    // ML-KEM key pair carries, and neither takes an `EncryptionContext`.
    GrantOperationProfile {
        operation: "Encapsulate",
        usages: &[KEY_USAGE_ENCAPSULATE_DECAPSULATE],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "Decapsulate",
        usages: &[KEY_USAGE_ENCAPSULATE_DECAPSULATE],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    // The public half exists only for a key pair, under any of the usages an
    // asymmetric spec can carry.
    GrantOperationProfile {
        operation: "GetPublicKey",
        usages: &[
            KEY_USAGE_ENCRYPT_DECRYPT,
            KEY_USAGE_SIGN_VERIFY,
            KEY_USAGE_KEY_AGREEMENT,
            KEY_USAGE_ENCAPSULATE_DECAPSULATE,
        ],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
];

/// The key type [`KeySpecProfile::family`] reports for a symmetric encryption key.
pub const FAMILY_SYMMETRIC: &str = "symmetric";
/// See [`FAMILY_SYMMETRIC`].
pub const FAMILY_ASYMMETRIC: &str = "asymmetric";
/// See [`FAMILY_SYMMETRIC`].
pub const FAMILY_HMAC: &str = "HMAC";

/// Why `key` does not support the grant operation `operation`, or `None` when it
/// does.
///
/// An operation [`GRANT_OPERATION_PROFILES`] does not describe is reported as
/// supported: the modelled `GrantOperation` enum refuses an unknown value before any
/// handler runs, so reaching here with one is not possible, and a value added to the
/// model later must not be refused by a table that has not caught up.
pub fn grant_operation_refusal(key: &Cmk, operation: &str) -> Option<String> {
    let profile = GRANT_OPERATION_PROFILES
        .iter()
        .find(|profile| profile.operation == operation)?;

    if !profile.usages.is_empty() && !profile.usages.contains(&key.key_usage.as_str()) {
        return Some(format!(
            "Operation {operation} is not permitted in a grant on {}, whose KeyUsage is \
             {}; it requires a KeyUsage of {}.",
            key.arn,
            key.key_usage,
            profile.usages.join(" or ")
        ));
    }

    let family = key_spec_profile(&key.key_spec).map(KeySpecProfile::family);
    if !profile.families.is_empty()
        && family.is_none_or(|family| !profile.families.contains(&family))
    {
        return Some(format!(
            "Operation {operation} is not permitted in a grant on {}, whose KeySpec {} is \
             {}; it requires {} key material.",
            key.arn,
            key.key_spec,
            family.unwrap_or("of an unknown type"),
            profile.families.join(" or ")
        ));
    }

    None
}

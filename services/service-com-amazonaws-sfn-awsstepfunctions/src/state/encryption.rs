// SPDX-License-Identifier: CC0-1.0

//! Whether a state machine's customer managed KMS key can be used for this call —
//! which is what reading data that key encrypts, and sealing data under it, needs.
//!
//! Two things decide it, in the order KMS itself decides them for a cryptographic
//! operation: the key's own KEY STATE, and then whether the authorities in force let
//! the CALLER have the key used on its behalf. Each answers the error class the
//! operations model for it ([`KeyRefusal`]).
//!
//! A state machine on `encryptionConfiguration` of
//! [`ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY`](super::ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY)
//! has its execution data and its definition encrypted under the key the account
//! named. Step Functions does not hold that key: it asks KMS to use it as the CALLER,
//! so a caller the key does not permit cannot have the data decrypted — or encrypted
//! — for it, and the call is refused with the KMS error class the operation models
//! (`KmsAccessDeniedException`, HTTP 400) — which is why every one of those
//! operations declares the `Kms*` family.
//!
//! # The key's state
//!
//! A key only some of whose key states admit cryptographic use of it cannot seal or
//! unseal anything while it is in one of the others — disabled, awaiting deletion,
//! awaiting imported key material, still being created, unavailable — so every call
//! that would use it is refused with `KmsInvalidStateException` (HTTP 400) for as long
//! as that lasts, carrying the key state the caller has to change as the `kmsKeyState`
//! member reports it. Nothing latches: the decision is taken against the key record
//! KMS holds at the instant of THIS call, so a key its owner puts back into a usable
//! state serves the very next call.
//!
//! No plaintext data key outlives a call here, so every call that needs the key
//! consults it — which `kmsDataKeyReusePeriodSeconds` permits: it is the "maximum
//! duration that Step Functions will reuse data keys"
//! (`smithy_query(query="shape",shape="EncryptionConfiguration")`), an upper bound on
//! how long a data key MAY serve without KMS being asked again, not a period during
//! which it must be reused.
//!
//! # The caller
//!
//! A READ asks for `kms:Decrypt`, the permission the `includedData` member documents
//! ("callers must have kms:Decrypt permission to decrypt the definition"). A WRITE of
//! data the caller supplied asks for the data key the payload is sealed with: "when
//! the period expires, Step Functions will call `GenerateDataKey`"
//! (`kmsDataKeyReusePeriodSeconds`), and a data key still inside that period is
//! RE-USED, which means decrypting the one already minted — so a caller allowed
//! neither cannot have its data sealed. A request that carries no such data seals
//! nothing and so asks nothing of the key.
//!
//! Two approximations are deliberate here, so a later reader does not take them for
//! the service's exact rule. Which of the two calls a given write makes depends on
//! how long ago a data key was minted, which no caller chooses, so a write asks for
//! both rather than for whichever one this instant would have made. And a read or a
//! write of an execution's own data is decided against the identity that signed the
//! REQUEST, which stands in for the caller the service makes that call as.
//!
//! STARTING an execution is not one of those: sealing the execution's data is work the
//! service does as the state machine's EXECUTION ROLE ("the IAM role to use for this
//! state machine"), so that use of the key is decided for the role, against both
//! authorities KMS weighs over it — the key's own resource-based policy and the role's
//! identity-based policies.
//!
//! # The encryption context
//!
//! Every cryptographic operation Step Functions makes for a resource is bound to an
//! ENCRYPTION CONTEXT naming that resource ([`STATE_MACHINE_CONTEXT_ENTRY`],
//! [`ACTIVITY_CONTEXT_ENTRY`]), which is what a key policy statement or a grant
//! conditioned on `kms:EncryptionContext:<entry>` is decided against: an authority
//! scoped to one resource's context covers that resource's own calls and no others,
//! which is how one key is confined to one workflow.
//!
//! The key is resolved in the KMS service's OWN store and the decision is KMS's own
//! ([`kms_authorization::authorize`]), so this service reads exactly what KMS's
//! handlers wrote and the two cannot disagree about a key's policy: a statement a
//! caller removes stops granting from the very next call, and one it puts back grants
//! again, because nothing here is cached.
//!
//! An Amazon Web Services owned key — what a state machine created without an
//! encryption configuration is on — needs no grant of the caller's: AWS manages that
//! key's policy for the service, so there is nothing to decide and the read proceeds.
//! The same is true when no KMS service is mounted beside this one, or when the
//! identifier the state machine carries names no key KMS holds: a condition nobody
//! attributed to the key never refuses a read.
//!
//! Resolving the caller materialises the session expiries the clock has already made
//! due — a write owed at that same instant whether or not anybody called — which is
//! why a REPORTING operation may make this decision: what a later caller sees is
//! unchanged by the read itself.

use ::std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::state::authorization as kms_authorization;

/// The KMS operation a read of data the key encrypts performs: the data was sealed
/// under that key, so reporting it means decrypting it.
const DECRYPT_OPERATION: &str = "Decrypt";

/// The KMS operation that MINTS the data key a payload is sealed with.
const GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// The KMS operation that reports the key itself — what it is, and which key state it
/// is in. Configuring a resource on a key, and using it to seal new data, begins with
/// it: the key has to be the kind of key the configuration needs before anything is
/// sealed under it.
const DESCRIBE_KEY_OPERATION: &str = "DescribeKey";

/// What reporting data the key sealed asks of that key.
const READ_OF_SEALED_DATA: &[&str] = &[DECRYPT_OPERATION];

/// What SEALING a resource's data under its key asks of the key and nothing more: the
/// data key the payload goes under is minted — the call `kmsDataKeyReusePeriodSeconds`
/// names ("when the period expires, Step Functions will call `GenerateDataKey`"). A call
/// that seals data it is handed, and reports none of the data the key already sealed,
/// asks for that ONE call.
///
/// Reading the key is not part of it: a key is READ where a resource is COMMITTED to it
/// ([`CONFIGURED_KEY_USE`]), which has already happened by the time anything is sealed,
/// so a call that only seals asks nothing of the key it does not make — exactly as a
/// write into an existing execution does ([`WRITE_OF_SEALED_DATA`]).
const SEAL_OF_CALLER_DATA: &[&str] = &[GENERATE_DATA_KEY_OPERATION];

/// What a call that neither seals nor unseals anything asks of the key: nothing. The
/// key's own key state still decides such a call, because a key state is a condition of
/// the KEY rather than of an identity, and nothing of the resource's data can be written
/// or read while it lasts.
const NOTHING_OF_THE_KEY: &[&str] = &[];

/// What sealing caller-supplied data INTO AN EXISTING execution asks of its key: the
/// data key that execution's payloads already go under is RE-USED, which decrypts it —
/// "when the period expires, Step Functions will call `GenerateDataKey`"
/// (`kmsDataKeyReusePeriodSeconds`), so a reply inside that period opens the data key
/// already minted and a reply past it mints the next one. A caller is allowed to have
/// its data sealed only when it is allowed both, since which of the two a given call
/// makes is not the caller's to choose; the reuse is asked FIRST, because that is what a
/// reply to a task an execution is already holding does.
const WRITE_OF_SEALED_DATA: &[&str] = &[DECRYPT_OPERATION, GENERATE_DATA_KEY_OPERATION];

/// What APPLYING an encryption configuration asks of the key the configuration names:
/// the key is READ first — a configuration commits the resource to a key of the kind it
/// needs, in a key state that can seal at all — and then the data key the resource's
/// payloads will go under is minted. A grant conditioned on the encryption context of
/// the resource being configured covers that mint, which is how one key is scoped to one
/// resource.
const CONFIGURED_KEY_USE: &[&str] = &[DESCRIBE_KEY_OPERATION, GENERATE_DATA_KEY_OPERATION];

/// The service name KMS's own error metadata carries.
const KMS_SERVICE_NAME: &str = "Kms";

/// The status KMS answers each of these refusals with, which is also the status the
/// `Kms*` shapes this service relays them as carry.
const KMS_REFUSAL_STATUS: u16 = 400;

/// How many attempts the refused KMS call took, which the relayed metadata reports
/// last: a key state and an authorization decision are both conditions retrying cannot
/// change, so the call that was refused was made once and not again.
const KMS_ATTEMPTS: u32 = 1;

/// The ENCRYPTION CONTEXT entry Step Functions binds every cryptographic operation on a
/// STATE MACHINE's data to: the name is the one a key policy or a grant conditioned on
/// `kms:EncryptionContext:<entry>` is written against, and its value is the ARN of the
/// state machine whose data the call is about.
///
/// Presenting it is what makes such a condition decidable at all: a grant scoped to one
/// state machine's context permits a call about THAT state machine and no other, which
/// is how one account's key is confined to one workflow.
pub const STATE_MACHINE_CONTEXT_ENTRY: &str = "aws:states:stateMachineArn";

/// The same entry for an ACTIVITY's data, whose `encryptionConfiguration` is the
/// activity's own: an activity is a resource in its own right, so a call about its data
/// is bound to its ARN rather than to any state machine's.
pub const ACTIVITY_CONTEXT_ENTRY: &str = "aws:states:activityArn";

/// The sentence `InvalidEncryptionConfiguration` carries when the key a configuration
/// names is in no key state that admits cryptographic use of it — "received when
/// encryptionConfiguration is specified but various conditions exist which make the
/// configuration invalid […] or the KMS key is not symmetric or inactive".
///
/// ENTIRELY FIXED text: the service states which MEMBER of the configuration is at
/// fault and what is wrong with the key it names, and carries no value of the request.
pub const KEY_NOT_ENABLED: &str = "kmsKeyId is not enabled";

/// One resource's key AS THIS SERVICE USES IT.
///
/// Both halves of a cryptographic operation Step Functions makes for a resource: the
/// `kmsKeyId` that resource carries, in whichever form the account named it, and the
/// ENCRYPTION CONTEXT the operation is bound to, which is what a policy statement or a
/// grant scoped to one resource is decided against.
#[derive(Debug, Clone)]
struct KeyUse {
    /// The key reference the resource's configuration carries.
    key: String,
    /// The ARN of the resource whose data the key seals.
    resource: String,
    /// The encryption context the call presents, one entry naming the resource.
    context: HashMap<String, String>,
}

impl KeyUse {
    /// The use `resource_arn` makes of `key`, bound to the context entry `entry` names
    /// that kind of resource by.
    fn of(key: &str, entry: &str, resource_arn: &str) -> Self {
        Self {
            key: key.to_string(),
            resource: resource_arn.to_string(),
            context: HashMap::from([(entry.to_string(), resource_arn.to_string())]),
        }
    }

    /// The name of the resource this use is for, which is the LAST field of its ARN:
    /// what the session Step Functions assumes the execution role for is named after.
    fn resource_name(&self) -> &str {
        self.resource.rsplit(':').next().unwrap_or_default()
    }
}

/// Why a state machine's customer managed key cannot be used for a call, in the
/// modelled error class the operations answer it with. The payload is the message, as
/// KMS worded it and this service relays it.
#[derive(Debug, Clone)]
pub enum KeyRefusal {
    /// The key's own key state admits no cryptographic use of it —
    /// `KmsInvalidStateException`, carrying that key state.
    InvalidState {
        /// The relayed sentence KMS refused the call with.
        message: String,
        /// The key state the caller has to change, as `KmsKeyState` spells it, and
        /// `None` for a state that enum does not name.
        key_state: Option<String>,
    },
    /// The authorities in force do not let this caller have the key used on its behalf
    /// — `KmsAccessDeniedException`.
    AccessDenied(String),
}

/// Why the encryption configuration a create or an update asks for cannot be applied.
///
/// A configuration is applied BEFORE anything is sealed under the key it names, so the
/// two conditions it can break are reported in that operation's own channel: a key no
/// cryptographic operation can use is a configuration the service cannot apply at all —
/// the modelled `InvalidEncryptionConfiguration` — while a key the caller may not have a
/// data key minted with is the `Kms*` refusal every other use of the key answers.
#[derive(Debug, Clone)]
pub enum ConfigurationRefusal {
    /// The key's own key state admits no cryptographic use of it, so naming it is an
    /// invalid encryption configuration ([`KEY_NOT_ENABLED`]). These operations model no
    /// `KmsInvalidStateException`: a configuration is refused as a configuration.
    NotEnabled,
    /// KMS does not let this caller mint a data key with the key —
    /// `KmsAccessDeniedException`, carrying the sentence KMS refused the call with.
    AccessDenied(String),
}

/// The refusal one of this service's operations answers a [`ConfigurationRefusal`]
/// with, in that operation's OWN error channel.
///
/// The same reason [`kms_key_refusal`] is a macro: every operation that accepts an
/// `encryptionConfiguration` has its own generated error enum, and all of them declare
/// both of these errors, so the mapping is expanded once per handler rather than being
/// worded differently in each.
#[macro_export]
macro_rules! kms_configuration_refusal {
    ($modeled:ident, $refusal:expr) => {{
        match $refusal {
            $crate::state::encryption::ConfigurationRefusal::NotEnabled => {
                $modeled::InvalidEncryptionConfiguration(
                    $crate::ops::types::InvalidEncryptionConfiguration {
                        message: Some($crate::state::encryption::KEY_NOT_ENABLED.to_string()),
                    },
                )
            }
            $crate::state::encryption::ConfigurationRefusal::AccessDenied(message) => {
                $modeled::KmsAccessDeniedException($crate::ops::types::KmsAccessDeniedException {
                    message: Some(message),
                })
            }
        }
    }};
}

/// The refusal one of this service's operations answers a [`KeyRefusal`] with, in that
/// operation's OWN error channel.
///
/// Each operation has its own generated error enum, so the mapping from a refusal to
/// the modelled error cannot be a function over one type; it is this macro instead,
/// expanded once per handler, so no two of them word or classify the same refusal
/// differently. `$modeled` is the handler's `{Op}ErrorModeled` enum, every one of which
/// declares both of these errors.
#[macro_export]
macro_rules! kms_key_refusal {
    ($modeled:ident, $refusal:expr) => {{
        match $refusal {
            $crate::state::encryption::KeyRefusal::InvalidState { message, key_state } => {
                $modeled::KmsInvalidStateException($crate::ops::types::KmsInvalidStateException {
                    kms_key_state: key_state,
                    message: Some(message),
                })
            }
            $crate::state::encryption::KeyRefusal::AccessDenied(message) => {
                $modeled::KmsAccessDeniedException($crate::ops::types::KmsAccessDeniedException {
                    message: Some(message),
                })
            }
        }
    }};
}

/// A refusal KMS answered, as the caller sees it: Step Functions does not word these
/// refusals itself, it RELAYS what its KMS call failed with, so the message is KMS's
/// own sentence followed by the metadata an AWS error carries — the service that
/// refused, the status and the id of the request that was refused, then how many
/// attempts it took.
fn relayed(message: &str) -> String {
    format!(
        "{message} (Service: {KMS_SERVICE_NAME}, Status Code: {KMS_REFUSAL_STATUS}, \
         Request ID: {}) (SDK Attempt Count: {KMS_ATTEMPTS})",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// The customer managed key `configuration` puts a resource on, or `None` when the
/// resource is encrypted with an Amazon Web Services owned key.
///
/// Only [`ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY`](super::ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY)
/// carries a `kmsKeyId` (`super::invalid_encryption_reason` refuses one under the
/// other type), so a configuration of any other type names no key for this gate to
/// consult.
fn customer_managed_key(
    configuration: Option<&crate::ops::types::EncryptionConfiguration>,
) -> Option<&str> {
    let configuration = configuration?;
    if configuration.r#type != super::ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY {
        return None;
    }
    configuration
        .kms_key_id
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
}

/// Why the data of the execution `execution_arn` identifies cannot be read for this
/// request, or `None` when it can be.
///
/// The key is the one the execution's STATE MACHINE is encrypted under, which is what
/// its execution data and its definition were sealed with.
///
/// `None` for an ARN that addresses no execution: that is the operation's own
/// `ExecutionDoesNotExist`, reported out of the store it goes on to read.
pub fn execution_read_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    execution_arn: &str,
) -> Option<KeyRefusal> {
    execution_key_denied(engine, ctx, region, execution_arn, READ_OF_SEALED_DATA)
}

/// Why data of this request's own cannot be SEALED into the execution `execution_arn`
/// identifies, or `None` when it can be.
///
/// The key is the one the execution's STATE MACHINE is encrypted under, which is what
/// everything the execution retains is sealed with, so the data this call records
/// goes under that same key.
///
/// `None` for an ARN that addresses no execution: that is the operation's own
/// `ExecutionDoesNotExist`, reported out of the store it goes on to write.
pub fn execution_write_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    execution_arn: &str,
) -> Option<KeyRefusal> {
    execution_key_denied(engine, ctx, region, execution_arn, WRITE_OF_SEALED_DATA)
}

/// The one resolution behind both of the above: the key the execution's data lives
/// under, judged for the `operations` this call makes on it.
fn execution_key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    execution_arn: &str,
    operations: &[&str],
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // The lock is released before KMS is consulted: nothing of this service's state
    // is read or written while the decision is being made.
    let key = {
        let store = state.lock();
        let machine_arn = store.executions.get(execution_arn)?.state_machine_arn.clone();
        state_machine_key(&store, &machine_arn)?
    };
    key_denied(engine, ctx, region, &key, operations)
}

/// Why data of this request's own cannot be SEALED into the execution the activity
/// task `token` belongs to, or `None` when it can be.
///
/// A task token names its task and its execution on its own, so a reply to one is
/// judged against every key the data it records goes under — the ACTIVITY's own key,
/// for a task a worker claimed off an activity, and the key of the state machine the
/// execution waiting at it runs — in that execution's own Region, since a KMS key is a
/// regional resource. Either key refusing is enough to refuse the reply: the first
/// refusal is what the caller is told.
///
/// `None` for a token the store holds no outstanding task under: that is the
/// operation's own refusal about a task that has been closed.
pub fn task_reply_write_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    token: &str,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // ONE acquisition resolves the whole chain — the task, the activity it was queued
    // under, its execution and that execution's state machine — so the reads cannot
    // disagree, and the guard is released before KMS is consulted.
    let (execution_arn, role_arn, uses) = {
        let store = state.lock();
        let task = store.activity_tasks.get(token)?;
        let execution_arn = task.execution_arn.clone();
        let mut uses: Vec<KeyUse> = Vec::new();
        // A CALLBACK task belongs to no activity, so there is no activity key for it:
        // the work was handed over by the `Task` state's own integration call.
        if let Some(activity_arn) = task.activity_arn.as_deref() {
            uses.extend(activity_key(&store, activity_arn));
        }
        let execution = store.executions.get(&execution_arn);
        let role_arn = execution.map(|execution| execution.ran.role_arn.clone());
        if let Some(machine_arn) = execution.map(|execution| execution.state_machine_arn.clone()) {
            uses.extend(state_machine_key(&store, &machine_arn));
        }
        (execution_arn, role_arn, uses)
    };
    let region = super::arn_region(&execution_arn);
    uses.iter().find_map(|used| {
        // The reply's data is recorded INTO the execution, which Step Functions does as
        // the state machine's EXECUTION ROLE — the identity it assumes for that
        // execution's work — so that role has to be allowed the key as well as the
        // caller: a least-privilege role the key does not permit cannot have the reply
        // sealed, whoever sent it. A token whose execution the store no longer holds
        // names no role, and no use of a key is decided for an identity that is not
        // there.
        key_denied(engine, ctx, &region, used, WRITE_OF_SEALED_DATA).or_else(|| {
            let role_arn = role_arn.as_deref()?;
            role_key_denied(engine, &region, used, role_arn, WRITE_OF_SEALED_DATA)
        })
    })
}

/// Fail every execution whose outcome the clock has made due and whose key cannot be
/// used for the work that outcome was owed by.
///
/// An execution of a state machine on a customer managed key does its remaining work
/// under that key: the events it records and the data it keeps are sealed with it, as
/// the state machine's EXECUTION ROLE. So what the clock makes due at the instant the
/// outcome is owed depends on the key at THAT instant — a key in a key state no
/// cryptographic operation can use, and a role the authorities in force no longer let
/// use it, each make the due outcome a FAILURE with the language's own runtime error
/// rather than the outcome the walk computed. The decision is the key's and the clock's
/// together, so it is owed whether or not anybody called, which is what lets a
/// REPORTING operation materialise it: an execution still owed its outcome is not
/// touched, and one whose outcome was already materialised is not revisited.
///
/// Call it with no store of this service locked, before `settle`: it takes KMS's and
/// IAM's own stores, and it leaves the outcome for `settle` to apply.
pub fn fail_due_runs_the_key_refuses<Eng: Send + Sync + 'static>(engine: &Eng, now: ::std::time::SystemTime) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    // What is owed at this instant, read under ONE acquisition and judged with none.
    let due: Vec<(String, String, String, KeyUse)> = {
        let store = state.lock();
        store
            .pending_executions
            .iter()
            .filter_map(|arn| {
                let execution = store.executions.get(arn)?;
                let pending = execution.pending.as_ref()?;
                if pending.due_at > now {
                    return None;
                }
                let key = customer_managed_key(execution.ran.encryption_configuration.as_ref())?;
                Some((
                    arn.clone(),
                    super::arn_region(&execution.state_machine_arn),
                    execution.ran.role_arn.clone(),
                    KeyUse::of(
                        key,
                        STATE_MACHINE_CONTEXT_ENTRY,
                        &execution.state_machine_arn,
                    ),
                ))
            })
            .collect()
    };
    let refused: Vec<(String, String)> = due
        .into_iter()
        .filter_map(|(arn, region, role_arn, used)| {
            let refusal = run_key_denied(engine, &region, &used, &role_arn)?;
            let cause = match refusal {
                KeyRefusal::InvalidState { message, .. } | KeyRefusal::AccessDenied(message) => {
                    message
                }
            };
            Some((arn, cause))
        })
        .collect();
    if refused.is_empty() {
        return;
    }
    let mut store = state.lock();
    for (arn, cause) in refused {
        store.fail_due_outcome(&arn, super::asl::RUNTIME_ERROR, &cause);
    }
}

/// Why the work an execution's own run does under `used` cannot be done as the role
/// `role_arn`, or `None` when it can be.
///
/// The key's own key state first, exactly as [`key_denied`] decides it for a call, and
/// then the role's use of the key. What the run asks of the key is the DECRYPT
/// ([`READ_OF_SEALED_DATA`]): an execution of an encrypted state machine opens what that
/// key already sealed — its definition and the data the run carries from state to state
/// — so a role the key no longer lets decrypt cannot do the execution's remaining work,
/// and nor can one whose own data another resource's key seals.
///
/// `None` wherever nothing of the kind can be shown: no KMS beside this service, an
/// identifier naming no key KMS holds, an Amazon Web Services managed key and a role
/// IAM holds no record for. Call it with no store of this service locked.
fn run_key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    used: &KeyUse,
    role_arn: &str,
) -> Option<KeyRefusal> {
    let kms = kms_store::service_state(engine)?;
    let state_refusal = {
        let store = kms.lock();
        let kms_store::KeyRef::Found(found) = store.resolve(region, &used.key) else {
            return None;
        };
        if found.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
            return None;
        }
        (!found.is_usable()).then(|| KeyRefusal::InvalidState {
            message: relayed(&kms_store::invalid_state_message(found)),
            key_state: modelled_key_state(found.reported_key_state()),
        })
    };
    if let Some(refusal) = state_refusal {
        return Some(refusal);
    }
    role_key_denied(engine, region, used, role_arn, READ_OF_SEALED_DATA)
}

/// Why the execution role `role_arn` cannot have the key of the resource `resource_arn`
/// used for the work a `Task` state's integration call makes it do, or `None` when it
/// can be.
///
/// A `Task` state that starts an execution of ANOTHER state machine hands that state
/// machine data the key of THAT resource seals, and Step Functions makes the call acting
/// as this execution's own role — so a role the target's key does not permit cannot have
/// the data sealed, which fails the task instead of starting work that cannot be
/// recorded. The decision is the one every other use of a key by a role is
/// ([`role_key_denied`]), over the key the TARGET resource carries.
///
/// `None` for an ARN that names no resource of this service, and wherever nothing of the
/// kind can be shown. Call it with no store of this service locked.
pub fn target_key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    resource_arn: &str,
    role_arn: &str,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    let used = {
        let store = state.lock();
        state_machine_key(&store, resource_arn).or_else(|| activity_key(&store, resource_arn))?
    };
    run_key_denied(engine, region, &used, role_arn)
}

/// Why a task of the activity `activity_arn` identifies cannot be DELIVERED to this
/// caller, or `None` when it can be.
///
/// An activity's `encryptionConfiguration` is the activity's own, and what a delivery
/// hands the worker is the task's `input` — sealed under that key — so the worker's
/// grant is decided on the ACTIVITY's key and bound to the activity's own encryption
/// context: a worker a key confines to one activity cannot be handed another's work.
///
/// `None` for an ARN that addresses no activity: that is the operation's own
/// `ActivityDoesNotExist`, reported out of the store it goes on to read.
pub fn activity_read_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    activity_arn: &str,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // The guard is released before KMS is consulted.
    let used = {
        let store = state.lock();
        activity_key(&store, activity_arn)?
    };
    key_denied(engine, ctx, region, &used, READ_OF_SEALED_DATA)
}

/// Why this request's own data cannot be SEALED under the key of the state machine
/// `state_machine_arn` identifies, or `None` when it can be.
///
/// This is the decision for a call that hands the service data to keep — the `input` a
/// start carries — which is sealed under the state machine's key as the CALLER: the
/// data key the payload goes under is minted on the caller's behalf, so a caller the
/// key does not permit cannot have its data sealed and nothing is started.
///
/// `None` for an ARN that addresses no state machine, which is the operation's own
/// `StateMachineDoesNotExist`.
pub fn state_machine_seal_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    state_machine_arn: &str,
) -> Option<KeyRefusal> {
    state_machine_key_denied(engine, ctx, region, state_machine_arn, SEAL_OF_CALLER_DATA)
}

/// Why a SYNCHRONOUS run of the state machine `state_machine_arn` identifies cannot be
/// served, or `None` when it can be.
///
/// Such a run keeps nothing of its own — it answers its outcome in the call that
/// started it — so what the CALLER needs the key for is the data the response reports:
/// "callers must have kms:Decrypt permission to decrypt the definition. Alternatively,
/// you can call the API with includedData = METADATA_ONLY to get a successful response
/// without the encrypted definition". `reports_sealed_data` is set for the whole-data
/// projection, which asks the caller for that decrypt; the metadata-only projection
/// reports none of it and asks the caller for nothing — while the key's own key state
/// still decides the run either way, since no run of an encrypted state machine happens
/// while the key admits no cryptographic use of it.
pub fn synchronous_run_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    state_machine_arn: &str,
    reports_sealed_data: bool,
) -> Option<KeyRefusal> {
    let operations = if reports_sealed_data {
        READ_OF_SEALED_DATA
    } else {
        NOTHING_OF_THE_KEY
    };
    state_machine_key_denied(engine, ctx, region, state_machine_arn, operations)
}

/// The one resolution behind both of the above: the state machine's key, judged for the
/// `operations` this call makes on it.
fn state_machine_key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    state_machine_arn: &str,
    operations: &[&str],
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    let used = {
        let store = state.lock();
        state_machine_key(&store, state_machine_arn)?
    };
    key_denied(engine, ctx, region, &used, operations)
}

/// Why the encryption configuration a create or an update of the STATE MACHINE
/// `state_machine_arn` asks for cannot be applied, or `None` when it can be.
pub fn state_machine_configuration_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configuration: Option<&crate::ops::types::EncryptionConfiguration>,
    state_machine_arn: &str,
) -> Option<ConfigurationRefusal> {
    configuration_refusal(
        engine,
        ctx,
        region,
        configuration,
        STATE_MACHINE_CONTEXT_ENTRY,
        state_machine_arn,
    )
}

/// Why the encryption configuration a create of the ACTIVITY `activity_arn` asks for
/// cannot be applied, or `None` when it can be.
pub fn activity_configuration_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configuration: Option<&crate::ops::types::EncryptionConfiguration>,
    activity_arn: &str,
) -> Option<ConfigurationRefusal> {
    configuration_refusal(
        engine,
        ctx,
        region,
        configuration,
        ACTIVITY_CONTEXT_ENTRY,
        activity_arn,
    )
}

/// The one decision behind both of the above: a configuration naming a customer managed
/// key is one the service can apply only while that key can seal the resource's data
/// for this caller.
///
/// The key itself is consulted, because that is what the configuration commits the
/// resource to: a key in a key state no cryptographic operation can use would seal
/// nothing, so the configuration is invalid rather than stored and left to fail later,
/// and a caller the key does not permit a data key of is refused the KMS refusal.
///
/// The configuration asks for the data key the resource's payloads will go under
/// ([`CONFIGURED_KEY_USE`]), bound to the encryption context the resource's ARN gives it
/// — which is what makes a grant scoped to one resource apply to the configuration of
/// that resource and of no other.
///
/// `None` wherever nothing of the kind can be shown, exactly as every other decision
/// here: a configuration on an Amazon Web Services owned key, no KMS beside this
/// service, and an identifier naming no key KMS holds.
fn configuration_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configuration: Option<&crate::ops::types::EncryptionConfiguration>,
    entry: &str,
    resource_arn: &str,
) -> Option<ConfigurationRefusal> {
    let key = customer_managed_key(configuration)?;
    let used = KeyUse::of(key, entry, resource_arn);
    match key_denied(engine, ctx, region, &used, CONFIGURED_KEY_USE)? {
        KeyRefusal::InvalidState { .. } => Some(ConfigurationRefusal::NotEnabled),
        KeyRefusal::AccessDenied(message) => Some(ConfigurationRefusal::AccessDenied(message)),
    }
}

/// `configuration` with its `kmsKeyId` RESOLVED to the ARN of the key it names.
///
/// `kmsKeyId` accepts "an alias, alias ARN, key ID, or key ARN of a symmetric
/// encryption KMS key", which are four ways of naming ONE key, and what the service
/// keeps — and every read reports — is the key itself: the ARN KMS holds it under. A
/// caller comparing a described configuration against a key ARN it holds therefore sees
/// the key it configured whichever reference form it configured it by.
///
/// The reference is left exactly as given wherever it resolves to no key of this
/// account in this Region: there is no ARN to report instead, and a configuration on an
/// Amazon Web Services owned key names no key at all.
///
/// Call it with no store of this service locked: it takes KMS's own.
pub fn with_resolved_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    configuration: Option<crate::ops::types::EncryptionConfiguration>,
) -> Option<crate::ops::types::EncryptionConfiguration> {
    let mut configuration = configuration?;
    let key = customer_managed_key(Some(&configuration))?.to_string();
    let kms = kms_store::service_state(engine)?;
    let resolved = {
        let store = kms.lock();
        match store.resolve(region, &key) {
            kms_store::KeyRef::Found(found) => Some(found.arn.clone()),
            _ => None,
        }
    };
    if let Some(arn) = resolved {
        configuration.kms_key_id = Some(arn);
    }
    Some(configuration)
}

/// Why the data of the state machine `state_machine_arn` identifies cannot be read for
/// this request, or `None` when it can be.
///
/// `None` for an ARN that addresses no state machine and no version, which is the
/// operation's own `StateMachineDoesNotExist`.
pub fn state_machine_read_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    state_machine_arn: &str,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // As above, the guard is dropped before the decision is made.
    let key = {
        let store = state.lock();
        state_machine_key(&store, state_machine_arn)?
    };
    key_denied(engine, ctx, region, &key, READ_OF_SEALED_DATA)
}

/// Why an execution of the state machine `state_machine_arn` identifies cannot be
/// STARTED under its customer managed key, or `None` when it can be.
///
/// Starting an execution seals the execution's data under the state machine's key, and
/// Step Functions does that AS THE STATE MACHINE'S EXECUTION ROLE: "the IAM role to
/// use for this state machine" is the identity the service acts under for the run, and
/// `kmsDataKeyReusePeriodSeconds` names the call that sealing makes ("when the period
/// expires, Step Functions will call `GenerateDataKey`"). So the decision here is
/// about the ROLE rather than about the caller: a role whose identity-based policies do
/// not allow that call on the key cannot have the execution's data sealed, and the
/// start is refused with the modelled `KmsAccessDeniedException` — "either your KMS key
/// policy or API caller does not have the required permissions" — having started
/// nothing.
///
/// `unassumable` says that IAM ANSWERED that this service cannot act as that role at all
/// ([`super::task_authorization::unassumable_role`]). Acting as the role comes before
/// anything the role may then do: credentials the assume never minted seal nothing, so the
/// mint the start has to make is left to be made as the only identity this service still
/// has for it — its OWN service principal, on behalf of the state machine
/// ([`kms_authorization::Caller::service_principal`]) — whose use of the key the KEY'S OWN
/// POLICY is the only authority over. A key policy that names this service for that state
/// machine serves the mint; one that does not refuses it, and the start carries the
/// sentence KMS answered with. It is weighed only for a role this account's IAM HOLDS, the
/// same evidence [`role_key_denied`] requires: a role no record answers for is the
/// deploy-time/run-time split this service exposes rather than a role that was refused.
///
/// The KEY STATE is deliberately not decided here: it is a condition of the key rather
/// than of an identity, and it is decided by the calls that read or write the sealed
/// data ([`key_denied`]), which is where this crate decides it for every operation
/// that touches that data. One question, one place.
///
/// `None` wherever nothing of the kind can be shown: a state machine on an Amazon Web
/// Services owned key (nothing of any identity's is asked of such a key), an engine
/// carrying no KMS or no IAM store, an identifier naming no key KMS holds, and a role
/// IAM holds no record for — each is evidence of nothing, so the start proceeds. Call
/// it with no store of this service locked.
pub fn execution_start_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    state_machine_arn: &str,
    role_arn: &str,
    unassumable: bool,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // The guard is released before KMS and IAM are consulted: no store of another
    // service is reached while this one's is held.
    let key = {
        let store = state.lock();
        state_machine_key(&store, state_machine_arn)?
    };
    if unassumable && super::task_authorization::iam_holds_role(engine, role_arn) {
        return service_principal_key_denied(engine, region, &key);
    }
    role_key_denied(
        engine,
        region,
        &key,
        role_arn,
        &[GENERATE_DATA_KEY_OPERATION],
    )
}

/// Why the data key `used` needs cannot be minted by this service AS ITSELF, as KMS words
/// that refusal, or `None` when the key's own policy lets that use through.
///
/// The decision KMS takes for a request another service made as its own service principal
/// on behalf of one of its resources ([`kms_authorization::Caller::service_principal`]):
/// such a caller is no principal of the account, so it carries no identity-based documents
/// and naming the account grants it nothing — the key's own policy is the whole authority
/// over it, decided against the state machine the call is made for, which is what a
/// statement scoped by `aws:SourceArn`/`aws:SourceAccount` is written against.
///
/// `None` for everything that shows nothing, exactly as [`role_key_denied`] reports it: an
/// engine carrying no KMS store, an identifier naming no key KMS holds, and an Amazon Web
/// Services managed key, whose policy AWS manages for the service that owns it.
fn service_principal_key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    used: &KeyUse,
) -> Option<KeyRefusal> {
    let kms = kms_store::service_state(engine)?;
    let caller = kms_authorization::Caller::service_principal(
        super::task_authorization::SERVICE_PRINCIPAL,
        &used.resource,
    );
    let via_service = super::task_authorization::regional_service_principal(region);
    let store = kms.lock();
    let kms_store::KeyRef::Found(found) = store.resolve(region, &used.key) else {
        return None;
    };
    if found.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
        return None;
    }
    kms_authorization::authorize_use(
        &caller,
        &store,
        region,
        found,
        kms_authorization::RequestedUse::through_service(GENERATE_DATA_KEY_OPERATION, &via_service),
        Some(&used.context),
        None,
    )
    .err()
    .map(|why| KeyRefusal::AccessDenied(relayed(&why)))
}

/// Why the role `role_arn` cannot have `key` used for `operations`, as KMS words that
/// refusal, or `None` when the authorities in force let the use through.
///
/// The decision is KMS's OWN, taken for the ROLE as the principal
/// ([`kms_authorization::role_caller`]): the key's resource-based policy and the role's
/// identity-based policies are the two authorities over such a use, either of which can
/// grant it — a key policy naming the role grants it outright, and one naming the
/// account delegates to the role's own documents — and an explicit deny in either
/// refuses it. Reading only one of the two is what makes a key granted the documented
/// way (its own policy) read as granting nothing.
///
/// The use is one Step Functions makes on the account's behalf rather than one the
/// account asked KMS for directly, so it is judged as arriving THROUGH this service
/// ([`kms_authorization::RequestedUse::through_service`]), which is what a key policy
/// scoped by `kms:ViaService` is written against.
///
/// `None` wherever nothing of the kind can be shown: an engine carrying no KMS store, an
/// identifier naming no key KMS holds, an Amazon Web Services managed key, whose policy
/// AWS manages for the service that owns it, and a role this account holds no record for,
/// whose documents cannot be read at all. Call it with no store of this service locked.
fn role_key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    used: &KeyUse,
    role_arn: &str,
    operations: &[&str],
) -> Option<KeyRefusal> {
    let kms = kms_store::service_state(engine)?;
    // A role the account does not hold is evidence of nothing: there are no documents to
    // weigh against the key's policy, so the use is not refused on them.
    if !super::task_authorization::iam_holds_role(engine, role_arn) {
        return None;
    }
    // The role's own documents are read BEFORE the KMS store is locked, so the IAM store
    // is never reached while another service's guard is held.
    let caller = kms_authorization::role_caller(engine, role_arn);
    let via_service = super::task_authorization::regional_service_principal(region);
    let store = kms.lock();
    let kms_store::KeyRef::Found(found) = store.resolve(region, &used.key) else {
        return None;
    };
    // An Amazon Web Services managed key's policy is managed by AWS for the service that
    // owns it, so no identity needs a grant of its own on it.
    if found.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
        return None;
    }
    operations.iter().find_map(|operation| {
        kms_authorization::authorize_use(
            &caller,
            &store,
            region,
            found,
            kms_authorization::RequestedUse::through_service(operation, &via_service),
            // The encryption context the use is bound to, so an authority scoped to
            // one resource's context is claimed for the resource it covers and for no
            // other ([`KeyUse`]).
            Some(&used.context),
            None,
        )
        .err()
        .map(|why| KeyRefusal::AccessDenied(relayed(&reported_as_session(&why, role_arn, used))))
    })
}

/// `why`, with the principal it names re-spelled as the SESSION Step Functions holds the
/// role under.
///
/// The service does not hold the role: it ASSUMES it for the resource's work, so the
/// identity a refusal about that use names is a session of the role — the spelling
/// `sts:AssumeRole` reports — and the session is named after the resource the work is
/// for. The DECISION stays the role's: the statements that grant to an assumed-role
/// session name the role, and `aws:PrincipalArn` of such a request is the role's ARN, so
/// only the sentence is re-spelled and nothing the decision reads is.
///
/// The principal is the sentence's FIRST variable part, so the first occurrence of the
/// role's ARN is the one re-spelled; a sentence that does not name the role (or an ARN
/// that names no session) is relayed as KMS worded it.
fn reported_as_session(why: &str, role_arn: &str, used: &KeyUse) -> String {
    match super::task_authorization::assumed_role_session(role_arn, used.resource_name()) {
        Some(session) => why.replacen(role_arn, &session, 1),
        None => why.to_string(),
    }
}
///
/// A version is its own stored record carrying its own encryption configuration, and
/// a version-qualified ARN names that record, so both maps are read: the version the
/// ARN names, else the state machine itself. The encryption context names the STATE
/// MACHINE either way — a version is a snapshot of that state machine's configuration,
/// not a resource a key is scoped to on its own.
fn state_machine_key(
    store: &super::ComAmazonawsSfnAwsstepfunctionsState,
    arn: &str,
) -> Option<KeyUse> {
    let (configuration, machine_arn) = match store.state_machine_versions.get(arn) {
        Some(version) => (
            version.encryption_configuration.as_ref(),
            version.state_machine_arn.as_str(),
        ),
        None => {
            let machine = store.state_machines.get(arn)?;
            (
                machine.encryption_configuration.as_ref(),
                machine.arn.as_str(),
            )
        }
    };
    let key = customer_managed_key(configuration)?;
    Some(KeyUse::of(key, STATE_MACHINE_CONTEXT_ENTRY, machine_arn))
}

/// The use the activity `arn` names makes of its customer managed key, bound to the
/// activity's own encryption context.
fn activity_key(
    store: &super::ComAmazonawsSfnAwsstepfunctionsState,
    arn: &str,
) -> Option<KeyUse> {
    let activity = store.activities.get(arn)?;
    let key = customer_managed_key(activity.encryption_configuration.as_ref())?;
    Some(KeyUse::of(key, ACTIVITY_CONTEXT_ENTRY, &activity.arn))
}

/// Why `key` cannot be used for this request, as KMS words that refusal, or `None`
/// when the key and the authorities in force let the call through.
///
/// `used` carries the resource's `kmsKeyId` in any form KMS accepts — a key id, a key
/// ARN, an alias or an alias ARN — together with the encryption context the operation
/// is bound to, and `region` is the Region the request is signed for, since a KMS key is
/// a regional resource. `operations` are the KMS operations the call makes on the key;
/// the FIRST one the authorities refuse is what the caller is told, since one refused
/// operation is already enough to stop the call.
///
/// The key's own key state is decided FIRST, the order KMS itself decides it in for a
/// cryptographic operation: a key no cryptographic operation can use refuses every
/// caller, whatever any policy says and whoever manages the key.
///
/// The use is one Step Functions makes ON THE CALLER'S BEHALF rather than one the
/// caller asked KMS for directly, so it is judged as arriving THROUGH this service
/// ([`kms_authorization::RequestedUse::through_service`]) — which is what a key policy
/// scoped by `kms:ViaService` is written against, and the form every key confined to
/// one service's use of it carries.
fn key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    used: &KeyUse,
    operations: &[&str],
) -> Option<KeyRefusal> {
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a lookup nothing answered lets the call through, so
        // without this line it would be indistinguishable from a key KMS permitted.
        ::tracing::warn!(
            key = used.key,
            "no KMS service is mounted beside Step Functions; the resource's key is accepted"
        );
        return None;
    };
    // The caller is resolved BEFORE the KMS store is locked: the STS and IAM stores it
    // reads are never reached while another service's guard is held.
    let caller = kms_authorization::caller(engine, ctx);
    let via_service = super::task_authorization::regional_service_principal(region);
    let store = kms.lock();
    let kms_store::KeyRef::Found(found) = store.resolve(region, &used.key) else {
        return None;
    };
    // Neither sealing data under the key nor unsealing data it sealed is something the
    // key can do while its key state admits no cryptographic use of it, so the call is
    // refused for as long as that key state lasts — and served again from the first
    // call after it changes, since nothing of this decision is kept.
    if !found.is_usable() {
        return Some(KeyRefusal::InvalidState {
            message: relayed(&kms_store::invalid_state_message(found)),
            key_state: modelled_key_state(found.reported_key_state()),
        });
    }
    // An Amazon Web Services managed key's policy is managed by AWS for the service
    // that owns it, so the caller needs no grant of its own on it.
    if found.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
        return None;
    }
    operations.iter().find_map(|operation| {
        kms_authorization::authorize_use(
            &caller,
            &store,
            region,
            found,
            kms_authorization::RequestedUse::through_service(operation, &via_service),
            // The encryption context this call is bound to, which is what a key policy
            // statement or a grant conditioned on one is decided against: an authority
            // scoped to one resource's context is claimed for that resource's own calls
            // and for no other ([`KeyUse`]).
            Some(&used.context),
            None,
        )
        .err()
        .map(|why| KeyRefusal::AccessDenied(relayed(&why)))
    })
}

/// A KMS `KeyState` as the `kmsKeyState` member of `KmsInvalidStateException` spells
/// it, or `None` for a key state that member's enum does not name.
///
/// The two models word the same condition differently — KMS's `KeyState` values are
/// camel case (`PendingDeletion`) and this service's `KmsKeyState` values are upper
/// snake case (`PENDING_DELETION`) — so each word boundary becomes an underscore,
/// which converts a value this slice does not reach yet without a case for it. A state
/// outside the enum is reported as no state rather than as a value the member does not
/// accept: the relayed message still says what the key is in.
fn modelled_key_state(key_state: &str) -> Option<String> {
    let mut modelled = String::with_capacity(key_state.len() + 2);
    for (index, character) in key_state.char_indices() {
        if character.is_ascii_uppercase() && index != 0 {
            modelled.push('_');
        }
        modelled.extend(character.to_uppercase());
    }
    crate::ops::types::KMS_KEY_STATE_VALUES
        .contains(&modelled.as_str())
        .then_some(modelled)
}

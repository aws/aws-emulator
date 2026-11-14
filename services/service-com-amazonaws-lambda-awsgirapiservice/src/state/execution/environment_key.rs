// SPDX-License-Identifier: CC0-1.0

//! Whether the key a function's ENVIRONMENT VARIABLES are encrypted with can decrypt them.
//!
//! A function's environment variables are encrypted at rest with a key: "The ARN of the
//! Key Management Service (KMS) customer managed key that's used to encrypt […] your
//! function's environment variables" (`KMSKeyArn`). Every invocation of such a function
//! therefore begins with a DECRYPT, and an invocation whose key cannot decrypt is refused
//! before the handler runs — which is what the four KMS failures `Invoke` binds are:
//! `KMSDisabledException`, `KMSInvalidStateException`, `KMSNotFoundException` and
//! `KMSAccessDeniedException`, each `@httpError(502)`.
//!
//! The key's state is not this service's to know: it belongs to the key service. So it is
//! ASKED, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — the framework's seam for one
//! emulated service calling another — and what that answer says decides which of the four
//! the invocation is refused with. Only a DEFINITE answer refuses: a key the key service
//! reports as usable, and an answer that establishes nothing at all (the key service is
//! not mounted beside this one, it answered about itself, or it was not reached), both
//! leave the invocation to run.
//!
//! A function with no `KMSKeyArn`, or with no environment variables to decrypt, has nothing
//! to ask about.
//!
//! The same decrypt is what a READ of the function's configuration performs, because
//! reporting `Environment$Variables` reports the plaintext: so a configuration read whose
//! key cannot decrypt reports the failure in the member the model carries it in —
//! `EnvironmentResponse$Error` -> `EnvironmentError{ErrorCode, Message}`, "error messages
//! for environment variables that couldn't be applied" — and reports no `Variables` at all,
//! because the plaintext is exactly what cannot be read ([`report_environment_key_fault`]).
//!
//! Either way the refusal RELAYS the key service's own answer: the exception class it
//! states the refusal under and the sentence it states about the key, inside Lambda's own
//! sentence for the decrypt it could not perform.

use std::collections::BTreeMap;

use crate::ops::types::{EnvironmentError, EnvironmentResponse};

/// Why the key a function's environment variables are encrypted with cannot decrypt them.
///
/// One variant per KMS failure the invocation operations bind, carrying the sentence the
/// refusal states.
#[derive(Debug, Clone)]
pub enum EnvironmentKeyFault {
    /// The key is disabled.
    Disabled(String),
    /// The key's state does not admit `Decrypt`.
    InvalidState(String),
    /// The key is not there.
    NotFound(String),
    /// The key is there and may not be used.
    AccessDenied(String),
}

/// The sentence a refusal states when the key is DISABLED.
///
/// Every one of these names the operation Lambda could not perform (decrypting the
/// environment variables), why, and where the caller fixes it — the form AWS words this
/// class of failure in, and the wording the modelled exception documents
/// (`smithy_query(query="shape", shape="KMSDisabledException")`: "Lambda couldn't decrypt
/// the environment variables because the KMS key used is disabled. Check the Lambda
/// function's KMS key settings.").
const DISABLED_MESSAGE: &str = "Lambda was unable to decrypt the environment variables because \
                                the KMS key used is disabled. Please check the function's KMS \
                                key settings.";

/// See [`DISABLED_MESSAGE`]; the state of the key does not admit a decrypt.
const INVALID_STATE_MESSAGE: &str = "Lambda was unable to decrypt the environment variables \
                                     because the state of the KMS key used is not valid for \
                                     Decrypt. Please check the function's KMS key settings.";

/// See [`DISABLED_MESSAGE`]; the key is not there.
const NOT_FOUND_MESSAGE: &str = "Lambda was unable to decrypt the environment variables because \
                                 the KMS key was not found. Please check the function's KMS key \
                                 settings.";

/// See [`DISABLED_MESSAGE`]; the key may not be used by this function.
const ACCESS_DENIED_MESSAGE: &str = "Lambda was unable to decrypt the environment variables \
                                     because KMS access was denied. Please check the function's \
                                     KMS permissions.";

/// The `KeyState` values that are not valid for `Decrypt`, other than `Disabled` — the
/// states a key is in while it is being created, imported, deleted or is unavailable
/// (source: `smithy_query(query="shape", shape="KeyState")` of the key service's model).
const STATES_INVALID_FOR_DECRYPT: &[&str] = &[
    "Creating",
    "PendingDeletion",
    "PendingImport",
    "PendingReplicaDeletion",
    "Unavailable",
    "Updating",
];

/// The `KeyState` a key that can decrypt is in.
const STATE_ENABLED: &str = "Enabled";

/// The `KeyState` of a key whose use its owner has turned off.
const STATE_DISABLED: &str = "Disabled";

/// The exception class the key service states a refusal of a DISABLED key under, which is
/// the class Lambda relays and the `ErrorCode` a configuration read reports
/// (`smithy_query(query="operation", operation="Decrypt")` of the key service's model:
/// `DisabledException`).
const KMS_DISABLED_CODE: &str = "DisabledException";

/// The exception class the key service states a refusal for every OTHER key state under
/// (`KMSInvalidStateException`, the same model's "the request was rejected because the state
/// of the specified resource is not valid for this request").
const KMS_INVALID_STATE_CODE: &str = "KMSInvalidStateException";

/// The name the SDK layer that relayed the decrypt reports the key service by, and the
/// status that service answers a refusal of this class with: a key state or a permission is
/// the caller's own request being rejected, which is answered at 400 whichever exception
/// shape carries it.
const KMS_RELAY_SERVICE: &str = "Kms";
const KMS_RELAY_STATUS: u16 = 400;

/// Which of the four KMS failures a relayed answer is.
#[derive(Debug, Clone, Copy)]
enum FaultClass {
    Disabled,
    InvalidState,
    NotFound,
    AccessDenied,
}

/// The key service's own answer about the key, as Lambda relays it: which of the four
/// failures it is, the exception class that service stated it under, and the sentence it
/// stated about the key — absent when the answer carried none, in which case nothing is
/// invented for it.
#[derive(Debug, Clone)]
struct RelayedKeyAnswer {
    class: FaultClass,
    code: String,
    reason: Option<String>,
}

/// How the key service words a refusal caused by the key's own key state: the key's ARN,
/// then the state as prose, as its `KeyState` values read once each camel-case word is
/// lowercased (`smithy_query(query="shape", shape="KeyState")`) — "… is disabled." for a
/// disabled key, "… is pending deletion." for one whose deletion is scheduled.
fn key_state_reason(key_arn: &str, key_state: &str) -> String {
    format!("{key_arn} is {}.", key_state_prose(key_state))
}

/// A `KeyState` value as prose: its camel-case words, lowercased and separated by spaces,
/// so a state this slice does not name explicitly is worded the same way without a case for
/// it.
fn key_state_prose(key_state: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    for letter in key_state.chars() {
        if letter.is_uppercase() && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.extend(letter.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words.join(" ")
}

/// The key service's answer about `key_arn`, or `None` when nothing it said means the key
/// cannot decrypt.
///
/// This is the ONE reading of that answer, so the data plane's refusal and the control
/// plane's report cannot judge the same key differently. Only a DEFINITE answer refuses: a
/// key the key service reports as usable, a key state this emulator does not know, and an
/// answer that establishes nothing at all (the key service is not mounted beside this one,
/// or it was not reached) all leave the key as usable.
async fn relayed_key_answer(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    key_arn: &str,
) -> Option<RelayedKeyAnswer> {
    let described = kms_client(callback, region)
        .describe_key()
        .key_id(key_arn)
        .send()
        .await;
    match described {
        Ok(answer) => {
            let state = answer
                .key_metadata()
                .and_then(::aws_sdk_kms::types::KeyMetadata::key_state)
                .map(|state| state.as_str().to_string());
            let (class, code, key_state) = match state.as_deref() {
                // A key the key service reports as usable decrypts, so nothing is refused.
                None | Some(STATE_ENABLED) => return None,
                Some(STATE_DISABLED) => {
                    (FaultClass::Disabled, KMS_DISABLED_CODE, STATE_DISABLED)
                }
                Some(state) if STATES_INVALID_FOR_DECRYPT.contains(&state) => {
                    (FaultClass::InvalidState, KMS_INVALID_STATE_CODE, state)
                }
                // A state this emulator does not know is not evidence that the key cannot
                // decrypt.
                Some(_) => return None,
            };
            Some(RelayedKeyAnswer {
                class,
                code: code.to_string(),
                reason: Some(key_state_reason(key_arn, key_state)),
            })
        }
        // Only an answer FROM the key service says anything about the key. A transport
        // failure — the key service is not mounted beside this one, or the in-process call
        // did not reach it — is read without the panicking accessor, because it carries no
        // service error at all, and leaves the key as usable.
        Err(::aws_sdk_kms::error::SdkError::ServiceError(answered)) => {
            let refused = answered.into_err();
            let class = if refused.is_not_found_exception() || refused.is_invalid_arn_exception() {
                FaultClass::NotFound
            } else if is_access_denied(&refused) {
                FaultClass::AccessDenied
            } else {
                // Every other answer about the key establishes nothing that says it cannot
                // decrypt.
                return None;
            };
            Some(RelayedKeyAnswer {
                class,
                // The class and the sentence are the key service's own, relayed as it
                // answered them; an answer that stated neither contributes neither.
                code: ::aws_sdk_kms::error::ProvideErrorMetadata::code(&refused)
                    .map_or_else(|| default_code(class).to_string(), str::to_string),
                reason: ::aws_sdk_kms::error::ProvideErrorMetadata::message(&refused)
                    .map(str::to_string),
            })
        }
        Err(_) => None,
    }
}

/// The exception class a refusal is relayed under when the key service's answer named none.
fn default_code(class: FaultClass) -> &'static str {
    match class {
        FaultClass::Disabled => KMS_DISABLED_CODE,
        FaultClass::InvalidState => KMS_INVALID_STATE_CODE,
        FaultClass::NotFound => "NotFoundException",
        FaultClass::AccessDenied => "AccessDeniedException",
    }
}

/// The key service's answer, stated after the sentence Lambda words the decrypt it could
/// not perform in: the exception class that service stated the refusal under and the
/// sentence it stated about the key.
///
/// `separator` is what stands between the class and the relayed sentence, which differs
/// between the two places AWS states this clause (see [`invocation_message`] and
/// [`report_message`]).
fn relay_clause(answer: &RelayedKeyAnswer, separator: &str) -> String {
    match &answer.reason {
        Some(reason) => format!(
            " KMS Exception: {code}{separator}KMS Message: {reason}",
            code = answer.code
        ),
        None => format!(" KMS Exception: {code}", code = answer.code),
    }
}

/// The message an INVOCATION is refused with: Lambda's sentence for the decrypt it could not
/// perform, the key service's answer, and the envelope the SDK layer that relayed that call
/// states around it — the service it was made to, the status that service answered with,
/// that call's own request id and the attempts it took
/// (`service_common::relayed_refusal`).
fn invocation_message(sentence: &str, answer: &RelayedKeyAnswer) -> String {
    format!(
        "{sentence}{clause}{envelope}",
        clause = relay_clause(answer, ""),
        envelope = ::service_common::relayed_refusal::relay_envelope(
            ::service_common::relayed_refusal::RelayedCall::once(
                KMS_RELAY_SERVICE,
                KMS_RELAY_STATUS,
            )
        )
    )
}

/// The message a configuration READ reports on `EnvironmentError`: the same two parts
/// without the relay envelope, because the control plane reports the error it recorded about
/// the environment rather than one call's own answer.
fn report_message(sentence: &str, answer: &RelayedKeyAnswer) -> String {
    format!("{sentence}{clause}", clause = relay_clause(answer, " "))
}

/// Why the environment variables of a function configured with `kms_key_arn` cannot be
/// decrypted, or `None` when nothing says they cannot.
///
/// `variables` is the function's own environment: a function with none has nothing
/// encrypted with the key, so there is nothing to decrypt and nothing to ask about.
///
/// `callback` is the in-process seam the key service is asked over, which a request reaches
/// through [`smithy_server_runtime::OperationContext::callback`] and the asynchronous
/// delivery loop ([`super::super::async_delivery`]) holds its own clone of: an invocation
/// begins with this decrypt however it arrived.
pub async fn environment_key_fault(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    kms_key_arn: Option<&str>,
    variables: &BTreeMap<String, String>,
) -> Option<EnvironmentKeyFault> {
    let key_arn = kms_key_arn?;
    if variables.is_empty() {
        return None;
    }
    // The key-service call's own future is BOXED rather than inlined into this one: an
    // invocation and a configuration read are both reached through chains of in-process calls
    // between services, and an SDK client's future is large enough that carrying it inside
    // every such future grows the stack frame of every link in those chains.
    let answer = Box::pin(relayed_key_answer(callback, region, key_arn)).await?;
    let message = invocation_message(invocation_sentence(answer.class), &answer);
    Some(match answer.class {
        FaultClass::Disabled => EnvironmentKeyFault::Disabled(message),
        FaultClass::InvalidState => EnvironmentKeyFault::InvalidState(message),
        FaultClass::NotFound => EnvironmentKeyFault::NotFound(message),
        FaultClass::AccessDenied => EnvironmentKeyFault::AccessDenied(message),
    })
}

/// The sentence an INVOCATION's refusal states the decrypt it could not perform in, for each
/// class of failure.
fn invocation_sentence(class: FaultClass) -> &'static str {
    match class {
        FaultClass::Disabled => DISABLED_MESSAGE,
        FaultClass::InvalidState => INVALID_STATE_MESSAGE,
        FaultClass::NotFound => NOT_FOUND_MESSAGE,
        FaultClass::AccessDenied => ACCESS_DENIED_MESSAGE,
    }
}

/// The sentence a configuration READ states on `EnvironmentError$Message`, for each class of
/// failure.
///
/// Said of the caller's OWN environment and the caller's own key settings, which is how AWS
/// words the error it reports about a configuration — where the data plane's refusal
/// ([`DISABLED_MESSAGE`] and its siblings) words the same failure as the refusal of one call.
fn report_sentence(class: FaultClass) -> &'static str {
    match class {
        FaultClass::Disabled => {
            "Lambda was unable to decrypt your environment variables because the KMS key used \
             is disabled. Please check your KMS key settings."
        }
        FaultClass::InvalidState => {
            "Lambda was unable to decrypt your environment variables because the state of the \
             KMS key used is not valid for Decrypt. Please check your KMS key settings."
        }
        FaultClass::NotFound => {
            "Lambda was unable to decrypt your environment variables because the KMS key was \
             not found. Please check your KMS key settings."
        }
        FaultClass::AccessDenied => {
            "Lambda was unable to decrypt your environment variables because KMS access was \
             denied. Please check your KMS permissions."
        }
    }
}

/// Report on `environment` what the key service says about the key its variables are
/// encrypted with, when that key cannot decrypt them.
///
/// Reporting `Environment$Variables` reports the PLAINTEXT of those variables, which is a
/// decrypt under the function's `KMSKeyArn`: so a projection of a function's configuration
/// carries the variables only while that key can perform it. When it cannot, the projection
/// carries the failure in the member the model gives it —
/// `EnvironmentResponse$Error` -> `EnvironmentError{ErrorCode, Message}`, the `ErrorCode`
/// being the exception class the key service stated the refusal under — and carries no
/// `Variables` at all, because the plaintext is what could not be read. Both clear by
/// themselves once the key can decrypt again: nothing is recorded, the key is asked at each
/// read.
///
/// An environment with no variables, a function with no `KMSKeyArn` and a key nothing
/// definite was said about are all left exactly as projected.
pub async fn report_environment_key_fault(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    kms_key_arn: Option<&str>,
    environment: &mut Option<EnvironmentResponse>,
) {
    let Some(key_arn) = kms_key_arn else {
        return;
    };
    let Some(reported) = environment.as_mut() else {
        return;
    };
    if reported
        .variables
        .as_ref()
        .is_none_or(std::collections::HashMap::is_empty)
    {
        return;
    }
    // Boxed for the reason [`environment_key_fault`] states.
    let Some(answer) = Box::pin(relayed_key_answer(callback, region, key_arn)).await else {
        return;
    };
    reported.variables = None;
    reported.error = Some(EnvironmentError {
        error_code: Some(answer.code.clone()),
        message: Some(report_message(report_sentence(answer.class), &answer)),
    });
}

/// How the refusal of a read of a durable execution's DATA is worded: the operation Lambda
/// could not perform (decrypting that execution's data), why, and WHICH key it could not
/// perform it under.
///
/// The sentence is the same class of failure [`DISABLED_MESSAGE`] and its siblings word for
/// the environment-variable decrypt, said of the data this read reports and naming the key
/// the function's `DurableConfig` names — the one variable part, taken from the stored
/// configuration.
fn durable_data_message(why: &str, key_arn: &str) -> String {
    format!(
        "Lambda was unable to decrypt the durable execution data because the KMS key \
         {key_arn} {why}. Please check the function's KMS key settings."
    )
}

/// Why the DATA of a durable execution encrypted under `kms_key_arn` cannot be read, or
/// `None` when nothing says it cannot.
///
/// Every operation that reports a durable execution's data binds the same four KMS
/// failures `Invoke` binds (`smithy_query(query="operation",
/// operation="GetDurableExecution,GetDurableExecutionHistory,GetDurableExecutionState")`:
/// `KMSAccessDeniedException`, `KMSDisabledException`, `KMSInvalidStateException`,
/// `KMSNotFoundException`, each `@httpError(502)`), because reporting that data decrypts it
/// under the key the function's `DurableConfig` names. So the key is asked about at the
/// point of use, over the same seam and with the same reading as
/// [`environment_key_fault`]: only a DEFINITE answer refuses.
///
/// It differs from that reading in ONE respect, and deliberately: a key the key service
/// holds no record of is NOT a refusal here. A durable function's key is named in its
/// configuration and may belong to another account, so an absent record establishes
/// nothing about it — while a key the key service HOLDS and reports unusable is evidence
/// that the data behind it cannot be read.
pub async fn durable_data_key_fault(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    kms_key_arn: Option<&str>,
) -> Option<EnvironmentKeyFault> {
    let key_arn = kms_key_arn?;
    let described = kms_client(callback, region)
        .describe_key()
        .key_id(key_arn)
        .send()
        .await;
    match described {
        Ok(answer) => {
            let state = answer
                .key_metadata()
                .and_then(::aws_sdk_kms::types::KeyMetadata::key_state)
                .map(|state| state.as_str().to_string());
            match state.as_deref() {
                None | Some(STATE_ENABLED) => None,
                Some("Disabled") => Some(EnvironmentKeyFault::Disabled(durable_data_message(
                    "is disabled",
                    key_arn,
                ))),
                Some(state) if STATES_INVALID_FOR_DECRYPT.contains(&state) => {
                    Some(EnvironmentKeyFault::InvalidState(durable_data_message(
                        "is not in a valid state for Decrypt",
                        key_arn,
                    )))
                }
                Some(_) => None,
            }
        }
        Err(failure) => match failure {
            ::aws_sdk_kms::error::SdkError::ServiceError(answered) => {
                match answered.into_err() {
                    error if is_access_denied(&error) => {
                        Some(EnvironmentKeyFault::AccessDenied(durable_data_message(
                            "may not be used",
                            key_arn,
                        )))
                    }
                    // Every other answer about the key — including one about a key this
                    // key service holds no record of — establishes nothing.
                    _ => None,
                }
            }
            _ => None,
        },
    }
}

/// Whether `error` is the key service saying this caller may not use the key.
///
/// Read off the error CODE rather than a variant, because an access denial is not one of
/// the modelled exceptions of `DescribeKey`: it arrives as the generic refusal every
/// authorization check answers with.
fn is_access_denied(error: &::aws_sdk_kms::operation::describe_key::DescribeKeyError) -> bool {
    ::aws_sdk_kms::error::ProvideErrorMetadata::code(error)
        .is_some_and(|code| code.contains("AccessDenied"))
}

/// A key-service client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// routes in process, so no network client is configured here. `region` is the Region the
/// invocation was addressed to, which is where the function's key lives.
fn kms_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each fault states what Lambda could not do and why — one row per class of KMS
    /// failure the invocation operations bind.
    #[test]
    fn every_fault_states_the_decrypt_it_could_not_perform() {
        let cases: &[(&str, EnvironmentKeyFault, &str)] = &[
            (
                "disabled",
                EnvironmentKeyFault::Disabled(DISABLED_MESSAGE.to_string()),
                "the KMS key used is disabled",
            ),
            (
                "an invalid state",
                EnvironmentKeyFault::InvalidState(INVALID_STATE_MESSAGE.to_string()),
                "not valid for Decrypt",
            ),
            (
                "not found",
                EnvironmentKeyFault::NotFound(NOT_FOUND_MESSAGE.to_string()),
                "the KMS key was not found",
            ),
            (
                "access denied",
                EnvironmentKeyFault::AccessDenied(ACCESS_DENIED_MESSAGE.to_string()),
                "KMS access was denied",
            ),
        ];
        for (case, fault, expected) in cases {
            let message = match fault {
                EnvironmentKeyFault::Disabled(message)
                | EnvironmentKeyFault::InvalidState(message)
                | EnvironmentKeyFault::NotFound(message)
                | EnvironmentKeyFault::AccessDenied(message) => message.clone(),
            };
            assert!(
                message.starts_with("Lambda was unable to decrypt the environment variables")
                    && message.contains(expected),
                "case {case}: expected the decrypt failure and {expected:?}, got {message:?}"
            );
        }
    }

    /// The key service words a key-state refusal with the key's ARN and the state as prose, so
    /// every `KeyState` value reads as its own lowercased words — one row per state a key can
    /// be in that does not admit a decrypt, plus the one that does.
    #[test]
    fn a_key_state_reads_as_its_own_words() {
        let rows: &[(&str, &str)] = &[
            (STATE_DISABLED, "disabled"),
            ("PendingDeletion", "pending deletion"),
            ("PendingReplicaDeletion", "pending replica deletion"),
            ("PendingImport", "pending import"),
            ("Creating", "creating"),
            ("Unavailable", "unavailable"),
            ("Updating", "updating"),
            (STATE_ENABLED, "enabled"),
        ];
        let key_arn = "arn:aws:kms:us-east-1:000000000000:key/a-key";
        for (state, prose) in rows {
            let reason = key_state_reason(key_arn, state);
            assert_eq!(
                reason,
                format!("{key_arn} is {prose}."),
                "case {state:?}: expected the key and its state as prose, got {reason:?}"
            );
        }
    }

    /// A relayed answer is stated after Lambda's own sentence: the exception class the key
    /// service stated the refusal under and the sentence it stated about the key — and an
    /// answer that stated no sentence contributes none rather than an invented one.
    #[test]
    fn a_relayed_answer_states_the_class_and_the_sentence_it_came_with() {
        let reason = "arn:aws:kms:us-east-1:000000000000:key/a-key is disabled.";
        let answered = RelayedKeyAnswer {
            class: FaultClass::Disabled,
            code: KMS_DISABLED_CODE.to_string(),
            reason: Some(reason.to_string()),
        };
        let reported = report_message(report_sentence(answered.class), &answered);
        assert_eq!(
            reported,
            format!(
                "Lambda was unable to decrypt your environment variables because the KMS key \
                 used is disabled. Please check your KMS key settings. KMS Exception: \
                 {KMS_DISABLED_CODE} KMS Message: {reason}"
            ),
            "case \"a reported disabled key\": got {reported:?}"
        );
        let refused = invocation_message(invocation_sentence(answered.class), &answered);
        assert!(
            refused.starts_with(&format!(
                "{DISABLED_MESSAGE} KMS Exception: {KMS_DISABLED_CODE}KMS Message: {reason} \
                 (Service: Kms, Status Code: 400, Request ID: "
            )) && refused.ends_with(") (SDK Attempt Count: 1)"),
            "case \"a refused invocation\": got {refused:?}"
        );
        let silent = RelayedKeyAnswer {
            class: FaultClass::AccessDenied,
            code: default_code(FaultClass::AccessDenied).to_string(),
            reason: None,
        };
        let reported = report_message(report_sentence(silent.class), &silent);
        assert!(
            reported.ends_with(" KMS Exception: AccessDeniedException"),
            "case \"an answer with no sentence\": expected the class alone, got {reported:?}"
        );
    }
}

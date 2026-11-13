// SPDX-License-Identifier: CC0-1.0

//! The ARN of the Amazon Web Services Secrets Manager secret an Amazon EVS request
//! named.
//!
//! A connector's credential secret is named on input by EITHER form — `secretIdentifier`
//! is "The ARN or name of the Amazon Web Services Secrets Manager secret", and the
//! modelled `SecretIdentifier` pattern spells both alternatives — while the member that
//! reports it back, `Connector$secretArn`, is "The Amazon Resource Name (ARN) of" that
//! secret. The input is therefore an identifier and the output is an ARN, so the
//! identifier is RESOLVED to an ARN before it is stored, and no connector ever reports a
//! bare name.
//!
//! An identifier that already is an ARN names the secret the caller meant and is reported
//! unchanged — rewriting it would report a different secret. A NAME is resolved the way
//! every other ARN of this emulator is built, from the Region the request was addressed to
//! and the account ([`::engine::emulator_arn`], as [`super::environment_arn`] uses it):
//! `arn:aws:secretsmanager:<region>:<account>:secret:<name>`. That is the PARTIAL-ARN form
//! of a secret — the ARN without the hyphen and six random characters Secrets Manager
//! appends when it creates one — which is a form Secrets Manager itself accepts as a
//! `SecretId`, so the value a connector reports is an ARN the caller can use directly.
//!
//! Resolution is a pure function of the request: it reads no store, writes nothing and
//! takes no lock, so one name reports one ARN on every call and for every operation.
//! Whether a secret by that name exists at all is a different question — refusing an
//! identifier that names no secret belongs to the validation of these operations' inputs,
//! not to the value this member reports.
//!
//! That validation is the other half of this module
//! ([`credential_secret_failures`]). A connector "establishes a connection to the given
//! appliance type using the credentials stored in Amazon Web Services Secrets Manager"
//! (`Connector` doc), and the secret storing them is one the ACCOUNT holds — Amazon EVS
//! creates none — so whether it exists is Secrets Manager's fact to state and is asked of
//! that service's OWN store accessor, exactly as [`super::ec2_resources`] asks Amazon EC2
//! about a VPC or a Dedicated Host. The per-type requirement on what the secret stores is
//! asked of the same record: "SDDC_MANAGER requires an apiKey field; OPERATIONS_MANAGER
//! and VCENTER require username and password fields"
//! (`CreateEnvironmentConnectorRequest$secretIdentifier` doc), so the secret has to state
//! the fields the chosen type names before a connection can be established with it.
//!
//! A store this emulator cannot read at all — an engine that is not the in-memory one —
//! is EVIDENCE OF NOTHING and refuses nothing, the reading [`super::ec2_resources`] makes
//! of the same case; so is a secret whose value is sealed under a customer managed key,
//! which no store read opens.
//!
//! LOCKING. The Secrets Manager store's lock is taken for the shortest span that answers
//! the question, nothing is awaited under it, and the caller holds this service's own
//! guard across the call, so the order is always EVS-then-Secrets Manager; Secrets Manager
//! never reaches this service, so no cycle exists.

use crate::ops::types::ValidationExceptionField;

use ::service_com_amazonaws_secretsmanager_secretsmanager::state as secrets;

/// Whether `secret_identifier` is in the ARN form of the modelled `SecretIdentifier`
/// rather than the bare-name form.
///
/// The pattern's two alternatives are `arn:aws:secretsmanager:<region>:<account>:secret:…`
/// and a name drawn from `[a-zA-Z0-9/_+=.@!-]`, which admits no colon — so a value
/// carrying an ARN's prefix is the ARN alternative and every other value is a name. The
/// test is the prefix and not the whole ARN grammar, because a value that already carries
/// one is reported as the caller wrote it: nesting it inside a newly built ARN would name
/// nothing.
fn is_arn_form(secret_identifier: &str) -> bool {
    secret_identifier.starts_with("arn:")
}

/// The ARN of the secret `secret_identifier` names, for `Connector$secretArn`: the
/// identifier itself when it already is an ARN, and otherwise the ARN of that name in
/// `region` and this account.
pub fn resolve_secret_arn(region: &str, secret_identifier: &str) -> String {
    if is_arn_form(secret_identifier) {
        return secret_identifier.to_string();
    }
    ::engine::emulator_arn(
        "secretsmanager",
        region,
        &format!("secret:{secret_identifier}"),
    )
}

/// The `apiKey` field an `SDDC_MANAGER` connector's secret stores.
const FIELD_API_KEY: &str = "apiKey";

/// The `username` field an `OPERATIONS_MANAGER` or `VCENTER` connector's secret stores.
const FIELD_USERNAME: &str = "username";

/// The `password` field those two also store.
const FIELD_PASSWORD: &str = "password";

/// The fields the credential secret of a connector of `connector_type` has to store:
/// "SDDC_MANAGER requires an apiKey field; OPERATIONS_MANAGER and VCENTER require
/// username and password fields"
/// (`CreateEnvironmentConnectorRequest$secretIdentifier` doc).
///
/// The requirement is the SECRET's, not one operation's, so the create and the update of a
/// connector read it from here — the create for the type the request names, the update for
/// the type the stored connector carries. A value outside the modelled `ConnectorType`
/// names no appliance type and so no required field; the boundary validator refuses one
/// before a handler sees it.
pub fn required_credential_fields(connector_type: &str) -> &'static [&'static str] {
    match connector_type {
        super::CONNECTOR_TYPE_SDDC_MANAGER => &[FIELD_API_KEY],
        super::CONNECTOR_TYPE_OPERATIONS_MANAGER | super::CONNECTOR_TYPE_VCENTER => {
            &[FIELD_USERNAME, FIELD_PASSWORD]
        }
        _ => &[],
    }
}

/// The account's Secrets Manager store in the shared engine, through that service's own
/// accessor.
fn secrets_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<secrets::ComAmazonawsSecretsmanagerSecretsmanagerState>> {
    secrets::service_state(engine)
}

/// The Region the secret `secret_identifier` names is held in: the Region segment of an
/// ARN, and the Region the request was addressed to for a bare name.
///
/// A secret is identified within one Region — the same name in another Region is another
/// secret — and an identifier that IS an ARN names the secret the caller meant, Region
/// included, which is also the secret the connector reports
/// ([`resolve_secret_arn`]). So the store is asked about the secret wherever its own
/// identifier says it lives, rather than only about the Region this request came in at.
fn secret_region<'a>(region: &'a str, secret_identifier: &'a str) -> &'a str {
    if !is_arn_form(secret_identifier) {
        return region;
    }
    // `arn:<partition>:secretsmanager:<region>:<account>:secret:<name>`: the Region is the
    // fourth field. An ARN that carries no fourth field names no Region of its own.
    match secret_identifier.split(':').nth(3) {
        Some(named) if !named.is_empty() => named,
        _ => region,
    }
}

/// The clause stated for a `secretIdentifier` that names no secret the account holds in
/// the Region the identifier names, or one marked for deletion, whose value no retrieval
/// returns.
///
/// `member` is the PATH of the member that named it. Stated in the same form as the
/// clauses [`super::ec2_resources`] states for an Amazon EC2 resource the account does not
/// hold, because it is the same kind of failure: a member naming a resource of the
/// caller's account that Amazon EVS does not create and cannot find.
pub fn invalid_secret_clause(member: &str, secret_identifier: &str) -> ValidationExceptionField {
    super::field_failure_stated(
        member,
        format!(
            "The secret {secret_identifier} given for {member} is not valid or does not \
             exist. Please verify the secret exists and is valid."
        ),
    )
}

/// The clause stated for a secret that exists but does not store every field the
/// connector's type requires ([`required_credential_fields`]).
///
/// It names the member, the secret as the request spelled it, the fields that are missing
/// and the type that requires them — all of it from the request and the fields the stored
/// secret states, never from what any field holds.
pub fn incomplete_secret_clause(
    member: &str,
    secret_identifier: &str,
    connector_type: &str,
    missing: &[&str],
) -> ValidationExceptionField {
    let fields = if missing.len() == 1 { "field" } else { "fields" };
    super::field_failure_stated(
        member,
        format!(
            "The secret {secret_identifier} given for {member} does not store the \
             {listed} {fields} that a {connector_type} connector requires. Please \
             verify the secret stores the required fields.",
            listed = join_and(missing),
        ),
    )
}

/// `missing` as a clause lists field names: `a`, `a and b`, `a, b and c`.
fn join_and(names: &[&str]) -> String {
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, head)) => format!("{} and {last}", head.join(", ")),
    }
}

/// Every failure of the precondition that a connector's credential secret is one the
/// connection can really be established with: the account holds it, and it stores the
/// fields the connector's type requires.
///
/// "A connector establishes a connection to the given appliance type using the credentials
/// stored in Amazon Web Services Secrets Manager" (`Connector` doc), so a
/// `secretIdentifier` naming no secret of the account names no credentials, and one whose
/// secret does not state the fields its type requires
/// ([`required_credential_fields`]) names credentials the appliance type cannot be reached
/// with. Either way there are no credentials to connect with, so the request is refused
/// rather than answered with a connector that reaches `ACTIVE`.
///
/// Reported as a list, and in the member path the caller wrote, so the create and the
/// update state this one rule with their own inputs and alongside whatever else they
/// refuse.
///
/// Nothing is refused on evidence that is not there: a Secrets Manager store this emulator
/// cannot read answers nothing, and neither does a secret whose value is sealed under a
/// customer managed key, which only a KMS call opens
/// (`secrets::Secret::stated_field_names`).
pub fn credential_secret_failures<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    member: &str,
    secret_identifier: &str,
    connector_type: &str,
) -> Vec<ValidationExceptionField> {
    let Some(state) = secrets_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    // A secret marked for deletion is one no retrieval of its value succeeds on, so the
    // credentials it held are no longer there to connect with.
    let Some(secret) = store
        .get(secret_region(region, secret_identifier), secret_identifier)
        .filter(|secret| !secret.scheduled_for_deletion())
    else {
        return vec![invalid_secret_clause(member, secret_identifier)];
    };
    let Some(stated) = secret.stated_field_names() else {
        return Vec::new();
    };
    let missing: Vec<&str> = required_credential_fields(connector_type)
        .iter()
        .copied()
        .filter(|required| !stated.iter().any(|stated| stated == required))
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    vec![incomplete_secret_clause(
        member,
        secret_identifier,
        connector_type,
        &missing,
    )]
}

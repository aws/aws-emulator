// SPDX-License-Identifier: CC0-1.0

//! The stored user pool: the record `CreateUserPool` mints, every read renders and
//! every pool-level write updates, with the configuration it holds.

use super::*;

/// One alphabet per character class a password policy can require — uppercase,
/// lowercase, a digit and a symbol (`PasswordPolicyType.RequireUppercase` /
/// `RequireLowercase` / `RequireNumbers` / `RequireSymbols`, this service's model) —
/// so a password drawing one character from each satisfies every combination of
/// those requirements ([`UserPool::generated_temporary_password`]).
///
/// The symbols are drawn from the printable, non-alphanumeric characters a policy
/// counts as symbols; none of them is whitespace, which `PasswordType` forbids.
const PASSWORD_CLASS_ALPHABETS: [&[u8]; 4] = [
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    b"abcdefghijklmnopqrstuvwxyz",
    b"0123456789",
    b"!#$%&*+-=?@^_~",
];

/// The longest password there is: `PasswordType` is `len 0..256` (this service's model), so
/// a generated one is never built longer than that however long a stored policy asks for
/// ([`UserPool::generated_temporary_password`]).
const MAX_PASSWORD_LENGTH: usize = 256;

/// One stored user pool, with the app clients and users it owns.
#[derive(Debug, Clone)]
pub struct UserPool {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub region: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub policies: UserPoolPolicyType,
    /// This pool's Lambda triggers, EMPTY for a pool that configures none: the
    /// service materialises the configuration every pool applies, so this is
    /// stored rather than left absent ([`effective_lambda_config`]).
    pub lambda_config: LambdaConfigType,
    pub mfa_configuration: String,
    pub deletion_protection: String,
    pub user_pool_tier: String,
    pub username_attributes: Vec<String>,
    pub alias_attributes: Vec<String>,
    pub auto_verified_attributes: Vec<String>,
    pub schema: Vec<SchemaAttributeType>,
    pub admin_create_user_config: AdminCreateUserConfigType,
    pub username_configuration: Option<UsernameConfigurationType>,
    pub device_configuration: Option<DeviceConfigurationType>,
    /// How this pool sends its email messages ([`effective_email_configuration`]).
    pub email_configuration: EmailConfigurationType,
    pub sms_configuration: Option<SmsConfigurationType>,
    /// The templates this pool's verification messages are rendered from
    /// ([`effective_verification_message_template`]).
    pub verification_message_template: VerificationMessageTemplateType,
    pub account_recovery_setting: Option<AccountRecoverySettingType>,
    /// Which attribute changes wait on verification
    /// ([`effective_user_attribute_update_settings`]).
    pub user_attribute_update_settings: UserAttributeUpdateSettingsType,
    pub user_pool_add_ons: Option<UserPoolAddOnsType>,
    /// The key this pool's data is encrypted under
    /// ([`effective_key_configuration`]).
    pub key_configuration: KeyConfigurationType,
    /// The issuer this pool's tokens carry ([`effective_issuer_configuration`]).
    pub issuer_configuration: IssuerConfigurationType,
    /// The ENDPOINT of this emulator this pool publishes its token issuer on
    /// ([`UserPool::issuer`]), as the `host[:port]` a client dials — the authority the
    /// issuer the pool announces carries. `None` for a
    /// pool created through a client that resolves AWS's own hostnames on an
    /// emulator that is served on no port at all, which is the one caller that can
    /// reach the host the issuer's documented form names.
    ///
    /// Settled by the create ([`issuer_endpoint_authority`]) and never rewritten by
    /// a read, because the `iss` claim of a token is what a verifier compares
    /// against the issuer it was configured with: a pool announces ONE issuer for
    /// its whole life, however each later caller addresses this emulator.
    pub issuer_endpoint: Option<String>,
    /// The custom names this pool reports for the four fixed authentication
    /// context class reference levels, ALWAYS all four: the levels are fixed and
    /// only their names are configurable, so a pool that overrides none of them
    /// still holds the service's default name for each
    /// ([`effective_acr_configuration`]).
    pub acr_configuration: HashMap<String, AcrLevelConfigType>,
    pub sms_authentication_message: Option<String>,
    /// This pool's TOTP MFA configuration: "Settings for time-based one-time
    /// password (TOTP) multi-factor authentication (MFA) in a user pool"
    /// (`SoftwareTokenMfaConfigType`). `None` until a call configures TOTP, so
    /// a pool that never did reports no TOTP configuration.
    pub software_token_mfa_configuration: Option<SoftwareTokenMfaConfigType>,
    /// This pool's email-message MFA configuration: the subject and body of the
    /// message template sign-in and MFA codes are delivered with
    /// (`EmailMfaConfigType`). `None` until a call configures it.
    pub email_mfa_configuration: Option<EmailMfaConfigType>,
    /// This pool's passkey configuration: relying-party id, user-verification
    /// requirement and whether a passkey satisfies MFA
    /// (`WebAuthnConfigurationType`). `None` until a call configures it.
    pub web_authn_configuration: Option<WebAuthnConfigurationType>,
    /// This pool's threat-protection settings, once a call has set them:
    /// `RiskConfigurationType.ClientId` is "the app client where this
    /// configuration is applied. When this parameter isn't present, the risk
    /// configuration applies to all user pool app clients that don't have
    /// client-level settings" — so this is the pool-level configuration, the one
    /// every app client of the pool that holds none of its own uses. `None`
    /// until a call sets it, so a pool nobody configured reports no settings and
    /// no modification instant.
    pub risk_configuration: Option<RiskConfiguration>,
    /// This pool's log delivery configuration: the `LogConfigurations` a
    /// `SetLogDeliveryConfiguration` call last stored, which is what
    /// `GetLogDeliveryConfiguration` reports. Empty for a pool nothing has
    /// configured logging for, and empty again once a call sets an empty
    /// collection — `LogDeliveryConfigurationType.LogConfigurations` is a
    /// modelled required member, so the collection is always reported, empty or
    /// not.
    pub log_configurations: Vec<LogConfigurationType>,
    pub tags: HashMap<String, String>,
    /// This pool's Amazon Cognito prefix domain, once one is created:
    /// `UserPoolType.Domain` is "The domain prefix, if the user pool has a
    /// domain associated with it" — a single value, so a pool holds at most one.
    pub domain: Option<UserPoolDomain>,
    /// This pool's custom domain, once one is created: `UserPoolType.CustomDomain`
    /// is a single value too, so a pool holds at most one of those as well.
    pub custom_domain: Option<UserPoolDomain>,
    /// This pool's SECONDARY replicas, keyed by the Region each was created in
    /// — one Region holds at most one replica of a pool, and the key is what
    /// `UpdateUserPoolReplica` and `DeleteUserPoolReplica` name. Ascending by
    /// Region, which is the order a listing of them serves. The PRIMARY replica
    /// is the pool itself in [`UserPool::region`], so it is not stored a second
    /// time here.
    pub replicas: BTreeMap<String, UserPoolReplica>,
    pub clients: HashMap<String, UserPoolClient>,
    /// Users keyed by [`UserPool::user_key`], so a pool configured
    /// case-insensitive treats `Alice` and `alice` as one user.
    pub users: HashMap<String, User>,
    /// This pool's groups keyed by `GroupName`, ordered so a read of several is
    /// stable. `GroupNameType` carries no case-insensitivity trait, so a name is
    /// matched exactly as it was created.
    pub groups: BTreeMap<String, Group>,
    /// This pool's OAuth 2.0 resource servers keyed by their `Identifier`,
    /// ordered so a read of several is stable. `ResourceServerIdentifierType`
    /// carries no case-insensitivity trait, so an identifier is matched exactly
    /// as it was created.
    pub resource_servers: BTreeMap<String, ResourceServer>,
    /// This pool's third-party identity providers keyed by their `ProviderName`,
    /// ordered so a read of several is stable. `ProviderNameType` carries no
    /// case-insensitivity trait, so a name is matched exactly as it was created.
    pub identity_providers: BTreeMap<String, IdentityProvider>,
    /// This pool's terms documents keyed by their minted `TermsId`, ordered so a
    /// read of several is stable. A terms document belongs to one app client of
    /// this pool, and "terms document names must be unique to the app client"
    /// (`TermsExistsException` in this service's model), so the (`ClientId`,
    /// `TermsName`) pair is unique across this map too. No separate bound is
    /// needed: `TermsNameType` admits two names and a document names a stored
    /// app client, so this map holds at most twice [`MAX_CLIENTS_PER_POOL`].
    pub terms: BTreeMap<String, Terms>,
    /// This pool's managed login branding styles keyed by their minted
    /// `ManagedLoginBrandingId`, ordered so a read of several is stable. A style
    /// "is linked to an app client until you delete it"
    /// (`CreateManagedLoginBranding.ClientId`) and an app client "that already
    /// has an assigned style" refuses a second one
    /// (`ManagedLoginBrandingExistsException`), so `ClientId` identifies at most
    /// one entry of this map and it holds at most [`MAX_CLIENTS_PER_POOL`]
    /// styles.
    pub managed_login_branding: BTreeMap<String, ManagedLoginBranding>,
    /// This pool's classic hosted UI branding keyed by the app client it
    /// customizes, with [`UI_CUSTOMIZATION_ALL_CLIENTS`] holding the pool-level
    /// default: "Set the default configuration for all clients with a ClientId
    /// of ALL. When the ClientId value is an app client ID, the settings you
    /// pass in this request apply to that app client and override the default
    /// ALL configuration" (`SetUICustomization`), so one entry per level and at
    /// most one more than [`MAX_CLIENTS_PER_POOL`]. Ordered, so a read of
    /// several is stable.
    pub ui_customizations: BTreeMap<String, UiCustomization>,
    /// This pool's user import jobs keyed by their minted `JobId`, ordered so a
    /// read of several is stable: "Import jobs are retained in user pool
    /// configuration so that you can stage, stop, start, review, and delete
    /// them" (`ListUserImportJobs`).
    pub user_import_jobs: BTreeMap<String, UserImportJob>,
    /// The key pairs this pool's tokens are signed with and whose public halves
    /// its key set publishes — one per token type — minted with the pool and kept
    /// for its lifetime, so every token the pool ever issued stays verifiable
    /// against the key set it publishes now.
    pub signing_keys: PoolSigningKeys,
    /// The X.509 certificate this pool signs its SAML 2.0 authentication
    /// requests with, as `GetSigningCertificate` reports it — issued to this
    /// pool when the pool was created ([`issue_signing_certificate`]), over one
    /// of the signing keys its key set publishes, and kept for its lifetime, so
    /// every read of one pool answers the same certificate and the identity
    /// provider an operator installed it at keeps verifying against it.
    ///
    /// `None` only when the certificate did not issue, which the operation that
    /// reports it answers its modelled server error for rather than reporting a
    /// certificate nothing signed.
    pub saml_signing_certificate: Option<String>,
    /// The `DeviceGroupKey` every device of this pool is reported under:
    /// "an identifier used in generating the `DEVICE_PASSWORD_VERIFIER` for
    /// device SRP authentication" (`NewDeviceMetadataType.DeviceGroupKey`).
    /// Minted with the pool and kept for its lifetime, so two sign-ins of the
    /// same pool report the same group key.
    pub device_group_key: String,
}

/// A user pool's MFA and passkey configuration, as the two operations that
/// report it render it: `MfaConfiguration` beside the configuration of each
/// factor the pool offers.
///
/// Built by [`UserPool::to_mfa_config`] from what the pool holds. It exists
/// because `GetUserPoolMfaConfig` and `SetUserPoolMfaConfig` model these same
/// five members in two distinct generated output structs, which would otherwise
/// assemble the stored configuration twice.
#[derive(Debug, Clone)]
pub struct MfaConfig {
    pub sms_mfa_configuration: Option<SmsMfaConfigType>,
    pub software_token_mfa_configuration: Option<SoftwareTokenMfaConfigType>,
    pub email_mfa_configuration: Option<EmailMfaConfigType>,
    pub mfa_configuration: String,
    pub web_authn_configuration: Option<WebAuthnConfigurationType>,
}

/// The threat-protection settings stored at one level — a user pool, or one app
/// client of it — beside the instant the last call that set them ran.
///
/// The three settings are exactly the ones `SetRiskConfiguration` carries, and
/// `RiskConfigurationType` reports them back with the level they were stored at
/// (the pool id alone, or the pool id and the app client id) and
/// `LastModifiedDate`, "the date and time when the item was modified".
///
/// A setting this struct holds as `None` is one no stored call configured: the
/// operation SETS the whole configuration of the level it names, so a request
/// that carries none of the three leaves a configuration with nothing in it —
/// which is what "to set the risk configuration for the user pool to defaults,
/// send this request with only the `UserPoolId` parameter" asks for.
#[derive(Debug, Clone)]
pub struct RiskConfiguration {
    pub compromised_credentials_risk_configuration:
        Option<CompromisedCredentialsRiskConfigurationType>,
    pub account_takeover_risk_configuration: Option<AccountTakeoverRiskConfigurationType>,
    pub risk_exception_configuration: Option<RiskExceptionConfigurationType>,
    pub last_modified_date: SystemTime,
}

impl RiskConfiguration {
    /// The stored settings as the two operations that report them render them:
    /// `user_pool_id` and `client_id` name the level they are stored at, so a
    /// client-level configuration carries the app client it applies to and a
    /// pool-level one carries none.
    pub fn to_risk_configuration_type(
        &self,
        user_pool_id: &str,
        client_id: Option<&str>,
    ) -> RiskConfigurationType {
        RiskConfigurationType {
            user_pool_id: Some(user_pool_id.to_string()),
            client_id: client_id.map(str::to_string),
            compromised_credentials_risk_configuration: self
                .compromised_credentials_risk_configuration
                .clone(),
            account_takeover_risk_configuration: self.account_takeover_risk_configuration.clone(),
            risk_exception_configuration: self.risk_exception_configuration.clone(),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

/// The attributes a pool that compares usernames WITHOUT case compares without
/// case too.
///
/// Setting `UsernameConfiguration.CaseSensitive` to `false` "also enables both
/// `preferred_username` and `email` alias to be case insensitive, in addition to
/// the `username` attribute" (this service's model), so these are the two values
/// beside the username itself that name their holder in any capitalization —
/// whether the holder is being resolved ([`UserPool::names_attribute_value`]) or
/// searched for (`ListUsers.Filter`). The ONE place the set is named, so the
/// lookups and the server-side search cannot disagree about it.
pub const CASE_INSENSITIVE_ALIAS_ATTRIBUTES: [&str; 2] = ["email", "preferred_username"];

/// The value `name` names, as a sentence that reports a submitted sign-in name
/// of the wrong shape names it: an address for `email` and a phone number for
/// `phone_number`, the two values `UsernameAttributeType` admits
/// (`smithy_query(query="shape",shape="UsernameAttributeType")`), and nothing
/// for a name that is neither.
fn sign_in_value_wording(name: &str) -> Option<&'static str> {
    match name {
        "email" => Some("an email"),
        "phone_number" => Some("a phone number"),
        _ => None,
    }
}

/// The sentence a creation under a name of no configured sign-in attribute's
/// shape is refused with, for a pool that signs its users in with
/// `username_attributes`, or `None` for a list that names no sign-in value at
/// all.
///
/// `UsernameAttributes` "specifies whether a user can use an email address or
/// phone number as a username when they sign up" (this service's model), so the
/// refusal states the value the pool's own configuration asks for: one sign-in
/// attribute names its value, and a pool that admits both names either of them.
/// Built from the STORED configuration rather than from the request, so the
/// sentence reports the rule the pool enforces and echoes nothing the caller
/// sent.
fn sign_in_name_shape_message(username_attributes: &[String]) -> Option<String> {
    let mut wordings: Vec<&'static str> = username_attributes
        .iter()
        .filter_map(|name| sign_in_value_wording(name))
        .collect();
    wordings.dedup();
    match wordings.as_slice() {
        [] => None,
        [one] => Some(format!("Username should be {one}.")),
        _ => Some(format!("Username should be either {}.", wordings.join(" or "))),
    }
}

/// The alias attribute whose values are NAMES a user signs in with rather than
/// addresses: `preferred_username`, the third value of `AliasAttributeType`
/// (`smithy_query(query="shape",shape="AliasAttributeType")`), beside the `email`
/// and `phone_number` whose values have a format of their own
/// ([`alias_format_wording`]).
pub const PREFERRED_USERNAME_ATTRIBUTE: &str = "preferred_username";

/// The sentence a SELF-SERVICE registration that submits a
/// `preferred_username` to a pool that ALIASES that attribute
/// ([`PREFERRED_USERNAME_ATTRIBUTE`]) is refused with.
///
/// An alias value names ONE user of the pool, and a registration creates an
/// account that is not confirmed yet — "their user account becomes active" only
/// once the registrant answers the code (`ConfirmSignUp` in this service's
/// model) — so an unconfirmed registrant cannot be handed a name the pool
/// aliases: it would hold the alias against every later owner of it. Amazon
/// Cognito states that as the stage it refuses at and the pool configuration
/// that refused it, in fixed text that echoes nothing the caller sent.
pub const SIGN_UP_PREFERRED_USERNAME_ALIAS_MESSAGE: &str =
    "Preferred username cannot be provided for unconfirmed account, since user pool is \
     configured for preferred username alias.";

/// The sentence a request whose submitted `Username` has the FORM of a value of
/// one of the pool's alias attributes is refused with, for the alias attribute
/// `attribute`, or `None` for an attribute whose values have no form of their own.
///
/// Amazon Cognito states the format the name has and the alias the pool is
/// configured for, both named by the attribute the pool configures: the whole
/// sentence is built from the POOL's configuration, so it echoes nothing the
/// caller sent.
fn alias_format_username_message(attribute: &str) -> Option<String> {
    let format = alias_format_wording(attribute)?;
    Some(format!(
        "Username cannot be of {format} format, since user pool is configured for {format} alias."
    ))
}

/// The FORMAT `name`'s values have, as the refusal of a username of that format
/// names it: `email` values are of email format and `phone_number` values of
/// phone number format — the two `AliasAttributeType` values whose values carry a
/// form of their own (`smithy_query(query="shape",shape="AliasAttributeType")`,
/// whose third value `preferred_username` holds a name like any other) — and
/// nothing for an attribute that is neither.
fn alias_format_wording(name: &str) -> Option<&'static str> {
    match name {
        "email" => Some("email"),
        "phone_number" => Some("phone number"),
        _ => None,
    }
}

/// The tag-key namespace Amazon Web Services keeps for the tags it writes
/// itself, which a caller of this service may neither assign nor remove
/// ([`SYSTEM_TAG_MUTATION_MESSAGE`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies inside the namespace [`RESERVED_TAG_KEY_PREFIX`] keeps,
/// compared without regard to letter case: the namespace is reserved however a
/// request spells it, so no capitalization of the prefix is a way into it.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The message a request that would write or remove a tag key inside the
/// reserved namespace is refused with.
///
/// Those tags are the system's own, so a caller of the API is not the writer of
/// them: Amazon Cognito states exactly that, in fixed text that names neither
/// the key the request carried nor the resource it addressed.
pub const SYSTEM_TAG_MUTATION_MESSAGE: &str =
    "Caller is an end user and not allowed to mutate system tags";

/// The message a tagging request whose `ResourceArn` is not the ARN of a user
/// pool is refused with.
///
/// `ArnType` constrains an ARN's length and general shape, but WHICH resource an
/// ARN names is this service's own rule: it reads the value as a user-pool ARN
/// ([`crate::state::user_pool_arn_parts`]) and reports a value it cannot read
/// one out of as the invalid ARN it is. The fixed text is `Invalid user pool
/// ARN: ` and the one variable part is the value the request submitted, so the
/// caller sees which ARN was refused.
pub fn invalid_user_pool_arn_message(resource_arn: &str) -> String {
    format!("Invalid user pool ARN: {resource_arn}")
}

/// The DNS zone this service's endpoints and token issuers are published in.
const ENDPOINT_DNS_SUFFIX: &str = "amazonaws.com";

/// The label an `UPDATED` issuer's host carries in front of this service's
/// endpoint prefix: "updated issuers have the format of
/// `https://issuer-cognito-idp.[region].amazonaws.com/[userPoolId]`"
/// (`IssuerConfigurationType.Type` in this service's model).
const UPDATED_ISSUER_HOST_LABEL_PREFIX: &str = "issuer-";

/// The LABELS a user-pool issuer host carries in front of its DNS zone, for a pool
/// of `region` configured with the issuer type `issuer_type`: this service's endpoint
/// prefix and the pool's own Region.
///
/// This is the form AWS's OWN endpoint host carries, which is what a verifier configured
/// from the documentation holds ([`is_pool_issuer`] reads it in whichever zone it is
/// spelled). It is NOT a name this emulator announces: a host of this form resolves
/// nowhere near this process, and an issuer is a URL a relying party dials
/// ([`UserPool::issuer`]).
///
/// An issuer is hosted in the POOL's own Region and names this service: "the
/// issuer URL is hosted in the user pool's region and provides OIDC endpoints
/// specific to that region. Original issuers have the format of
/// `https://cognito-idp.[region].amazonaws.com/[userPoolId]`", while an
/// `UPDATED` pool's issuer carries the further host label
/// ([`UPDATED_ISSUER_HOST_LABEL_PREFIX`]) "where region is the primary Amazon
/// Web Services Region of your user pool" (`IssuerConfigurationType.Type` in
/// this service's model). A type this service does not know carries the labels the
/// default one does ([`DEFAULT_ISSUER_TYPE`]), since a pool's tokens always carry
/// one issuer.
fn issuer_host_labels(region: &str, issuer_type: &str) -> String {
    let label_prefix = if issuer_type == UPDATED_ISSUER_TYPE {
        UPDATED_ISSUER_HOST_LABEL_PREFIX
    } else {
        ""
    };
    format!("{label_prefix}{SIGNING_NAME}.{region}")
}

/// The host a user pool of `region` publishes its token issuer on for a client that
/// resolves AWS's OWN hostnames: the documented form ([`issuer_host_labels`]) in the zone
/// AWS serves its endpoints in ([`ENDPOINT_DNS_SUFFIX`]).
fn issuer_host(region: &str, issuer_type: &str) -> String {
    format!(
        "{}.{ENDPOINT_DNS_SUFFIX}",
        issuer_host_labels(region, issuer_type)
    )
}

/// The endpoint a pool created by the request being served publishes its token
/// ISSUER on: the `host[:port]` of THIS emulator, which is the authority the issuer
/// is announced at ([`UserPool::issuer`]), since the issuer is a URL a
/// relying party fetches later — a verifier reads the key set from
/// `<iss>/.well-known/jwks.json` — and a name this emulator announces has to
/// lead back to this emulator.
///
/// It is the endpoint the request itself reached when that endpoint is the
/// emulator's own ([`::service_common::data_plane_addressing::reached_emulator_endpoint`]),
/// which is a client that resolves nothing else; otherwise the one address this
/// process answers on
/// ([`::service_common::data_plane_addressing::served_endpoint_authority`]), so a
/// pool minted by an in-process call of another service publishes an address that
/// service's caller can reach too. `None` only for an emulator served on no port
/// at all, where there is no endpoint to publish and the pool falls back to the
/// host its documented form names ([`issuer_host`]).
///
/// Both readings are the workspace's shared ones
/// ([`::service_common::data_plane_addressing::reached_emulator_endpoint_of_parts`] and
/// [`::service_common::data_plane_addressing::served_endpoint_authority`]), so this service
/// writes no second rule for which host is the emulator's own.
#[must_use]
pub fn issuer_endpoint_authority(
    parts: &::http::request::Parts,
    served_base: &str,
) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint_of_parts(parts)
        .or_else(|| ::service_common::data_plane_addressing::served_endpoint_authority(served_base))
}

/// Whether `issuer` is a token issuer of the user pool `pool_id` of `region` —
/// the reading every verifier of a user-pool token shares, here and in the
/// services that resolve one (an API Gateway `COGNITO_USER_POOLS` authorizer, a
/// policy store's user-pool identity source, an identity pool's login, a
/// web-identity federation).
///
/// `issuer` names the pool when its whole path is that pool's id and its
/// authority is one a pool of that Region answers at ([`serves_pool`]): a host carrying the
/// issuer FORM such a pool publishes ([`names_a_pool_issuer_host`]), which is what a verifier
/// configured from AWS's own documentation holds, or an address this emulator is reached at,
/// which is what one configured from this emulator's endpoint holds.
/// Everything else is a FOREIGN issuer, which names this pool no more than it
/// names any other — and what proves a token is the pool's own remains its
/// signature against the key set the pool publishes.
pub fn is_pool_issuer(region: &str, pool_id: &str, issuer: &str) -> bool {
    let Some((origin, named)) = issuer.trim().trim_end_matches('/').rsplit_once('/') else {
        return false;
    };
    if named != pool_id {
        return false;
    }
    let Some(authority) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
    else {
        return false;
    };
    serves_pool(region, pool_id, authority)
}

/// Whether `authority` is an address the pool `pool_id` of `region` answers at: a host
/// carrying the issuer FORM a pool of that Region publishes ([`names_a_pool_issuer_host`], in
/// whichever DNS zone it is published), or a host-shaped address naming NO service, which is
/// how the emulator's own endpoint is addressed — an operator binds it wherever they
/// like and a client on this machine resolves nothing else.
///
/// An ADDRESS carries no Region label, so what names the Region there is the POOL's own id
/// ([`id_names_region`]): `<region>_<suffix>` is the form every `UserPoolId` is minted in, so
/// an id naming another Region than the one asked about names no pool of this one, exactly as
/// an issuer host carrying another Region does.
///
/// An authority naming a DIFFERENT service's endpoint — including this service's
/// endpoint in another region, where the pool does not live — is served by that
/// service and not by this pool.
fn serves_pool(region: &str, pool_id: &str, authority: &str) -> bool {
    names_a_pool_issuer_host(region, authority)
        || (id_names_region(pool_id, region)
            && ::service_common::data_plane_addressing::is_host_shaped(authority)
            && !::service_common::data_plane_addressing::names_a_service(authority))
}

/// Whether `pool_id` is the id of a pool of `region`.
///
/// A `UserPoolId` is `<region>_<suffix>` — the form this service mints one in, and the form
/// the modelled pattern `^[\w-]+_[0-9a-zA-Z]+$` admits, whose first half is the Region — so
/// the id itself states which Region its pool lives in. The separator is the LAST underscore,
/// because a Region may be spelled with one. Compared without regard to letter case, since a
/// Region names the same Region however it is spelled.
fn id_names_region(pool_id: &str, region: &str) -> bool {
    pool_id
        .rsplit_once('_')
        .is_some_and(|(named, _)| named.eq_ignore_ascii_case(region))
}

/// Whether `authority`'s host is an ISSUER HOST of a pool of `region`: the labels such a
/// host carries ([`issuer_host_labels`], one spelling per issuer type) followed by a DNS
/// zone of at least one label.
///
/// The ZONE is not part of the question. One pool publishes its issuer in exactly one zone,
/// but the holder of a token and the verifier configured for the pool may each name it in
/// the other's — AWS's own (`amazonaws.com`) or the zone this emulator answers in — and both
/// name the same pool of the same Region. What the host must carry is this service's
/// endpoint prefix and the pool's Region; a host carrying another service's prefix, or
/// another Region, names no pool of this one.
fn names_a_pool_issuer_host(region: &str, authority: &str) -> bool {
    // A value carrying anything a `host[:port]` cannot — a path segment above all — is no
    // host, so it names no issuer host however its labels read.
    if !::service_common::data_plane_addressing::is_host_shaped(authority) {
        return false;
    }
    let host = ::service_common::data_plane_addressing::host_only(authority)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    ISSUER_TYPE_VALUES.iter().any(|issuer_type| {
        let labels = format!("{}.", issuer_host_labels(region, issuer_type)).to_ascii_lowercase();
        host.strip_prefix(labels.as_str())
            .is_some_and(|zone| !zone.is_empty() && !zone.split('.').any(str::is_empty))
    })
}

impl UserPool {
    /// The issuer type this pool's tokens are issued under: the one it is
    /// configured with ([`effective_issuer_configuration`], which materialises
    /// one for every pool), and [`DEFAULT_ISSUER_TYPE`] for a configuration
    /// naming none.
    pub fn issuer_type(&self) -> &str {
        self.issuer_configuration
            .r#type
            .as_deref()
            .unwrap_or(DEFAULT_ISSUER_TYPE)
    }

    /// This pool's TOKEN ISSUER: the user-pool issuer URL it publishes, with this
    /// pool's id as the whole path.
    ///
    /// The HOST is the one thing the deployment decides, and the issuer is a URL the
    /// holder of a token DIALS — it is the discovery root a verifier reads
    /// `<iss>/.well-known/jwks.json` from (Amazon Cognito Developer Guide, "Verifying a
    /// JSON web token") — so the host has to be one this emulator answers on:
    ///
    /// * a pool created through an endpoint of THIS emulator
    ///   ([`UserPool::issuer_endpoint`]) publishes the issuer AT that endpoint's own
    ///   `host[:port]`, which is the address its creator dialled and the one address a
    ///   relying party on this machine resolves. This emulator answers the issuer's
    ///   routes there (`crate::service`), so the locator a token leads a verifier to
    ///   leads back here;
    /// * a pool created by a client that resolves AWS's own hostnames, on an emulator
    ///   served on no port at all, has no endpoint of this emulator to publish and
    ///   announces the documented host instead ([`issuer_host`]):
    ///   `https://cognito-idp.<region>.amazonaws.com/<UserPoolId>` for a pool issuing
    ///   the original way, which is the only deployment whose client resolves that host.
    ///
    /// Only the HOST is the emulator's; the PATH is the one AWS gives the issuer, the
    /// pool's own id, so a client that reads the pool out of an issuer reads the same
    /// segment it would against AWS.
    ///
    /// Either way it is rendered from what the pool STORES, so one pool announces
    /// ONE issuer however its later callers address this emulator: the `iss` claim
    /// of a token is what a verifier compares against the issuer it was configured
    /// with.
    pub fn issuer(&self) -> String {
        format!("https://{}/{}", self.issuer_authority(), self.id)
    }

    /// The AUTHORITY this pool publishes its issuer on: the endpoint of this emulator the
    /// pool was created through, as the `host[:port]` its creator dialled, and the
    /// documented regional host for a pool with no endpoint of this emulator to publish
    /// (see [`UserPool::issuer`]).
    ///
    /// The stored endpoint is published as it was reached, with no label inserted in front
    /// of it: a client resolves the authority an issuer names and dials it, and the only
    /// authority it can resolve to this process is the one it already reached this service
    /// at.
    fn issuer_authority(&self) -> String {
        match self.issuer_endpoint.as_deref() {
            Some(endpoint) => endpoint.to_string(),
            None => issuer_host(&self.region, self.issuer_type()),
        }
    }

    /// Where this pool publishes the key set its tokens are verified against:
    /// `<issuer>/.well-known/jwks.json`, the location the token's `iss` claim
    /// leads a verifier to (Amazon Cognito Developer Guide, "Verifying a JSON
    /// web token") — built by the shared locator every OIDC issuer this emulator serves
    /// publishes its key set at ([`::service_common::openid_discovery::jwks_uri`]).
    pub fn jwks_uri(&self) -> String {
        ::service_common::openid_discovery::jwks_uri(&self.issuer())
    }

    /// Whether `issuer` is a token issuer OF THIS POOL — the question a verifier
    /// holding a token asks to find the pool whose key set verifies it.
    ///
    /// The pool's OWN announced issuer ([`UserPool::issuer`]) names it, compared as a
    /// base URI so a trailing `/` names the same provider: whatever address this
    /// deployment publishes — including a host an operator serves this emulator under,
    /// which no reading without the pool in hand can tell from another service's — the
    /// value the pool itself announces is its issuer. Beside it, [`is_pool_issuer`] is
    /// the reading a verifier with no pool record shares: the documented host form of a
    /// pool of this Region, in whichever zone it is spelled, which is what a verifier
    /// configured from AWS's documentation holds.
    pub fn publishes_issuer(&self, issuer: &str) -> bool {
        let presented = issuer.trim().trim_end_matches('/');
        presented == self.issuer().trim_end_matches('/')
            || is_pool_issuer(&self.region, &self.id, issuer)
    }

    /// This pool's token-signing keys, minted when the pool was created.
    pub fn signing_keys(&self) -> &PoolSigningKeys {
        &self.signing_keys
    }

    /// This pool's SAML 2.0 signing certificate, issued when the pool was
    /// created, or `None` when it did not issue.
    ///
    /// A pure read: it renders the stored certificate and issues nothing.
    pub fn saml_signing_certificate(&self) -> Option<&str> {
        self.saml_signing_certificate.as_deref()
    }

    /// Issue this pool's SAML 2.0 signing certificate, to the entity id its
    /// CURRENT id implies, dated from its creation and over this pool's own
    /// signing key — the key pair its key set publishes — so the certificate and
    /// that document describe one key pair.
    ///
    /// Called where a pool's id is settled — when the pool is built, and again
    /// when a minted id turned out to be taken ([`UserPool::with_new_id`]) —
    /// because the certificate is issued to the pool's SAML service-provider
    /// entity id, which is derived from that id.
    pub(super) fn issue_saml_signing_certificate(&mut self) {
        self.saml_signing_certificate = issue_signing_certificate(
            &self.id,
            self.creation_date,
            self.signing_keys.saml_signing_key(),
        );
    }

    /// This pool's JSON Web Key Set (RFC 7517 §5): the public half of every key
    /// its tokens are signed with, each entry found by the `kid` the token
    /// header carries. This is the document a verifier reads from
    /// [`UserPool::jwks_uri`], and it holds only PUBLIC key parameters.
    ///
    /// A pure read: it renders the stored keys and changes nothing.
    pub fn jwks(&self) -> serde_json::Value {
        self.signing_keys.to_key_set()
    }

    /// This pool with a freshly minted id, and the ARN that id implies. Used
    /// when a minted id turns out to name a pool that already exists.
    ///
    /// The pool's SAML signing certificate is issued again with it: the
    /// certificate names the pool's SAML service-provider entity id, which the
    /// new id changes.
    #[must_use]
    pub fn with_new_id(mut self, region: &str) -> Self {
        self.id = new_pool_id(region);
        self.arn = ::engine::emulator_arn(SIGNING_NAME, region, &format!("userpool/{}", self.id));
        self.issue_saml_signing_certificate();
        self
    }

    /// Whether usernames in this pool are compared case-sensitively.
    ///
    /// `UsernameConfiguration.CaseSensitive` is the pool's setting; a pool that
    /// sets none compares case-sensitively.
    pub fn case_sensitive(&self) -> bool {
        self.username_configuration
            .as_ref()
            .is_none_or(|configuration| configuration.case_sensitive)
    }

    /// The key `username` is stored under in [`UserPool::users`].
    pub fn user_key(&self, username: &str) -> String {
        if self.case_sensitive() {
            username.to_string()
        } else {
            username.to_lowercase()
        }
    }

    /// Whether the value `stored` under this pool's sign-in or alias attribute
    /// `name` is the value `submitted` names.
    ///
    /// `UsernameConfiguration.CaseSensitive` set to `false` "also enables both
    /// `preferred_username` and `email` alias to be case insensitive, in
    /// addition to the `username` attribute" (this service's model), so in such
    /// a pool a value of either of those attributes names its holder in any
    /// capitalization, exactly as the username itself does
    /// ([`UserPool::user_key`]). Every other attribute's value, and every value
    /// of a pool that compares usernames case-sensitively, names its holder only
    /// as it was stored. The comparison alone folds case: the STORED spelling is
    /// untouched, so a read still reports the value the write submitted.
    ///
    /// Asked once per candidate user of a lookup, so the folded comparison
    /// streams the two values rather than lower-casing either into a new string.
    pub fn names_attribute_value(&self, name: &str, stored: &str, submitted: &str) -> bool {
        if self.case_sensitive() || !CASE_INSENSITIVE_ALIAS_ATTRIBUTES.contains(&name) {
            return stored == submitted;
        }
        stored
            .chars()
            .flat_map(char::to_lowercase)
            .eq(submitted.chars().flat_map(char::to_lowercase))
    }

    /// The stored user `username` names: by a NAME of this pool's own namespace
    /// ([`UserPool::user_storage_key`]).
    pub fn user(&self, username: &str) -> Option<&User> {
        let key = self.user_storage_key(username)?;
        self.users.get(&key)
    }

    /// The key the stored user `username` names is held under, or `None` when no
    /// such user exists.
    ///
    /// Every NAME this pool resolves one of its users by, in the order a stored
    /// name beats a derived one: the username itself, under the pool's own case
    /// comparison ([`UserPool::user_key`]); the username a link to a third-party
    /// identity gives it ([`UserPool::federated_user_key`]); then any sign-in or
    /// alias value of the pool the value is
    /// ([`UserPool::sign_in_name_holder_key`]). A `Username` "is typically your
    /// user's username, but it can be any of their alias attributes. If username
    /// isn't an alias attribute in your user pool, this value must be the sub of a
    /// local user or the username of a user from a third-party IdP"
    /// (`AdminGetUser.Username` in this service's model).
    ///
    /// A user's `sub` is NOT one of those names: it is the id this service holds
    /// as an ATTRIBUTE of the user rather than a value in the pool's username
    /// namespace, so a request carrying one reaches a user only where the pool
    /// itself minted the username from it — a pool that signs its users in with an
    /// attribute value, whose users' usernames ARE their `sub` and so answer the
    /// first lookup below. The id still IDENTIFIES its user, which is what an
    /// alias value may not take ([`UserPool::names_another_user`]), and names it
    /// for a reader that holds an id rather than a name
    /// ([`UserPool::user_of_sub`]).
    pub fn user_storage_key(&self, username: &str) -> Option<String> {
        let key = self.user_key(username);
        if self.users.contains_key(&key) {
            return Some(key);
        }
        if let Some(stored_key) = self.federated_user_key(username) {
            return Some(stored_key);
        }
        self.sign_in_name_holder_key(username)
    }

    /// The key of the stored user `value` IDENTIFIES: the user a NAME of this
    /// pool resolves to ([`UserPool::user_storage_key`]), or the user whose own
    /// `sub` the value is ([`UserPool::sub_holder_key`]).
    fn identified_user_key(&self, value: &str) -> Option<String> {
        self.user_storage_key(value)
            .or_else(|| self.sub_holder_key(value))
    }

    /// The key of the stored user whose own `sub` is `sub`, or `None` when this
    /// pool holds none. The lowest-ordered key answers, so the lookup is stable
    /// whatever order the users are held in.
    pub fn sub_holder_key(&self, sub: &str) -> Option<String> {
        self.users
            .iter()
            .filter(|(_, user)| user.sub == sub)
            .map(|(key, _)| key.clone())
            .min()
    }

    /// The stored user whose own `sub` is `sub`, or `None` when this pool holds
    /// none.
    ///
    /// The lookup by the user's ID rather than by a name it answers to
    /// ([`UserPool::user_storage_key`]): a token's subject claim names the user it
    /// was issued to by that id, which no name of the pool need resolve.
    pub fn user_of_sub(&self, sub: &str) -> Option<&User> {
        let key = self.sub_holder_key(sub)?;
        self.users.get(&key)
    }

    /// The key of the user this pool holds the THIRD-PARTY IdP username
    /// `username`'s link on, or `None` when no user of this pool holds one.
    ///
    /// A `Username` "must be the sub of a local user or the username of a user
    /// from a third-party IdP" (`AdminGetUser.Username` in this service's
    /// model), and a third-party user's username is that of its provider and
    /// its id with that provider ([`federated_username`]). An external identity
    /// `AdminLinkProviderForUser` linked belongs to the local profile it was
    /// linked to — "Amazon Cognito returns this user when the new user (with the
    /// linked IdP attribute) signs in" (`AdminLinkProviderForUser.DestinationUser`)
    /// — so that profile is the user the federated username names. The name is
    /// compared the way this pool compares usernames
    /// ([`UserPool::user_key`]), so a case-insensitive pool resolves it in any
    /// case and a case-sensitive one only as the link stored it. The
    /// lowest-ordered key answers, so the lookup is stable whatever order the
    /// users are held in.
    pub fn federated_user_key(&self, username: &str) -> Option<String> {
        let asked_for = self.user_key(username);
        self.users
            .iter()
            .filter(|(_, user)| {
                user.provider_links.keys().any(|identity| {
                    self.user_key(&federated_username(
                        &identity.provider_name,
                        &identity.provider_attribute_value,
                    )) == asked_for
                })
            })
            .map(|(key, _)| key.clone())
            .min()
    }

    /// The key of the user whose SIGN-IN NAME `value` is, or `None` when no user
    /// of this pool answers to it.
    ///
    /// `UsernameAttributes` "specifies whether a user can use an email address
    /// or phone number as a username when they sign up", while `AliasAttributes`
    /// are "attributes supported as an alias for this user pool" (this service's
    /// model). A username attribute's value IS the name the user signs in with,
    /// so it resolves whatever its verified flag says; an alias attribute's
    /// value resolves only while it is the user's ACTIVE alias
    /// ([`User::alias_is_active`]), since an unconfirmed alias names no user
    /// yet. Each value is compared the way this pool compares that attribute
    /// ([`UserPool::names_attribute_value`]), so a pool that compares usernames
    /// without case resolves an email or `preferred_username` sign-in value in
    /// any capitalization. At most one user holds a given sign-in value, and the lowest-ordered
    /// key answers, so the lookup is stable whatever order the users are held
    /// in. The work is bounded by the attributes the POOL configures, not by
    /// anything the caller sends.
    pub fn sign_in_name_holder_key(&self, value: &str) -> Option<String> {
        let username_names = self
            .username_attributes
            .iter()
            .map(|name| (name.as_str(), false));
        let alias_names = self
            .alias_attributes
            .iter()
            .map(|name| (name.as_str(), true));
        for (name, only_while_active) in username_names.chain(alias_names) {
            let holder = self
                .users
                .iter()
                .filter(|(_, user)| {
                    user.attributes
                        .get(name)
                        .is_some_and(|stored| self.names_attribute_value(name, stored, value))
                        && (!only_while_active || user.alias_is_active(name))
                })
                .map(|(key, _)| key.clone())
                .min();
            if holder.is_some() {
                return holder;
            }
        }
        None
    }

    /// The attribute a pool whose sign-in name IS an attribute value stores
    /// `submitted` under, or `None` for a pool whose users sign in with a
    /// username of their own.
    ///
    /// "If your user pool only supports phone numbers or email addresses as
    /// sign-in attributes, Amazon Cognito automatically generates a username
    /// value" (`AdminCreateUser.Username` in this service's model), so in such a
    /// pool the submitted value is not a username at all: it is the value of the
    /// sign-in attribute it has the shape of — an address for `email`, a number
    /// for `phone_number` ([`attribute_shape_of`] judges which). A value of no
    /// configured sign-in attribute's shape is no sign-in value of this pool, so
    /// none is reported and nothing is stored under an attribute the value does
    /// not belong to.
    pub fn sign_in_attribute(&self, submitted: &str) -> Option<&str> {
        let shape = attribute_shape_of(submitted)?;
        self.username_attributes
            .iter()
            .map(String::as_str)
            .find(|name| *name == shape)
    }

    /// Why `submitted` is no name a user of this pool may be created under, or
    /// `None` when it is one.
    ///
    /// "You can only provide a value if usernames are a valid sign-in attribute
    /// for your user pool. If your user pool only supports phone numbers or
    /// email addresses as sign-in attributes, Amazon Cognito automatically
    /// generates a username value" (`AdminCreateUser.Username` in this service's
    /// model): in such a pool the submitted value is not a username but the
    /// value of a sign-in attribute, so it has to be a value of one of the
    /// attributes the pool signs users in with — an address for `email`, a
    /// number for `phone_number` ([`UserPool::sign_in_attribute`] resolves
    /// which). A value of no configured sign-in attribute's shape names no user
    /// of such a pool at all, so a creation under it is a bad parameter, and the
    /// sentence states which value the pool expects. A pool whose users sign in
    /// with a username of their own takes the submitted name as it is, so
    /// nothing is refused there.
    ///
    /// The value a pool signs users in with also has to have the form THAT
    /// attribute's values take ([`crate::state::attribute_value_defect`]): the
    /// submitted value is stored under the attribute, so it is judged as a value
    /// of it.
    ///
    /// And whatever the pool signs its users in with, a name that has the form of
    /// one of its ALIAS attributes' values is no username of it at all
    /// ([`UserPool::alias_format_username_defect`]).
    pub fn username_defect(&self, submitted: &str) -> Option<String> {
        // The alias namespace is judged for every pool: a name of an aliased
        // attribute's format is no username of this pool.
        if let Some(defect) = self.alias_format_username_defect(submitted) {
            return Some(defect);
        }
        if self.username_attributes.is_empty() {
            return None;
        }
        match self.sign_in_attribute(submitted) {
            Some(attribute) => attribute_value_defect(attribute, submitted).map(str::to_string),
            None => sign_in_name_shape_message(&self.username_attributes),
        }
    }

    /// Why `submitted` is no USERNAME of this pool because it has the format of a
    /// value of one of the pool's ALIAS attributes, or `None` when it has no such
    /// format or the pool aliases no attribute of that format.
    ///
    /// `AliasAttributes` are "attributes supported as an alias for this user
    /// pool" (this service's model) and an alias value names ONE user of the pool
    /// ([`UserPool::sign_in_name_holder_key`]), so in such a pool a value of that
    /// attribute's format belongs to the ALIAS namespace: a name of the same
    /// format would shadow the alias of whichever user holds that value. Amazon
    /// Cognito therefore refuses such a value wherever a request submits a NAME a
    /// user is to answer to — the `Username` a registration stores a user under,
    /// and the `preferred_username` an attribute write stores for one
    /// ([`PREFERRED_USERNAME_ATTRIBUTE`], itself an alias attribute, so its value
    /// is a sign-in name too). A name that merely ADDRESSES a stored user is
    /// unaffected: a `Username` "can be any of their alias attributes"
    /// (`AdminGetUser.Username`), which is how an alias value resolves its holder.
    ///
    /// Only an alias attribute whose values have a format of their own can be
    /// shadowed ([`alias_format_wording`]), and the format of a submitted value is
    /// read the one way this service reads one
    /// ([`crate::state::attribute_shape_of`]).
    pub fn alias_format_username_defect(&self, submitted: &str) -> Option<String> {
        let shape = attribute_shape_of(submitted)?;
        // The pool's ALIAS attributes alone: a pool whose `UsernameAttributes`
        // name the attribute signs its users in BY that value, so there the
        // submitted name is the attribute's own value rather than a name
        // shadowing it.
        if !self
            .alias_attributes
            .iter()
            .any(|attribute| attribute == shape)
        {
            return None;
        }
        alias_format_username_message(shape)
    }

    /// Whether this pool resolves a username it does not hold through its
    /// migrate-user function before a request naming that username is refused.
    ///
    /// `LambdaConfig.UserMigration` configures a trigger that "can create user
    /// profiles when users sign in or attempt to reset their password with
    /// credentials that don't exist yet" (`LambdaConfigType$UserMigration` in
    /// this service's model), so in a pool that configures one an absent
    /// username is not an unknown user until that function says so
    /// ([`crate::state::lambda_triggers::user_migration`]).
    pub fn migrates_absent_users(&self) -> bool {
        self.lambda_config
            .user_migration
            .as_deref()
            .is_some_and(|function_arn| !function_arn.is_empty())
    }

    /// Whether this pool remembers devices at all: `DeviceConfigurationType` is
    /// "the device-remembering configuration for a user pool", and "when you
    /// provide a value for any property of `DeviceConfiguration`, you activate
    /// the device remembering for the user pool" (this service's model), so a
    /// pool that stores none has no device surface to serve.
    pub fn remembers_devices(&self) -> bool {
        self.device_configuration.is_some()
    }

    /// The names of this pool's groups `user` belongs to, in stored (name) order.
    ///
    /// Only groups the pool still HOLDS are reported: `DeleteGroup` documents that
    /// a deleted group "no longer contributes to users' `cognito:preferred_group`
    /// or `cognito:groups` claims", so a membership of a group that is gone is not
    /// rendered.
    pub fn user_group_names(&self, user: &User) -> Vec<String> {
        user.groups
            .iter()
            .filter(|name| self.groups.contains_key(*name))
            .cloned()
            .collect()
    }

    /// The names of this pool's groups `user` belongs to as a TOKEN CLAIM carries
    /// them: ordered by ascending `Precedence`, which is the order the group
    /// claim's own precedence rule states.
    ///
    /// `CreateGroup`'s `Precedence` (this service's model): "Zero is the highest
    /// precedence value. Groups with lower `Precedence` values take precedence
    /// over groups with higher or null `Precedence` values", and the claim is what
    /// that precedence is read out of — "Amazon Cognito ID tokens have a
    /// `cognito:preferred_role` claim that presents the highest-precedence group
    /// that a user belongs to. Both ID and access tokens also contain a
    /// `cognito:groups` claim" — so the list is ordered by precedence, best first,
    /// with a group that states none behind every group that states one
    /// ([`precedence_rank`]). The sort is STABLE, so groups tied at one precedence
    /// keep the stored (name) order [`UserPool::user_group_names`] reports them in
    /// and a second sign-in renders the same list.
    pub fn user_group_claim_names(&self, user: &User) -> Vec<String> {
        let mut ranked: Vec<(PrecedenceRank, String)> = user
            .groups
            .iter()
            .filter_map(|name| self.groups.get(name))
            .map(|group| (precedence_rank(group.precedence), group.group_name.clone()))
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.into_iter().map(|(_, name)| name).collect()
    }

    /// The role claims this pool's groups put in `user`'s identity token: every role its
    /// groups grant, best precedence first, and the ONE role the best-precedence group
    /// prefers where the groups name it unambiguously.
    ///
    /// `CreateGroup`'s own `Precedence` documentation is the rule (this service's model):
    /// "Zero is the highest precedence value. Groups with lower `Precedence` values take
    /// precedence over groups with higher or null `Precedence` values. If a user belongs to
    /// two or more groups, it is the group with the lowest precedence value whose role ARN is
    /// given in the user's tokens for the `cognito:roles` and `cognito:preferred_role`
    /// claims. Two groups can have the same `Precedence` value. If this happens, neither
    /// group takes precedence over the other. If two groups with the same `Precedence` have
    /// the same role ARN, that role is used in the `cognito:preferred_role` claim in tokens
    /// for users in each group. If the two groups have different role ARNs, the
    /// `cognito:preferred_role` claim isn't set."
    ///
    /// So a group that grants NO role contributes to neither claim, a stated precedence
    /// ranks ahead of none at all, and the preference is withheld exactly when the groups
    /// tied at the best precedence disagree about the role. Only groups the pool still
    /// HOLDS count, for the reason [`UserPool::user_group_names`] states.
    pub fn user_group_role_claims(&self, user: &User) -> GroupRoleClaims {
        // Stable-sorted by precedence alone, so groups tied at one precedence keep their
        // stored (name) order and a read renders the same claim twice.
        let mut granting: Vec<(PrecedenceRank, &str)> = user
            .groups
            .iter()
            .filter_map(|name| self.groups.get(name))
            .filter_map(|group| {
                group
                    .role_arn
                    .as_deref()
                    .map(|role_arn| (precedence_rank(group.precedence), role_arn))
            })
            .collect();
        granting.sort_by_key(|(rank, _)| *rank);

        let mut roles: Vec<String> = Vec::new();
        for (_, role_arn) in &granting {
            if !roles.iter().any(|held| held == role_arn) {
                roles.push((*role_arn).to_string());
            }
        }
        let preferred_role = granting.first().and_then(|(best, role_arn)| {
            granting
                .iter()
                .take_while(|(rank, _)| rank == best)
                .all(|(_, tied)| tied == role_arn)
                .then(|| (*role_arn).to_string())
        });
        GroupRoleClaims {
            roles,
            preferred_role,
        }
    }

    /// The keys in [`UserPool::users`] of the users that belong to
    /// `group_name`, in ascending key order — the stable order a paginated
    /// member listing indexes into.
    ///
    /// `GroupNameType` carries no case-insensitivity trait, so the name is
    /// matched exactly as it was created, and a user is reported only while its
    /// stored membership names that group.
    pub fn group_member_keys(&self, group_name: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .users
            .iter()
            .filter(|(_, user)| user.groups.contains(group_name))
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys
    }

    /// The storage key of the user `identity` is linked to, or `None` when no
    /// user of this pool holds a link for it.
    ///
    /// A provider identity links to ONE local profile — `AdminLinkProviderForUser`
    /// links a `SourceUser` to "an existing user account in a user pool, or
    /// `DestinationUser`" (this service's model) — so at most one key answers;
    /// the lowest-ordered one is taken, so the lookup is stable whatever order
    /// the users are held in.
    pub fn linked_user_key(&self, identity: &ProviderIdentity) -> Option<String> {
        self.users
            .iter()
            .filter(|(_, user)| user.provider_links.contains_key(identity))
            .map(|(key, _)| key.clone())
            .min()
    }

    /// The stored user `username` names, mutably.
    pub fn user_mut(&mut self, username: &str) -> Option<&mut User> {
        let key = self.user_storage_key(username)?;
        self.users.get_mut(&key)
    }

    /// Write `plan` ([`plan_attribute_update`]) onto this pool as of `now` and
    /// return the delivery of each verification message it sent, or `None` when
    /// the user it applies to is no longer stored.
    ///
    /// The plan's own user takes its attributes
    /// ([`User::apply_attribute_update`]), and every OTHER holder of an alias
    /// value the plan moves loses the verified flag that made that value
    /// theirs: an alias names ONE user of the pool, so the write that gives the
    /// value to this user is the write that takes it from them.
    pub fn apply_attribute_update(
        &mut self,
        plan: &AttributeUpdatePlan,
        now: SystemTime,
    ) -> Option<Vec<CodeDeliveryDetailsType>> {
        for (key, flag) in &plan.demotions {
            if let Some(holder) = self.users.get_mut(key) {
                holder
                    .attributes
                    .insert((*flag).to_string(), FLAG_UNCONFIRMED.to_string());
                holder.user_last_modified_date = now;
            }
        }
        let user = self.user_mut(&plan.username)?;
        Some(user.apply_attribute_update(plan, now))
    }

    /// The effective password policy: the pool's own, with the documented
    /// defaults filled in for what it leaves unset.
    pub fn password_policy(&self) -> PasswordPolicyType {
        let mut policy = self
            .policies
            .password_policy
            .clone()
            .unwrap_or_else(default_password_policy);
        if policy.minimum_length.is_none() {
            policy.minimum_length = Some(DEFAULT_PASSWORD_MIN_LENGTH);
        }
        if policy.temporary_password_validity_days == 0 {
            policy.temporary_password_validity_days = DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS;
        }
        policy
    }

    /// `None` when `password` satisfies this pool's password policy, else the
    /// requirement it misses — the message a client has to fix the request
    /// from, worded as the asking operation's own API words it
    /// ([`PasswordPolicyWording`]).
    ///
    /// The message is the fixed clause for the requirement missed, behind that
    /// wording's prefix: it reports WHICH requirement the password failed and
    /// never the value this pool configured for it.
    pub fn check_password(
        &self,
        password: &str,
        wording: PasswordPolicyWording,
    ) -> Option<String> {
        let policy = self.password_policy();
        let minimum = policy.minimum_length.unwrap_or(DEFAULT_PASSWORD_MIN_LENGTH);
        if i64::try_from(password.chars().count()).unwrap_or(i64::MAX) < i64::from(minimum) {
            return Some(password_policy_message(wording, PASSWORD_TOO_SHORT));
        }
        if policy.require_uppercase && !password.chars().any(char::is_uppercase) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_UPPERCASE));
        }
        if policy.require_lowercase && !password.chars().any(char::is_lowercase) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_LOWERCASE));
        }
        if policy.require_numbers && !password.chars().any(|c| c.is_ascii_digit()) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_NUMBERS));
        }
        if policy.require_symbols && !password.chars().any(is_symbol) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_SYMBOLS));
        }
        None
    }

    /// Whether this pool's password policy restricts `user` from reusing
    /// `password`.
    ///
    /// "The number of previous passwords that you want Amazon Cognito to
    /// restrict each user from reusing. Users can't set a password that matches
    /// any of `n` previous passwords, where `n` is the value of
    /// `PasswordHistorySize`" (`PasswordPolicyType.PasswordHistorySize` in this
    /// service's model): the restricted set is the `n` passwords this user set
    /// most recently — the one it holds now among them, being the one set last
    /// ([`User::recent_passwords`]) — so a password set further back than the
    /// window reaches can be set again. A pool whose policy configures no size
    /// restricts none, which is what a pool created with the default policy
    /// does.
    pub fn restricts_password_reuse(&self, user: &User, password: &str) -> bool {
        let restricted = self
            .password_policy()
            .password_history_size
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(0);
        user.recent_passwords()
            .take(restricted)
            .any(|previous| previous == password)
    }

    /// How long a temporary password this pool issues stays usable.
    pub fn temporary_password_validity(&self) -> Duration {
        let days = self.password_policy().temporary_password_validity_days;
        // Saturating, so no stored policy value can ask for a number of seconds
        // the arithmetic cannot represent.
        Duration::from_secs(u64::try_from(days).unwrap_or(0).saturating_mul(24 * 60 * 60))
    }

    /// A temporary password this pool issues ITSELF, for an invitation whose request
    /// submitted none: "if you don't specify a value, Amazon Cognito generates one for
    /// you" (`AdminCreateUser.TemporaryPassword`).
    ///
    /// What it generates satisfies the policy a SUBMITTED temporary password is held to
    /// ([`Self::check_password`]) — one character of each class a policy can require,
    /// then alphanumeric filler up to the length it requires — so the credential the
    /// invitation carries is one its holder can actually sign in with, and it carries no
    /// whitespace, which `PasswordType` (`^[\S]+$`) forbids.
    ///
    /// The length is CLAMPED to what a password may be: `PasswordType` is `len 0..256`, so
    /// no stored policy can ask this to build a longer one.
    #[must_use]
    pub fn generated_temporary_password(&self) -> String {
        let required_length = usize::try_from(
            self.password_policy()
                .minimum_length
                .unwrap_or(DEFAULT_PASSWORD_MIN_LENGTH),
        )
        .unwrap_or(0)
        .min(MAX_PASSWORD_LENGTH);
        let mut password = String::new();
        for alphabet in PASSWORD_CLASS_ALPHABETS {
            password.push_str(&random_from_alphabet(alphabet, 1));
        }
        password.push_str(&random_alphanumeric_token(
            required_length.saturating_sub(password.chars().count()),
        ));
        password
    }

    /// Whether this pool's schema declares the attribute `name` — a standard
    /// attribute every pool carries, or an entry its own schema holds. An
    /// attribute a pool does not declare cannot hold a value.
    pub fn declares_attribute(&self, name: &str) -> bool {
        is_standard_attribute(name)
            || self
                .schema
                .iter()
                .any(|entry| entry.name.as_deref() == Some(name))
    }

    /// `None` when every name in `names` is an attribute this pool's schema
    /// declares, else the first one that is not: an attribute a pool does not
    /// declare cannot hold a value.
    pub fn unknown_attribute(&self, names: &[String]) -> Option<String> {
        names
            .iter()
            .find(|name| !self.declares_attribute(name))
            .cloned()
    }

    /// This pool's schema entry for the attribute STORED under `name` — the
    /// prefixed name for a custom or developer-only attribute — or `None` when
    /// the schema declares no such attribute.
    pub fn schema_attribute(&self, name: &str) -> Option<&SchemaAttributeType> {
        self.schema
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name))
    }

    /// How many of this pool's schema entries are custom or developer-only
    /// attributes: the ones added beyond the standard attributes every pool
    /// carries.
    pub fn custom_attribute_count(&self) -> usize {
        self.schema
            .iter()
            .filter(|entry| {
                entry
                    .name
                    .as_deref()
                    .is_some_and(|name| !is_standard_attribute(name))
            })
            .count()
    }

    /// Whether this pool's schema lets `name`'s stored value be CHANGED:
    /// `SchemaAttributeType.Mutable` — "Specifies whether the value of the
    /// attribute can be changed" (this service's model). An attribute the
    /// schema does not declare has no entry to read, so it is reported
    /// unchangeable.
    pub fn attribute_mutable(&self, name: &str) -> bool {
        self.schema_attribute(name)
            .and_then(|entry| entry.mutable)
            .unwrap_or(false)
    }

    /// Whether this pool's schema marks `name` a required attribute.
    pub fn attribute_required(&self, name: &str) -> bool {
        self.schema_attribute(name)
            .and_then(|entry| entry.required)
            .unwrap_or(false)
    }

    /// Whether a sign-in name of this pool resolves to `name`'s value:
    /// `AliasAttributes` are "attributes supported as an alias for this user
    /// pool" and `UsernameAttributes` "specifies whether a user can use an email
    /// address or phone number as a username when they sign up" (this service's
    /// model). Either way the value names at most one user of the pool.
    pub fn is_alias_attribute(&self, name: &str) -> bool {
        self.alias_attributes
            .iter()
            .chain(self.username_attributes.iter())
            .any(|attribute| attribute == name)
    }

    /// Whether this pool verifies `name` itself: `AutoVerifiedAttributes` are
    /// "the attributes that are auto-verified in a user pool" (this service's
    /// model), so a new value of one of them is what a verification message is
    /// sent for.
    pub fn auto_verifies(&self, name: &str) -> bool {
        self.auto_verified_attributes
            .iter()
            .any(|attribute| attribute == name)
    }

    /// The attribute a sign-up confirmation message for `user` is addressed to,
    /// and the stored value it goes to, or `None` when this pool has no such
    /// destination for that user.
    ///
    /// "Amazon Cognito sends confirmation codes to the user attribute in the
    /// `AutoVerifiedAttributes` property of your user pool"
    /// (`ResendConfirmationCode` in this service's model), so the pool's own
    /// auto-verified attributes decide where the code goes, in the order the
    /// pool declares them; the user has to hold a value for one of them for a
    /// message to have an address at all.
    pub fn confirmation_destination(&self, user: &User) -> Option<(String, String)> {
        self.auto_verified_attributes
            .iter()
            .filter(|attribute| delivery_medium(attribute).is_some())
            .find_map(|attribute| {
                user.destination_of(attribute)
                    .map(|destination| (attribute.clone(), destination))
            })
    }

    /// Where a password-reset message for `user` goes, as this pool's
    /// `AccountRecoverySetting` decides, for an ADMINISTRATIVE reset — one whose
    /// answer reports no delivery of its own.
    ///
    /// "If `verified_email` has a priority of 1 and `verified_phone_number` has
    /// a priority of 2, your user pool sends account-recovery messages to a
    /// verified email address but falls back to an SMS message if the user has a
    /// verified phone number. The `admin_only` option prevents self-service
    /// account recovery" (`RecoveryOptionType` in this service's model) \u2014 so the
    /// mechanisms are tried in ascending priority and only a VERIFIED value is a
    /// destination.
    pub fn recovery_destination(&self, user: &User) -> RecoveryDestination {
        self.ranked_recovery_destination(user, false)
    }

    /// Where a SELF-SERVICE password reset's message goes: the same ranking, held
    /// to the mechanisms this pool can actually deliver over.
    ///
    /// `ForgotPassword` REPORTS the destination it chose (`CodeDeliveryDetails`),
    /// and a mechanism whose medium the pool has no delivery path for sends
    /// nothing: a text message leaves a pool only through the caller its
    /// `SmsConfiguration` names or the function its `CustomSMSSender` names
    /// ([`Self::delivers_sms`]), so on a pool that configures neither the
    /// `verified_phone_number` mechanism does not qualify and the pool falls
    /// through to the next mechanism by priority \u2014 leaving a user with no other
    /// qualifying mechanism with no destination at all. Recorded behaviour: a pool
    /// ranking the phone mechanism first and storing no `SmsConfiguration` answers
    /// a reset for a user holding a verified number of its verified EMAIL, and
    /// refuses the reset of a user holding only that number.
    pub fn self_service_recovery_destination(&self, user: &User) -> RecoveryDestination {
        self.ranked_recovery_destination(user, true)
    }

    /// The ranked walk both doors share. `needs_a_delivery_path` is whether a
    /// mechanism has to be one this pool can deliver over to qualify, which is the
    /// ONE difference between them.
    fn ranked_recovery_destination(
        &self,
        user: &User,
        needs_a_delivery_path: bool,
    ) -> RecoveryDestination {
        let mut mechanisms: Vec<(i32, String)> = self
            .account_recovery_setting
            .as_ref()
            .and_then(|setting| setting.recovery_mechanisms.clone())
            .map(|mechanisms| {
                mechanisms
                    .into_iter()
                    .map(|mechanism| (mechanism.priority, mechanism.name))
                    .collect()
            })
            .unwrap_or_else(|| {
                DEFAULT_RECOVERY_MECHANISMS
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        (
                            i32::try_from(index).unwrap_or_default() + 1,
                            (*name).to_string(),
                        )
                    })
                    .collect()
            });
        if mechanisms
            .iter()
            .any(|(_, name)| name == RECOVERY_ADMIN_ONLY)
        {
            return RecoveryDestination::AdminOnly;
        }
        mechanisms.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        // "With this setting, SMS doesn't qualify for a valid password recovery
        // mechanism if the user also has SMS multi-factor authentication (MFA)
        // activated" (`AccountRecoverySettingType` in this service's model): the
        // number a reset code would be texted to is this user's second sign-in
        // factor, so the mechanism that delivers over it does not qualify and the
        // pool falls through to the next mechanism by priority — leaving a user
        // with no other qualifying mechanism with no destination at all.
        let sms_mfa_activated = user
            .mfa_setting_list
            .iter()
            .any(|setting| setting == SMS_MFA_SETTING);
        // That activated second factor is one thing documented to disqualify a
        // mechanism the pool ranks. The other is the pool having no path for the
        // medium at all, which only the door that REPORTS the destination it chose
        // is held to (`needs_a_delivery_path`): `SmsConfiguration` is "user pool
        // configuration for delivery of SMS messages with Amazon Simple
        // Notification Service" (`SmsConfigurationType` in this service's model),
        // so a pool that names neither it nor a `CustomSMSSender` has nobody to
        // send a text message through and the mechanism that delivers over one
        // does not qualify. Everything else about this decision is the values the
        // USER holds verified, taken in the pool's own order.
        let sms_qualifies = !sms_mfa_activated && (!needs_a_delivery_path || self.delivers_sms());
        mechanisms
            .iter()
            .filter_map(|(_, name)| recovered_attribute(name))
            .filter(|attribute| *attribute != SMS_RECOVERED_ATTRIBUTE || sms_qualifies)
            .find_map(|attribute| {
                user.verified_destination_of(attribute)
                    .map(|destination| RecoveryDestination::Delivered {
                        attribute: attribute.to_string(),
                        destination,
                    })
            })
            .unwrap_or(RecoveryDestination::None)
    }

    /// Whether this pool can send an SMS message at all.
    ///
    /// A text message leaves the pool over one of the two paths a pool can
    /// configure for one — the caller its `SmsConfiguration` names or the
    /// function its `CustomSMSSender` names — which is the ONE reading of that
    /// question ([`names_an_sms_delivery_path`]). A pool that configures neither
    /// has nobody to send through and nobody to hand the message to, so nothing
    /// it would deliver by SMS can be delivered.
    pub fn delivers_sms(&self) -> bool {
        names_an_sms_delivery_path(self.sms_configuration.as_ref(), Some(&self.lambda_config))
    }

    /// Whether `factor` is a first authentication factor this pool admits:
    /// `SignInPolicyType.AllowedFirstAuthFactors` is "the sign-in methods that a
    /// user pool supports as the first factor" (this service's model), and a pool
    /// that configures none admits `PASSWORD` alone
    /// ([`effective_policies`]).
    pub fn allows_first_auth_factor(&self, factor: &str) -> bool {
        self.policies
            .sign_in_policy
            .as_ref()
            .and_then(|policy| policy.allowed_first_auth_factors.as_ref())
            .is_some_and(|factors| factors.iter().any(|allowed| allowed == factor))
    }

    /// The `AuthFactorType` values `user` is CONFIGURED for in a choice-based
    /// (`USER_AUTH`) sign-in of this pool, in [`AUTH_FACTOR_ORDER`].
    ///
    /// A factor is reported when the USER is set up to present it as a first
    /// authentication factor ([`User::configures_auth_factor`]), has it
    /// available as an MFA factor ([`UserPool::offers_mfa_auth_factor`]) —
    /// "EMAIL_OTP, SMS_OTP, and WEB_AUTHN can be used as either a first
    /// authentication factor or an MFA factor" (`ConfiguredUserAuthFactors` in
    /// this service's model) — or has SET the factor UP as an MFA factor
    /// ([`User::sets_up_mfa_auth_factor`]), which is what a registered TOTP
    /// authenticator is before anything activates it. A factor the user is set
    /// up for in none of those ways is not one it has configured at all, so it
    /// is not reported.
    ///
    /// What the POOL admits as a first authentication factor
    /// (`SignInPolicyType.AllowedFirstAuthFactors`,
    /// [`UserPool::allows_first_auth_factor`]) does not narrow the report: this
    /// member reports the factors configured FOR THE USER, and the sign-in policy
    /// is read where a factor is actually presented. The passkey factor is the one
    /// the USER itself holds a switch over, so a user that deactivated passkey MFA
    /// is reported without it whatever this pool's passkey configuration says
    /// ([`User::configures_auth_factor`]).
    ///
    /// A PURE read, computed from the stored pool and the stored user.
    pub fn configured_user_auth_factors(&self, user: &User) -> Vec<String> {
        AUTH_FACTOR_ORDER
            .iter()
            .copied()
            .filter(|factor| {
                user.configures_auth_factor(factor)
                    || self.offers_mfa_auth_factor(user, factor)
                    || user.sets_up_mfa_auth_factor(factor)
            })
            .map(str::to_string)
            .collect()
    }

    /// Whether `user` can present `factor` as an MFA factor of a choice-based
    /// sign-in of this pool.
    ///
    /// A factor the user has ACTIVATED is available as one
    /// ([`User::activates_mfa_auth_factor`]). `WEB_AUTHN` is the one factor the
    /// POOL has to enable as well: it "is available as an MFA factor only
    /// when passkey MFA is enabled at the user pool level"
    /// (`ConfiguredUserAuthFactors`), which
    /// `WebAuthnConfigurationType.FactorConfiguration` is "when set to
    /// `MULTI_FACTOR_WITH_USER_VERIFICATION`, passkey authentication with user
    /// verification satisfies MFA requirements. When set to `SINGLE_FACTOR` or not
    /// set, passkeys are a single authentication factor" (both in this service's
    /// model) — and presenting it still needs a registered passkey the user has
    /// not deactivated passkey MFA for ([`User::activates_passkey_mfa`]).
    pub fn offers_mfa_auth_factor(&self, user: &User, factor: &str) -> bool {
        if factor == WEB_AUTHN_AUTH_FACTOR {
            return self.passkeys_satisfy_mfa()
                && !user.web_authn_credentials.is_empty()
                && user.activates_passkey_mfa();
        }
        user.activates_mfa_auth_factor(factor)
    }

    /// Whether a passkey of this pool satisfies an MFA requirement:
    /// `WebAuthnConfigurationType.FactorConfiguration` set to
    /// [`WEB_AUTHN_MULTI_FACTOR`], the one of that enum's values that makes passkey
    /// authentication "satisfy MFA requirements" (this service's model). A pool
    /// that stores no passkey configuration at all has not enabled it.
    pub fn passkeys_satisfy_mfa(&self) -> bool {
        self.web_authn_configuration
            .as_ref()
            .and_then(|config| config.factor_configuration.as_deref())
            .is_some_and(|configuration| configuration == WEB_AUTHN_MULTI_FACTOR)
    }

    /// Whether this pool holds a new value of `name` PENDING until the user
    /// confirms it:
    /// `UserAttributeUpdateSettings.AttributesRequireVerificationBeforeUpdate`
    /// "requires that your user verifies their email address, phone number, or
    /// both before Amazon Cognito updates the value of that attribute" (this
    /// service's model).
    pub fn requires_verification_before_update(&self, name: &str) -> bool {
        self.user_attribute_update_settings
            .attributes_require_verification_before_update
            .as_ref()
            .is_some_and(|names| names.iter().any(|attribute| attribute == name))
    }

    /// Whether a user of this pool OTHER than `except_username` already answers
    /// to or is identified by `value` — so a second user cannot be given it as an
    /// alias.
    ///
    /// Every name this pool resolves one of its users by
    /// ([`UserPool::user_storage_key`]) — the username the value is, a sign-in or
    /// alias value a user currently holds, a federated username linked to one —
    /// and the id a user is its own ([`UserPool::sub_holder_key`]). An alias value
    /// identifies ONE user of the pool, so a value that already identifies
    /// another user is not one a second user may be given as its alias.
    pub fn names_another_user(&self, value: &str, except_username: &str) -> bool {
        let except = self.user_key(except_username);
        self.identified_user_key(value)
            .is_some_and(|holder| holder != except)
    }

    /// Whether a user of this pool OTHER than `except_username` already holds
    /// `value` as its active alias value of `name`.
    ///
    /// An alias value identifies ONE user of the pool, so no two users may hold
    /// the same one at once: for `email` and `phone_number` the value is an
    /// alias only while the user's matching verified flag says the value was
    /// confirmed, and an alias attribute with no such flag counts whatever its
    /// value (see [`User::alias_is_active`]). The value is compared the way this
    /// pool compares that attribute ([`UserPool::names_attribute_value`]), so a
    /// pool that compares usernames without case holds one alias for every
    /// capitalization of it.
    pub fn alias_taken(&self, name: &str, value: &str, except_username: &str) -> bool {
        !self.alias_holder_keys(name, value, except_username).is_empty()
    }

    /// The keys of the users of this pool OTHER than `except_username` that hold
    /// `value` as their active alias value of `name`, in ascending key order.
    ///
    /// These are the users an update that gives `value` to another user takes
    /// the alias away from ([`AttributeUpdatePlan::demotions`]).
    pub fn alias_holder_keys(
        &self,
        name: &str,
        value: &str,
        except_username: &str,
    ) -> Vec<String> {
        let except = self.user_key(except_username);
        self.users
            .iter()
            .filter(|(key, user)| {
                *key != &except
                    && user
                        .attributes
                        .get(name)
                        .is_some_and(|stored| self.names_attribute_value(name, stored, value))
                    && user.alias_is_active(name)
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The names of the attributes this pool's schema marks required that `user`
    /// has no value for: what a `NEW_PASSWORD_REQUIRED` challenge asks the user
    /// to supply. `sub` is never among them — the pool mints it.
    pub fn missing_required_attributes(&self, user: &User) -> Vec<String> {
        self.unset_required_attributes(&user.attributes)
    }

    /// The names of the attributes this pool's schema marks required that
    /// `attributes` holds no value for — the same rule as
    /// [`UserPool::missing_required_attributes`], asked of an attribute set that
    /// is not stored yet.
    pub fn unset_required_attributes(&self, attributes: &BTreeMap<String, String>) -> Vec<String> {
        self.schema
            .iter()
            .filter(|entry| entry.required == Some(true))
            .filter_map(|entry| entry.name.clone())
            .filter(|name| name != SUB_ATTRIBUTE && !attributes.contains_key(name))
            .collect()
    }

    /// This pool's domain under the name `domain` — its prefix domain or its
    /// custom one — or `None` when this pool holds no domain of that name.
    pub fn domain_named(&self, domain: &str) -> Option<&UserPoolDomain> {
        [self.domain.as_ref(), self.custom_domain.as_ref()]
            .into_iter()
            .flatten()
            .find(|stored| stored.domain == domain)
    }

    /// This pool's domain under the name `domain`, mutably — the same lookup as
    /// [`UserPool::domain_named`], for an operation that writes the stored
    /// domain back.
    pub fn domain_named_mut(&mut self, domain: &str) -> Option<&mut UserPoolDomain> {
        [self.domain.as_mut(), self.custom_domain.as_mut()]
            .into_iter()
            .flatten()
            .find(|stored| stored.domain == domain)
    }

    /// Take this pool's domain under the name `domain` out of the pool — its
    /// prefix domain or its custom one — and hand back the record that was
    /// stored, or `None` when this pool holds no domain of that name.
    ///
    /// The two slots are the same resource kind under `UserPoolType.Domain` and
    /// `UserPoolType.CustomDomain`, so a delete clears whichever one holds the
    /// name and leaves the other in place.
    pub fn remove_domain_named(&mut self, domain: &str) -> Option<UserPoolDomain> {
        [&mut self.domain, &mut self.custom_domain]
            .into_iter()
            .find(|slot| slot.as_ref().is_some_and(|stored| stored.domain == domain))
            .and_then(Option::take)
    }

    /// Whether `scope` is a custom scope of a resource server of this pool.
    ///
    /// "Amazon Cognito represents scopes in the access token in the format
    /// `$resource-server-identifier/$scope`" (`CreateResourceServer.Identifier`
    /// in this service's model), so a scope value names the stored resource
    /// server and one of ITS scope names. The value is matched against the
    /// STORED identifier and then against that server's own scope names, rather
    /// than split on the separator, because a resource server identifier may
    /// itself carry one.
    pub fn defines_scope(&self, scope: &str) -> bool {
        self.resource_servers.values().any(|server| {
            let Some(name) = scope
                .strip_prefix(server.identifier.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
            else {
                return false;
            };
            server
                .scopes
                .iter()
                .any(|defined| defined.scope_name == name)
        })
    }

    /// The CSV header of this pool: "a comma-separated value (CSV) list
    /// populated with available user attributes in the user pool ... the header
    /// for the CSV file that determines the users in a user import job"
    /// (`GetCSVHeader`), as the list of column names it is modelled as.
    ///
    /// COMPUTED from this pool's STORED schema, so a pool whose schema carries a
    /// custom attribute reports a column for it: one column per attribute of
    /// this pool a user import can carry, then the columns the import format
    /// adds. [`SUB_ATTRIBUTE`] is not among them — the pool mints it for every
    /// user and no import supplies it — and neither are the attributes the
    /// service maintains itself ([`SERVICE_MAINTAINED_SCHEMA`]), which no import
    /// supplies either.
    ///
    /// The ORDER of those columns is the import template's own
    /// ([`csv_header_columns`]) rather than the order this pool happens to store
    /// its schema in, so every pool holding the same attributes hands out the
    /// same template however each one's schema was configured.
    pub fn csv_header(&self) -> Vec<String> {
        csv_header_columns(&self.schema)
    }

    /// Whether `scope` is one an app client of this pool may be permitted to
    /// authorize access with: a standard scope every pool serves
    /// ([`STANDARD_OAUTH_SCOPES`]), or a custom scope a resource server of THIS
    /// pool defines ([`UserPool::defines_scope`]).
    ///
    /// "Scope values include `phone`, `email`, `openid`, and `profile`. The
    /// `aws.cognito.signin.user.admin` scope authorizes user self-service
    /// operations. Custom scopes with resource servers authorize access to
    /// external APIs" (`CreateUserPoolClient.AllowedOAuthScopes` in this
    /// service's model), so a value that is neither names a scope this pool has
    /// no definition of. Every writer of `AllowedOAuthScopes` asks this one
    /// question, so the create and the update path cannot disagree about which
    /// scopes exist.
    pub fn authorizes_scope(&self, scope: &str) -> bool {
        STANDARD_OAUTH_SCOPES.contains(&scope) || self.defines_scope(scope)
    }

    /// This pool's terms documents that app client `client_id` holds under
    /// `terms_name`, or `None` when the client holds none of that name.
    ///
    /// A terms document is assigned to one app client and its name is unique to
    /// that client (`TermsExistsException` in this service's model), so at most
    /// one stored document can answer.
    pub fn terms_for_client(&self, client_id: &str, terms_name: &str) -> Option<&Terms> {
        self.terms
            .values()
            .find(|stored| stored.client_id == client_id && stored.terms_name == terms_name)
    }

    /// This pool's managed login branding style assigned to app client
    /// `client_id`, or `None` when that app client has none.
    ///
    /// "Each style is linked to an app client until you delete it"
    /// (`CreateManagedLoginBranding.ClientId`) and a second style for an app
    /// client "that already has an assigned style" is refused
    /// (`ManagedLoginBrandingExistsException`), so at most one stored style can
    /// answer.
    pub fn branding_for_client(&self, client_id: &str) -> Option<&ManagedLoginBranding> {
        self.managed_login_branding
            .values()
            .find(|stored| stored.client_id == client_id)
    }

    /// The classic hosted UI branding in force for `client_id`, or `None` when
    /// neither that app client nor the pool has any.
    ///
    /// "Returns user-pool level branding information if no app client branding
    /// is applied, or if you don't specify an app client ID"
    /// (`GetUICustomization`), so a client-level entry answers when the named
    /// app client has one and the pool-level default answers otherwise. A caller
    /// that names [`UI_CUSTOMIZATION_ALL_CLIENTS`] names that default itself.
    pub fn ui_customization(&self, client_id: Option<&str>) -> Option<&UiCustomization> {
        client_id
            .filter(|id| *id != UI_CUSTOMIZATION_ALL_CLIENTS)
            .and_then(|id| self.ui_customizations.get(id))
            .or_else(|| self.ui_customizations.get(UI_CUSTOMIZATION_ALL_CLIENTS))
    }

    /// Store the classic hosted UI branding of `client_id`, keeping every value
    /// the request leaves out.
    ///
    /// `SetUICustomization` models `CSS` and `ImageFile` as optional members of
    /// a call that "configures UI branding settings", so a member the request
    /// omits names no new value for it and the stored one stays in force; an
    /// `ImageFile` of no bytes (`ImageFileType`'s length admits the empty blob)
    /// carries no image, so it stores none.
    pub fn apply_ui_customization(
        &mut self,
        client_id: &str,
        css: Option<String>,
        image_file: Option<Vec<u8>>,
        now: SystemTime,
    ) {
        let version = version_stamp(now);
        let stored = self
            .ui_customizations
            .entry(client_id.to_string())
            .or_insert_with(|| UiCustomization {
                client_id: client_id.to_string(),
                css: None,
                image_file: None,
                css_version: version.clone(),
                creation_date: now,
                last_modified_date: now,
            });
        if let Some(css) = css {
            stored.css = Some(css);
        }
        if let Some(image_file) = image_file {
            stored.image_file = (!image_file.is_empty()).then_some(image_file);
        }
        stored.css_version = version;
        stored.last_modified_date = now;
    }

    /// The modelled view of the stored classic hosted UI branding
    /// `customization` of this pool, rendered from what is stored, with its
    /// stored logo image and its stored stylesheet located on the endpoint
    /// `host` the reading request arrived at.
    pub fn to_ui_customization_type(
        &self,
        customization: &UiCustomization,
        host: Option<&str>,
    ) -> crate::ops::types::UICustomizationType {
        crate::ops::types::UICustomizationType {
            user_pool_id: Some(self.id.clone()),
            client_id: Some(customization.client_id.clone()),
            image_url: customization.image_file.as_ref().and_then(|_| {
                host.and_then(|host| {
                    hosted_logo_url(host, &self.id, &customization.client_id)
                })
            }),
            css: customization.css.clone(),
            css_version: Some(customization.css_version.clone()),
            last_modified_date: Some(to_date_time(customization.last_modified_date)),
            creation_date: Some(to_date_time(customization.creation_date)),
            // The locator addresses the CSS document THIS level has stored,
            // under the version it is stored at, so a level holding no
            // stylesheet publishes none to fetch.
            css_url: customization.css.as_ref().and_then(|_| {
                host.and_then(|host| {
                    hosted_css_url(
                        host,
                        &self.id,
                        &customization.client_id,
                        &customization.css_version,
                    )
                })
            }),
        }
    }

    /// The CSS document this pool publishes for `client_id` at version
    /// `css_version`, which is the stylesheet stored for THAT level at THAT
    /// version: the locator a read handed out names one level and one version,
    /// so it serves that level's document and never another's, and never a
    /// later write's.
    pub fn hosted_css(&self, client_id: &str, css_version: &str) -> Option<&str> {
        let stored = self.ui_customizations.get(client_id)?;
        (stored.css_version == css_version).then_some(stored.css.as_deref()?)
    }

    /// The logo image this pool serves for `client_id`, which is the image
    /// stored for THAT level: the locator a read handed out names one level, so
    /// it serves that level's image and never another's.
    pub fn hosted_logo(&self, client_id: &str) -> Option<&[u8]> {
        self.ui_customizations
            .get(client_id)
            .and_then(|stored| stored.image_file.as_deref())
    }

    /// The domain this pool's passkeys name as their relying party, or `None`
    /// when the pool has neither a configured relying party id nor a domain to
    /// serve as one.
    ///
    /// `WebAuthnConfigurationType.RelyingPartyId` "Sets or displays the
    /// authentication domain, typically your user pool domain, that passkey
    /// providers must use as a relying party (RP) in their configuration" (this
    /// service's model), so the configured value is what is in force when the
    /// pool sets one. A pool that sets none falls back to its own domain, the
    /// CUSTOM one first: "when you create a custom domain, the passkey RP ID
    /// defaults to the custom domain. If you had a prefix domain active, this
    /// will cause passkey integration for your prefix domain to stop working due
    /// to a mismatch in RP ID. To keep the prefix domain passkey integration
    /// working, you can explicitly set RP ID to the prefix domain"
    /// (`CreateUserPoolDomain.CustomDomainConfig`). A custom domain's stored
    /// value IS the domain, while a prefix domain's is the prefix alone: "for
    /// custom domains, this is the fully-qualified domain name, such as
    /// auth.example.com. For prefix domains, this is the prefix alone, such as
    /// myprefix. A prefix value of myprefix for a user pool in the us-east-1
    /// Region results in a domain of myprefix.auth.us-east-1.amazoncognito.com"
    /// (`CreateUserPoolDomain.Domain`), so only the prefix is expanded. A pool
    /// with neither is the pool that "doesn't have a configured relying party id
    /// or a user pool domain" (`WebAuthnConfigurationMissingException`).
    pub fn relying_party_id(&self) -> Option<String> {
        let configured = self
            .web_authn_configuration
            .as_ref()
            .and_then(|config| config.relying_party_id.as_deref())
            .filter(|id| !id.is_empty());
        if let Some(configured) = configured {
            return Some(configured.to_string());
        }
        if let Some(custom) = &self.custom_domain {
            return Some(custom.domain.clone());
        }
        self.domain
            .as_ref()
            .map(|prefix| format!("{}.auth.{}.amazoncognito.com", prefix.domain, self.region))
    }

    /// The user-verification requirement this pool's passkey registrations ask
    /// for.
    ///
    /// `WebAuthnConfigurationType.UserVerification`: "When `required`, users can
    /// only register and sign in users with passkeys that are capable of user
    /// verification. When `preferred`, your user pool doesn't require the use of
    /// authenticators with user verification but encourages it" (this service's
    /// model), so a pool that configures none does not require it, which is the
    /// other of `UserVerificationType`'s two values.
    pub fn web_authn_user_verification(&self) -> String {
        self.web_authn_configuration
            .as_ref()
            .and_then(|config| config.user_verification.as_deref())
            .filter(|requirement| !requirement.is_empty())
            .unwrap_or(USER_VERIFICATION_PREFERRED)
            .to_string()
    }

    /// Whether this pool's user pool add-ons are enabled, which is what the
    /// threat-protection surface rests on.
    ///
    /// `UserPoolAddOns` "contains settings for activation of threat protection"
    /// (`CreateUserPool`) and threat protection is changed "to audit-only or
    /// off" by updating that member (`SetRiskConfiguration`), so add-ons are on
    /// exactly when the pool stores a mode other than
    /// [`ADVANCED_SECURITY_MODE_OFF`] — and a pool that stores no
    /// `UserPoolAddOns` at all has them off.
    pub fn add_ons_enabled(&self) -> bool {
        self.user_pool_add_ons
            .as_ref()
            .is_some_and(|add_ons| add_ons.advanced_security_mode != ADVANCED_SECURITY_MODE_OFF)
    }

    /// Whether this pool requires a second authentication factor from every
    /// sign-in: `MfaConfiguration` "sets multi-factor authentication (MFA) to be
    /// on, off, or optional" and [`MFA_REQUIRED`] is the mode in which "MFA
    /// tokens are required for all user sign-ins" (this service's model). Under
    /// the optional mode a user that has activated no factor signs in without
    /// one, so only the required mode makes a factor a precondition.
    pub fn requires_mfa(&self) -> bool {
        self.mfa_configuration == MFA_REQUIRED
    }

    /// The `UserMFASettingList` settings `user` can SET UP as an MFA factor of
    /// this pool, in the order that member documents its values in.
    ///
    /// A method is available to set up when this POOL offers it — `SetUserPoolMfaConfig`
    /// is what "sets the user pool multi-factor authentication (MFA) and passkey
    /// configuration" and each of `SmsMfaConfiguration`,
    /// `EmailMfaConfiguration` and `SoftwareTokenMfaConfiguration` is the
    /// configuration of one method (this service's model) — and the user has not
    /// activated it already ([`User::mfa_setting_list`]): a factor the user holds
    /// is one to present, not one to set up. `SoftwareTokenMfaConfigType` carries
    /// its own activation state ("the activation state of TOTP MFA"), while the
    /// SMS and email methods are offered exactly while the pool stores the
    /// configuration each is delivered through. An email or SMS method is set up
    /// by verifying the address or number it is delivered to, and the software
    /// token by registering an authenticator, so neither is filtered on what the
    /// user holds today.
    pub fn mfa_settings_available_for_setup(&self, user: &User) -> Vec<String> {
        [
            (SMS_MFA_SETTING, self.to_sms_mfa_config().is_some()),
            (
                EMAIL_OTP_MFA_SETTING,
                self.email_mfa_configuration.is_some(),
            ),
            (
                SOFTWARE_TOKEN_MFA_SETTING,
                self.mfa_factors_offered().software_token,
            ),
        ]
        .into_iter()
        .filter(|(setting, offered)| {
            *offered && !user.mfa_setting_list.iter().any(|held| held == setting)
        })
        .map(|(setting, _)| setting.to_string())
        .collect()
    }

    /// Which MFA factors this pool TURNS ON, as the preference writes judge an
    /// activation against ([`User::set_mfa_preference`]).
    ///
    /// `SetUserPoolMfaConfig` is what "sets the user pool multi-factor
    /// authentication (MFA) and passkey configuration" and
    /// `SoftwareTokenMfaConfigType.Enabled` is "the activation state of TOTP MFA"
    /// (this service's model), so a pool that was never given that configuration
    /// turns the factor on for nobody — the pool's own precondition, which is
    /// separate from the enrolment each USER completes for a factor the pool does
    /// offer.
    pub fn mfa_factors_offered(&self) -> MfaFactorsOffered {
        MfaFactorsOffered {
            software_token: self
                .software_token_mfa_configuration
                .as_ref()
                .is_some_and(|config| config.enabled),
        }
    }

    /// The modelled view of this pool's MFA and passkey configuration, rendered
    /// from what is stored.
    ///
    /// `GetUserPoolMfaConfig` and `SetUserPoolMfaConfig` model the same five
    /// members in two generated output structs, so the stored configuration is
    /// assembled once here and each handler maps it into its own output.
    pub fn to_mfa_config(&self) -> MfaConfig {
        MfaConfig {
            sms_mfa_configuration: self.to_sms_mfa_config(),
            software_token_mfa_configuration: self.software_token_mfa_configuration.clone(),
            email_mfa_configuration: self.email_mfa_configuration.clone(),
            mfa_configuration: self.mfa_configuration.clone(),
            web_authn_configuration: self.web_authn_configuration.clone(),
        }
    }

    /// The modelled view of this pool's SMS MFA configuration, rendered from the
    /// pool's own SMS settings.
    ///
    /// `SmsMfaConfigType` is a view over them rather than a second copy: "You
    /// can set `SmsConfiguration` in `CreateUserPool` and `UpdateUserPool`, or
    /// in `SetUserPoolMfaConfig`" (`SmsMfaConfigType.SmsConfiguration` in this
    /// service's model), and `SmsAuthenticationMessage` is a member of both this
    /// structure and the pool itself. `None` when the pool holds neither, so a
    /// pool with no SMS settings reports no SMS MFA configuration.
    pub fn to_sms_mfa_config(&self) -> Option<SmsMfaConfigType> {
        if self.sms_configuration.is_none() && self.sms_authentication_message.is_none() {
            return None;
        }
        Some(SmsMfaConfigType {
            sms_authentication_message: self.sms_authentication_message.clone(),
            sms_configuration: self.sms_configuration.clone(),
        })
    }

    /// The modelled view of this pool's log delivery configuration, rendered
    /// from what is stored.
    ///
    /// `GetLogDeliveryConfiguration` and `SetLogDeliveryConfiguration` model the
    /// same `LogDeliveryConfigurationType`, so both answer the pool this is
    /// read from beside the collection it holds. A pool with no logging
    /// configured reports an EMPTY collection rather than none: the member is
    /// modelled required.
    pub fn to_log_delivery_configuration(&self) -> LogDeliveryConfigurationType {
        LogDeliveryConfigurationType {
            user_pool_id: self.id.clone(),
            log_configurations: self.log_configurations.clone(),
        }
    }

    /// The ARN of this pool in `region`: a replicated user pool keeps ONE id
    /// across the Regions it is replicated to (`CreateUserPoolReplica` takes
    /// the pool's id and a Region and mints no second id), so a replica's ARN
    /// is this pool's id under that Region. For [`UserPool::region`] this is
    /// the pool's own stored ARN.
    pub fn arn_in_region(&self, region: &str) -> String {
        if region == self.region {
            return self.arn.clone();
        }
        ::engine::emulator_arn(SIGNING_NAME, region, &format!("userpool/{}", self.id))
    }

    /// The modelled view of this pool's PRIMARY replica: the pool itself, in
    /// the Region it was created in. `ListUserPoolReplicas` lists "both primary
    /// and secondary replicas", and the pool serves every operation in its own
    /// Region, which is what [`REPLICA_STATUS_ACTIVE`] reports.
    pub fn to_primary_replica(&self) -> UserPoolReplicaType {
        UserPoolReplicaType {
            region_name: Some(self.region.clone()),
            status: Some(REPLICA_STATUS_ACTIVE.to_string()),
            role: Some(REPLICA_ROLE_PRIMARY.to_string()),
            user_pool_arn: Some(self.arn.clone()),
        }
    }

    /// Every Region this pool holds a replica in as of `now`, ASCENDING: its
    /// own Region, where the primary lives, beside every secondary a create
    /// added and a delete has not finished removing. The stable order a page
    /// token indexes into.
    pub fn replica_regions(&self, now: SystemTime) -> Vec<String> {
        let mut regions: Vec<String> = self
            .replicas
            .values()
            .filter(|replica| !replica.removed(now))
            .map(|replica| replica.region_name.clone())
            .collect();
        regions.push(self.region.clone());
        regions.sort();
        regions.dedup();
        regions
    }

    /// The modelled view of this pool's replica in `region` as of `now`, or
    /// `None` when the pool holds none there. A secondary replica whose
    /// deletion the clock has finished is gone, whether or not anything has
    /// pruned it from the store yet.
    pub fn to_replica_in(&self, region: &str, now: SystemTime) -> Option<UserPoolReplicaType> {
        if let Some(replica) = self.live_replica(region, now) {
            return Some(replica.to_user_pool_replica_type(now));
        }
        (region == self.region).then(|| self.to_primary_replica())
    }

    /// The stored SECONDARY replica in `region`, unless the clock has already
    /// made its removal due.
    pub fn live_replica(&self, region: &str, now: SystemTime) -> Option<&UserPoolReplica> {
        self.replicas
            .get(region)
            .filter(|replica| !replica.removed(now))
    }

    /// Drop the replicas whose removal the clock has already made due, so a
    /// later create can use the Region again. A write only an operation that
    /// mutates this pool's replicas performs; a read computes the same
    /// disappearance with [`UserPool::live_replica`] instead.
    pub fn prune_removed_replicas(&mut self, now: SystemTime) {
        self.replicas.retain(|_, replica| !replica.removed(now));
    }

    /// The schema a READ of this pool reports: the attributes the pool is
    /// CONFIGURED with ([`UserPool::schema`]) and, behind the standard ones, the
    /// attributes the service maintains for every pool
    /// ([`SERVICE_MAINTAINED_SCHEMA`]).
    ///
    /// A service-maintained attribute belongs to the pool rather than to the
    /// request that created it, so it sits with the standard attributes a pool
    /// starts with, ahead of the custom attributes added behind them; an
    /// attribute the pool's own schema already declares under that name is left
    /// as the pool declares it.
    pub fn reported_schema(&self) -> Vec<SchemaAttributeType> {
        let mut reported = self.schema.clone();
        // Behind the last standard attribute the pool carries, so the custom
        // attributes appended after them stay behind them.
        let behind_standard = reported
            .iter()
            .rposition(|entry| {
                entry
                    .name
                    .as_deref()
                    .is_some_and(is_standard_attribute)
            })
            .map_or(0, |at| at + 1);
        for (offset, standard) in SERVICE_MAINTAINED_SCHEMA
            .iter()
            .filter(|standard| self.schema_attribute(standard.name).is_none())
            .enumerate()
        {
            reported.insert(behind_standard + offset, standard_schema_attribute(standard));
        }
        reported
    }

    /// The modelled view of this pool, rendered from what is stored.
    pub fn to_user_pool_type(&self) -> UserPoolType {
        self.to_user_pool_type_with(self.reported_schema())
    }

    /// The modelled view of this pool as the CREATE that configured it answers:
    /// [`UserPool::to_user_pool_type`] carrying the schema the request
    /// CONFIGURED, without the attributes the service maintains itself
    /// ([`SERVICE_MAINTAINED_SCHEMA`]).
    ///
    /// A create answers with the pool it stored from the request; the service's
    /// own attributes are the pool's from then on, and every later read of the
    /// pool reports them beside the configured ones.
    pub fn to_configured_user_pool_type(&self) -> UserPoolType {
        self.to_user_pool_type_with(self.schema.clone())
    }

    /// The modelled view of this pool carrying `schema_attributes`, which is
    /// what the two views above differ in.
    pub(super) fn to_user_pool_type_with(&self, schema_attributes: Vec<SchemaAttributeType>) -> UserPoolType {
        UserPoolType {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            policies: Some(self.policies.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
            lambda_config: Some(self.lambda_config.clone()),
            // `UserPoolType.Status` is `@deprecated` ("This property is no longer
            // available."), so it is left out rather than invented.
            status: None,
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            schema_attributes: Some(schema_attributes),
            auto_verified_attributes: non_empty(&self.auto_verified_attributes),
            alias_attributes: non_empty(&self.alias_attributes),
            username_attributes: non_empty(&self.username_attributes),
            sms_verification_message: None,
            email_verification_message: None,
            email_verification_subject: None,
            verification_message_template: Some(self.verification_message_template.clone()),
            sms_authentication_message: self.sms_authentication_message.clone(),
            user_attribute_update_settings: Some(self.user_attribute_update_settings.clone()),
            mfa_configuration: Some(self.mfa_configuration.clone()),
            device_configuration: self.device_configuration.clone(),
            // Computed from what the pool holds, never from a request.
            estimated_number_of_users: i32::try_from(self.users.len()).unwrap_or(i32::MAX),
            email_configuration: Some(self.email_configuration.clone()),
            sms_configuration: self.sms_configuration.clone(),
            user_pool_tags: Some(self.tags.clone()),
            sms_configuration_failure: None,
            email_configuration_failure: None,
            domain: self.domain.as_ref().map(|domain| domain.domain.clone()),
            custom_domain: self.custom_domain.as_ref().map(|domain| domain.domain.clone()),
            admin_create_user_config: Some(self.admin_create_user_config.clone()),
            user_pool_add_ons: self.user_pool_add_ons.clone(),
            username_configuration: self.username_configuration.clone(),
            arn: Some(self.arn.clone()),
            account_recovery_setting: self.account_recovery_setting.clone(),
            user_pool_tier: Some(self.user_pool_tier.clone()),
            key_configuration: Some(self.key_configuration.clone()),
            issuer_configuration: Some(self.issuer_configuration.clone()),
            acr_configuration: Some(self.acr_configuration.clone()),
        }
    }

    /// The SHORT modelled view of this pool, rendered from what is stored: the
    /// entry a listing of several pools serves ("A short description of a user
    /// pool", `UserPoolDescriptionType`). Every member it shares with
    /// [`UserPool::to_user_pool_type`] renders the same stored value, so a
    /// listing entry and a read of the one pool never disagree.
    pub fn to_user_pool_description_type(&self, now: SystemTime) -> UserPoolDescriptionType {
        UserPoolDescriptionType {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            lambda_config: Some(self.lambda_config.clone()),
            // `UserPoolDescriptionType.Status` is `@deprecated` ("This property
            // is no longer available."), so it is left out rather than
            // invented — as `to_user_pool_type` leaves it out too.
            status: None,
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            // "A list of Amazon Web Services Regions where replicas of this
            // user pool exist": exactly the Regions `ListUserPoolReplicas`
            // reports a replica in, so the two readers never disagree. Every
            // pool holds its PRIMARY replica in the Region it was created in, so
            // a pool nothing was replicated to reports that one Region, and so
            // does one whose only secondary the clock has already removed.
            replica_regions: Some(self.replica_regions(now)),
        }
    }
}

/// A new user pool with `name` in `region`, with every configuration default
/// filled in and `signing_keys` as the key pairs its tokens are signed with. The
/// caller stores it under [`UserPool::id`].
///
/// `issuer_endpoint` is the address of THIS emulator the pool publishes its token
/// issuer on ([`issuer_endpoint_authority`], which reads it off the request being
/// served): the issuer is a URL a relying party fetches later, so it is settled
/// once, here, and never re-derived from the endpoint a later read happened to
/// reach.
///
/// The keys are passed IN rather than minted here: generating them is
/// milliseconds of CPU per key ([`PoolSigningKeys::generate`]), so the caller
/// runs that on the blocking pool and only after the store's own bound has
/// admitted the pool.
///
/// The pool's SAML signing certificate IS issued here, once its id is settled:
/// the certificate is issued to the SAML entity id that id implies, over the
/// pool's own signing key — the RSA key pair its key set publishes — so the
/// certificate and that document describe one key pair.
pub fn new_user_pool(
    name: String,
    region: &str,
    now: SystemTime,
    signing_keys: PoolSigningKeys,
    issuer_endpoint: Option<String>,
) -> UserPool {
    let region = sanitize_region(region);
    let id = new_pool_id(&region);
    let arn = ::engine::emulator_arn(SIGNING_NAME, &region, &format!("userpool/{id}"));
    let mut pool = UserPool {
        id,
        name,
        arn,
        region,
        creation_date: now,
        last_modified_date: now,
        policies: effective_policies(None),
        lambda_config: effective_lambda_config(None),
        mfa_configuration: effective_mfa_configuration(None),
        deletion_protection: effective_deletion_protection(None),
        user_pool_tier: DEFAULT_USER_POOL_TIER.to_string(),
        username_attributes: Vec::new(),
        alias_attributes: Vec::new(),
        auto_verified_attributes: Vec::new(),
        schema: effective_schema(None),
        admin_create_user_config: effective_admin_create_user_config(None),
        username_configuration: None,
        device_configuration: None,
        email_configuration: effective_email_configuration(None),
        sms_configuration: None,
        verification_message_template: effective_verification_message_template(None),
        account_recovery_setting: None,
        user_attribute_update_settings: effective_user_attribute_update_settings(None),
        user_pool_add_ons: None,
        key_configuration: effective_key_configuration(None),
        issuer_configuration: effective_issuer_configuration(None),
        issuer_endpoint,
        acr_configuration: effective_acr_configuration(None),
        sms_authentication_message: None,
        software_token_mfa_configuration: None,
        email_mfa_configuration: None,
        web_authn_configuration: None,
        log_configurations: Vec::new(),
        tags: HashMap::new(),
        domain: None,
        custom_domain: None,
        replicas: BTreeMap::new(),
        clients: HashMap::new(),
        users: HashMap::new(),
        groups: BTreeMap::new(),
        resource_servers: BTreeMap::new(),
        identity_providers: BTreeMap::new(),
        terms: BTreeMap::new(),
        managed_login_branding: BTreeMap::new(),
        ui_customizations: BTreeMap::new(),
        user_import_jobs: BTreeMap::new(),
        // A pool's token-signing keys exist from the moment the pool does, as on
        // AWS, so its key set is publishable before anything signs in: one key
        // pair per token type, minted by the caller before the pool is built and
        // never again.
        signing_keys,
        // Issued below, once the pool's id — the name the certificate is issued
        // to — is settled.
        saml_signing_certificate: None,
        device_group_key: new_device_group_key(),
        risk_configuration: None,
    };
    // A pool's SAML signing certificate exists from the moment the pool does, as
    // on AWS: `GetSigningCertificate` answers one for a pool nothing has
    // federated yet, and the pool keeps the certificate it was issued.
    pool.issue_saml_signing_certificate();
    pool
}

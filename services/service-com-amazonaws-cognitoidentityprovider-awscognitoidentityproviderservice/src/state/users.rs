// SPDX-License-Identifier: CC0-1.0

//! The stored user of a user pool: its status, its attributes, the codes it holds
//! and the import jobs that create users in bulk.

use super::*;

/// A user's confirmation state. The value set is `UserStatusType` from this
/// service's model (`smithy_query(query="shape",shape="UserStatusType")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    Unconfirmed,
    Confirmed,
    Archived,
    Compromised,
    Unknown,
    ResetRequired,
    ForceChangePassword,
    ExternalProvider,
}

impl UserStatus {
    /// Whether a user in this status holds a password of its OWN, which is what
    /// a password reset applies to.
    ///
    /// A user that has not confirmed its sign-up (`UNCONFIRMED`) has not
    /// finished registering, and one an administrator created holds a temporary
    /// password it must replace at its first sign-in
    /// (`FORCE_CHANGE_PASSWORD`) — "the user must change their password to
    /// complete sign-in" — so neither owns a password yet. Every other status
    /// names a user whose registration is complete.
    pub fn holds_own_password(self) -> bool {
        !matches!(self, Self::Unconfirmed | Self::ForceChangePassword)
    }

    /// The wire value of this status.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unconfirmed => "UNCONFIRMED",
            Self::Confirmed => "CONFIRMED",
            Self::Archived => "ARCHIVED",
            Self::Compromised => "COMPROMISED",
            Self::Unknown => "UNKNOWN",
            Self::ResetRequired => "RESET_REQUIRED",
            Self::ForceChangePassword => "FORCE_CHANGE_PASSWORD",
            Self::ExternalProvider => "EXTERNAL_PROVIDER",
        }
    }

    /// The status `wire` names, or `None` for a value `UserStatusType` does not
    /// define.
    ///
    /// The inverse of [`UserStatus::as_str`], for the one place a status
    /// arrives as text from outside this service: the `finalUserStatus` a
    /// migrate-user function answers with.
    pub fn named(wire: &str) -> Option<Self> {
        [
            Self::Unconfirmed,
            Self::Confirmed,
            Self::Archived,
            Self::Compromised,
            Self::Unknown,
            Self::ResetRequired,
            Self::ForceChangePassword,
            Self::ExternalProvider,
        ]
        .into_iter()
        .find(|status| status.as_str() == wire)
    }
}

/// One stored user of a user pool.
#[derive(Debug, Clone)]
pub struct User {
    pub username: String,
    /// The user's immutable id, also its `sub` attribute.
    pub sub: String,
    /// Attributes keyed by name, `sub` included, ordered so a read is stable.
    pub attributes: BTreeMap<String, String>,
    /// The values this user has asked for but not confirmed yet, keyed by
    /// attribute name: a pool that requires verification before an update
    /// "doesn't change the value of the attribute until your user responds to
    /// the verification message and confirms the new value"
    /// (`UserAttributeUpdateSettings.AttributesRequireVerificationBeforeUpdate`),
    /// and `VerifyUserAttribute` "updates the affected attribute to its pending
    /// value" (this service's model). Never rendered: a pending value is not
    /// the user's value yet.
    pub pending_attributes: BTreeMap<String, String>,
    pub password: Option<String>,
    /// The passwords this user held BEFORE the one it holds now, most recently
    /// superseded first: "users can't set a password that matches any of `n`
    /// previous passwords, where `n` is the value of `PasswordHistorySize`"
    /// (`PasswordPolicyType.PasswordHistorySize` in this service's model), so
    /// every path that replaces a password records the one it replaced here
    /// ([`User::set_password`]). Never rendered — no modelled member reports a
    /// password — and bounded by [`MAX_PASSWORD_HISTORY_PER_USER`].
    pub password_history: Vec<String>,
    /// Whether [`User::password`] is permanent; a temporary one must be changed
    /// at the next sign-in and expires with the pool's policy.
    pub password_permanent: bool,
    pub password_set_at: Option<SystemTime>,
    pub status: UserStatus,
    pub enabled: bool,
    pub mfa_options: Vec<MFAOptionType>,
    pub preferred_mfa_setting: Option<String>,
    pub mfa_setting_list: Vec<String>,
    pub user_create_date: SystemTime,
    pub user_last_modified_date: SystemTime,
    pub groups: BTreeSet<String>,
    /// The devices remembered for this user, keyed by `DeviceKey` and ordered so
    /// a listing of several is stable. `DeviceKeyType` carries no
    /// case-insensitivity trait, so a key is matched exactly as it was stored.
    pub devices: BTreeMap<String, Device>,
    /// The verification codes outstanding for this user, keyed by what each was
    /// issued for and ordered so a read is stable.
    ///
    /// A fresh code for one purpose REPLACES the one issued for it before, so a
    /// user holds at most one code per purpose and this map is bounded by
    /// [`CodePurpose`]'s variants and the attributes a message is sent for at all
    /// ([`delivery_medium`]).
    pub outstanding_codes: BTreeMap<CodePurpose, VerificationCode>,
    /// The passkey credentials registered to this user, in REGISTRATION order:
    /// `WebAuthnCredentialDescription.CreatedAt` is "the date and time when the
    /// item was created" (this service's model), and a completed registration
    /// appends, so the order the credentials are held in is the order they were
    /// created in. Bounded by [`MAX_WEB_AUTHN_CREDENTIALS_PER_USER`].
    pub web_authn_credentials: Vec<WebAuthnCredential>,
    /// The external identity-provider identities linked to this local profile,
    /// keyed by the identity: `AdminLinkProviderForUser` "links an existing
    /// user account in a user pool, or `DestinationUser`, to an identity from
    /// an external IdP, or `SourceUser`" (this service's model), so a link is
    /// held on the local profile it resolves to and is gone once that profile
    /// is. Ordered, so a read of several is stable, and bounded by
    /// [`MAX_LINKED_PROVIDER_IDENTITIES`]. Rendered as the profile's
    /// [`IDENTITIES_ATTRIBUTE`].
    pub provider_links: BTreeMap<ProviderIdentity, LinkedProvider>,
    /// Whether this user has REGISTERED a time-based one-time password (TOTP)
    /// authenticator: "users must register a TOTP authenticator before they set
    /// this as their preferred MFA method"
    /// (`SetUserMFAPreference.SoftwareTokenMfaSettings` in this service's model),
    /// so this is the delivery configuration TOTP MFA is delivered with
    /// ([`User::holds_mfa_delivery_config`]). A user registers one by associating
    /// a software token and verifying a code from it, so `false` until that
    /// registration succeeds.
    pub software_token_registered: bool,
    /// The private key of the software token this user is setting up, as the last
    /// association minted it ([`SoftwareTokenKey`]): "a unique private key that
    /// Amazon Cognito generates and returns in the API response"
    /// (`AssociateSoftwareToken` in this service's model), and `None` for a user
    /// no association has minted one for.
    ///
    /// It is the key a presented one-time password is checked against
    /// ([`User::software_token_step_of`]), which is what registers the
    /// authenticator, so it is held beside the registration rather than with the
    /// call that minted it. Never rendered: no modelled member reports a user's
    /// key, and the one response that carries it is the association's own.
    pub software_token_key: Option<SoftwareTokenKey>,
    /// The time step of the LAST one-time password this user's software token
    /// SPENT, or `None` while it has spent none.
    ///
    /// One time step's password is good for ONE use: "the verifier MUST NOT
    /// accept the second attempt of the OTP generated for the same time step"
    /// (RFC 6238 §5.2, the resynchronisation rule the drift window comes from),
    /// so a code presented again after it AUTHENTICATED proves nothing more.
    /// What spends a step is the sign-in a code completes
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::answer_mfa_challenge`]),
    /// so a code accepted by one challenge response completes no second one.
    /// Registering the authenticator spends none
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::register_software_token`]):
    /// it proves possession of the key rather than authenticating anything, and
    /// the setup it completes is answered by the registration itself. Never
    /// rendered: no modelled member reports it.
    pub software_token_spent_step: Option<u64>,
    /// Whether this user has activated or deactivated passkey MFA for itself
    /// (`SetUserMFAPreference.WebAuthnMfaSettings.Enabled`), or `None` while the
    /// user has expressed no preference and the pool's passkey configuration
    /// alone decides ([`User::activates_passkey_mfa`]).
    pub web_authn_mfa_enabled: Option<bool>,
    /// The authentication events this user's pool LOGGED about it, oldest first:
    /// the history "of user activity and any risks detected as part of Amazon
    /// Cognito threat protection" (`AdminListUserAuthEvents` in this service's
    /// model). A pool whose threat protection is off logs none, the reader serves
    /// them most recent first ([`User::auth_events_newest_first`]) and the log is
    /// bounded by [`auth_events::MAX_AUTH_EVENTS_PER_USER`]. It belongs to the
    /// user, so a deleted user's history is gone with it.
    pub auth_events: Vec<auth_events::AuthEvent>,
}

impl User {
    /// The stored attributes as the modelled list, in name order, with the
    /// [`IDENTITIES_ATTRIBUTE`] this profile's provider links render among
    /// them.
    pub fn attribute_list(&self) -> Vec<AttributeType> {
        let mut attributes: Vec<AttributeType> = self
            .attributes
            .iter()
            .map(|(name, value)| AttributeType {
                name: name.clone(),
                value: Some(value.clone()),
            })
            .collect();
        if let Some(identities) = self.identities_attribute() {
            attributes.push(AttributeType {
                name: IDENTITIES_ATTRIBUTE.to_string(),
                value: Some(identities),
            });
            attributes.sort_by(|left, right| left.name.cmp(&right.name));
        }
        attributes
    }

    /// The value this profile's [`IDENTITIES_ATTRIBUTE`] renders: the links it
    /// holds as the JSON array Amazon Cognito reports them in, oldest link
    /// first. `None` for a profile that holds no link, which renders no such
    /// attribute at all.
    pub fn identities_attribute(&self) -> Option<String> {
        if self.provider_links.is_empty() {
            return None;
        }
        let mut links: Vec<(&ProviderIdentity, &LinkedProvider)> =
            self.provider_links.iter().collect();
        links.sort_by(|(left_identity, left), (right_identity, right)| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left_identity.cmp(right_identity))
        });
        let rendered: Vec<serde_json::Value> = links
            .into_iter()
            .map(|(identity, link)| {
                serde_json::json!({
                    // The identity's own value is the id the external IdP knows
                    // this user by, and the provider is the pool IdP it
                    // federates through, whose stored type is reported beside
                    // it.
                    "userId": identity.provider_attribute_value,
                    "providerName": identity.provider_name,
                    "providerType": link.provider_type,
                    // The issuer of a linked identity is known only once the
                    // external IdP has asserted one, so none is invented.
                    "issuer": serde_json::Value::Null,
                    // The local profile the link resolves to is the primary
                    // identity: a linked external identity is an additional way
                    // into that profile, not the identity that created it.
                    "primary": false,
                    "dateCreated": unix_millis(link.created_at),
                })
            })
            .collect();
        Some(serde_json::Value::Array(rendered).to_string())
    }

    /// The modelled view of this user, rendered from what is stored.
    pub fn to_user_type(&self) -> UserType {
        UserType {
            username: Some(self.username.clone()),
            attributes: Some(self.attribute_list()),
            user_create_date: Some(to_date_time(self.user_create_date)),
            user_last_modified_date: Some(to_date_time(self.user_last_modified_date)),
            enabled: self.enabled,
            user_status: Some(self.status.as_str().to_string()),
            mfa_options: non_empty(&self.mfa_options),
        }
    }

    /// Whether this user's stored value of `name` is an ACTIVE alias value: an
    /// `email` or a `phone_number` counts only while its verified flag says the
    /// value was confirmed ([`verified_flag`]), since an unconfirmed value is
    /// not one a sign-in resolves; an alias attribute that carries no such flag
    /// counts whatever its value.
    pub fn alias_is_active(&self, name: &str) -> bool {
        match verified_flag(name) {
            Some(flag) => self
                .attributes
                .get(flag)
                .is_some_and(|value| value == FLAG_CONFIRMED),
            None => true,
        }
    }

    /// The keys of the devices this user's OWN device surface reports, in
    /// ascending order — the stable order a paginated self-service listing
    /// indexes into.
    ///
    /// `ListDevices` lists "the devices that Amazon Cognito has registered to
    /// the currently signed-in user", `ConfirmDevice` "confirms a device that a
    /// user wants to remember" and `UpdateDeviceStatus` marks a device "as
    /// remembered or not remembered" (this service's model), so the
    /// self-service surface answers for the devices the user still REMEMBERS:
    /// a device a sign-in only minted a key for is not reported until it is
    /// confirmed, and one whose remembered status was cleared leaves the
    /// listing. The admin surface is wider — `AdminGetDevice` "returns details
    /// for a user's device" and reports a registered device whatever its
    /// remembered status ([`User::confirmed_device_keys`]) — exactly as
    /// [`User::remembered_device`] is narrower than [`User::confirmed_device`].
    pub fn device_keys(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|(_, device)| device.confirmed && device.remembered)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The keys of every device REGISTERED to this user, in ascending order:
    /// what the admin device surface reports, whether or not the user still
    /// remembers each ([`User::device_keys`]).
    pub fn confirmed_device_keys(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|(_, device)| device.confirmed)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Store `options` as this user's SMS MFA configuration, as of `now`.
    ///
    /// The submitted list REPLACES what the user held, so an empty list leaves
    /// the user with no MFA option at all. SMS MFA is activated exactly while an
    /// option is stored for it, so `UserMFASettingList` gains
    /// [`SMS_MFA_SETTING`] with the first option and loses it with the last, and
    /// a factor that is not activated is not one the user prefers, so a
    /// preference for SMS MFA goes with it. Every other factor's setting is
    /// untouched: an `MFAOptionType` configures SMS and nothing else.
    pub fn set_sms_mfa_options(&mut self, options: Vec<MFAOptionType>, now: SystemTime) {
        let activated = !options.is_empty();
        self.mfa_options = options;
        self.mfa_setting_list
            .retain(|setting| setting != SMS_MFA_SETTING);
        if activated {
            self.mfa_setting_list.push(SMS_MFA_SETTING.to_string());
        } else if self.preferred_mfa_setting.as_deref() == Some(SMS_MFA_SETTING) {
            self.preferred_mfa_setting = None;
        }
        // A successful call stores the submitted configuration over the one the
        // user held, which is a change to the user record itself.
        self.user_last_modified_date = now;
    }

    /// Whether this user has ACTIVATED `factor` as an MFA factor.
    ///
    /// `UserMFASettingList` reports "the MFA options that are activated for the
    /// user. The possible values in this list are `SMS_MFA`, `EMAIL_OTP`, and
    /// `SOFTWARE_TOKEN_MFA`", and `ConfiguredUserAuthFactors` reports the same
    /// factors under `AuthFactorType`'s names (both in this service's model), so
    /// each activated setting activates the factor it maps to
    /// ([`mfa_auth_factor_of`]) and a setting that maps to none activates nothing.
    pub fn activates_mfa_auth_factor(&self, factor: &str) -> bool {
        self.mfa_setting_list
            .iter()
            .any(|setting| mfa_auth_factor_of(setting.as_str()) == Some(factor))
    }

    /// Whether this user has SET `factor` UP as an MFA factor: it holds the
    /// delivery configuration that factor reaches it through
    /// ([`User::holds_mfa_delivery_config`]), whether or not it has ACTIVATED
    /// the setting ([`User::activates_mfa_auth_factor`]).
    ///
    /// `ConfiguredUserAuthFactors` reports "the authentication types that are
    /// available to the user with their current configuration" (this service's
    /// model), and what a factor is configured WITH is what the user holds for
    /// it: a registered TOTP authenticator is the software token's configuration
    /// from the registration on, so it is reported from then on rather than only
    /// once a preference activates it. A factor whose configuration the user
    /// does not hold — an unregistered authenticator, an unverified address — is
    /// set up by nothing.
    pub fn sets_up_mfa_auth_factor(&self, factor: &str) -> bool {
        MFA_SETTING_ORDER.iter().any(|setting| {
            mfa_auth_factor_of(setting) == Some(factor) && self.holds_mfa_delivery_config(setting)
        })
    }

    /// Whether this user is SET UP to present `factor` as a first authentication
    /// factor of a choice-based (`USER_AUTH`) sign-in.
    ///
    /// Each factor is presented with something the user has to hold first: the
    /// password factor with a stored password, a one-time-password factor with an
    /// address the code can be addressed to ([`User::destination_of`]), and the
    /// passkey factor with a registered passkey. `SOFTWARE_TOKEN` "can only be
    /// used as an MFA factor" (`ConfiguredUserAuthFactors` in this service's
    /// model), so nothing the user configures makes it a first factor; it is
    /// reported as the MFA factor it is set up as
    /// ([`User::sets_up_mfa_auth_factor`]).
    ///
    /// This is what the USER is configured for, not what a sign-in attempt would
    /// be allowed to do with it: whether the pool ADMITS the factor as a first one
    /// (`SignInPolicyType.AllowedFirstAuthFactors`,
    /// [`UserPool::allows_first_auth_factor`]) and whether the address has been
    /// proven ([`User::verified_destination_of`]) are read where the factor is
    /// actually presented, not here. The one switch the USER holds over a factor
    /// it has configured is passkey MFA — `WebAuthnMfaSettings.Enabled`
    /// "specifies whether passkey MFA is activated for a user" (this service's
    /// model) — so a user that deactivated it has switched its registered passkey
    /// off ([`User::activates_passkey_mfa`]) and no passkey factor is reported for
    /// it.
    pub fn configures_auth_factor(&self, factor: &str) -> bool {
        match factor {
            PASSWORD_AUTH_FACTOR => self.password.is_some(),
            EMAIL_OTP_AUTH_FACTOR => self.destination_of("email").is_some(),
            SMS_OTP_AUTH_FACTOR => self.destination_of("phone_number").is_some(),
            WEB_AUTHN_AUTH_FACTOR => {
                !self.web_authn_credentials.is_empty() && self.activates_passkey_mfa()
            }
            _ => false,
        }
    }

    /// Whether this user holds the delivery configuration the
    /// `UserMFASettingList` setting `setting` is delivered with.
    ///
    /// Each of the three factors reaches the user through something the user has
    /// to hold first: SMS message MFA through the verified phone number the text
    /// message is sent to, email message MFA through the verified email address
    /// the message is sent to ([`User::verified_destination_of`]), and TOTP MFA
    /// through the registered authenticator the one-time password is read from
    /// ([`User::software_token_registered`]). A value that names none of the three
    /// settings is delivered by nothing.
    pub fn holds_mfa_delivery_config(&self, setting: &str) -> bool {
        match setting {
            SMS_MFA_SETTING => self.verified_destination_of("phone_number").is_some(),
            EMAIL_OTP_MFA_SETTING => self.verified_destination_of("email").is_some(),
            SOFTWARE_TOKEN_MFA_SETTING => self.software_token_registered,
            _ => false,
        }
    }

    /// Apply `requested` as this user's MFA preference as of `now`, or report why
    /// it is refused. `offered` is what the user's POOL turns on
    /// ([`UserPool::mfa_factors_offered`]).
    ///
    /// Each submitted setting REPLACES what this user held for the factor it
    /// configures, and a factor the request names none of is left exactly as it
    /// was. Activating a factor needs the delivery configuration it reaches the
    /// user through ([`User::holds_mfa_delivery_config`]), so a request that asks
    /// for one the user cannot receive is refused for the precondition that is
    /// missing ([`MfaPreferenceRejection::for_activation`]); and a factor the
    /// request does
    /// not activate is not one it can also make PREFERRED, so a setting that asks
    /// for the preference without the activation is refused for the preference
    /// itself. The WHOLE request is checked before anything is written, so a
    /// refused one leaves this user untouched.
    ///
    /// "Only one factor can be set as preferred" (`SetUserMFAPreference` in this
    /// service's model), so the preference a request expresses replaces the one
    /// the user held, and a request expressing TWO of them is refused for that
    /// conflict before any factor's own availability is looked at; and a factor
    /// that is not activated is not one the user prefers, so deactivating the
    /// preferred factor, or submitting it as no longer preferred, leaves the user
    /// with no preference at all.
    pub fn set_mfa_preference(
        &mut self,
        requested: &[MfaSettingRequest<'_>],
        offered: MfaFactorsOffered,
        now: SystemTime,
    ) -> Result<(), MfaPreferenceRejection> {
        // "Only one factor can be set as preferred" (`SetUserMFAPreference` in
        // this service's model), so a request marking two of them preferred
        // conflicts with ITSELF: it is judged before anything the user holds or
        // any one factor is looked at, and reported as that conflict, whichever
        // factors carry it and whatever the pool has configured for them.
        if requested
            .iter()
            .filter(|setting| setting.preferred)
            .take(2)
            .count()
            > 1
        {
            return Err(MfaPreferenceRejection::TwoPreferred);
        }
        // A factor the request does not ACTIVATE is not one it can also make
        // preferred, whatever the user holds for it, so that is judged next and
        // reported as the preference it refuses.
        if requested
            .iter()
            .any(|setting| setting.preferred && !setting.activates())
        {
            return Err(MfaPreferenceRejection::PreferredWhileDisabled);
        }
        if let Some(refused) = requested
            .iter()
            .find(|setting| setting.activates() && !self.holds_mfa_delivery_config(setting.setting))
        {
            return Err(MfaPreferenceRejection::for_activation(
                refused.setting,
                offered,
            ));
        }
        for setting in requested {
            if setting.setting == SMS_MFA_SETTING {
                // SMS MFA is activated exactly while an MFA option is stored for
                // it ([`User::set_sms_mfa_options`]), so activating the factor
                // stores the one option that configures it and deactivating the
                // factor stores none.
                let options = if setting.enabled {
                    vec![sms_mfa_option()]
                } else {
                    Vec::new()
                };
                self.set_sms_mfa_options(options, now);
            } else {
                self.mfa_setting_list.retain(|held| held != setting.setting);
                if setting.enabled {
                    self.mfa_setting_list.push(setting.setting.to_string());
                }
            }
            if setting.enabled && setting.preferred {
                self.preferred_mfa_setting = Some(setting.setting.to_string());
            } else if self.preferred_mfa_setting.as_deref() == Some(setting.setting) {
                self.preferred_mfa_setting = None;
            }
        }
        if !requested.is_empty() {
            self.user_last_modified_date = now;
        }
        Ok(())
    }

    /// Register `credential` to this user, APPENDING it: the credentials are
    /// held in registration order ([`User::web_authn_credentials`]), so the
    /// passkey registered last is read last.
    pub fn register_web_authn_credential(&mut self, credential: WebAuthnCredential) {
        self.web_authn_credentials.push(credential);
    }

    /// Whether this user already holds the passkey credential `credential_id`.
    pub fn holds_web_authn_credential(&self, credential_id: &str) -> bool {
        self.web_authn_credentials
            .iter()
            .any(|credential| credential.credential_id == credential_id)
    }

    /// Delete the passkey credential `credential_id` from this user, reporting
    /// whether one was held under that id.
    ///
    /// A credential id identifies ONE passkey, so the FIRST credential held
    /// under that id is the one removed and the credentials that remain keep
    /// their registration order ([`User::web_authn_credentials`]).
    pub fn delete_web_authn_credential(&mut self, credential_id: &str) -> bool {
        let Some(held) = self
            .web_authn_credentials
            .iter()
            .position(|credential| credential.credential_id == credential_id)
        else {
            return false;
        };
        self.web_authn_credentials.remove(held);
        true
    }

    /// Store whether passkey MFA is activated for this user, as of `now`.
    pub fn set_web_authn_mfa_enabled(&mut self, enabled: bool, now: SystemTime) {
        self.web_authn_mfa_enabled = Some(enabled);
        self.user_last_modified_date = now;
    }

    /// Whether passkey MFA is activated for this user.
    ///
    /// `WebAuthnMfaSettings.Enabled` "specifies whether passkey MFA is activated
    /// for a user" (this service's model), so a user that deactivated it presents
    /// no passkey as an MFA factor. A user that expressed no preference leaves the
    /// pool's passkey configuration to decide ([`UserPool::passkeys_satisfy_mfa`]).
    pub fn activates_passkey_mfa(&self) -> bool {
        self.web_authn_mfa_enabled.is_none_or(|enabled| enabled)
    }

    /// Replace this user's password with `password`, as of `now`, recording the
    /// password it replaces in this user's history.
    ///
    /// Every path that sets a password for an EXISTING user goes through here,
    /// so the history a pool's `PasswordHistorySize` is read against
    /// ([`UserPool::restricts_password_reuse`]) holds every password the user
    /// has had, whichever operation set it. `permanent` is whether the user may
    /// sign in with it straight away or owes a change at its next sign-in
    /// ([`User::password_permanent`]).
    pub fn set_password(&mut self, password: String, permanent: bool, now: SystemTime) {
        if let Some(replaced) = self.password.replace(password) {
            self.password_history.insert(0, replaced);
            self.password_history.truncate(MAX_PASSWORD_HISTORY_PER_USER);
        }
        self.password_permanent = permanent;
        self.password_set_at = Some(now);
        self.user_last_modified_date = now;
    }

    /// This user's passwords, most recently set FIRST: the one it holds now,
    /// then the ones it held before it, as far back as
    /// [`MAX_PASSWORD_HISTORY_PER_USER`] keeps them.
    pub fn recent_passwords(&self) -> impl Iterator<Item = &str> {
        self.password
            .as_deref()
            .into_iter()
            .chain(self.password_history.iter().map(String::as_str))
    }

    /// Whether the stored password is a temporary one that has outlived
    /// `validity` as of `now`.
    pub fn temporary_password_expired(&self, validity: Duration, now: SystemTime) -> bool {
        if self.password_permanent {
            return false;
        }
        // The deadline is computed with the CHECKED addition: a window whose end
        // is not a representable instant is a deadline that never arrives, so
        // such a password is not expired rather than a failure of this read.
        self.password_set_at
            .and_then(|set_at| set_at.checked_add(validity))
            .is_some_and(|deadline| now > deadline)
    }

    /// Store `device` for this user, evicting the device authenticated longest
    /// ago while the user is already at [`MAX_DEVICES_PER_USER`].
    pub fn store_device(&mut self, device: Device) {
        while self.devices.len() >= MAX_DEVICES_PER_USER {
            let Some(oldest) = self
                .devices
                .iter()
                .min_by_key(|(_, stored)| stored.last_authenticated_date)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.devices.remove(&oldest);
        }
        self.devices.insert(device.device_key.clone(), device);
    }

    /// The stored device `device_key` names, once `ConfirmDevice` has confirmed
    /// it. `GetDevice` "returns information about a remembered device" and
    /// `ConfirmDevice` "confirms a device that a user wants to remember" (this
    /// service's model), so a device a sign-in minted and nothing confirmed is
    /// not yet a device of this user pool.
    pub fn confirmed_device(&self, device_key: &str) -> Option<&Device> {
        self.devices
            .get(device_key)
            .filter(|device| device.confirmed)
    }

    /// The stored device `device_key` names, mutably, once it is confirmed.
    pub fn confirmed_device_mut(&mut self, device_key: &str) -> Option<&mut Device> {
        self.devices
            .get_mut(device_key)
            .filter(|device| device.confirmed)
    }

    /// The stored device `device_key` names, once it is confirmed AND remembered.
    ///
    /// The self-service reader is narrower than the admin one: `GetDevice`
    /// "returns information about a remembered device for the current user",
    /// while `AdminGetDevice` "returns details for a user's device" and
    /// `ListDevices` lists "the devices that Amazon Cognito has registered to the
    /// currently signed-in user" (this service's model). So a device the user has
    /// not (yet) chosen to remember is registered and listed, but is not a
    /// remembered device.
    pub fn remembered_device(&self, device_key: &str) -> Option<&Device> {
        self.confirmed_device(device_key)
            .filter(|device| device.remembered)
    }

    /// The stored value of `attribute` a message can be addressed to, or `None`
    /// when this user holds no value for it: a code is a message, so it needs an
    /// address to be sent to.
    pub fn destination_of(&self, attribute: &str) -> Option<String> {
        self.attributes
            .get(attribute)
            .filter(|value| !value.is_empty())
            .cloned()
    }

    /// The stored value of `attribute` whose verified flag says this user
    /// confirmed it ([`verified_flag`]) \u2014 the only kind of destination a
    /// password-reset message is sent to.
    pub fn verified_destination_of(&self, attribute: &str) -> Option<String> {
        self.destination_of(attribute)
            .filter(|_| self.alias_is_active(attribute))
    }

    /// Record `code` as the code outstanding for `purpose`, REPLACING whatever
    /// was outstanding for it before \u2014 which is what makes a re-sent code
    /// supersede the one before it \u2014 and return what was stored.
    pub fn issue_code(&mut self, purpose: CodePurpose, code: VerificationCode) -> VerificationCode {
        self.outstanding_codes.insert(purpose, code.clone());
        code
    }

    /// The code outstanding for `purpose`, or `None` when nothing is.
    pub fn outstanding_code(&self, purpose: &CodePurpose) -> Option<&VerificationCode> {
        self.outstanding_codes.get(purpose)
    }

    /// Check `presented` against the code outstanding for `purpose` as of `now`.
    ///
    /// A PURE read: accepting a code is what spends it ([`User::consume_code`]),
    /// so a refused attempt leaves the outstanding code exactly as it was and a
    /// caller can present the right one afterwards.
    ///
    /// A presented value is compared only against a LIVE code. Where none is
    /// live for `purpose` — none was ever issued, the one that was has been
    /// spent, or its validity window has closed — there is nothing to compare
    /// against at all, so the answer is [`CodeRejection::Expired`]: the
    /// remediation is to ask for a code. Only a value that differs from a code
    /// that IS live, a code a later one superseded among them, is the
    /// [`CodeRejection::Mismatch`] whose remediation is to present the right
    /// code.
    pub fn check_code(
        &self,
        purpose: &CodePurpose,
        presented: &str,
        now: SystemTime,
    ) -> Result<&VerificationCode, CodeRejection> {
        let Some(outstanding) = self.outstanding_code(purpose) else {
            return Err(CodeRejection::Expired);
        };
        if outstanding.expired(now) {
            return Err(CodeRejection::Expired);
        }
        if outstanding.code != presented {
            return Err(CodeRejection::Mismatch);
        }
        Ok(outstanding)
    }

    /// Spend the code outstanding for `purpose`: a delivered code is accepted
    /// ONCE, so presenting it again finds nothing outstanding.
    pub fn consume_code(&mut self, purpose: &CodePurpose) -> Option<VerificationCode> {
        self.outstanding_codes.remove(purpose)
    }

    /// Write `plan` ([`plan_attribute_update`]) onto this user and return the
    /// delivery of each verification message it sent, in the order the plan
    /// lists them.
    ///
    /// The plan was decided against this user as it stood, so applying it never
    /// refuses anything: a request is judged in full BEFORE any of it is
    /// written, and only a plan reaches here.
    pub fn apply_attribute_update(
        &mut self,
        plan: &AttributeUpdatePlan,
        now: SystemTime,
    ) -> Vec<CodeDeliveryDetailsType> {
        for update in &plan.updates {
            match &update.value {
                // A blank value deletes the attribute, so neither a stored nor
                // a pending value of it survives.
                None => {
                    self.attributes.remove(&update.name);
                    self.pending_attributes.remove(&update.name);
                }
                // A pool that requires verification first keeps the stored
                // value and holds the new one pending.
                Some(value) if update.deferred => {
                    self.pending_attributes
                        .insert(update.name.clone(), value.clone());
                }
                Some(value) => {
                    self.attributes.insert(update.name.clone(), value.clone());
                    self.pending_attributes.remove(&update.name);
                }
            }
            if let Some(flag) = update.stale_verified_flag {
                if update.value.is_none() {
                    // The attribute it vouches for is gone, so the flag has
                    // nothing left to describe.
                    self.attributes.remove(flag);
                } else {
                    self.attributes
                        .insert(flag.to_string(), FLAG_UNCONFIRMED.to_string());
                }
            }
        }
        // A call that names no attribute to change modifies nothing, so the
        // user's last-modified instant stands.
        if !plan.updates.is_empty() {
            self.user_last_modified_date = now;
        }

        // A reported delivery is a code that was SENT, so each one is minted
        // and recorded against the attribute it was sent for — which is the
        // code `VerifyUserAttribute` accepts for it, and which supersedes any
        // code outstanding for that attribute before. The destination is the
        // value the message went to: the pending one where the pool holds the
        // update until the user confirms it.
        let mut deliveries = Vec::new();
        for (attribute, value) in &plan.verifications {
            let code = VerificationCode::issued(attribute, value, now);
            let stored = self.issue_code(CodePurpose::AttributeVerification(attribute.clone()), code);
            deliveries.extend(stored.delivery_details());
        }
        deliveries
    }

    /// Write `plan` ([`plan_attribute_deletion`]) onto this user.
    ///
    /// The plan was decided against this user as it stood, so applying it never
    /// refuses anything: a request is judged in full BEFORE any of it is
    /// written, and only a plan reaches here.
    pub fn apply_attribute_deletion(&mut self, plan: &AttributeDeletionPlan, now: SystemTime) {
        // Naming an attribute the user holds no value of removes nothing, so it
        // is the values that WERE held that make this call a modification.
        let mut removed = false;
        for name in &plan.deletions {
            removed |= self.attributes.remove(name).is_some();
            // A value asked for but not confirmed yet is a value of the same
            // attribute, so it goes with the attribute rather than outliving it.
            removed |= self.pending_attributes.remove(name).is_some();
        }
        // A call that removed nothing changed nothing, so the user's
        // last-modified instant stands.
        if removed {
            self.user_last_modified_date = now;
        }
    }
}

/// One stored link from an external IdP identity to a local profile: what the
/// identity's own triple does not carry, so a read can render the link.
#[derive(Debug, Clone)]
pub struct LinkedProvider {
    /// The `ProviderType` of the pool IdP the identity federates through, as
    /// `CreateIdentityProvider` stored it.
    pub provider_type: String,
    /// When the link was made, which is what the rendered identity reports as
    /// its creation instant.
    pub created_at: SystemTime,
}

/// The attribute a local profile reports its provider links under: Amazon
/// Cognito renders a linked user's identities as a JSON array under the
/// `identities` attribute name, so a profile nothing is linked to reports no
/// such attribute.
pub const IDENTITIES_ATTRIBUTE: &str = "identities";

/// The username a user of a third-party IdP signs in under: the provider's name
/// and the user's id with that provider, joined by `_` — the form a `Username`
/// naming "the username of a user from a third-party IdP" (`AdminGetUser.Username`
/// in this service's model) carries. The ONE place the two parts are joined, so
/// every lookup of a federated username agrees
/// ([`UserPool::federated_user_key`]).
#[must_use]
pub fn federated_username(provider_name: &str, provider_user_id: &str) -> String {
    format!("{provider_name}_{provider_user_id}")
}

/// One provider identity: the (`ProviderName`, `ProviderAttributeName`,
/// `ProviderAttributeValue`) triple a `ProviderUserIdentifierType` names.
///
/// It is the key a link is stored and looked up under, because the attribute
/// identifying the external user is part of the identity: de-linking requires
/// that "the `ProviderAttributeName` and `ProviderAttributeValue` must be the
/// same values that were used for the `SourceUser` when the identities were
/// originally linked" (`AdminDisableProviderForUser` in this service's model).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProviderIdentity {
    pub provider_name: String,
    pub provider_attribute_name: String,
    pub provider_attribute_value: String,
}

/// The member of a `ProviderUserIdentifierType` a submitted identifier left
/// out, as [`ProviderIdentity::from_identifier`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingProviderMember {
    ProviderName,
    ProviderAttributeName,
    ProviderAttributeValue,
}

impl MissingProviderMember {
    /// The modelled member name, as a refusal names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderName => "ProviderName",
            Self::ProviderAttributeName => "ProviderAttributeName",
            Self::ProviderAttributeValue => "ProviderAttributeValue",
        }
    }
}

impl ProviderIdentity {
    /// The identity `identifier` names, or the first member it left out.
    ///
    /// Every member of a `ProviderUserIdentifierType` is optional in the model,
    /// so an identifier naming no provider, no attribute or no value identifies
    /// nothing and the refusal names the member that is missing. An empty value
    /// is no value either: `ProviderNameType` starts at length 1, and an
    /// identity is what a link is keyed by.
    pub fn from_identifier(
        identifier: &ProviderUserIdentifierType,
    ) -> Result<Self, MissingProviderMember> {
        Ok(Self {
            provider_name: provider_member(&identifier.provider_name)
                .ok_or(MissingProviderMember::ProviderName)?,
            provider_attribute_name: provider_member(&identifier.provider_attribute_name)
                .ok_or(MissingProviderMember::ProviderAttributeName)?,
            provider_attribute_value: provider_member(&identifier.provider_attribute_value)
                .ok_or(MissingProviderMember::ProviderAttributeValue)?,
        })
    }

    /// The value that names the local profile a DESTINATION identifier resolves
    /// to, or the member it left out.
    ///
    /// "The `ProviderAttributeName` of the `DestinationUser` is ignored"
    /// (`AdminLinkProviderForUser` in this service's model), so a destination is
    /// read as the provider it names and the value that names the profile —
    /// each under the same rule [`ProviderIdentity::from_identifier`] applies to
    /// every member, so the two identifiers cannot disagree about what an
    /// absent member is.
    pub fn destination_value(
        identifier: &ProviderUserIdentifierType,
    ) -> Result<String, MissingProviderMember> {
        provider_member(&identifier.provider_name).ok_or(MissingProviderMember::ProviderName)?;
        provider_member(&identifier.provider_attribute_value)
            .ok_or(MissingProviderMember::ProviderAttributeValue)
    }

    /// Whether this identity names a user pool's own users rather than a
    /// third-party IdP ([`NATIVE_PROVIDER_NAME`]).
    pub fn is_native(&self) -> bool {
        self.provider_name == NATIVE_PROVIDER_NAME
    }
}

/// One member of a submitted `ProviderUserIdentifierType`, or `None` when the
/// request left it out: a member sent as whitespace or as an empty string names
/// nothing either, and `ProviderNameType` starts at length 1.
pub(super) fn provider_member(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// What one stored verification code was issued for.
///
/// Amazon Cognito sends a code for several distinct purposes, and each is
/// answered by its own operation: the sign-up confirmation code by
/// `ConfirmSignUp`, the password-reset code by `ConfirmForgotPassword` and an
/// attribute's code by `VerifyUserAttribute`. Keying the outstanding codes by
/// purpose is what keeps them independent — a resent sign-up code does not
/// disturb an outstanding reset code — while a second code for the SAME purpose
/// supersedes the first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodePurpose {
    /// The code that confirms a new user's sign-up, sent by `SignUp` and
    /// re-sent by `ResendConfirmationCode`.
    SignUpConfirmation,
    /// The code that authorises a password reset, sent by `ForgotPassword`.
    PasswordReset,
    /// The code that confirms the value of ONE attribute, sent by
    /// `GetUserAttributeVerificationCode` and by an update of an attribute the
    /// pool verifies, and carried for the attribute it names.
    AttributeVerification(String),
}

/// One verification code issued to a user: what the operation that sent the
/// code recorded, and the delivery it reported.
#[derive(Debug, Clone)]
pub struct VerificationCode {
    /// The code itself ([`new_verification_code`]). The real service delivers
    /// it out of band — by mail or text message — so no response carries it;
    /// it is stored because it is what the operation that consumes this code
    /// checks the presented one against.
    pub code: String,
    /// The user attribute the message was addressed to (`VerifiedAttributeType`),
    /// which is what `CodeDeliveryDetailsType.AttributeName` reports and what
    /// decides the medium it travelled over ([`delivery_medium`]).
    pub attribute: String,
    /// The attribute value the code was sent to: the address or number the
    /// message was addressed to, which is what
    /// `CodeDeliveryDetailsType.Destination` reports, masked.
    pub destination: String,
    /// When this code was issued, which is what its validity window is counted
    /// from.
    pub issued_at: SystemTime,
}

impl VerificationCode {
    /// A fresh verification code for `attribute`, addressed to its stored
    /// `destination`.
    pub fn issued(attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self::minted(new_verification_code(), attribute, destination, now)
    }

    /// A fresh email one-time password for `attribute`, addressed to its stored
    /// `destination`.
    pub fn issued_email_otp(attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self::minted(new_email_otp_code(), attribute, destination, now)
    }

    /// `code`, recorded as issued for `attribute` at `now`.
    pub(super) fn minted(code: String, attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self {
            code,
            attribute: attribute.to_string(),
            destination: destination.to_string(),
            issued_at: now,
        }
    }

    /// The delivery this code's message reported, rendered from what is stored.
    pub fn delivery_details(&self) -> Option<CodeDeliveryDetailsType> {
        code_delivery_details(&self.attribute, &self.destination)
    }

    /// Whether this code's validity window has closed by `now`.
    pub fn expired(&self, now: SystemTime) -> bool {
        now >= self.issued_at + VERIFICATION_CODE_VALIDITY
    }
}

/// Why a presented code was not accepted, which decides the modelled error the
/// operation that was given it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRejection {
    /// A code IS live for the purpose and the presented value is not it —
    /// including the case where a later code for the same purpose superseded
    /// the presented one.
    Mismatch,
    /// No live code is there to compare against: none was ever issued for the
    /// purpose, the one that was has been spent, or its validity window has
    /// closed.
    Expired,
}

/// How long an issued code stays acceptable: Amazon Cognito's verification and
/// confirmation codes expire 24 hours after they are sent (Amazon Cognito
/// Developer Guide, "Signing up and confirming user accounts").
pub const VERIFICATION_CODE_VALIDITY: Duration = Duration::from_secs(24 * 60 * 60);

/// The message a value that is not the live code is refused with
/// ([`CodeRejection::Mismatch`]): it asks for the code to be presented again,
/// because one is there to present.
pub const CODE_MISMATCH_MESSAGE: &str = "Invalid verification code provided, please try again.";

/// The message a check against no live code is refused with
/// ([`CodeRejection::Expired`]): it asks for a code to be REQUESTED, because
/// there is none to present.
pub const EXPIRED_CODE_MESSAGE: &str = "Invalid code provided, please request a code again.";

/// One device of a user: the device a sign-in minted a `DeviceKey` for, as the
/// device operations stored it, and what every later device read of this user
/// renders.
///
/// A sign-in on a pool that remembers devices stores one of these for a device
/// it does not recognise; `ConfirmDevice` is what confirms it — which is what
/// makes the user pool report it — and what decides whether it is remembered.
#[derive(Debug, Clone)]
pub struct Device {
    /// The key this device is known by (`DeviceKeyType`, len 1..55, pattern
    /// `^[\w-]+_[0-9a-f-]+$`), which every device operation names it with.
    pub device_key: String,
    /// The device's attributes keyed by name, ordered so a read is stable.
    pub attributes: BTreeMap<String, String>,
    /// The SRP verifier `ConfirmDevice` registered for this device, when it
    /// carried one. It is what the device's own authentication checks: the
    /// proof of possession a remembered device owes runs over it
    /// ([`crate::state::device_auth`]).
    pub secret_verifier: Option<DeviceSecretVerifierConfigType>,
    /// Whether `ConfirmDevice` has confirmed this device, which is what makes it
    /// a device the user pool reports.
    pub confirmed: bool,
    /// Whether this device is remembered, i.e. eligible for device
    /// authentication: `ConfirmDevice` sets it unless the pool asks for the
    /// user's own prompt first, and `UpdateDeviceStatus` is what the user's
    /// answer reaches.
    pub remembered: bool,
    pub create_date: SystemTime,
    pub last_modified_date: SystemTime,
    /// When this device last authenticated a sign-in, absent until one has:
    /// `DeviceType.DeviceLastAuthenticatedDate` is "the date when the device was
    /// last authenticated" (this service's model), which is nothing yet for a
    /// device that has not authenticated.
    pub last_authenticated_date: Option<SystemTime>,
}

impl Device {
    /// The device a sign-in just stored: known to this user pool, not confirmed
    /// and so not remembered, and last authenticated at that sign-in.
    pub fn signed_in(device_key: String, now: SystemTime) -> Self {
        Self {
            device_key,
            attributes: BTreeMap::new(),
            secret_verifier: None,
            confirmed: false,
            remembered: false,
            create_date: now,
            last_modified_date: now,
            last_authenticated_date: Some(now),
        }
    }

    /// The stored attributes as the modelled list, in name order.
    pub fn attribute_list(&self) -> Vec<AttributeType> {
        self.attributes
            .iter()
            .map(|(name, value)| AttributeType {
                name: name.clone(),
                value: Some(value.clone()),
            })
            .collect()
    }

    /// The modelled view of this device, rendered from what is stored.
    pub fn to_device_type(&self) -> DeviceType {
        DeviceType {
            device_key: Some(self.device_key.clone()),
            device_attributes: non_empty(&self.attribute_list()),
            device_create_date: Some(to_date_time(self.create_date)),
            device_last_modified_date: Some(to_date_time(self.last_modified_date)),
            device_last_authenticated_date: self.last_authenticated_date.map(to_date_time),
        }
    }
}

/// The `DeviceAttributes` name a device's `ConfirmDevice.DeviceName` — "A
/// friendly name for the device" — is reported under.
pub const DEVICE_NAME_ATTRIBUTE: &str = "device_name";

/// `DeviceRememberedStatusType`'s value for a device that is remembered, i.e.
/// eligible for device authentication; its only other value is the one for a
/// device that is not (`crate::ops::types::DEVICE_REMEMBERED_STATUS_TYPE_VALUES`).
pub const DEVICE_REMEMBERED_STATUS: &str = "remembered";

/// Why an access token does not identify a signed-in user
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::signed_in_user`]).
/// The operation that asked maps each one to a modelled error IT declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// Not a token this emulator issued, or not an ACCESS token.
    NotAnAccessToken,
    /// A compact JWS naming a pool of this service as its issuer whose SIGNATURE does
    /// not check out against the key set that pool publishes — a token rewritten after
    /// it was signed, or one signed by a key the pool does not publish
    /// ([`ACCESS_TOKEN_SIGNATURE_UNVERIFIED_MESSAGE`]).
    SignatureNotVerified,
    /// An access token this emulator issued and has since revoked, which
    /// authorizes nothing ([`ACCESS_TOKEN_REVOKED_MESSAGE`]).
    Revoked,
    /// An access token whose lifetime the clock is past.
    Expired,
    /// A live access token whose scopes do not authorize the user self-service
    /// operations ([`TokenRecord::authorizes_user_self_service`]).
    MissingRequiredScopes,
    /// The token names a user pool this emulator no longer holds.
    PoolGone,
    /// The token names a user its pool no longer holds.
    UserGone,
    /// The token names a user Amazon Cognito is waiting on a new password from
    /// ([`UserStatus::ResetRequired`]), which authorizes no self-service call
    /// until that password is set.
    PasswordResetRequired,
}

/// The most groups ONE user of a pool belongs to: Amazon Cognito's published
/// account limit "Groups per user" (Amazon Cognito Developer Guide, "Quotas in
/// Amazon Cognito"), which the refusal of a membership past it states in full
/// ([`groups_per_user_limit_message`]).
///
/// It bounds a user's own memberships rather than the pool's groups
/// ([`crate::state::MAX_GROUPS_PER_POOL`]), so a pool may hold more groups than
/// any one of its users can belong to.
pub const MAX_GROUPS_PER_USER: usize = 100;

/// The message the refusal of a membership past [`MAX_GROUPS_PER_USER`] carries.
///
/// Amazon Cognito names the API whose limit was reached and the limit itself:
/// the fixed text is `The limit for addUserToGroup is: ` and the trailing `.`,
/// and the one variable part is the quota, so the sentence and the ceiling the
/// handler enforces can never drift apart.
pub fn groups_per_user_limit_message() -> String {
    format!("The limit for addUserToGroup is: {MAX_GROUPS_PER_USER}.")
}

/// How ONE group's `Precedence` ranks against another's, lowest first.
///
/// `CreateGroup`'s `Precedence` (this service's model): "Groups with lower `Precedence`
/// values take precedence over groups with higher or null `Precedence` values", so a group
/// that states none ranks behind every group that states one — which the leading flag
/// orders — while two groups stating the same value rank equally.
pub(super) type PrecedenceRank = (bool, i32);

/// [`PrecedenceRank`] for a group whose stored `Precedence` is `precedence`.
pub(super) fn precedence_rank(precedence: Option<i32>) -> PrecedenceRank {
    (precedence.is_none(), precedence.unwrap_or_default())
}

/// What a user's group memberships put in its identity token
/// ([`UserPool::user_group_role_claims`]).
#[derive(Debug, Clone, Default)]
pub struct GroupRoleClaims {
    /// The `cognito:roles` claim: each distinct role the user's groups grant, once, best
    /// precedence first. Empty when no group of the user's grants a role, which is a token
    /// that carries no such claim at all.
    pub roles: Vec<String>,
    /// The `cognito:preferred_role` claim: the role of the best-precedence group, and `None`
    /// where the groups tied at that precedence name different roles — the case
    /// `CreateGroup`'s `Precedence` documents the claim "isn't set" for.
    pub preferred_role: Option<String>,
}

/// One stored group of a user pool: what `CreateGroup` stored and every later
/// read of that group renders.
#[derive(Debug, Clone)]
pub struct Group {
    pub group_name: String,
    pub user_pool_id: String,
    pub description: Option<String>,
    pub role_arn: Option<String>,
    pub precedence: Option<i32>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl Group {
    /// The modelled view of this group, rendered from what is stored. An
    /// optional member the creating request left out stays absent rather than
    /// being invented.
    pub fn to_group_type(&self) -> GroupType {
        GroupType {
            group_name: Some(self.group_name.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            description: self.description.clone(),
            role_arn: self.role_arn.clone(),
            precedence: self.precedence,
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

/// One stored OAuth 2.0 resource server of a user pool: what
/// `CreateResourceServer` stored and every later read of it renders.
#[derive(Debug, Clone)]
pub struct ResourceServer {
    pub user_pool_id: String,
    pub identifier: String,
    pub name: String,
    /// The custom scopes of this resource server, in the order it HOLDS them:
    /// the collection is keyed by the scope name, and the order is the one the
    /// table keyed on those names is walked
    /// ([`crate::state::held_resource_server_scopes`]), so every read of the
    /// resource server renders that one order whatever sequence declared the
    /// scopes. A resource server declared with no `Scopes` holds none.
    pub scopes: Vec<ResourceServerScopeType>,
}

impl ResourceServer {
    /// The modelled view of this resource server, rendered from what is stored.
    pub fn to_resource_server_type(&self) -> ResourceServerType {
        ResourceServerType {
            user_pool_id: Some(self.user_pool_id.clone()),
            identifier: Some(self.identifier.clone()),
            name: Some(self.name.clone()),
            scopes: Some(self.scopes.clone()),
        }
    }
}

/// One stored user import job of a user pool: what `CreateUserImportJob` stored
/// and every later read of it renders.
///
/// The job's `Status` is not stored: it is COMPUTED from the instants below and
/// the clock ([`UserImportJob::status`]), so a read never has to move it.
#[derive(Debug, Clone)]
pub struct UserImportJob {
    pub job_id: String,
    pub job_name: String,
    pub user_pool_id: String,
    /// The role "that has permission to log import-job results to Amazon
    /// CloudWatch Logs" (`CreateUserImportJob.CloudWatchLogsRoleArn`), kept
    /// exactly as it was submitted.
    pub cloud_watch_logs_role_arn: String,
    /// The hashing algorithm the CSV's password hashes were made with, absent on
    /// a job created without one: the model documents no default.
    pub password_hashing_algorithm: Option<String>,
    /// Where a client uploads this job's CSV file, as it was minted when the job
    /// was created, so every read of the job reports the same locator. Absent
    /// when the create named no endpoint to build one from.
    pub pre_signed_url: Option<String>,
    /// The instant a CSV file was uploaded to this job's
    /// [`UserImportJob::pre_signed_url`], absent until one is: a job "imports
    /// users from a CSV file" (`StartUserImportJob`), so a job whose upload
    /// target was never written to has nothing to import.
    pub csv_uploaded_at: Option<SystemTime>,
    /// The CSV file that arrived at this job's upload target and has not been
    /// imported yet: the file a start READS to create the pool's users
    /// ([`run_user_import`]), which the start then RELEASES, since nothing
    /// reads it again. Bounded by [`MAX_USER_IMPORT_CSV_BYTES`], which the upload
    /// target enforces.
    pub csv: Vec<u8>,
    pub creation_date: SystemTime,
    /// When `StartUserImportJob` accepted this job, absent until it does.
    pub start_date: Option<SystemTime>,
    /// The instant the clock owes this job's `Pending` -> `InProgress` move.
    /// Stored, never slept on: a read computes the status from it.
    pub in_progress_at: Option<SystemTime>,
    /// The instant the clock owes this job's `Stopping` -> `Stopped` move,
    /// absent until `StopUserImportJob` accepted it.
    pub stopped_at: Option<SystemTime>,
    /// The instant the clock owes this job its TERMINAL status, absent until the
    /// job is started. Stored, never slept on: a read computes the status from it.
    pub completed_at: Option<SystemTime>,
    /// What the import run this job's start made did, absent until it was
    /// started: the counters the job reports and the terminal status it earned
    /// ([`ImportOutcome`]).
    pub outcome: Option<ImportOutcome>,
}

impl UserImportJob {
    /// A new job of `user_pool_id`, `Created` until it is started or the clock
    /// makes its expiry due.
    pub fn new(
        job_id: String,
        job_name: String,
        user_pool_id: String,
        cloud_watch_logs_role_arn: String,
        password_hashing_algorithm: Option<String>,
        pre_signed_url: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            job_id,
            job_name,
            user_pool_id,
            cloud_watch_logs_role_arn,
            password_hashing_algorithm,
            pre_signed_url,
            csv_uploaded_at: None,
            csv: Vec::new(),
            creation_date: now,
            start_date: None,
            in_progress_at: None,
            stopped_at: None,
            completed_at: None,
            outcome: None,
        }
    }

    /// This job's `UserImportJobStatusType` as of `now`, computed from the
    /// stored instants: a stopped job is `Stopping` until the clock has made its
    /// stop due and `Stopped` from then on, a started one `Pending` until the
    /// clock has made the import due, `InProgress` from then on and its terminal
    /// status once the clock has made the import's completion due
    /// ([`ImportOutcome::terminal_status`]), and one that was never started
    /// `Created` until its expiry is due.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if let Some(stopped_at) = self.stopped_at {
            return if now >= stopped_at {
                JOB_STATUS_STOPPED
            } else {
                JOB_STATUS_STOPPING
            };
        }
        if let Some(outcome) = self.outcome.as_ref() {
            if self.completed_at.is_some_and(|due| now >= due) {
                return outcome.terminal_status();
            }
        }
        if let Some(in_progress_at) = self.in_progress_at {
            return if now >= in_progress_at {
                JOB_STATUS_IN_PROGRESS
            } else {
                JOB_STATUS_PENDING
            };
        }
        if now >= self.creation_date + JOB_EXPIRY_SPAN {
            JOB_STATUS_EXPIRED
        } else {
            JOB_STATUS_CREATED
        }
    }

    /// Whether this job is importing users as of `now` — `Pending` or
    /// `InProgress` — which is the state a stop acts on.
    pub fn is_running(&self, now: SystemTime) -> bool {
        matches!(
            self.status(now),
            JOB_STATUS_PENDING | JOB_STATUS_IN_PROGRESS
        )
    }

    /// Release the file this job holds once the clock has EXPIRED it: "you created
    /// a job, but did not start the job within 24-48 hours. All data associated
    /// with the job was deleted, and the job can't be started"
    /// (`UserImportJobType.Status`), so an expired job holds no file to import.
    ///
    /// A change the clock already made due, materialised rather than decided here,
    /// which is what bounds the bytes a pool retains for files no start will ever
    /// read.
    pub fn release_expired_csv(&mut self, now: SystemTime) {
        if self.status(now) == JOB_STATUS_EXPIRED {
            self.csv = Vec::new();
        }
    }

    /// Record that a CSV of `body` arrived at this job's upload target at `now`.
    /// The latest upload is what the job holds: a client that writes the target
    /// twice has replaced the file the job is going to import.
    pub fn record_csv_upload(&mut self, body: &[u8], now: SystemTime) {
        self.csv_uploaded_at = Some(now);
        self.csv = body.to_vec();
    }

    /// Whether a CSV file is waiting at this job's target for a start to import.
    /// An EMPTY body uploads no CSV: a file of no bytes carries neither the header
    /// row nor a user. A start CONSUMES the file, so this reports `false` for a job
    /// whose import has already read it.
    #[must_use]
    pub fn has_uploaded_csv(&self) -> bool {
        self.csv_uploaded_at.is_some() && !self.csv.is_empty()
    }

    /// Record that the import was started at `now`, which makes the job `Pending`,
    /// the `InProgress` move due one [`JOB_START_SPAN`] later and the terminal
    /// status due one [`JOB_IMPORT_SPAN`] after the start.
    ///
    /// The run's own result arrives with [`UserImportJob::record_import`] once the
    /// import has placed its rows; until then the job reports the outcome of a run
    /// that has placed none.
    pub fn start(&mut self, now: SystemTime) {
        self.start_date = Some(now);
        self.in_progress_at = Some(now + JOB_START_SPAN);
        self.completed_at = Some(now + JOB_IMPORT_SPAN);
        self.outcome = Some(ImportOutcome::default());
    }

    /// Record what the import run this job's start made did, which is what every
    /// later read of the job reports ([`ImportOutcome`]).
    pub fn record_import(&mut self, outcome: ImportOutcome) {
        self.outcome = Some(outcome);
    }

    /// Record that the stop was asked for at `now`, which makes the job
    /// `Stopping` and the `Stopped` move due one [`JOB_STOP_SPAN`] later.
    pub fn stop(&mut self, now: SystemTime) {
        self.stopped_at = Some(now + JOB_STOP_SPAN);
    }

    /// The modelled view of this job as of `now`, rendered from what is stored.
    pub fn to_user_import_job_type(&self, now: SystemTime) -> UserImportJobType {
        let status = self.status(now);
        // The run this job's start made, once the clock has made its completion
        // due: a job that has not completed reports no result of a run that is
        // still going.
        let completed = self
            .outcome
            .as_ref()
            .filter(|_| status == JOB_STATUS_SUCCEEDED || status == JOB_STATUS_FAILED);
        UserImportJobType {
            job_name: Some(self.job_name.clone()),
            job_id: Some(self.job_id.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            // The locator the job was minted with, reported exactly as it is
            // stored, so a later read of the job names the same target.
            pre_signed_url: self.pre_signed_url.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            start_date: self.start_date.map(to_date_time),
            // "The date when the user import job was completed"
            // (`UserImportJobType.CompletionDate`): the instant the job stopped
            // importing users, reported once the clock has made that move due —
            // the stop for a job a caller stopped, the completion of the run for
            // one that ran to its terminal status.
            completion_date: self
                .stopped_at
                .filter(|_| status == JOB_STATUS_STOPPED)
                .or_else(|| self.completed_at.filter(|_| completed.is_some()))
                .map(to_date_time),
            status: Some(status.to_string()),
            cloud_watch_logs_role_arn: Some(self.cloud_watch_logs_role_arn.clone()),
            // The three counters of the run this job's start made, which are the
            // zero the model defaults them to until a run has placed a row. They
            // are reported from the instant the run exists rather than only once
            // the job has completed: "the number of users that were successfully
            // imported" is a count of what the run did, which a client watches
            // while the job "has started, and users are being imported"
            // (`UserImportJobType.Status`). `CompletionMessage` and
            // `CompletionDate` are the completion's own, so they wait for it.
            imported_users: self
                .outcome
                .as_ref()
                .map_or(0, |outcome| outcome.imported_users),
            skipped_users: self
                .outcome
                .as_ref()
                .map_or(0, |outcome| outcome.skipped_users),
            failed_users: self
                .outcome
                .as_ref()
                .map_or(0, |outcome| outcome.failed_users),
            // "The message returned when the user import job is completed"
            // (`UserImportJobType.CompletionMessage`), so a job that has not
            // completed reports none.
            completion_message: completed
                .map(|outcome| outcome.completion_message().to_string()),
            password_hashing_algorithm: self.password_hashing_algorithm.clone(),
        }
    }
}

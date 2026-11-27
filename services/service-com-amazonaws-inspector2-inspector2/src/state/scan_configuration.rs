// SPDX-License-Identifier: CC0-1.0

//! One account's Amazon Inspector SCAN CONFIGURATION — the ECR automated re-scan setting,
//! the Amazon EC2 scan mode and the Amazon Machine Image scan setting — which
//! `UpdateConfiguration` writes and `GetConfiguration` reports.
//!
//! The configuration is held per account, as an OVERRIDE: an account that has never been
//! configured individually has no record here and reads the delegated administrator's
//! configuration, which is how a configuration the administrator writes reaches every
//! member account that has not been configured on its own
//! (`smithy_query(query="doc", shape="UpdateConfigurationRequest")` — a call naming no
//! `accountId` "updates the delegated administrator's configuration and propagates it to
//! member accounts that have not been individually configured"). An administrator that has
//! never configured one either leaves the account on the values the service applies until
//! it is asked for others ([`EcrRescanSettings::default`], [`Ec2ScanSettings::default`]).
//!
//! An inheritance reset ([`INHERIT_FROM_ADMIN`]) removes the member's own override for one
//! scan type, so the member goes back to reading the administrator's — the absence of a
//! record IS the inheritance, so nothing has to be copied between accounts and an
//! administrator's later change reaches the member at once.
//!
//! BOUNDED without a bound of its own: a record lives on an account's enablement entry, and
//! [`super::ComAmazonawsInspector2Inspector2State::account_mut`] — which this module's
//! write goes through — owns the bound on the account map, so a caller looping
//! `UpdateConfiguration` with fresh account ids cannot grow it.

use std::time::{Duration, SystemTime};

use super::{timestamp, ComAmazonawsInspector2Inspector2State};
use crate::ops::types::{
    AmiConfigurationState, AmiScanModeConfiguration, AmiTagFilter, Ec2ConfigurationState,
    Ec2ScanModeState, EcrConfigurationState, EcrRescanDurationState, VMScannerState,
};

/// The `EcrRescanDuration` token of an image that stays actively scanned for as long as it
/// is in the registry — the setting an account has until it asks for a window.
///
/// `EcrRescanDurationState` documents the window as the thing that makes an image's
/// monitoring state become inactive and closes its findings
/// (`smithy_query(query="shape", shape="EcrRescanDurationState")`), and this token is the
/// one that never does: Amazon Inspector keeps scanning a pushed image until the account
/// asks it to stop after a number of days.
/// Source: `smithy_query(query="shape", shape="EcrRescanDuration")` —
/// `LIFETIME | DAYS_30 | DAYS_180 | DAYS_14 | DAYS_60 | DAYS_90 | DAYS_3 | DAYS_7`.
pub const DEFAULT_ECR_RESCAN_DURATION: &str = "LIFETIME";

/// The `Ec2ScanMode` token an account scans Amazon EC2 instances under until it asks for
/// the other one.
///
/// `Ec2Configuration` — the member `UpdateConfiguration` carries to change the mode — is
/// documented as "Enables agent-based scanning, which scans instances that are not managed
/// by SSM" (`smithy_query(query="shape", shape="Ec2Configuration")`), so the update exists
/// to move an account ONTO agent-based scanning and the mode it moves off is the hybrid one.
/// Source: `smithy_query(query="shape", shape="Ec2ScanMode")` —
/// `EC2_SSM_AGENT_BASED | EC2_HYBRID`.
pub const DEFAULT_EC2_SCAN_MODE: &str = "EC2_HYBRID";

/// The status token a scan-type setting carries once the service has applied it.
/// Source: `smithy_query(query="shape", shape="EcrRescanDurationStatus,Ec2ScanModeStatus,VMScannerStatus")`
/// — `SUCCESS` is a token of all three, and every one of this surface's status members
/// targets one of them.
pub const CONFIGURATION_SUCCESS: &str = "SUCCESS";

/// The status token a scan-type setting carries while the change the last update asked for
/// is still being applied.
/// Source: `smithy_query(query="shape", shape="EcrRescanDurationStatus,Ec2ScanModeStatus,VMScannerStatus")`
/// — `PENDING` is a token of all three.
pub const CONFIGURATION_PENDING: &str = "PENDING";

/// How long a scan configuration change stays [`CONFIGURATION_PENDING`] before the clock
/// owes the account its terminal status. The real service applies the change
/// asynchronously, and no AWS document fixes how long it takes, so this span is this
/// emulator's own: non-zero, so a caller reading straight after the write still observes
/// the transient status, and short enough that polling settles quickly.
pub const CONFIGURATION_TRANSITION: Duration = Duration::from_secs(3);

/// The one `InheritanceMode` token: the member account's configuration for that scan type
/// goes back to following the delegated administrator's.
/// Source: `smithy_query(query="shape", shape="InheritanceMode")` — `INHERIT_FROM_ADMIN`.
pub const INHERIT_FROM_ADMIN: &str = "INHERIT_FROM_ADMIN";

/// What an update states when it asks for an inheritance reset AND carries a scan-type
/// configuration of its own: the two ask opposite things of the same setting — one that the
/// account follow the administrator, the other that it hold values of its own — so the
/// request states no single configuration to apply.
///
/// `UpdateConfigurationRequest$updateConfigurationInheritance` documents the requirement
/// (`smithy_query(query="doc", shape="UpdateConfigurationRequest",
/// member="updateConfigurationInheritance")`): "When specified, ec2Configuration and
/// ecrConfiguration must be absent, and accountId must also be present." FIXED text: the
/// requirement is the same whichever member the request carried.
pub const INHERITANCE_WITH_CONFIGURATION_MESSAGE: &str =
    "updateConfigurationInheritance must not be specified with ec2Configuration or \
     ecrConfiguration.";

/// What an update states when it asks for an inheritance reset without naming the account
/// to reset: inheritance is a member account's relationship to the delegated
/// administrator's configuration, and the administrator's own configuration inherits from
/// nothing, so there is no account for the reset to apply to. From the same documented
/// requirement as [`INHERITANCE_WITH_CONFIGURATION_MESSAGE`].
pub const INHERITANCE_WITHOUT_ACCOUNT_MESSAGE: &str =
    "updateConfigurationInheritance must be specified with accountId.";

/// The member both inheritance refusals are stated at: the request's own
/// `updateConfigurationInheritance`, which is the member whose presence the requirement is
/// about.
pub const INHERITANCE_MEMBER: &str = "updateConfigurationInheritance";

/// The `AmiScanMode` token an account scans Amazon Machine Images under until it asks for
/// the other one: every AMI of the account.
///
/// `AmiScanModeConfiguration$scanMode` is documented as deciding "whether all AMIs are
/// scanned or only those matching the specified tags"
/// (`smithy_query(query="shape", shape="AmiScanModeConfiguration")`), and the tag filters
/// the other mode selects with are what an account has to configure, so the mode that needs
/// no filters is the one it starts on.
/// Source: `smithy_query(query="shape", shape="AmiScanMode")` —
/// `SCAN_ALL | TARGET_SCANNING`.
pub const DEFAULT_AMI_SCAN_MODE: &str = "SCAN_ALL";

/// The VALUES of one account's ECR automated re-scan setting: what a read reports and what
/// a write is measured against, with none of the instants the state also carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcrRescanSettings {
    /// The `EcrRescanDuration` configured for an image's push date.
    pub rescan_duration: String,
    /// The `EcrPullDateRescanDuration` configured for an image's pull date, absent while
    /// the account has never configured one — which is a different answer from a duration,
    /// so the absence is kept out of that token space.
    pub pull_date_rescan_duration: Option<String>,
    /// The `EcrPullDateRescanMode` in force, absent for the same reason.
    pub pull_date_rescan_mode: Option<String>,
}

impl Default for EcrRescanSettings {
    /// The ECR setting an account has until it configures one: images stay actively
    /// scanned ([`DEFAULT_ECR_RESCAN_DURATION`]) and neither pull-date member has been
    /// configured.
    fn default() -> Self {
        Self {
            rescan_duration: DEFAULT_ECR_RESCAN_DURATION.to_string(),
            pull_date_rescan_duration: None,
            pull_date_rescan_mode: None,
        }
    }
}

/// One account's stored ECR automated re-scan setting: its values, the instant they last
/// changed and the instant the last change falls due.
#[derive(Debug, Clone, Default)]
pub struct EcrScanConfiguration {
    pub settings: EcrRescanSettings,
    /// The instant a value of this setting last changed, absent while none ever has — which
    /// is what `EcrRescanDurationState$updatedAt` reports, so an account whose setting has
    /// never changed reports no instant rather than one this emulator made up.
    pub changed_at: Option<SystemTime>,
    /// The instant the last change stops being [`CONFIGURATION_PENDING`] — the transition
    /// the CLOCK owes, stored rather than slept for, so the status is derived on every read
    /// and reporting it writes nothing.
    pub settled_at: Option<SystemTime>,
}

/// The VALUES of one account's Amazon EC2 scan setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ec2ScanSettings {
    /// The `Ec2ScanMode` instances of this account are scanned under.
    pub scan_mode: String,
    /// Whether the Amazon Inspector VM scanner is activated, absent while the account has
    /// never asked either way — which is what keeps a read from reporting a scanner state
    /// for an account that has never configured one.
    pub vm_scanner_activated: Option<bool>,
}

impl Default for Ec2ScanSettings {
    /// The EC2 setting an account has until it configures one: the mode
    /// [`DEFAULT_EC2_SCAN_MODE`] names, and no VM scanner decision of its own.
    fn default() -> Self {
        Self {
            scan_mode: DEFAULT_EC2_SCAN_MODE.to_string(),
            vm_scanner_activated: None,
        }
    }
}

/// One account's stored Amazon EC2 scan setting: its values, the instant its VM scanner was
/// activated and the instant the last change falls due.
#[derive(Debug, Clone, Default)]
pub struct Ec2ScanConfiguration {
    pub settings: Ec2ScanSettings,
    /// The instant the VM scanner was activated, kept from the update that switched it on
    /// and absent while it is off — which is what `VMScannerState$activatedAt` reports.
    pub vm_scanner_activated_at: Option<SystemTime>,
    /// The instant the last change stops being [`CONFIGURATION_PENDING`], as
    /// [`EcrScanConfiguration::settled_at`].
    pub settled_at: Option<SystemTime>,
}

/// The VALUES of one account's Amazon Machine Image scan setting.
///
/// Source: `smithy_query(query="shape", shape="AmiConfiguration,AmiScanModeConfiguration")`
/// — `scanModeConfiguration` is required and carries a required `scanMode`; the two scan
/// windows and the two exclusions are optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmiScanSettings {
    /// The `AmiScanDuration` scanning is restricted to AMIs last launched within, absent
    /// while the account has never configured one — a different answer from a duration, so
    /// the absence is kept out of that token space.
    pub last_launched_within: Option<String>,
    /// The `AmiScanDuration` scanning is restricted to AMIs created within, absent for the
    /// same reason.
    pub created_within: Option<String>,
    /// The `AmiScanMode` AMIs of this account are selected by.
    pub scan_mode: String,
    /// The tag filters targeted scanning selects AMIs with, each a key and an optional
    /// value, absent while the account configured none.
    pub tags: Option<Vec<(String, Option<String>)>>,
    /// Whether AMIs copied from other AMIs are excluded, absent while the account has
    /// never asked either way.
    pub exclude_copied_amis: Option<bool>,
    /// Whether AMIs created by Amazon Data Lifecycle Manager backups are excluded, absent
    /// for the same reason.
    pub exclude_backup_dlm_amis: Option<bool>,
}

impl Default for AmiScanSettings {
    /// The AMI setting an account has until it configures one: every AMI selected
    /// ([`DEFAULT_AMI_SCAN_MODE`]), no window restricting which ones, no tag filter and no
    /// exclusion decision of its own.
    fn default() -> Self {
        Self {
            last_launched_within: None,
            created_within: None,
            scan_mode: DEFAULT_AMI_SCAN_MODE.to_string(),
            tags: None,
            exclude_copied_amis: None,
            exclude_backup_dlm_amis: None,
        }
    }
}

/// One account's stored Amazon Machine Image scan setting: its values, the instant they
/// last changed and the instant the last change falls due.
#[derive(Debug, Clone, Default)]
pub struct AmiScanConfiguration {
    pub settings: AmiScanSettings,
    /// The instant a value of this setting last changed, absent while none ever has —
    /// which is what `AmiConfigurationState$updatedAt` reports.
    pub changed_at: Option<SystemTime>,
    /// The instant the last change stops being [`CONFIGURATION_PENDING`], as
    /// [`EcrScanConfiguration::settled_at`].
    pub settled_at: Option<SystemTime>,
}

/// One account's own scan configuration: a scan type with no record here is not configured
/// individually and follows the delegated administrator's.
#[derive(Debug, Clone, Default)]
pub struct ScanConfiguration {
    pub ecr: Option<EcrScanConfiguration>,
    pub ec2: Option<Ec2ScanConfiguration>,
    pub ami: Option<AmiScanConfiguration>,
}

/// What one `UpdateConfiguration` call asks of an account's scan configuration: the values
/// each scan type's member named, and whether that scan type is to be inherited from the
/// delegated administrator again.
///
/// A scan type the call mentions in neither way is asked nothing of and is left exactly as
/// it was.
#[derive(Debug, Clone, Default)]
pub struct ScanConfigurationUpdate {
    /// The `ecrConfiguration` the request carried: `rescanDuration` is a required member of
    /// that structure, so a request that carries it names a duration; its two optional
    /// members are absent when the request left them out.
    pub ecr: Option<EcrRescanSettings>,
    /// The `ec2Configuration` the request carried, read the same way: `scanMode` is
    /// required, `activateVMScanner` optional.
    pub ec2: Option<Ec2ScanSettings>,
    /// Whether `updateConfigurationInheritance$ecrConfiguration` asked for the ECR setting
    /// to follow the delegated administrator again.
    pub inherit_ecr: bool,
    /// Whether `updateConfigurationInheritance$ec2Configuration` asked the same of the EC2
    /// setting.
    pub inherit_ec2: bool,
    /// The `amiConfiguration` the request carried, read the same way:
    /// `scanModeConfiguration` is required and names a `scanMode`, and every other member
    /// is optional.
    pub ami: Option<AmiScanSettings>,
    /// Whether `updateConfigurationInheritance$amiConfiguration` asked the AMI setting to
    /// follow the delegated administrator again.
    pub inherit_ami: bool,
}

impl ScanConfigurationUpdate {
    /// Whether this call asks anything at all of the account's scan configuration. A call
    /// that asks nothing changes nothing, so it never reaches a write and never records an
    /// account.
    pub fn asks_for_nothing(&self) -> bool {
        self.ecr.is_none()
            && self.ec2.is_none()
            && self.ami.is_none()
            && !self.inherit_ecr
            && !self.inherit_ec2
            && !self.inherit_ami
    }
}

/// The status a setting whose last change falls due at `settled_at` reports at `now`:
/// [`CONFIGURATION_PENDING`] while the service is still applying it and
/// [`CONFIGURATION_SUCCESS`] from the instant it has.
///
/// DERIVED, so a read reports it without writing anything and a later caller sees exactly
/// what this one did.
pub fn applied_status(settled_at: Option<SystemTime>, now: SystemTime) -> &'static str {
    if settled_at.is_some_and(|due| now < due) {
        CONFIGURATION_PENDING
    } else {
        CONFIGURATION_SUCCESS
    }
}

/// The wire `EcrConfigurationState` for the ECR setting `stored` — the service's own values
/// when no account in the inheritance chain has configured one.
///
/// Source: `smithy_query(query="shape", shape="EcrConfigurationState,EcrRescanDurationState")`
/// — every member of both is optional.
pub fn ecr_configuration_state(
    stored: Option<&EcrScanConfiguration>,
    now: SystemTime,
) -> EcrConfigurationState {
    let unconfigured = EcrScanConfiguration::default();
    let stored = stored.unwrap_or(&unconfigured);
    EcrConfigurationState {
        rescan_duration_state: Some(EcrRescanDurationState {
            rescan_duration: Some(stored.settings.rescan_duration.clone()),
            status: Some(applied_status(stored.settled_at, now).to_string()),
            updated_at: stored.changed_at.map(timestamp),
            pull_date_rescan_duration: stored.settings.pull_date_rescan_duration.clone(),
            pull_date_rescan_mode: stored.settings.pull_date_rescan_mode.clone(),
        }),
    }
}

/// The wire `Ec2ConfigurationState` for the EC2 setting `stored`, read the same way.
///
/// `vmScannerState` is reported only for an account whose configuration carries a VM
/// scanner decision: the scanner state of an account that has never asked either way is not
/// known, which is a different answer from a scanner that is off.
///
/// Source: `smithy_query(query="shape", shape="Ec2ConfigurationState,Ec2ScanModeState,VMScannerState")`
/// — every member of all three is optional.
pub fn ec2_configuration_state(
    stored: Option<&Ec2ScanConfiguration>,
    now: SystemTime,
) -> Ec2ConfigurationState {
    let unconfigured = Ec2ScanConfiguration::default();
    let stored = stored.unwrap_or(&unconfigured);
    let status = applied_status(stored.settled_at, now).to_string();
    Ec2ConfigurationState {
        scan_mode_state: Some(Ec2ScanModeState {
            scan_mode: Some(stored.settings.scan_mode.clone()),
            scan_mode_status: Some(status.clone()),
        }),
        vm_scanner_state: stored
            .settings
            .vm_scanner_activated
            .map(|activated| VMScannerState {
                activated: Some(activated),
                activated_at: stored.vm_scanner_activated_at.map(timestamp),
                status: Some(status),
            }),
    }
}

/// The wire `AmiConfigurationState` for the AMI setting `stored` — the service's own values
/// when no account in the inheritance chain has configured one.
///
/// Source: `smithy_query(query="shape", shape="AmiConfigurationState")` — every member of
/// it is optional, and its `scanModeConfiguration` carries the `AmiScanMode` in force.
pub fn ami_configuration_state(
    stored: Option<&AmiScanConfiguration>,
    now: SystemTime,
) -> AmiConfigurationState {
    let unconfigured = AmiScanConfiguration::default();
    let stored = stored.unwrap_or(&unconfigured);
    AmiConfigurationState {
        last_launched_within: stored.settings.last_launched_within.clone(),
        created_within: stored.settings.created_within.clone(),
        scan_mode_configuration: Some(AmiScanModeConfiguration {
            scan_mode: stored.settings.scan_mode.clone(),
            // Reported only for an account whose configuration carries tag filters: no
            // filter at all is a different answer from an empty filter list.
            tags: stored.settings.tags.as_ref().map(|tags| {
                tags.iter()
                    .map(|(key, value)| AmiTagFilter {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect()
            }),
            exclude_copied_amis: stored.settings.exclude_copied_amis,
            exclude_backup_dlm_amis: stored.settings.exclude_backup_dlm_amis,
        }),
        status: Some(applied_status(stored.settled_at, now).to_string()),
        updated_at: stored.changed_at.map(timestamp),
    }
}

impl ComAmazonawsInspector2Inspector2State {
    /// The scan configuration IN FORCE for `account_id`: its own overrides where it has
    /// them, and the delegated administrator `admin_id`'s where it does not.
    ///
    /// A pure read: it resolves the inheritance rather than materialising it, so an
    /// administrator's later change reaches every account that has not been configured
    /// individually and reporting one account's configuration writes nothing anywhere.
    pub fn effective_scan_configuration(
        &self,
        account_id: &str,
        admin_id: &str,
    ) -> ScanConfiguration {
        let own = self
            .accounts
            .get(account_id)
            .map(|account| account.scan_configuration.clone())
            .unwrap_or_default();
        let administrator = self
            .accounts
            .get(admin_id)
            .map(|account| account.scan_configuration.clone())
            .unwrap_or_default();
        ScanConfiguration {
            ecr: own.ecr.or(administrator.ecr),
            ec2: own.ec2.or(administrator.ec2),
            ami: own.ami.or(administrator.ami),
        }
    }

    /// Apply `update` to `account_id`'s own scan configuration, as the delegated
    /// administrator `admin_id`'s configuration stands.
    ///
    /// A scan type the update names is stored as that account's own override: the values
    /// the request named, with an OPTIONAL member it left out keeping whatever was in force
    /// for that account, exactly as an omitted member of an EC2 deep inspection update
    /// leaves that half of the configuration alone. A scan type the update asks to inherit
    /// loses its override, so the account reads the administrator's again.
    ///
    /// Only a CHANGE of value starts a transition and moves `updatedAt`: a request asking
    /// for the values already in force has nothing for the service to apply, so the setting
    /// stays terminal and keeps the instant it last really changed.
    ///
    /// Adds at most one entry to the account map, through
    /// [`ComAmazonawsInspector2Inspector2State::account_mut`], which owns the bound on it —
    /// so `false`, meaning the configuration was NOT applied, is the answer once that bound
    /// is reached and `account_id` is not already tracked. The caller refuses the request
    /// with the modelled error rather than reporting a configuration it did not store.
    #[must_use]
    pub fn update_scan_configuration(
        &mut self,
        account_id: &str,
        admin_id: &str,
        update: ScanConfigurationUpdate,
        now: SystemTime,
    ) -> bool {
        // The values in force BEFORE the write — the account's own where it has them and
        // the administrator's where it does not — which is what a change is measured
        // against, so configuring a member account with what it already inherits is not a
        // change.
        let before = self.effective_scan_configuration(account_id, admin_id);
        let Some(account) = self.account_mut(account_id) else {
            return false;
        };
        let configuration = &mut account.scan_configuration;

        if update.inherit_ecr {
            configuration.ecr = None;
        }
        if update.inherit_ec2 {
            configuration.ec2 = None;
        }
        if update.inherit_ami {
            configuration.ami = None;
        }

        if let Some(requested) = update.ecr {
            let previous = before.ecr.unwrap_or_default();
            let settings = EcrRescanSettings {
                rescan_duration: requested.rescan_duration,
                pull_date_rescan_duration: requested
                    .pull_date_rescan_duration
                    .or_else(|| previous.settings.pull_date_rescan_duration.clone()),
                pull_date_rescan_mode: requested
                    .pull_date_rescan_mode
                    .or_else(|| previous.settings.pull_date_rescan_mode.clone()),
            };
            let changed = settings != previous.settings;
            configuration.ecr = Some(EcrScanConfiguration {
                settings,
                changed_at: if changed { Some(now) } else { previous.changed_at },
                settled_at: if changed {
                    Some(now + CONFIGURATION_TRANSITION)
                } else {
                    previous.settled_at
                },
            });
        }

        if let Some(requested) = update.ec2 {
            let previous = before.ec2.unwrap_or_default();
            let settings = Ec2ScanSettings {
                scan_mode: requested.scan_mode,
                vm_scanner_activated: requested
                    .vm_scanner_activated
                    .or(previous.settings.vm_scanner_activated),
            };
            let changed = settings != previous.settings;
            // The instant the scanner was activated is the instant the activation that put
            // it on happened: a scanner that stays on keeps it, one switched on now takes
            // this instant, and one that is off has none to report.
            let vm_scanner_activated_at = if settings.vm_scanner_activated == Some(true) {
                if previous.settings.vm_scanner_activated == Some(true) {
                    previous.vm_scanner_activated_at.or(Some(now))
                } else {
                    Some(now)
                }
            } else {
                None
            };
            configuration.ec2 = Some(Ec2ScanConfiguration {
                settings,
                vm_scanner_activated_at,
                settled_at: if changed {
                    Some(now + CONFIGURATION_TRANSITION)
                } else {
                    previous.settled_at
                },
            });
        }

        if let Some(requested) = update.ami {
            let previous = before.ami.unwrap_or_default();
            let settings = AmiScanSettings {
                last_launched_within: requested
                    .last_launched_within
                    .or_else(|| previous.settings.last_launched_within.clone()),
                created_within: requested
                    .created_within
                    .or_else(|| previous.settings.created_within.clone()),
                scan_mode: requested.scan_mode,
                tags: requested.tags.or_else(|| previous.settings.tags.clone()),
                exclude_copied_amis: requested
                    .exclude_copied_amis
                    .or(previous.settings.exclude_copied_amis),
                exclude_backup_dlm_amis: requested
                    .exclude_backup_dlm_amis
                    .or(previous.settings.exclude_backup_dlm_amis),
            };
            let changed = settings != previous.settings;
            configuration.ami = Some(AmiScanConfiguration {
                settings,
                changed_at: if changed { Some(now) } else { previous.changed_at },
                settled_at: if changed {
                    Some(now + CONFIGURATION_TRANSITION)
                } else {
                    previous.settled_at
                },
            });
        }

        true
    }
}

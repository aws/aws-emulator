// SPDX-License-Identifier: CC0-1.0

//! The service-owned platform catalog: the platform branches Elastic Beanstalk
//! publishes, the solution stacks (platform versions) inside them, and the
//! projections the platform operations report them through.
//!
//! Split out of [`super`] so the catalog's reference data lives beside the code
//! that reads it. Nothing here is created by a caller — a CUSTOM platform
//! version is held in the store itself.

use super::*;

/// One solution stack this service offers: the platform BRANCH it is a version
/// within, that version, and the source-bundle file types it permits.
///
/// A solution stack is a concrete platform version inside a published platform
/// branch, so a stack is identified by the branch it belongs to rather than by
/// a second spelling of the runtime: that is what keeps this catalog and the
/// branch catalog [`MANAGED_PLATFORM_BRANCHES`] (what `ListPlatformBranches`
/// reports) from disagreeing about which runtimes the service offers.
#[derive(Debug, Clone, Copy)]
pub struct SolutionStack {
    /// The `BranchName` of the [`MANAGED_PLATFORM_BRANCHES`] entry this stack is
    /// a version within, e.g. `Python 3.13 running on 64bit Amazon Linux 2023`.
    pub branch_name: &'static str,
    /// The platform version: the `v<major>.<minor>.<patch>` component of the
    /// reported `SolutionStackName`.
    ///
    /// A version number belongs to the platform FAMILY's release, not to the
    /// branch, so every row reads it from the one constant its family's current
    /// release is named by (see [`RELEASE_PYTHON_AL2023`] and the constants
    /// beside it) rather than carrying a number of its own.
    pub platform_version: &'static str,
    pub permitted_file_types: &'static [&'static str],
    /// The `PlatformCategory` of every version in this branch — "the category of
    /// platform version", the platform FAMILY the runtime belongs to
    /// (`Python`, `Docker`, `Tomcat`, `.NET Core`). It is catalog data, not a
    /// spelling of the branch name: Elastic Beanstalk publishes the .NET-on-Linux
    /// family as the category `.NET Core` under the platform name `.NET Core on
    /// Linux`, so the two cannot be derived from each other.
    pub platform_category: &'static str,
    /// The `ProgrammingLanguages` this version runs, as `(Name, Version)` pairs.
    /// Catalog data for the same reason as [`Self::platform_category`]: the
    /// branch name states the runtime's marketing name (`Corretto 21`), not the
    /// language and version a client filters and displays. A platform that runs
    /// no language of its own (Docker runs the image's) states none.
    pub programming_languages: &'static [(&'static str, &'static str)],
}

/// The `SolutionStackName` the version `platform_version` of the platform branch
/// `branch_name` is reported under.
///
/// AWS publishes a branch as `<runtime> running on <os>` and a solution stack
/// within it as `<os> v<version> running <runtime>` — the Python 3.13 branch's
/// current version is `64bit Amazon Linux 2023 v4.6.0 running Python 3.13` — so
/// the name is DERIVED from the branch rather than written out a second time and
/// left to drift from it.
pub fn solution_stack_name_of(branch_name: &str, platform_version: &str) -> String {
    match branch_name.split_once(" running on ") {
        Some((runtime, os)) => format!("{os} v{platform_version} running {runtime}"),
        // A branch name not in the published form is reported as it stands
        // rather than rewritten into a shape it does not have.
        None => branch_name.to_string(),
    }
}

/// The `PlatformArn` the version `platform_version` of the platform branch
/// `branch_name` is named by in `region`:
/// `arn:aws:elasticbeanstalk:<region>::platform/<branch name>/<version>`.
///
/// A managed platform version belongs to the SERVICE, not to an account, so its
/// ARN carries no account id — which is why it is not built with
/// [`::engine::emulator_arn`], the form every account-owned resource of this
/// service uses.
pub fn platform_arn_of(region: &str, branch_name: &str, platform_version: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::platform/{branch_name}/{platform_version}")
}

impl SolutionStack {
    /// The `SolutionStackName` this stack's CURRENT version is reported under.
    /// See [`solution_stack_name_of`].
    pub fn name(&self) -> String {
        solution_stack_name_of(self.branch_name, self.platform_version)
    }

    /// The `PlatformArn` this stack's CURRENT version is named by in `region`.
    /// See [`platform_arn_of`].
    pub fn platform_arn(&self, region: &str) -> String {
        platform_arn_of(region, self.branch_name, self.platform_version)
    }

    /// The version of this branch that [`Self::platform_version`] SUPERSEDED, or
    /// `None` when it is the first release of its line.
    ///
    /// Elastic Beanstalk publishes every platform version of a branch it has
    /// released, not only the newest: a release the next one superseded stays
    /// `Ready` and launchable — which is what makes a MINOR PLATFORM UPDATE
    /// available to an environment running it, and so what lets the service
    /// schedule a managed update for that environment — while only the newest
    /// release of the branch is the branch's `Recommended` one.
    ///
    /// Which version that is comes out of the catalog's own numbering, the way
    /// the solution stack name and the ARN do, rather than being written out per
    /// row: a release is numbered `<major>.<minor>.<patch>` and the release
    /// before it is the one with the lowest-order component a step lower — the
    /// patch below it where this version carries one, and the preceding minor
    /// release otherwise. A branch is published at its family's release
    /// ([`Self::platform_version`]), so the release it superseded is the family's
    /// previous one and every branch of the family superseded the same number.
    /// (The version components themselves are this emulator's own; see
    /// [`SOLUTION_STACKS`].)
    pub fn superseded_version(&self) -> Option<String> {
        let mut components = self.platform_version.split('.');
        let major: u32 = components.next()?.parse().ok()?;
        let minor: u32 = components.next()?.parse().ok()?;
        let patch: u32 = components.next()?.parse().ok()?;
        if components.next().is_some() {
            return None;
        }
        match (minor, patch) {
            (_, 1..) => Some(format!("{major}.{minor}.{}", patch - 1)),
            (1.., 0) => Some(format!("{major}.{}.0", minor - 1)),
            // The first release of the major line superseded nothing.
            (0, 0) => None,
        }
    }

    /// Every version of this branch the catalog publishes, NEWEST FIRST: the
    /// branch's current version and the one it superseded
    /// ([`Self::superseded_version`]).
    pub fn published_versions(&self) -> Vec<String> {
        let mut versions = vec![self.platform_version.to_string()];
        versions.extend(self.superseded_version());
        versions
    }

    /// The runtime and the operating system this stack's branch name is built
    /// from: `<runtime> running on <os>`, e.g. `("Python 3.13", "64bit Amazon
    /// Linux 2023")`. A branch name not in the published form carries no
    /// operating system, so nothing is read out of it.
    pub(super) fn runtime_and_operating_system(&self) -> Option<(&'static str, &'static str)> {
        self.branch_name.split_once(" running on ")
    }

    /// The `Description` of this published version: "`<os> running <runtime>`" —
    /// its [`Self::name`] without the version component, which is how Elastic
    /// Beanstalk words a managed platform version's description (`64bit Amazon
    /// Linux 2023 running Python 3.13`). DERIVED from the branch, like the
    /// solution stack name, so the two cannot drift apart.
    pub fn description(&self) -> String {
        match self.runtime_and_operating_system() {
            Some((runtime, os)) => format!("{os} running {runtime}"),
            None => self.branch_name.to_string(),
        }
    }

    /// The `OperatingSystemName` and `OperatingSystemVersion` of this version.
    ///
    /// Elastic Beanstalk names a branch's operating system inside the branch
    /// name, as `64bit <name> <version>` (`64bit Amazon Linux 2023`, `64bit
    /// Windows Server 2019`), so both members are READ OFF the branch rather
    /// than written out a second time: the width prefix is dropped, a trailing
    /// numeric component is the version, and what precedes it is the name. An
    /// operating system named without a version component (`64bit Amazon Linux`)
    /// reports no version.
    pub fn operating_system(&self) -> Option<(&'static str, Option<&'static str>)> {
        let (_, os) = self.runtime_and_operating_system()?;
        let os = os.strip_prefix("64bit ").unwrap_or(os);
        match os.rsplit_once(' ') {
            Some((name, version))
                if !version.is_empty()
                    && version
                        .chars()
                        .all(|character| character.is_ascii_digit() || character == '.') =>
            {
                Some((name, Some(version)))
            }
            _ => Some((os, None)),
        }
    }

    /// The modelled `ProgrammingLanguages` of this published version. A catalog
    /// entry that states no version for its language reports the language
    /// without one rather than an empty version string.
    pub fn to_programming_languages(&self) -> Vec<PlatformProgrammingLanguage> {
        self.programming_languages
            .iter()
            .map(|(name, version)| PlatformProgrammingLanguage {
                name: Some((*name).to_string()),
                version: stated(version),
            })
            .collect()
    }

    /// The names of the programming languages this version runs — the attribute
    /// a `ListPlatformVersions` filter term of type `ProgrammingLanguageName`
    /// is applied to.
    pub fn programming_language_names(&self) -> Vec<String> {
        self.programming_languages
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect()
    }

    /// The modelled `PlatformDescription` for the version `platform_version` of
    /// this branch — one of the versions the catalog publishes for it
    /// ([`Self::published_versions`]).
    ///
    /// Everything the catalog states about the version is projected here, and
    /// everything the STORE holds about it comes from `store`: the
    /// [`MANAGED_PLATFORM_BRANCHES`] entry it is a version within — the one place
    /// its supported tiers and its branch life-cycle state are held, so
    /// `ListPlatformBranches` and this call cannot disagree about them — and when
    /// the catalog was published. A stack whose branch the store does not hold
    /// reports neither branch member rather than a guessed one.
    ///
    /// `Frameworks` describes the platform's own DEFINITION rather than the
    /// catalog's entry for it: it is what the owner of the platform states in the
    /// platform definition it built the version from, and a version Elastic
    /// Beanstalk itself publishes states none, so it reports no framework at all
    /// rather than an invented one. The machine images the version runs are the
    /// service's own ([`managed_custom_ami_list`] and
    /// [`managed_default_ami_list`]).
    pub fn to_description(
        &self,
        region: &str,
        store: &ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
        platform_version: &str,
    ) -> PlatformDescription {
        let operating_system = self.operating_system();
        let branch = store.platform_branch_named(self.branch_name);
        PlatformDescription {
            platform_arn: Some(platform_arn_of(region, self.branch_name, platform_version)),
            platform_owner: Some(MANAGED_PLATFORM_OWNER.to_string()),
            platform_name: Some(self.branch_name.to_string()),
            platform_version: Some(platform_version.to_string()),
            platform_branch_name: Some(self.branch_name.to_string()),
            solution_stack_name: Some(solution_stack_name_of(self.branch_name, platform_version)),
            // A published version is usable as it stands; nothing is building it.
            platform_status: Some(PLATFORM_STATUS_READY.to_string()),
            // The catalog is service-owned reference data: it was published when
            // this store was created and has not been re-published since.
            date_created: Some(store.platform_catalog_published_at),
            date_updated: Some(store.platform_catalog_published_at),
            platform_category: Some(self.platform_category.to_string()),
            description: Some(self.description()),
            maintainer: Some(MANAGED_PLATFORM_MAINTAINER.to_string()),
            operating_system_name: operating_system.map(|(name, _)| name.to_string()),
            operating_system_version: operating_system
                .and_then(|(_, version)| version)
                .map(str::to_string),
            programming_languages: Some(self.to_programming_languages()),
            frameworks: Some(managed_frameworks()),
            custom_ami_list: Some(managed_custom_ami_list(
                region,
                self.branch_name,
                platform_version,
            )),
            default_ami_list: Some(managed_default_ami_list(
                region,
                self.branch_name,
                platform_version,
            )),
            supported_tier_list: branch.map(|branch| branch.supported_tiers.clone()),
            // Both lists follow the BRANCH, so a stack whose branch the store
            // does not hold reports neither rather than one of the two.
            supported_addon_list: branch
                .map(|branch| managed_supported_addons(&branch.supported_tiers)),
            // Only the NEWEST version of a branch is its recommended one; the
            // release it superseded is still `Ready` but carries no life-cycle
            // state at all on THIS shape, whose member documents "Possible
            // values: `Recommended` | `null`" — unlike `PlatformSummary`'s,
            // which documents an empty value (see
            // [`reported_platform_lifecycle_state`]).
            platform_lifecycle_state: (platform_version == self.platform_version)
                .then(|| PLATFORM_LIFECYCLE_RECOMMENDED.to_string()),
            platform_branch_lifecycle_state: branch
                .map(|branch| reported_branch_lifecycle_state(&branch.lifecycle_state)),
        }
    }
}

/// A catalog value as a modelled member: the value itself, or no value at all
/// when the catalog states none (the empty string).
pub(super) fn stated(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// The `Frameworks` a MANAGED platform version reports — "the frameworks
/// supported by the platform version": ONE entry, naming no framework and no
/// version.
///
/// A framework is stated by the platform definition the account that BUILT a
/// platform version supplied. A version Elastic Beanstalk publishes states
/// none, which is what the real service answers: the member is reported, and
/// the entry in it carries neither a name nor a version rather than an
/// application server this service would have had to invent.
pub fn managed_frameworks() -> Vec<PlatformFramework> {
    vec![PlatformFramework {
        name: Some(String::new()),
        version: Some(String::new()),
    }]
}

/// The `CustomAmiList` a MANAGED platform version reports — "the custom AMIs
/// supported by the platform version": one entry per
/// [`PLATFORM_VIRTUALIZATION_TYPES`] value, each naming its virtualization type
/// and the machine image the version runs for it
/// ([`managed_platform_image_id`]).
///
/// The list is the same for every call about one version, so it is derived per
/// call rather than held in the store; the image it names is derived from the
/// version itself, so the same version reports the same image on every read and
/// no two versions report one image.
pub fn managed_custom_ami_list(
    region: &str,
    branch_name: &str,
    platform_version: &str,
) -> Vec<CustomAmi> {
    PLATFORM_VIRTUALIZATION_TYPES
        .iter()
        .map(|virtualization_type| CustomAmi {
            virtualization_type: Some((*virtualization_type).to_string()),
            // Reported for every entry; the image id on the entry whose
            // virtualization type the version ships an image for, and no value
            // on an entry whose type it ships none for.
            image_id: Some(
                managed_platform_image_id(
                    region,
                    branch_name,
                    platform_version,
                    virtualization_type,
                )
                .unwrap_or_default(),
            ),
        })
        .collect()
}

/// The `DefaultAmiList` a MANAGED platform version reports — "the AMIs that
/// Elastic Beanstalk uses by default for the platform version, one for each
/// supported processor architecture": one entry per architecture an AMI of a
/// platform version can have, naming that architecture and the default machine
/// image the version runs on it.
///
/// Which architectures those are is the model's own `AmiArchitecture`, the shape
/// `DefaultAmi$Architecture` targets — [`AMI_ARCHITECTURE_VALUES`], not a second
/// list written out here. A managed version is published for every one of them,
/// so every entry names an image; the entries are derived per call from the
/// version itself ([`managed_image_id`]), like [`managed_custom_ami_list`]'s.
pub fn managed_default_ami_list(
    region: &str,
    branch_name: &str,
    platform_version: &str,
) -> Vec<DefaultAmi> {
    AMI_ARCHITECTURE_VALUES
        .iter()
        .map(|architecture| DefaultAmi {
            architecture: Some((*architecture).to_string()),
            image_id: Some(managed_image_id(
                region,
                branch_name,
                platform_version,
                architecture,
            )),
        })
        .collect()
}

/// The `ImageId` of the machine image the version `platform_version` of the
/// branch `branch_name` runs in `region` under the virtualization type
/// `virtualization_type` — and `None` where the version ships no image for that
/// type.
///
/// Elastic Beanstalk builds the machine image of a platform version it publishes
/// for the HVM virtualization type, the one every current EC2 instance
/// generation boots ([`PLATFORM_VIRTUALIZATION_TYPE_HVM`]); the paravirtual type
/// precedes those generations, so a published version ships no image for it and
/// its entry names none, which is what the recorded `DescribePlatformVersion`
/// responses of a managed version carry.
fn managed_platform_image_id(
    region: &str,
    branch_name: &str,
    platform_version: &str,
    virtualization_type: &str,
) -> Option<String> {
    if virtualization_type != PLATFORM_VIRTUALIZATION_TYPE_HVM {
        return None;
    }
    Some(managed_image_id(
        region,
        branch_name,
        platform_version,
        virtualization_type,
    ))
}

/// The `ImageId` of the machine image the version `platform_version` of the
/// branch `branch_name` runs in `region` for `qualifier` — the virtualization
/// type or the processor architecture the image is the version's image FOR.
///
/// An image is a REGIONAL EC2 resource of one platform version, so the id is
/// derived from exactly what identifies it — the region, the version's ARN
/// components and that qualifier — in the form EC2 issues an image id
/// in ([`MANAGED_IMAGE_ID_PREFIX`] and [`MANAGED_IMAGE_ID_DIGITS`] lowercase
/// hexadecimal digits under a leading `0`, the published `ami-0123456789abcdef0`
/// shape). Deriving it means every read of one version reports the same image,
/// no two versions report one image, and nothing has to mint or store an id.
/// The digest and its hexadecimal rendering are the `sha2` and `hex` crates', as
/// in [`super::managed_action_schedule`].
fn managed_image_id(
    region: &str,
    branch_name: &str,
    platform_version: &str,
    qualifier: &str,
) -> String {
    use ::sha2::Digest;

    let name = format!(
        "urn:{ARN_NAMESPACE}:{region}:platform/{branch_name}/{platform_version}:{qualifier}"
    );
    let digest = ::sha2::Sha256::digest(name.as_bytes());
    // A leading `0` and the digits the digest supplies, two per byte.
    let body = ::hex::encode(&digest[..(MANAGED_IMAGE_ID_DIGITS - 1) / 2]);
    format!("{MANAGED_IMAGE_ID_PREFIX}0{body}")
}

/// The prefix EC2 issues an image id under, which is the form an `ImageId`
/// naming one takes. See [`managed_platform_image_id`].
const MANAGED_IMAGE_ID_PREFIX: &str = "ami-";
/// How many hexadecimal digits an image id EC2 has issued carries — the long id
/// form every current EC2 resource id takes. See [`managed_platform_image_id`].
const MANAGED_IMAGE_ID_DIGITS: usize = 17;

/// The `SupportedAddonList` of a managed platform version whose branch supports
/// `supported_tiers` — "the additions associated with the platform version".
///
/// Every platform version Elastic Beanstalk publishes streams its logs to Amazon
/// S3 and to CloudWatch Logs and reports enhanced health through the health
/// daemon; the SQS daemon is what runs a WORKER environment, so it is an
/// addition only of a version whose branch supports the worker tier. That is the
/// rule, so the list follows the branch's tiers rather than being written out
/// per catalog entry.
pub fn managed_supported_addons(supported_tiers: &[String]) -> Vec<String> {
    SUPPORTED_ADDONS
        .iter()
        .filter(|addon| {
            **addon != ADDON_WORKER_DAEMON_SQSD
                || supported_tiers.iter().any(|tier| tier == TIER_WORKER)
        })
        .map(|addon| (*addon).to_string())
        .collect()
}

/// A platform branch's life-cycle state as the WIRE carries it, on every shape
/// that reports one — `PlatformBranchSummary$LifecycleState`
/// (`ListPlatformBranches`), `PlatformSummary$PlatformBranchLifecycleState` and
/// `PlatformDescription$PlatformBranchLifecycleState`: the catalog's own state,
/// capitalized.
///
/// The model spells this value set twice and differently — `PlatformSummary`
/// and `PlatformBranchSummary` document "`beta` | `supported` | `deprecated` |
/// `retired`" while `PlatformDescription` documents "`Beta` | `Supported` |
/// `Deprecated` | `Retired`" — and both cannot describe one attribute. The
/// recorded responses of the real service settle it: a branch life-cycle state
/// goes on the wire CAPITALIZED on all three shapes, while the lower-case
/// spellings are the value set a `ListPlatformBranches` `LifecycleState` FILTER
/// draws from. So the catalog HOLDS the documented lower-case state
/// ([`PLATFORM_BRANCH_LIFECYCLE_STATES`], which is what a filter term is
/// matched against) and every reporting path renders it through here. The
/// rendering is IDEMPOTENT: a state already capitalized comes back unchanged.
pub fn reported_branch_lifecycle_state(state: &str) -> String {
    let mut characters = state.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// One version of the AWS-managed platform catalog: the branch's catalog entry
/// and WHICH of the versions that branch publishes
/// ([`SolutionStack::published_versions`]) this is.
///
/// A branch's current release and the release it superseded are two distinct
/// platform versions of one catalog row — same languages, same category, same
/// branch, different `PlatformArn`, `SolutionStackName` and
/// `PlatformLifecycleState` — so a resolved ARN carries both halves.
#[derive(Debug, Clone)]
pub struct ManagedPlatformVersion {
    /// The catalog row this is a version of.
    pub stack: &'static SolutionStack,
    /// The version itself, one of [`SolutionStack::published_versions`].
    pub platform_version: String,
}

impl ManagedPlatformVersion {
    /// Whether this is its branch's newest, `Recommended`, release.
    pub fn is_recommended(&self) -> bool {
        self.platform_version == self.stack.platform_version
    }

    /// The `SolutionStackName` this version is reported under.
    pub fn name(&self) -> String {
        solution_stack_name_of(self.stack.branch_name, &self.platform_version)
    }

    /// The `PlatformArn` this version is named by in `region`.
    pub fn platform_arn(&self, region: &str) -> String {
        platform_arn_of(region, self.stack.branch_name, &self.platform_version)
    }

    /// The modelled `PlatformDescription` for this version.
    pub fn to_description(
        &self,
        region: &str,
        store: &ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    ) -> PlatformDescription {
        self.stack
            .to_description(region, store, &self.platform_version)
    }
}

/// The published managed platform version `platform_arn` names in `region`, if
/// the catalog publishes one.
///
/// This is the other half of resolving a `PlatformArn`: a CUSTOM version lives in
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::platform_versions`],
/// a MANAGED one in [`SOLUTION_STACKS`], and an ARN that answers to neither names
/// no platform version this service can launch. Every version the catalog
/// publishes resolves here, not only each branch's newest one, so an environment
/// can be launched on the release a branch's current version superseded — which
/// is what having a platform update available means.
pub fn managed_platform_version(region: &str, platform_arn: &str) -> Option<ManagedPlatformVersion> {
    SOLUTION_STACKS.iter().find_map(|stack| {
        stack
            .published_versions()
            .into_iter()
            .find(|platform_version| {
                platform_arn_of(region, stack.branch_name, platform_version) == platform_arn
            })
            .map(|platform_version| ManagedPlatformVersion {
                stack,
                platform_version,
            })
    })
}

/// The published managed platform version reported under the
/// `SolutionStackName` `name`, if the catalog publishes one.
///
/// A solution stack name and a platform version ARN name the SAME platform
/// version from the one catalog ([`SOLUTION_STACKS`]), so an operation handed
/// either one resolves the other through here rather than reporting only the
/// form the caller happened to use. The names this resolves are the ones
/// `ListAvailableSolutionStacks` offers — each branch's current version — so a
/// superseded release is named by its ARN, the form `ListPlatformVersions`
/// reports it under.
pub fn solution_stack_named(name: &str) -> Option<&'static SolutionStack> {
    SOLUTION_STACKS.iter().find(|stack| stack.name() == name)
}

/// The message this service refuses a `SolutionStackName` that names no
/// published solution stack with, built from the request:
/// `No Solution Stack named '<name>' found.`
///
/// The same shape as the not-found sentences every other create answers an
/// absent resource with, read from the one place every caller reads it.
pub fn missing_solution_stack_message(solution_stack_name: &str) -> String {
    format!("No Solution Stack named '{solution_stack_name}' found.")
}

/// The refusal a request that names the platform version to run by
/// `SolutionStackName` is owed when that name is not one this service publishes
/// — and `None` when it is, or when the request names no solution stack at all.
///
/// A solution stack name names a platform version out of the SERVICE's own
/// catalog ([`SOLUTION_STACKS`], what `ListAvailableSolutionStacks` reports):
/// a name the catalog does not offer names no platform version, so there is
/// nothing to launch, configure or describe against it and the request is
/// refused rather than accepted against a platform that cannot exist. The ONE
/// place that check is written, so every operation taking the member refuses the
/// same names with the same sentence.
pub fn unpublished_solution_stack_refusal(solution_stack_name: Option<&str>) -> Option<String> {
    let name = solution_stack_name?;
    solution_stack_named(name)
        .is_none()
        .then(|| missing_solution_stack_message(name))
}

/// The current release of each platform family this catalog publishes, as the
/// `v<major>.<minor>.<patch>` component every branch of that family is
/// published at.
///
/// Elastic Beanstalk issues a platform version number per platform-family
/// RELEASE, not per branch: it re-publishes a whole family — one platform
/// category on one operating system generation — at a time, so every branch of
/// that family carries the SAME `PlatformVersion` and they move up together
/// (the recorded platform-version responses show two different runtime branches
/// of the .NET-on-Linux family at one version). A branch no longer receiving
/// releases stays at the number of the release it last got, which is why each
/// operating system generation has a line of its own.
///
/// The numbers themselves are this emulator's own — AWS re-publishes its
/// platforms continuously and no emulator can track those releases — but a
/// family's number is named ONCE here and read by every row of
/// [`SOLUTION_STACKS`] in that family, so no two branches of one family can
/// drift apart.
const RELEASE_DOTNET_AL2023: &str = "3.5.0";
/// See [`RELEASE_DOTNET_AL2023`]: the .NET-on-Windows family, which has a
/// release line per Windows Server generation.
const RELEASE_WINDOWS_2025: &str = "2.20.0";
/// See [`RELEASE_WINDOWS_2025`].
const RELEASE_WINDOWS_2022: &str = "2.18.0";
/// See [`RELEASE_WINDOWS_2025`].
const RELEASE_WINDOWS_2019: &str = "2.16.0";
/// See [`RELEASE_WINDOWS_2025`]: the generation whose branches have deprecated,
/// which stays at the release it last received.
const RELEASE_WINDOWS_2016: &str = "2.12.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_DOCKER_AL2023: &str = "4.2.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_GO_AL2023: &str = "3.5.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_CORRETTO_AL2023: &str = "4.5.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_NODEJS_AL2023: &str = "6.7.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_PHP_AL2023: &str = "4.6.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_PYTHON_AL2023: &str = "4.7.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_RUBY_AL2023: &str = "3.6.0";
/// See [`RELEASE_DOTNET_AL2023`].
const RELEASE_TOMCAT_AL2023: &str = "5.4.0";

/// The solution stacks this emulator offers, which is what
/// `ListAvailableSolutionStacks` reports: the current version of every platform
/// branch in [`MANAGED_PLATFORM_BRANCHES`] that has not retired, because a
/// retired branch has no version an account can still deploy on.
///
/// This is the SERVICE's own platform catalog, not anything a caller creates,
/// so it is a table here rather than a map in
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState`]. Each entry
/// names its branch, so the two catalogs agree by construction, and each reads
/// its version off its family's release ([`RELEASE_DOTNET_AL2023`] and the
/// constants beside it) rather than carrying a number of its own.
/// `PermittedFileTypes` is "the permitted file types allowed for a solution
/// stack": the archive that runtime deploys from — a Tomcat platform deploys a
/// `war`, a Java SE (Corretto) platform a `jar`, and every platform accepts a
/// `zip` source bundle.
///
/// `platform_category` and `programming_languages` are the platform FAMILY and
/// the language runtime Elastic Beanstalk publishes for each of its platforms
/// (the "Elastic Beanstalk supported platforms" document), carried here because
/// neither can be derived from the branch name: the .NET-on-Linux family's
/// category is `.NET Core`, and a Corretto branch runs the language `Java`. A
/// platform that runs no language of its own (Docker runs whatever the image
/// holds) states none, and a runtime whose version the catalog does not state is
/// reported without one.
///
/// ORDER IS CONTRACTUAL: the operation reports the stacks "with the public
/// version first and then in reverse chronological order", and the handler
/// reports this table as it stands, so the table is written newest branch first
/// within each platform, in [`MANAGED_PLATFORM_BRANCHES`]' own order.
pub const SOLUTION_STACKS: &[SolutionStack] = &[
    // .NET on Linux.
    SolutionStack {
        branch_name: ".NET 10 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_DOTNET_AL2023,
        permitted_file_types: &["zip"],
        platform_category: ".NET Core",
        programming_languages: &[(".NET", "10")],
    },
    SolutionStack {
        branch_name: ".NET 9 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_DOTNET_AL2023,
        permitted_file_types: &["zip"],
        platform_category: ".NET Core",
        programming_languages: &[(".NET", "9")],
    },
    SolutionStack {
        branch_name: ".NET 8 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_DOTNET_AL2023,
        permitted_file_types: &["zip"],
        platform_category: ".NET Core",
        programming_languages: &[(".NET", "8")],
    },
    // .NET on Windows Server.
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2025",
        platform_version: RELEASE_WINDOWS_2025,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server Core 2025",
        platform_version: RELEASE_WINDOWS_2025,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2022",
        platform_version: RELEASE_WINDOWS_2022,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server Core 2022",
        platform_version: RELEASE_WINDOWS_2022,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2019",
        platform_version: RELEASE_WINDOWS_2019,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server Core 2019",
        platform_version: RELEASE_WINDOWS_2019,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server 2016",
        platform_version: RELEASE_WINDOWS_2016,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    SolutionStack {
        branch_name: "IIS 10.0 running on 64bit Windows Server Core 2016",
        platform_version: RELEASE_WINDOWS_2016,
        permitted_file_types: &["zip"],
        platform_category: ".NET",
        programming_languages: &[(".NET", "")],
    },
    // Docker: the single-container branch and the ECS-managed one.
    SolutionStack {
        branch_name: "Docker running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_DOCKER_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Docker",
        programming_languages: &[],
    },
    SolutionStack {
        branch_name: "ECS running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_DOCKER_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Docker",
        programming_languages: &[],
    },
    // Go.
    SolutionStack {
        branch_name: "Go 1 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_GO_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Go",
        programming_languages: &[("Go", "1")],
    },
    // Java SE (Corretto): a Java SE platform deploys a `jar`.
    SolutionStack {
        branch_name: "Corretto 25 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_CORRETTO_AL2023,
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "25")],
    },
    SolutionStack {
        branch_name: "Corretto 21 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_CORRETTO_AL2023,
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "21")],
    },
    SolutionStack {
        branch_name: "Corretto 17 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_CORRETTO_AL2023,
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "17")],
    },
    SolutionStack {
        branch_name: "Corretto 11 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_CORRETTO_AL2023,
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "11")],
    },
    SolutionStack {
        branch_name: "Corretto 8 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_CORRETTO_AL2023,
        permitted_file_types: &["jar", "zip"],
        platform_category: "Java",
        programming_languages: &[("Java", "8")],
    },
    // Node.js.
    SolutionStack {
        branch_name: "Node.js 24 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_NODEJS_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Node.js",
        programming_languages: &[("Node.js", "24")],
    },
    SolutionStack {
        branch_name: "Node.js 22 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_NODEJS_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Node.js",
        programming_languages: &[("Node.js", "22")],
    },
    // PHP.
    SolutionStack {
        branch_name: "PHP 8.5 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PHP_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "PHP",
        programming_languages: &[("PHP", "8.5")],
    },
    SolutionStack {
        branch_name: "PHP 8.4 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PHP_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "PHP",
        programming_languages: &[("PHP", "8.4")],
    },
    SolutionStack {
        branch_name: "PHP 8.3 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PHP_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "PHP",
        programming_languages: &[("PHP", "8.3")],
    },
    SolutionStack {
        branch_name: "PHP 8.2 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PHP_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "PHP",
        programming_languages: &[("PHP", "8.2")],
    },
    // Python.
    SolutionStack {
        branch_name: "Python 3.14 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PYTHON_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Python",
        programming_languages: &[("Python", "3.14")],
    },
    SolutionStack {
        branch_name: "Python 3.13 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PYTHON_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Python",
        programming_languages: &[("Python", "3.13")],
    },
    SolutionStack {
        branch_name: "Python 3.12 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PYTHON_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Python",
        programming_languages: &[("Python", "3.12")],
    },
    SolutionStack {
        branch_name: "Python 3.11 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_PYTHON_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Python",
        programming_languages: &[("Python", "3.11")],
    },
    // Ruby.
    SolutionStack {
        branch_name: "Ruby 4.0 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_RUBY_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Ruby",
        programming_languages: &[("Ruby", "4.0")],
    },
    SolutionStack {
        branch_name: "Ruby 3.4 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_RUBY_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Ruby",
        programming_languages: &[("Ruby", "3.4")],
    },
    SolutionStack {
        branch_name: "Ruby 3.3 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_RUBY_AL2023,
        permitted_file_types: &["zip"],
        platform_category: "Ruby",
        programming_languages: &[("Ruby", "3.3")],
    },
    // Tomcat: a Tomcat platform deploys a `war`.
    SolutionStack {
        branch_name: "Tomcat 11 with Corretto 25 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "25")],
    },
    SolutionStack {
        branch_name: "Tomcat 11 with Corretto 21 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "21")],
    },
    SolutionStack {
        branch_name: "Tomcat 11 with Corretto 17 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "17")],
    },
    SolutionStack {
        branch_name: "Tomcat 10 with Corretto 21 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "21")],
    },
    SolutionStack {
        branch_name: "Tomcat 10 with Corretto 17 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "17")],
    },
    SolutionStack {
        branch_name: "Tomcat 9 with Corretto 17 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "17")],
    },
    SolutionStack {
        branch_name: "Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "11")],
    },
    SolutionStack {
        branch_name: "Tomcat 9 with Corretto 8 running on 64bit Amazon Linux 2023",
        platform_version: RELEASE_TOMCAT_AL2023,
        permitted_file_types: &["war", "zip"],
        platform_category: "Tomcat",
        programming_languages: &[("Java", "8")],
    },
];

/// A platform branch: one line of the managed platform catalog Elastic
/// Beanstalk publishes for an account in a region, which is what
/// `ListPlatformBranches` reports.
#[derive(Debug, Clone)]
pub struct PlatformBranch {
    pub platform_name: String,
    pub branch_name: String,
    /// One of [`PLATFORM_BRANCH_LIFECYCLE_STATES`].
    pub lifecycle_state: String,
    /// "A larger `BranchOrder` value designates a newer platform branch within
    /// the platform."
    pub branch_order: i32,
    /// A subset of [`SUPPORTED_TIERS`].
    pub supported_tiers: Vec<String>,
}

impl PlatformBranch {
    /// The identity of a branch in the catalog: a branch name is unique within
    /// its platform, so the pair is the store's key and the value a
    /// `NextToken` names.
    pub fn key(&self) -> String {
        format!("{}/{}", self.platform_name, self.branch_name)
    }

    /// Whether `token` is this branch's [`Self::key`] — the same test, without
    /// building the key, for the page-boundary scan.
    pub fn has_key(&self, token: &str) -> bool {
        token
            .strip_prefix(self.platform_name.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| rest == self.branch_name)
    }

    /// The modelled `PlatformBranchSummary` for this record. `LifecycleState`
    /// goes on the wire in the capitalized form the service answers with (see
    /// [`reported_branch_lifecycle_state`]), the same rendering a platform
    /// version's `PlatformBranchLifecycleState` carries, so the two cannot
    /// disagree about one branch's state.
    pub fn to_summary(&self) -> PlatformBranchSummary {
        PlatformBranchSummary {
            platform_name: Some(self.platform_name.clone()),
            branch_name: Some(self.branch_name.clone()),
            lifecycle_state: Some(reported_branch_lifecycle_state(&self.lifecycle_state)),
            branch_order: self.branch_order,
            supported_tier_list: Some(self.supported_tiers.clone()),
        }
    }
}

/// Short names for the life-cycle states the catalog table below holds, from
/// [`PLATFORM_BRANCH_LIFECYCLE_STATES`].
pub(super) const SUPPORTED: &str = BRANCH_LIFECYCLE_SUPPORTED;
/// See [`SUPPORTED`].
pub(super) const DEPRECATED: &str = BRANCH_LIFECYCLE_DEPRECATED;
/// See [`SUPPORTED`].
pub(super) const RETIRED: &str = BRANCH_LIFECYCLE_RETIRED;

/// The managed platform branches an account can see in a region, as
/// `(PlatformName, BranchName, LifecycleState, BranchOrder, SupportedTierList)`.
///
/// Elastic Beanstalk's platform catalog is service-owned reference data, not
/// something a caller creates, so it is seeded into the store by
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::default`] and
/// read from there like any other record.
///
/// SOURCE: Elastic Beanstalk's own published platform-branch inventory — the
/// "Elastic Beanstalk supported platforms" and "Retired platform branches"
/// documents, which is what the real `ListPlatformBranches` enumerates. The
/// catalog is transcribed WHOLE, not sampled: AWS reports every branch of a
/// platform it has ever published, so each platform here carries its current
/// branches AND the deprecated and retired generations behind them, in the
/// documented `PlatformName` groups (a legacy preconfigured-Docker runtime is
/// its own platform, as `GlassFish` is).
///
/// `BranchOrder` is "an ordinal number that designates the order in which
/// platform branches have been added to a platform" and "a larger `BranchOrder`
/// value designates a newer platform branch within the platform": AWS issues it
/// per platform in STEPS OF 100, newest branch highest, leaving the operating
/// system generations in descending bands (the Amazon Linux 2023 branches above
/// the Amazon Linux 2 ones above the Amazon Linux ones), and two branches added
/// together share one ordinal. It is NOT a 0,1,2… position in this table, so it
/// is carried per row.
///
/// The life-cycle state is held in the documented lower-case spelling, which is
/// the value set a `LifecycleState` filter draws from; it reaches the wire
/// capitalized through [`reported_branch_lifecycle_state`]. The tier lists cover
/// both of [`SUPPORTED_TIERS`]: a Linux platform runs web server and worker
/// environments, a Windows platform only the web server tier, and a platform
/// whose branch is not a runtime an application is deployed on at all (the
/// Packer builder) supports NO tier.
#[allow(clippy::type_complexity)]
pub(super) const MANAGED_PLATFORM_BRANCHES: &[(&str, &str, &str, i32, &[&str])] = &[
    // .NET on Linux.
    (
        ".NET Core on Linux",
        ".NET 10 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11300,
        BOTH_TIERS,
    ),
    (
        ".NET Core on Linux",
        ".NET 9 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11200,
        BOTH_TIERS,
    ),
    (
        ".NET Core on Linux",
        ".NET 8 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11100,
        BOTH_TIERS,
    ),
    (
        ".NET Core on Linux",
        ".NET 6 running on 64bit Amazon Linux 2023",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    // The Amazon Linux 2 generation of this platform published ONE branch, the
    // unversioned `.NET Core` one, published alongside the oldest Amazon Linux
    // 2023 branch and so carrying the same ordinal.
    (
        ".NET Core on Linux",
        ".NET Core running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    // .NET on Windows Server: no worker tier runs on Windows.
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server 2025",
        SUPPORTED,
        10500,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server Core 2025",
        SUPPORTED,
        10400,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server 2022",
        SUPPORTED,
        10200,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server Core 2022",
        SUPPORTED,
        10100,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server 2019",
        SUPPORTED,
        10000,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server Core 2019",
        SUPPORTED,
        9900,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server 2016",
        DEPRECATED,
        9800,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 10.0 running on 64bit Windows Server Core 2016",
        DEPRECATED,
        9700,
        WEB_SERVER_ONLY,
    ),
    // The Windows Server generations behind the IIS 10.0 ones, retired whole:
    // each generation published its Server and (from 2012 R2) its Server Core
    // edition, under the IIS version that generation shipped.
    (
        ".NET on Windows Server",
        "IIS 8.5 running on 64bit Windows Server 2012 R2",
        RETIRED,
        9600,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 8.5 running on 64bit Windows Server Core 2012 R2",
        RETIRED,
        9500,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 8 running on 64bit Windows Server 2012",
        RETIRED,
        9400,
        WEB_SERVER_ONLY,
    ),
    (
        ".NET on Windows Server",
        "IIS 7.5 running on 64bit Windows Server 2008 R2",
        RETIRED,
        9300,
        WEB_SERVER_ONLY,
    ),
    // Docker: the Docker platform publishes the single-container Docker branch
    // and the ECS-managed multi-container branch of each generation.
    (
        "Docker",
        "Docker running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12000,
        BOTH_TIERS,
    ),
    (
        "Docker",
        "ECS running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11500,
        BOTH_TIERS,
    ),
    // The Amazon Linux 2 generation published a PREVIEW branch beside the
    // general one, so both carry that generation's ordinal.
    (
        "Docker",
        "(BETA) Docker running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Docker",
        "Docker running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Docker",
        "ECS running on 64bit Amazon Linux 2",
        RETIRED,
        10500,
        BOTH_TIERS,
    ),
    (
        "Docker",
        "Multi-container Docker running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Docker",
        "Docker running on 64bit Amazon Linux",
        RETIRED,
        9000,
        BOTH_TIERS,
    ),
    // GlassFish: a legacy preconfigured-Docker platform, retired whole. A
    // branch name states the application server's version AND the Java version
    // it runs, and the operating system as the catalog publishes it (`64bit
    // Debian`, with no distribution code name).
    (
        "GlassFish",
        "Preconfigured Docker - GlassFish 5.0 with Java 8 running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "GlassFish",
        "Preconfigured Docker - GlassFish 4.1 with Java 8 running on 64bit Amazon Linux",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    (
        "GlassFish",
        "Preconfigured Docker - GlassFish 4.1 with Java 8 running on 64bit Debian",
        RETIRED,
        9800,
        BOTH_TIERS,
    ),
    (
        "GlassFish",
        "Preconfigured Docker - GlassFish 4.0 with Java 7 running on 64bit Debian",
        RETIRED,
        9700,
        BOTH_TIERS,
    ),
    // Go. The Amazon Linux 2 generation shipped a preview branch beside the
    // general one, so both carry that generation's ordinal.
    (
        "Go",
        "Go 1 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12000,
        BOTH_TIERS,
    ),
    (
        "Go",
        "(BETA) Go 1 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Go",
        "Go 1 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Go",
        "Go 1 running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Go",
        "Preconfigured Docker - Go 1.4 running on 64bit Debian",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    (
        "Go",
        "Preconfigured Docker - Go 1.3 running on 64bit Debian",
        RETIRED,
        9800,
        BOTH_TIERS,
    ),
    // Java: ONE platform holds every Java SE branch Elastic Beanstalk has
    // published — the current Corretto branches on Amazon Linux 2023, the
    // Corretto generation on Amazon Linux 2 behind them, and the legacy `Java
    // <n>` branches on Amazon Linux the Corretto ones replaced.
    (
        "Java",
        "Corretto 25 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11400,
        BOTH_TIERS,
    ),
    (
        "Java",
        "Corretto 21 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11300,
        BOTH_TIERS,
    ),
    (
        "Java",
        "Corretto 17 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11200,
        BOTH_TIERS,
    ),
    (
        "Java",
        "Corretto 11 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Java",
        "Corretto 8 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        11000,
        BOTH_TIERS,
    ),
    // The Amazon Linux 2 generation of the same platform: its newest branch is
    // the Corretto 17 one, and the Corretto 11 and Corretto 8 branches were
    // each published beside the PREVIEW branch that previewed them, so a
    // preview carries the ordinal of the general branch it previewed.
    (
        "Java",
        "Corretto 17 running on 64bit Amazon Linux 2",
        RETIRED,
        10100,
        BOTH_TIERS,
    ),
    (
        "Java",
        "(BETA) Corretto 11 running on 64bit Amazon Linux 2",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Java",
        "Corretto 11 running on 64bit Amazon Linux 2",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Java",
        "(BETA) Corretto 8 running on 64bit Amazon Linux 2",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    (
        "Java",
        "Corretto 8 running on 64bit Amazon Linux 2",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    // The Amazon Linux generation, whose branches named the Java version
    // itself rather than the Corretto distribution, in the ordinal band below
    // the Amazon Linux 2 one.
    (
        "Java",
        "Java 8 running on 64bit Amazon Linux",
        RETIRED,
        9800,
        BOTH_TIERS,
    ),
    (
        "Java",
        "Java 7 running on 64bit Amazon Linux",
        RETIRED,
        9700,
        BOTH_TIERS,
    ),
    // Node.js.
    (
        "Node.js",
        "Node.js 24 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12300,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 22 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12200,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 20 running on 64bit Amazon Linux 2023",
        RETIRED,
        12100,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 18 running on 64bit Amazon Linux 2023",
        RETIRED,
        12000,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 18 running on 64bit Amazon Linux 2",
        RETIRED,
        11400,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 16 running on 64bit Amazon Linux 2",
        RETIRED,
        11300,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 14 running on 64bit Amazon Linux 2",
        RETIRED,
        11200,
        BOTH_TIERS,
    ),
    // Node.js: the Amazon Linux 2 generation published PREVIEW branches beside
    // the general ones, and the oldest generation ran on 32-bit as well as
    // 64-bit Amazon Linux.
    (
        "Node.js",
        "(BETA) Node.js 12 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 12 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js 10 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "(BETA) Node.js 10 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Node.js",
        "Node.js running on 32bit Amazon Linux",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    // PHP.
    (
        "PHP",
        "PHP 8.5 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12400,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.4 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12300,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.3 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12200,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.2 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12100,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.1 running on 64bit Amazon Linux 2023",
        RETIRED,
        12000,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.1 running on 64bit Amazon Linux 2",
        RETIRED,
        11400,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 8.0 running on 64bit Amazon Linux 2",
        RETIRED,
        11300,
        BOTH_TIERS,
    ),
    // The Amazon Linux 2 generation published a PREVIEW branch beside each
    // general one, so the two carry that branch's ordinal.
    (
        "PHP",
        "(BETA) PHP 7.4 running on 64bit Amazon Linux 2",
        RETIRED,
        11200,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 7.4 running on 64bit Amazon Linux 2",
        RETIRED,
        11200,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "(BETA) PHP 7.3 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 7.3 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "(BETA) PHP 7.2 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 7.2 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    // The Amazon Linux generation, whose newest branch carries that
    // generation's highest ordinal. That generation published both
    // architectures Amazon Linux ran on, so its 32-bit branches sit among its
    // 64-bit ones in the order the platform added them.
    (
        "PHP",
        "PHP 7.3 running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 7.2 running on 64bit Amazon Linux",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 7.1 running on 64bit Amazon Linux",
        RETIRED,
        9800,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 7.0 running on 64bit Amazon Linux",
        RETIRED,
        9700,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 5.6 running on 64bit Amazon Linux",
        RETIRED,
        9600,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 5.5 running on 64bit Amazon Linux",
        RETIRED,
        9500,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 5.4 running on 64bit Amazon Linux",
        RETIRED,
        9400,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 5.5 running on 32bit Amazon Linux",
        RETIRED,
        9300,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 5.4 running on 32bit Amazon Linux",
        RETIRED,
        9200,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 5.3 running on 64bit Amazon Linux",
        RETIRED,
        9100,
        BOTH_TIERS,
    ),
    (
        "PHP",
        "PHP 5.3 running on 32bit Amazon Linux",
        RETIRED,
        9000,
        BOTH_TIERS,
    ),
    // Packer: the platform whose one branch is the builder Elastic Beanstalk
    // runs to BUILD a custom platform version, rather than a runtime an
    // application is deployed on. It therefore supports no environment tier at
    // all, which is what its empty `SupportedTierList` says, and it publishes
    // no solution stack (see [`SOLUTION_STACKS`]).
    (
        "Packer",
        "Elastic Beanstalk Packer Builder",
        SUPPORTED,
        10000,
        NO_TIERS,
    ),
    // Python.
    (
        "Python",
        "Python 3.14 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        13500,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.13 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        13400,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.12 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        13300,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.11 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        13200,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.9 running on 64bit Amazon Linux 2023",
        RETIRED,
        13000,
        BOTH_TIERS,
    ),
    // The Amazon Linux 2 generation, one ordinal band per runtime version, with
    // a PREVIEW branch beside the general 3.7 one. The generation's oldest
    // runtime is Python 3.7: Python 3.6 ran on the Amazon Linux generation
    // below, never on Amazon Linux 2.
    (
        "Python",
        "Python 3.8 running on 64bit Amazon Linux 2",
        RETIRED,
        12000,
        BOTH_TIERS,
    ),
    (
        "Python",
        "(BETA) Python 3.7 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.7 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    // The Amazon Linux generation, which published each runtime on both
    // architectures Amazon Linux ran on, 64-bit branches first.
    (
        "Python",
        "Python 3.6 running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.4 running on 64bit Amazon Linux",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 2.7 running on 64bit Amazon Linux",
        RETIRED,
        9800,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 2.6 running on 64bit Amazon Linux",
        RETIRED,
        9700,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 3.4 running on 32bit Amazon Linux",
        RETIRED,
        9600,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 2.7 running on 32bit Amazon Linux",
        RETIRED,
        9500,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Python 2.6 running on 32bit Amazon Linux",
        RETIRED,
        9400,
        BOTH_TIERS,
    ),
    (
        "Python",
        "Preconfigured Docker - Python 3.4 running on 64bit Debian",
        RETIRED,
        9300,
        BOTH_TIERS,
    ),
    // Ruby, whose Amazon Linux generation published one branch per application
    // server (Puma, Passenger Standalone) rather than one per runtime version.
    (
        "Ruby",
        "Ruby 4.0 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12400,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 3.4 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12300,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 3.3 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12200,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 3.2 running on 64bit Amazon Linux 2023",
        RETIRED,
        12100,
        BOTH_TIERS,
    ),
    // The Amazon Linux 2 generation, whose newest branch is the Ruby 3.0 one
    // and which published a PREVIEW branch beside each of the branches below
    // it, so the two carry that branch's ordinal.
    (
        "Ruby",
        "Ruby 3.0 running on 64bit Amazon Linux 2",
        RETIRED,
        11300,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "(BETA) Ruby 2.7 running on 64bit Amazon Linux 2",
        RETIRED,
        11200,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 2.7 running on 64bit Amazon Linux 2",
        RETIRED,
        11200,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "(BETA) Ruby 2.6 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 2.6 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "(BETA) Ruby 2.5 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 2.5 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    // The Amazon Linux generation, whose branch names LEAD with the application
    // server the runtime is served behind: the Puma branches of that generation
    // first, then the Passenger Standalone branch of each of the same runtimes,
    // and below them the runtimes that predate the split, which name no
    // application server at all.
    (
        "Ruby",
        "Puma with Ruby 2.6 running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Puma with Ruby 2.5 running on 64bit Amazon Linux",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Puma with Ruby 2.4 running on 64bit Amazon Linux",
        RETIRED,
        9800,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Puma with Ruby 2.3 running on 64bit Amazon Linux",
        RETIRED,
        9700,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Puma with Ruby 2.2 running on 64bit Amazon Linux",
        RETIRED,
        9600,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Puma with Ruby 2.1 running on 64bit Amazon Linux",
        RETIRED,
        9500,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Puma with Ruby 2.0 running on 64bit Amazon Linux",
        RETIRED,
        9400,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Passenger Standalone with Ruby 2.6 running on 64bit Amazon Linux",
        RETIRED,
        9300,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Passenger Standalone with Ruby 2.5 running on 64bit Amazon Linux",
        RETIRED,
        9200,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Passenger Standalone with Ruby 2.4 running on 64bit Amazon Linux",
        RETIRED,
        9100,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Passenger Standalone with Ruby 2.3 running on 64bit Amazon Linux",
        RETIRED,
        9000,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Passenger Standalone with Ruby 2.2 running on 64bit Amazon Linux",
        RETIRED,
        8900,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Passenger Standalone with Ruby 2.1 running on 64bit Amazon Linux",
        RETIRED,
        8800,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Passenger Standalone with Ruby 2.0 running on 64bit Amazon Linux",
        RETIRED,
        8700,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 1.9.3 running on 64bit Amazon Linux",
        RETIRED,
        8600,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 1.8.7 running on 64bit Amazon Linux",
        RETIRED,
        8500,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 1.9.3 running on 32bit Amazon Linux",
        RETIRED,
        8400,
        BOTH_TIERS,
    ),
    (
        "Ruby",
        "Ruby 1.8.7 running on 32bit Amazon Linux",
        RETIRED,
        8300,
        BOTH_TIERS,
    ),
    // Tomcat: a branch names its servlet container AND the JDK it runs on.
    (
        "Tomcat",
        "Tomcat 11 with Corretto 25 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12800,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 11 with Corretto 21 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12700,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 11 with Corretto 17 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12600,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 10 with Corretto 21 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12500,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 10 with Corretto 17 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12400,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 9 with Corretto 17 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12200,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 9 with Corretto 11 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12100,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 9 with Corretto 8 running on 64bit Amazon Linux 2023",
        SUPPORTED,
        12000,
        BOTH_TIERS,
    ),
    // The Amazon Linux 2 generation, which published a PREVIEW branch beside
    // each general one, so the two carry that branch's ordinal.
    (
        "Tomcat",
        "(BETA) Tomcat 8.5 with Corretto 11 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 8.5 with Corretto 11 running on 64bit Amazon Linux 2",
        RETIRED,
        11100,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "(BETA) Tomcat 8.5 with Corretto 8 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 8.5 with Corretto 8 running on 64bit Amazon Linux 2",
        RETIRED,
        11000,
        BOTH_TIERS,
    ),
    // The Amazon Linux generation, whose branches name the Java version rather
    // than the Corretto distribution, on both architectures that generation
    // ran on.
    (
        "Tomcat",
        "Tomcat 8.5 with Java 8 running on 64bit Amazon Linux",
        RETIRED,
        10200,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 8 with Java 8 running on 64bit Amazon Linux",
        RETIRED,
        10100,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 7 with Java 7 running on 64bit Amazon Linux",
        RETIRED,
        10000,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 7 with Java 6 running on 64bit Amazon Linux",
        RETIRED,
        9900,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 6 with Java 6 running on 64bit Amazon Linux",
        RETIRED,
        9800,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 7 with Java 7 running on 32bit Amazon Linux",
        RETIRED,
        9700,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 7 with Java 6 running on 32bit Amazon Linux",
        RETIRED,
        9600,
        BOTH_TIERS,
    ),
    (
        "Tomcat",
        "Tomcat 6 with Java 6 running on 32bit Amazon Linux",
        RETIRED,
        9500,
        BOTH_TIERS,
    ),
    // Windows: the legacy IIS-on-Windows-Server branches, published under a
    // platform name of their own rather than under ".NET on Windows Server" —
    // the way the legacy Java SE and preconfigured-Docker runtimes are platforms
    // of their own. The platform has retired whole, and the two branches were
    // published together, so they share one ordinal.
    (
        "Windows",
        "Legacy - IIS 7.5 running on 64bit Windows Server",
        RETIRED,
        9700,
        WEB_SERVER_ONLY,
    ),
    (
        "Windows",
        "Legacy - IIS 8 running on 64bit Windows Server",
        RETIRED,
        9700,
        WEB_SERVER_ONLY,
    ),
];

/// [`MANAGED_PLATFORM_BRANCHES`] as stored records, keyed by
/// [`PlatformBranch::key`].
pub(super) fn managed_platform_branch_catalog() -> BTreeMap<String, PlatformBranch> {
    MANAGED_PLATFORM_BRANCHES
        .iter()
        .map(
            |(platform_name, branch_name, lifecycle_state, branch_order, supported_tiers)| {
                let branch = PlatformBranch {
                    platform_name: (*platform_name).to_string(),
                    branch_name: (*branch_name).to_string(),
                    lifecycle_state: (*lifecycle_state).to_string(),
                    branch_order: *branch_order,
                    supported_tiers: supported_tiers
                        .iter()
                        .map(|tier| (*tier).to_string())
                        .collect(),
                };
                (branch.key(), branch)
            },
        )
        .collect()
}

/// `PlatformLifecycleState` for the newest version of a branch.
///
/// The model spells this value twice and differently — `PlatformSummary`
/// documents "Possible values: `recommended` | empty", `PlatformDescription`
/// "Possible values: `Recommended` | `null`" — and the recorded
/// platform-version responses of the real service carry the CAPITALIZED form on
/// both shapes, so that is the value a platform version reports. What a version
/// that is NOT its branch's recommended one reports is the other half of those
/// two sentences, and differs per shape: see
/// [`reported_platform_lifecycle_state`].
pub const PLATFORM_LIFECYCLE_RECOMMENDED: &str = "Recommended";

/// The `PlatformSummary$PlatformLifecycleState` of a published platform version,
/// given whether it is its branch's recommended one.
///
/// The member is reported for EVERY summary: it documents "Possible values:
/// `recommended` | empty" and "If an empty value is returned, the platform
/// version is supported but isn't the recommended one for its branch" — so a
/// version that is not its branch's recommended one is reported with an empty
/// value rather than with no member at all, which is how a client reading the
/// summary learns the version is supported and not recommended instead of
/// learning nothing. The recommended version of a branch carries
/// [`PLATFORM_LIFECYCLE_RECOMMENDED`].
///
/// `PlatformDescription$PlatformLifecycleState` is the other sentence —
/// "`Recommended` | `null`" — so there the member is absent for a version that
/// is not recommended (see [`SolutionStack::to_description`]).
pub fn reported_platform_lifecycle_state(is_recommended: bool) -> String {
    match is_recommended {
        true => PLATFORM_LIFECYCLE_RECOMMENDED.to_string(),
        false => String::new(),
    }
}

/// Order two version strings, and more generally two platform attribute values.
///
/// A dotted-numeric value — a `PlatformVersion` such as `4.0.11`, an
/// `OperatingSystemVersion` — is ordered component by component NUMERICALLY, so
/// `4.0.11` is above `4.0.9` as a version ordering requires, and a shorter
/// prefix (`4.0`) below a longer value that extends it (`4.0.1`). A value that
/// is not dotted-numeric orders by bytes among its own kind and AFTER every
/// dotted-numeric one.
///
/// That last rule is what makes this a TOTAL order, which `sort_by` requires:
/// comparing numerically within one kind and by bytes across kinds would not be
/// transitive (`10` > `9` numerically, `9` > `1a` by bytes, yet `10` < `1a` by
/// bytes), and a caller can put such a mix in the store, since
/// `CreatePlatformVersion` takes `PlatformVersion` as a string. The ordering is
/// equivalent to sorting on the key `(is not dotted-numeric, components, bytes)`.
///
/// The newest version of a platform branch is decided by this ordering, and so
/// are `ListPlatformVersions`' `<`/`<=`/`>`/`>=` filter operators, so the two
/// cannot disagree.
pub fn version_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    match (numeric_components(left), numeric_components(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.cmp(right),
    }
}

/// `value`'s `.`-separated components when every one of them is a decimal
/// number, `None` otherwise.
pub(super) fn numeric_components(value: &str) -> Option<Vec<u64>> {
    if value.is_empty() {
        return None;
    }
    value
        .split('.')
        .map(|component| component.parse::<u64>().ok())
        .collect()
}

/// One platform version as `ListPlatformVersions` reports AND filters it: the
/// modelled `PlatformSummary` the response carries, plus the names of the
/// programming languages the version runs.
///
/// The languages ride alongside because `PlatformFilter$Type` names
/// `ProgrammingLanguageName` as a platform-version attribute a term may select
/// on, while `PlatformSummary` models no language member — so a summary alone
/// cannot answer such a term, and the catalog's languages would be unreachable
/// to the filter.
#[derive(Debug, Clone)]
pub struct ListedPlatformVersion {
    pub summary: PlatformSummary,
    pub programming_language_names: Vec<String>,
}

/// The branch named `branch_name` in `branches`, if the catalog holds it: the
/// ONE branch-by-name lookup, so a caller holding the catalog map and a caller
/// holding the whole store
/// ([`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::platform_branch_named`])
/// resolve a version's branch the same way. A branch name is unique within the
/// catalog, so at most one answers.
pub fn platform_branch_named<'a>(
    branches: &'a BTreeMap<String, PlatformBranch>,
    branch_name: &str,
) -> Option<&'a PlatformBranch> {
    branches
        .values()
        .find(|branch| branch.branch_name == branch_name)
}

/// The AWS-managed platform versions a request in `region` can list.
///
/// The catalog is the service's own: [`SOLUTION_STACKS`] holds the published
/// version of every platform branch, and `branches` — the branch catalog the
/// store holds and `ListPlatformBranches` reports — supplies that branch's
/// life-cycle state and supported tiers, so the two operations cannot disagree
/// about them. Only the ARNs are regional, so the list is rendered per request
/// rather than copied into the store, where it would go stale against the
/// region the caller signed for.
///
/// Every member the catalog states is projected — the platform's category, the
/// operating system it runs on, the add-ons it supports, its branch and the
/// life-cycle state of version and branch — so a client filtering or displaying
/// on one of them reads the same value `DescribePlatformVersion` reports.
/// `PlatformLifecycleState` is reported on every summary
/// ([`reported_platform_lifecycle_state`]): [`PLATFORM_LIFECYCLE_RECOMMENDED`]
/// on each branch's NEWEST release and empty on the release it superseded, which
/// the catalog publishes beside it ([`SolutionStack::published_versions`]). A
/// stack whose branch the store does not hold is not published: a version of a
/// branch nobody can see is not available to the account either.
pub fn managed_platform_listings(
    region: &str,
    branches: &BTreeMap<String, PlatformBranch>,
) -> Vec<ListedPlatformVersion> {
    SOLUTION_STACKS
        .iter()
        .flat_map(|stack| {
            stack
                .published_versions()
                .into_iter()
                .map(move |platform_version| ManagedPlatformVersion {
                    stack,
                    platform_version,
                })
        })
        .filter_map(|version| {
            let stack = version.stack;
            let branch = platform_branch_named(branches, stack.branch_name)?;
            let operating_system = stack.operating_system();
            Some(ListedPlatformVersion {
                summary: PlatformSummary {
                    platform_arn: Some(version.platform_arn(region)),
                    platform_owner: Some(MANAGED_PLATFORM_OWNER.to_string()),
                    platform_status: Some(PLATFORM_STATUS_READY.to_string()),
                    platform_version: Some(version.platform_version.clone()),
                    platform_branch_name: Some(branch.branch_name.clone()),
                    platform_branch_lifecycle_state: Some(reported_branch_lifecycle_state(
                        &branch.lifecycle_state,
                    )),
                    supported_tier_list: Some(branch.supported_tiers.clone()),
                    platform_lifecycle_state: Some(reported_platform_lifecycle_state(
                        version.is_recommended(),
                    )),
                    platform_category: Some(stack.platform_category.to_string()),
                    operating_system_name: operating_system.map(|(name, _)| name.to_string()),
                    operating_system_version: operating_system
                        .and_then(|(_, version)| version)
                        .map(str::to_string),
                    supported_addon_list: Some(managed_supported_addons(&branch.supported_tiers)),
                },
                programming_language_names: stack.programming_language_names(),
            })
        })
        .collect()
}

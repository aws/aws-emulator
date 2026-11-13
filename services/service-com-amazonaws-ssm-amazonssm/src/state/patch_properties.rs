// SPDX-License-Identifier: CC0-1.0

//! The patch metadata Patch Manager PUBLISHES for an operating system: the products,
//! product families, classifications and severities the patches available for that
//! operating system carry.
//!
//! `DescribePatchProperties` reports those values — "Lists the properties of available
//! patches organized by product, product family, classification, severity, and other
//! properties of available patches" — so what it answers is a projection of the catalogue
//! below ([`PUBLISHED_PATCH_METADATA`]) rather than anything a caller wrote: this
//! metadata exists in every account and Region without anybody creating it, the way the
//! documents of [`super::PUBLISHED_DOCUMENTS`] and the baselines of
//! [`super::patch_baselines::PREDEFINED_BASELINES`] do.
//!
//! WHICH property an operating system publishes is part of the catalogue rather than a
//! rule laid over it: the operation's own doc lists, per operating system, the properties
//! "that can be used in filters" — `PRODUCT | CLASSIFICATION | SEVERITY` for the
//! RPM-based Linux distributions, `PRODUCT | PRIORITY` for the Debian-based ones,
//! `PRODUCT | CLASSIFICATION` for macOS and
//! `PRODUCT | PRODUCT_FAMILY | CLASSIFICATION | MSRC_SEVERITY` for Windows — so a group
//! below carries values for exactly the properties its operating system publishes, and a
//! property it publishes none of is no property of its patches at all: a read of one is
//! refused ([`unsupported_property_rejection`]) rather than answered with nothing.
//!
//! Windows is the one operating system whose published patches are split into two PATCH
//! SETS: "Indicates whether to list patches for the Windows operating system or for
//! applications released by Microsoft. Not applicable for the Linux or macOS operating
//! systems" (`DescribePatchProperties$PatchSet`), which is why a group carries the patch
//! set it belongs to and why the member is ignored for every other operating system.

use std::collections::{BTreeMap, HashMap};

/// The key every entry of a reported property list carries, as the operation's own
/// example output spells it (`{"Properties": [{"Name": "AmazonLinux2018.03"}]}`) — within
/// the modelled `AttributeName` length (`len 1..64`).
pub const PROPERTY_ENTRY_NAME_KEY: &str = "Name";

/// The key a reported PRODUCT entry carries the product's family under, where the
/// operating system publishes product families: a Windows product is reported with the
/// family it belongs to beside its name (`{"Name": "Office 2019", "ProductFamily":
/// "Office"}`), which is why an entry is a map of attributes rather than one value.
pub const PROPERTY_ENTRY_PRODUCT_FAMILY_KEY: &str = "ProductFamily";

/// The patch set the Windows OS patches belong to, and the one every other operating
/// system's patches are reported under. One of the modelled `PatchSet` values
/// (`crate::ops::types::PATCH_SET_VALUES`).
pub const PATCH_SET_OS: &str = "OS";

/// The patch set the patches for "applications released by Microsoft" belong to
/// (`DescribePatchProperties$PatchSet`). The other modelled `PatchSet` value.
pub const PATCH_SET_APPLICATION: &str = "APPLICATION";

/// The one operating system whose published patches are split into two patch sets: the
/// member is "not applicable for the Linux or macOS operating systems", so it is Windows
/// alone that has an application set beside its operating-system set.
pub const PATCH_SET_OPERATING_SYSTEM: &str = "WINDOWS";

/// The property whose values are the specific products a patch applies to, e.g.
/// `WindowsServer2016` or `AmazonLinux2018.03` (`Patch$Product`). One of the modelled
/// `PatchProperty` values (`crate::ops::types::PATCH_PROPERTY_VALUES`).
pub const PROPERTY_PRODUCT: &str = "PRODUCT";

/// The property whose values are the product families a patch applies to, e.g. `Windows`
/// or `Office` (`DescribeAvailablePatches$Filters`). One of the modelled `PatchProperty`
/// values.
pub const PROPERTY_PRODUCT_FAMILY: &str = "PRODUCT_FAMILY";

/// The property whose values are patch classifications, e.g. `SecurityUpdates`,
/// `Updates` or `CriticalUpdates` (`Patch$Classification`). One of the modelled
/// `PatchProperty` values.
pub const PROPERTY_CLASSIFICATION: &str = "CLASSIFICATION";

/// The property whose values are the Microsoft Security Response Center severities, e.g.
/// `Critical`, `Important` or `Moderate` — "Applies to Windows patches only"
/// (`Patch$MsrcSeverity`). One of the modelled `PatchProperty` values.
pub const PROPERTY_MSRC_SEVERITY: &str = "MSRC_SEVERITY";

/// The property whose values are the package priorities a Debian-based distribution
/// publishes. One of the modelled `PatchProperty` values.
pub const PROPERTY_PRIORITY: &str = "PRIORITY";

/// The property whose values are patch severity levels, e.g. `Critical` or `Moderate`
/// (`Patch$Severity`). One of the modelled `PatchProperty` values.
pub const PROPERTY_SEVERITY: &str = "SEVERITY";

/// The lower bound the modelled `MaxResults` of this read carries
/// (`DescribePatchPropertiesRequest$MaxResults`, range 1..50).
pub const MIN_PATCH_PROPERTIES_PAGE_SIZE: i32 = 1;

/// The upper bound of the same member, and the page size a request naming no
/// `MaxResults` is served up to: "The maximum number of items to return for this call"
/// bounds a page the caller asked to be bounded, so a caller asking for no bound is
/// served up to the modelled maximum.
pub const MAX_PATCH_PROPERTIES_PAGE_SIZE: i32 = 50;

/// The page size a patch-property read serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the boundary validator already refuses, so this is
/// a backstop rather than the first line of defence (see [`super::page_size`]).
pub fn page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(
        max_results,
        MIN_PATCH_PROPERTIES_PAGE_SIZE,
        MAX_PATCH_PROPERTIES_PAGE_SIZE,
    )
}

/// A property an operating system publishes no values for at all.
const NO_VALUES: &[&str] = &[];

/// The classifications Windows patches carry, as `Patch$Classification` and the patch
/// filter docs spell them (`ServicePacks | SecurityUpdates | DefinitionUpdates`,
/// `CriticalUpdates`, `Updates`).
const WINDOWS_CLASSIFICATIONS: &[&str] = &[
    "CriticalUpdates",
    "DefinitionUpdates",
    "Drivers",
    "FeaturePacks",
    "SecurityUpdates",
    "ServicePacks",
    "Tools",
    "UpdateRollups",
    "Updates",
    "Upgrades",
];

/// The Microsoft Security Response Center severities Windows patches carry: "such as
/// `Critical`, `Important`, or `Moderate`" (`Patch$MsrcSeverity`), beside the `Low` and
/// `Unspecified` the patch filter docs list (`MSRC_SEVERITY` sample values).
const WINDOWS_MSRC_SEVERITIES: &[&str] =
    &["Critical", "Important", "Low", "Moderate", "Unspecified"];

/// The classifications an RPM-based distribution's advisories carry — the update kinds
/// `yum updateinfo` publishes, which is what Patch Manager reports for Amazon Linux,
/// CentOS, Oracle Linux, Red Hat Enterprise Linux and the Red Hat rebuilds.
const RPM_CLASSIFICATIONS: &[&str] = &[
    "Bugfix",
    "Enhancement",
    "Newpackage",
    "Recommended",
    "Security",
];

/// The severity levels an RPM-based distribution's advisories carry: "For example,
/// `CRITICAL` or `MODERATE`" (`Patch$Severity`), in the casing the advisories publish.
const RPM_SEVERITIES: &[&str] = &["Critical", "Important", "Low", "Moderate"];

/// The severity levels an Amazon Linux security advisory (ALAS) carries, which names its
/// middle level `Medium` where a Red Hat advisory names it `Moderate`.
const AMAZON_LINUX_SEVERITIES: &[&str] = &["Critical", "Important", "Low", "Medium"];

/// The classifications a SUSE patch carries — the categories `zypper` publishes for a
/// SUSE Linux Enterprise repository.
const SUSE_CLASSIFICATIONS: &[&str] = &[
    "Document",
    "Feature",
    "Optional",
    "Recommended",
    "Security",
    "Yast",
];

/// The package priorities a Debian-based distribution publishes, which is the property
/// `DescribePatchProperties` reports for Debian, Raspberry Pi OS and Ubuntu in place of a
/// classification.
const DEBIAN_PRIORITIES: &[&str] = &[
    "Extra",
    "Important",
    "Optional",
    "Required",
    "Standard",
];

/// The classifications a macOS software update carries, the only property macOS publishes
/// beside its products.
const MACOS_CLASSIFICATIONS: &[&str] = &["Recommended", "Security"];

/// One group of published patch metadata: the products of ONE product family of one
/// operating system's patch set, and the values the patches for them carry for every
/// other property that operating system publishes.
///
/// A property the operating system publishes nothing for holds [`NO_VALUES`], so what
/// `DescribePatchProperties` can report for an operating system is exactly what this
/// catalogue holds for it.
#[derive(Debug, Clone, Copy)]
pub struct PublishedPatchGroup {
    /// The operating system these patches are published for: one of the modelled
    /// `OperatingSystem` values (`crate::ops::types::OPERATING_SYSTEM_VALUES`).
    pub operating_system: &'static str,
    /// The patch set the group belongs to ([`PATCH_SET_OS`] or
    /// [`PATCH_SET_APPLICATION`]).
    pub patch_set: &'static str,
    /// The product family these products belong to, or `""` for an operating system that
    /// publishes no product family at all.
    pub product_family: &'static str,
    /// The specific products the patches of this group apply to.
    pub products: &'static [&'static str],
    /// The classifications those patches carry.
    pub classifications: &'static [&'static str],
    /// The Microsoft Security Response Center severities those patches carry.
    pub msrc_severities: &'static [&'static str],
    /// The package priorities those patches carry.
    pub priorities: &'static [&'static str],
    /// The severity levels those patches carry.
    pub severities: &'static [&'static str],
}

/// The published metadata of an RPM-based distribution, whose valid properties are
/// `PRODUCT | CLASSIFICATION | SEVERITY`.
const fn rpm_linux(
    operating_system: &'static str,
    products: &'static [&'static str],
    classifications: &'static [&'static str],
    severities: &'static [&'static str],
) -> PublishedPatchGroup {
    PublishedPatchGroup {
        operating_system,
        patch_set: PATCH_SET_OS,
        product_family: "",
        products,
        classifications,
        msrc_severities: NO_VALUES,
        priorities: NO_VALUES,
        severities,
    }
}

/// The published metadata of a Debian-based distribution, whose valid properties are
/// `PRODUCT | PRIORITY`.
const fn debian_linux(
    operating_system: &'static str,
    products: &'static [&'static str],
) -> PublishedPatchGroup {
    PublishedPatchGroup {
        operating_system,
        patch_set: PATCH_SET_OS,
        product_family: "",
        products,
        classifications: NO_VALUES,
        msrc_severities: NO_VALUES,
        priorities: DEBIAN_PRIORITIES,
        severities: NO_VALUES,
    }
}

/// The published metadata of one Windows product family, whose valid properties are
/// `PRODUCT | PRODUCT_FAMILY | CLASSIFICATION | MSRC_SEVERITY`.
const fn windows(
    patch_set: &'static str,
    product_family: &'static str,
    products: &'static [&'static str],
) -> PublishedPatchGroup {
    PublishedPatchGroup {
        operating_system: PATCH_SET_OPERATING_SYSTEM,
        patch_set,
        product_family,
        products,
        classifications: WINDOWS_CLASSIFICATIONS,
        msrc_severities: WINDOWS_MSRC_SEVERITIES,
        priorities: NO_VALUES,
        severities: NO_VALUES,
    }
}

/// The patch metadata Patch Manager publishes, with a group per product family of every
/// operating system the modelled `OperatingSystem` enum declares — so every operating
/// system a request may name has a catalogue to report, and the products it names are
/// the ones that operating system's patches apply to (`Patch$Product`: "For example,
/// `WindowsServer2016` or `AmazonLinux2018.03`").
pub const PUBLISHED_PATCH_METADATA: &[PublishedPatchGroup] = &[
    rpm_linux(
        "ALMA_LINUX",
        &["AlmaLinux8", "AlmaLinux9"],
        RPM_CLASSIFICATIONS,
        RPM_SEVERITIES,
    ),
    rpm_linux(
        "AMAZON_LINUX",
        &["AmazonLinux2017.09", "AmazonLinux2018.03"],
        RPM_CLASSIFICATIONS,
        AMAZON_LINUX_SEVERITIES,
    ),
    rpm_linux(
        "AMAZON_LINUX_2",
        &["AmazonLinux2", "AmazonLinux2.0"],
        RPM_CLASSIFICATIONS,
        AMAZON_LINUX_SEVERITIES,
    ),
    rpm_linux(
        "AMAZON_LINUX_2022",
        &["AmazonLinux2022"],
        RPM_CLASSIFICATIONS,
        AMAZON_LINUX_SEVERITIES,
    ),
    rpm_linux(
        "AMAZON_LINUX_2023",
        &["AmazonLinux2023"],
        RPM_CLASSIFICATIONS,
        AMAZON_LINUX_SEVERITIES,
    ),
    rpm_linux(
        "CENTOS",
        &["CentOS7", "CentOS8"],
        RPM_CLASSIFICATIONS,
        RPM_SEVERITIES,
    ),
    debian_linux("DEBIAN", &["Debian10", "Debian11", "Debian12"]),
    PublishedPatchGroup {
        operating_system: "MACOS",
        patch_set: PATCH_SET_OS,
        product_family: "",
        products: &["MacOS12.0", "MacOS13.0", "MacOS14.0", "MacOS15.0"],
        classifications: MACOS_CLASSIFICATIONS,
        msrc_severities: NO_VALUES,
        priorities: NO_VALUES,
        severities: NO_VALUES,
    },
    rpm_linux(
        "ORACLE_LINUX",
        &["OracleLinux7", "OracleLinux8", "OracleLinux9"],
        RPM_CLASSIFICATIONS,
        RPM_SEVERITIES,
    ),
    debian_linux("RASPBIAN", &["Raspbian10", "Raspbian11"]),
    rpm_linux(
        "REDHAT_ENTERPRISE_LINUX",
        &[
            "RedhatEnterpriseLinux7",
            "RedhatEnterpriseLinux8",
            "RedhatEnterpriseLinux9",
        ],
        RPM_CLASSIFICATIONS,
        RPM_SEVERITIES,
    ),
    rpm_linux(
        "ROCKY_LINUX",
        &["RockyLinux8", "RockyLinux9"],
        RPM_CLASSIFICATIONS,
        RPM_SEVERITIES,
    ),
    rpm_linux(
        "SUSE",
        &["Suse12", "Suse15"],
        SUSE_CLASSIFICATIONS,
        RPM_SEVERITIES,
    ),
    debian_linux(
        "UBUNTU",
        &["Ubuntu18.04", "Ubuntu20.04", "Ubuntu22.04", "Ubuntu24.04"],
    ),
    windows(
        PATCH_SET_OS,
        "Windows",
        &[
            "Windows10",
            "Windows11",
            "WindowsServer2012",
            "WindowsServer2012R2",
            "WindowsServer2016",
            "WindowsServer2019",
            "WindowsServer2022",
            "WindowsServer2025",
        ],
    ),
    windows(
        PATCH_SET_APPLICATION,
        "Exchange",
        &["Exchange Server 2016", "Exchange Server 2019"],
    ),
    windows(
        PATCH_SET_APPLICATION,
        "Office",
        &["Office 2013", "Office 2016", "Office 2019", "Office 2021"],
    ),
    windows(
        PATCH_SET_APPLICATION,
        "SQL Server",
        &["SQL Server 2017", "SQL Server 2019", "SQL Server 2022"],
    ),
    windows(
        PATCH_SET_APPLICATION,
        "Developer Tools, Runtimes, and Redistributables",
        &["Visual Studio 2019", "Visual Studio 2022"],
    ),
    // The Microsoft anti-malware product the patch filter docs name beside the Windows
    // Server and Office products (`DescribeAvailablePatches$Filters`: `PRODUCT` sample
    // values `WindowsServer2012 | Office 2010 | MicrosoftDefenderAntivirus`,
    // `PRODUCT_FAMILY` sample values `Windows | Office`), which ships its updates as an
    // application rather than as part of the operating system.
    windows(
        PATCH_SET_APPLICATION,
        "Windows",
        &["MicrosoftDefenderAntivirus"],
    ),
];

/// The patch set a request is about: the one it names for Windows, and the
/// operating-system set for every other operating system, because the member is "not
/// applicable for the Linux or macOS operating systems" and for them the catalogue holds
/// one set.
///
/// A Windows request that names none is about the operating system's own patches rather
/// than the Microsoft applications', which is the set a request has to ASK for.
pub fn addressed_patch_set(operating_system: &str, patch_set: Option<&str>) -> &'static str {
    if operating_system != PATCH_SET_OPERATING_SYSTEM {
        return PATCH_SET_OS;
    }
    match patch_set {
        Some(PATCH_SET_APPLICATION) => PATCH_SET_APPLICATION,
        _ => PATCH_SET_OS,
    }
}

/// Whether `operating_system` publishes `property` at all — whether it is one of the
/// properties the operation's doc lists as valid for it ("DEBIAN Valid properties:
/// PRODUCT | PRIORITY").
///
/// Read from the catalogue rather than from a list beside it: an operating system
/// publishes the properties its own patch metadata carries values for, in either of its
/// patch sets, so there is one place to extend when a group is added.
pub fn publishes_property(operating_system: &str, property: &str) -> bool {
    PUBLISHED_PATCH_METADATA
        .iter()
        .filter(|group| group.operating_system == operating_system)
        .any(|group| match property {
            PROPERTY_PRODUCT => !group.products.is_empty(),
            PROPERTY_PRODUCT_FAMILY => published_family(group).is_some(),
            PROPERTY_CLASSIFICATION => !group.classifications.is_empty(),
            PROPERTY_MSRC_SEVERITY => !group.msrc_severities.is_empty(),
            PROPERTY_PRIORITY => !group.priorities.is_empty(),
            PROPERTY_SEVERITY => !group.severities.is_empty(),
            // A property outside the modelled `PatchProperty` enum, which the boundary
            // walk refuses before a handler reaches this.
            _ => false,
        })
}

/// The order the operation's doc lists an operating system's valid properties in
/// ("WINDOWS Valid properties: PRODUCT | PRODUCT_FAMILY | CLASSIFICATION |
/// MSRC_SEVERITY", "AMAZON_LINUX_2 Valid properties: PRODUCT | CLASSIFICATION |
/// SEVERITY", "DEBIAN Valid properties: PRODUCT | PRIORITY"), which is the order a
/// refusal names them in.
const PROPERTY_ORDER: &[&str] = &[
    PROPERTY_PRODUCT,
    PROPERTY_PRODUCT_FAMILY,
    PROPERTY_CLASSIFICATION,
    PROPERTY_MSRC_SEVERITY,
    PROPERTY_PRIORITY,
    PROPERTY_SEVERITY,
];

/// The properties `operating_system` publishes, in the order the operation's doc lists
/// them ([`PROPERTY_ORDER`]).
pub fn published_properties(operating_system: &str) -> Vec<&'static str> {
    PROPERTY_ORDER
        .iter()
        .copied()
        .filter(|property| publishes_property(operating_system, property))
        .collect()
}

/// Why `operating_system` cannot be asked about `property`, or `None` when it publishes
/// it.
///
/// A property an operating system does not publish is no property of its patches, so the
/// read it asks for cannot be served at all — not served as nothing, which would read as
/// "this operating system has no products". The sentence is the one this service words a
/// member carrying a value outside its legal set with — the member, that it is invalid,
/// and the values that are legal here, which for this member are the properties the named
/// operating system publishes ([`published_properties`]).
pub fn unsupported_property_rejection(operating_system: &str, property: &str) -> Option<String> {
    (!publishes_property(operating_system, property)).then(|| {
        format!(
            "{PROPERTY_MEMBER} {property} is invalid, valid values are {}",
            published_properties(operating_system).join(", ")
        )
    })
}

/// How the request member a property arrives in is named in a refusal: the modelled
/// member `Property`.
pub const PROPERTY_MEMBER: &str = "Property";

/// The product family a group's products belong to, or `None` for an operating system
/// that publishes no product family at all.
fn published_family(group: &PublishedPatchGroup) -> Option<&'static str> {
    (!group.product_family.is_empty()).then_some(group.product_family)
}

/// One value a patch-property read reports.
///
/// A value is reported as a MAP of attributes rather than as a bare string, so a product
/// of an operating system that publishes product families carries the family it belongs
/// to beside its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPropertyValue {
    /// The value itself, reported under [`PROPERTY_ENTRY_NAME_KEY`].
    pub name: String,
    /// The family this product belongs to, reported under
    /// [`PROPERTY_ENTRY_PRODUCT_FAMILY_KEY`]; `None` for a value that is not a product of
    /// an operating system that publishes product families.
    pub product_family: Option<String>,
}

impl PublishedPropertyValue {
    /// The entry a read reports for this value: its name, and the product family where
    /// the catalogue publishes one for it.
    pub fn to_entry(&self) -> HashMap<String, String> {
        let mut entry = HashMap::from([(PROPERTY_ENTRY_NAME_KEY.to_string(), self.name.clone())]);
        if let Some(product_family) = &self.product_family {
            entry.insert(
                PROPERTY_ENTRY_PRODUCT_FAMILY_KEY.to_string(),
                product_family.clone(),
            );
        }
        entry
    }
}

/// The values the published patch metadata of `operating_system`'s `patch_set` carries
/// for `property`, each once and in ascending order by name.
///
/// A property `operating_system` publishes nothing for — a Windows severity for a Debian
/// priority, a product family for any Linux distribution — yields no values at all, so
/// the list reports what the catalogue holds rather than what the request hoped for. The
/// order is total, which is what lets a page resume at the value it stopped on.
pub fn published_property_values(
    operating_system: &str,
    property: &str,
    patch_set: Option<&str>,
) -> Vec<PublishedPropertyValue> {
    let patch_set = addressed_patch_set(operating_system, patch_set);
    // Keyed by the value, so a value two groups both publish is reported once, and the
    // walk is in ascending order.
    let mut values: BTreeMap<&'static str, Option<&'static str>> = BTreeMap::new();
    for group in PUBLISHED_PATCH_METADATA
        .iter()
        .filter(|group| group.operating_system == operating_system && group.patch_set == patch_set)
    {
        if property == PROPERTY_PRODUCT {
            // A product is reported with the family it belongs to, where the operating
            // system publishes families.
            for product in group.products {
                values.insert(product, published_family(group));
            }
            continue;
        }
        if property == PROPERTY_PRODUCT_FAMILY {
            if let Some(product_family) = published_family(group) {
                values.insert(product_family, None);
            }
            continue;
        }
        let published: &[&'static str] = match property {
            PROPERTY_CLASSIFICATION => group.classifications,
            PROPERTY_MSRC_SEVERITY => group.msrc_severities,
            PROPERTY_PRIORITY => group.priorities,
            PROPERTY_SEVERITY => group.severities,
            // A property outside the modelled `PatchProperty` enum, which the boundary
            // walk refuses before a handler reaches this.
            _ => NO_VALUES,
        };
        for value in published {
            values.insert(value, None);
        }
    }
    values
        .into_iter()
        .map(|(name, product_family)| PublishedPropertyValue {
            name: name.to_string(),
            product_family: product_family.map(str::to_string),
        })
        .collect()
}

impl super::ComAmazonawsSsmAmazonssmState {
    /// The values this store reports for `property` of `operating_system`'s `patch_set`
    /// ([`published_property_values`]).
    ///
    /// The one door a patch-property read goes through, the way a published document
    /// resolves through [`super::ComAmazonawsSsmAmazonssmState::resolve_document`]: the
    /// values are Patch Manager's own published metadata, held by the store rather than
    /// by any one request, so two reads of one property report the same values and a call
    /// changes none of them.
    pub fn patch_property_values(
        &self,
        operating_system: &str,
        property: &str,
        patch_set: Option<&str>,
    ) -> Vec<PublishedPropertyValue> {
        published_property_values(operating_system, property, patch_set)
    }
}

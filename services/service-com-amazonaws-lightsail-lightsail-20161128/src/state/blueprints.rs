// SPDX-License-Identifier: CC0-1.0

//! The published blueprint catalog: the operating systems and applications an
//! instance can be created from, and the rules a request's blueprint is resolved
//! against.

use super::*;

/// The default user name for connecting to an instance of each blueprint family,
/// from the Lightsail user guide's "Connecting to your instance" tables: the
/// operating-system blueprints connect as `ec2-user` except Debian (`admin`) and
/// Ubuntu (`ubuntu`), Windows connects as `Administrator`, and every
/// Bitnami-packaged application blueprint connects as `bitnami`.
///
/// Matching is by longest id prefix. A blueprint family that is not listed
/// reports NO `username`: the member is optional, and guessing one would be worse
/// than saying nothing.
pub(super) const BLUEPRINT_USERNAMES: &[(&str, &str)] = &[
    ("almalinux", "ec2-user"),
    ("amazon_linux", "ec2-user"),
    ("centos", "ec2-user"),
    ("debian", "admin"),
    ("freebsd", "ec2-user"),
    ("opensuse", "ec2-user"),
    ("ubuntu", "ubuntu"),
    ("windows", "Administrator"),
    ("django", "bitnami"),
    ("drupal", "bitnami"),
    ("ghost", "bitnami"),
    ("gitlab", "bitnami"),
    ("joomla", "bitnami"),
    ("lamp", "bitnami"),
    ("magento", "bitnami"),
    ("mean", "bitnami"),
    ("nginx", "bitnami"),
    ("nodejs", "bitnami"),
    ("prestashop", "bitnami"),
    ("redmine", "bitnami"),
    ("wordpress", "bitnami"),
];

/// The user name an instance of `blueprint_id` connects as, or `None` when the
/// blueprint is not one of the families in [`BLUEPRINT_USERNAMES`].
pub fn blueprint_username(blueprint_id: &str) -> Option<&'static str> {
    BLUEPRINT_USERNAMES
        .iter()
        .filter(|(prefix, _)| blueprint_id.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, username)| *username)
}

/// The friendly name of the blueprint `blueprint_id`, as `Instance$blueprintName`
/// reports it for an instance created from it: the name the catalog publishes for
/// the image ([`BlueprintSpec::name`]) when this service publishes it, and the
/// id's words title-cased ([`blueprint_name`]) for a blueprint the catalog does
/// not hold — one name per blueprint, wherever it is reported.
pub fn blueprint_display_name(blueprint_id: &str) -> String {
    blueprint_spec(blueprint_id)
        .map(|spec| spec.name())
        .unwrap_or_else(|| blueprint_name(blueprint_id))
}

/// Whether `blueprint_id` is a Windows blueprint. Every Lightsail Windows
/// blueprint id begins `windows_` (`windows_server_2022`); every other blueprint
/// is a Linux/Unix one.
pub fn blueprint_is_windows(blueprint_id: &str) -> bool {
    blueprint_id.starts_with("windows")
}

/// The friendly blueprint name, rendered from the blueprint id by title-casing
/// its underscore-separated words.
///
/// That is the derivation the modelled `Instance$blueprintName` documentation
/// shows: blueprint id `amazon_linux_2023` is reported as `Amazon Linux 2023`.
pub fn blueprint_name(blueprint_id: &str) -> String {
    blueprint_id
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One entry of the instance blueprint catalog: a virtual private server image
/// an instance can be created from.
///
/// This is a PUBLISHED catalog, not per-account state — every account sees the
/// same entries — which is why it is a `const` table rather than a field on the
/// store, exactly like the bundle and Region catalogs above.
#[derive(Debug, Clone, Copy)]
pub struct BlueprintSpec {
    /// `Blueprint$blueprintId`: the id a `CreateInstances` request names
    /// (`amazon_linux_2023`, `wordpress`).
    pub blueprint_id: &'static str,
    /// `Blueprint$name` where the catalog's own name for the image is the name of
    /// the PRODUCT it installs rather than the words of its id (`SQL Server 2022
    /// Express` for `windows_server_2022_sql_2022_express`), and `None` where the
    /// catalog's name is the id's words title-cased — the derivation
    /// `Instance$blueprintName` reports ([`blueprint_name`]).
    pub published_name: Option<&'static str>,
    /// `Blueprint$minPower` where the image requires a LARGER plan than its
    /// platform's own floor, and `None` where the floor is the platform's
    /// ([`BlueprintSpec::min_power`]).
    ///
    /// The minimum is the image's own requirement, which is why the catalog
    /// publishes it per entry: an application image that installs a database
    /// engine beside the operating system does not run on the plans the bare
    /// operating system runs on.
    pub published_min_power: Option<i32>,
    /// `Blueprint$group` — "The group name of the blueprint (`amazon-linux`)".
    ///
    /// The group is PUBLISHED per entry rather than derived from the id: the
    /// catalog groups an image family under the name the service gives that
    /// family, which is not always the id's own words — the recorded
    /// `windows_server_2025` entry is grouped `windows_2025`, where its family
    /// carries the release and drops the `server` word.
    pub group: &'static str,
    /// `Blueprint$type`, one of the modelled `BlueprintType` enum's two values
    /// (`smithy_query(query="shape",shape="BlueprintType")`): `os` for an
    /// operating-system image, `app` for an application stack.
    pub blueprint_type: &'static str,
    /// `Blueprint$isActive`. From that member's own documentation: "Inactive
    /// blueprints are listed to support customers with existing instances but
    /// are not necessarily available for launch of new instances" — and
    /// `CreateInstances$blueprintId` says "Use active blueprints when creating
    /// new instances", so only an active entry can be created from.
    pub is_active: bool,
    /// `Blueprint$productUrl` — "The product URL to learn more about the image
    /// or blueprint", the page the image's VENDOR publishes about it, or empty
    /// where the catalog publishes none and the member is reported absent.
    pub product_url: &'static str,
    /// `Blueprint$licenseUrl` — "The end-user license agreement URL for the
    /// image or blueprint", the licence terms that vendor publishes, or empty
    /// where the catalog publishes none and the member is reported absent.
    pub license_url: &'static str,
}

/// One value of the modelled `BlueprintType` enum: an operating-system image.
pub const BLUEPRINT_TYPE_OS: &str = "os";

/// One value of the modelled `BlueprintType` enum: an application stack.
pub const BLUEPRINT_TYPE_APP: &str = "app";

/// The pages Microsoft publishes about Windows Server and about the licence
/// terms its images are offered under — `Blueprint$productUrl` and
/// `Blueprint$licenseUrl` of every Windows entry of [`BLUEPRINTS`], which the
/// recorded answer reports both of.
pub(super) const WINDOWS_SERVER_PRODUCT_URL: &str = "https://www.microsoft.com/en-us/windows-server";
/// See [`WINDOWS_SERVER_PRODUCT_URL`].
pub(super) const WINDOWS_SERVER_LICENSE_URL: &str =
    "https://www.microsoft.com/licensing/docs/view/Product-Terms";

/// The licence agreement Bitnami publishes for the application stacks it
/// packages — `Blueprint$licenseUrl` of every Bitnami-packaged entry of
/// [`BLUEPRINTS`].
pub(super) const BITNAMI_LICENSE_URL: &str = "https://bitnami.com/agreement";

/// `Blueprint$versionCode` — "The version code" — of every entry of
/// [`BLUEPRINTS`]: Lightsail's own revision counter for the image it publishes
/// at that version, which starts at the first revision and is the value the
/// recorded entries carry.
pub const BLUEPRINT_VERSION_CODE: &str = "1";

/// `Blueprint$version` — "The version number of the operating system,
/// application, or stack (`2016.03.0`)" — of every entry of [`BLUEPRINTS`]: the
/// publication date of the catalog generation these images belong to, in the
/// `YYYY.MM.DD` form that example shows.
///
/// It is the CATALOG's version rather than the image's own upstream release: the
/// recorded answer reports ONE version across entries of different releases — a
/// Windows Server 2025 operating-system image and a SQL Server 2022 application
/// image carry the same one — so the member pins the generation of the catalog a
/// client read, not the year its image's software was published. Re-publishing
/// the images moves this date, and moves it for every row at once.
pub const BLUEPRINT_CATALOG_VERSION: &str = "2026.09.17";

/// `Blueprint$minPower` of an application image that installs the Windows SQL
/// Server database engine beside the operating system: it needs the 4 GB plan,
/// so its floor is that plan's [`BundleSpec::power`].
pub const WINDOWS_SQL_BLUEPRINT_MIN_POWER: i32 = 2_000;

/// `Blueprint$minPower` of a Windows image — "The minimum bundle power required
/// to run this blueprint".
///
/// A Windows image does not run on the smallest instance plan, so its floor is
/// the next plan's power: the `micro` plan's [`BundleSpec::power`], which is the
/// value the recorded Windows entries report. A Linux/Unix image this catalog
/// publishes runs on every plan, which `0` states — "`0` indicates that the
/// blueprint runs on all instance sizes".
pub const WINDOWS_BLUEPRINT_MIN_POWER: i32 = 500;

/// `Blueprint$minPower` of an image that runs on every plan this service
/// publishes. See [`WINDOWS_BLUEPRINT_MIN_POWER`].
pub const BLUEPRINT_MIN_POWER_ANY_SIZE: i32 = 0;

/// The instance blueprints this service publishes — the catalog `GetBlueprints`
/// reports and `CreateInstances` resolves `blueprintId` against.
///
/// SOURCE: the blueprint ids Lightsail's instance-image list publishes (the
/// Lightsail console's "Select a blueprint" catalog, which `GetBlueprints`
/// answers from): the operating-system families `windows_server`,
/// `amazon_linux`, `ubuntu`, `debian`, `almalinux`, `centos_stream`, `freebsd`
/// and `opensuse`, each at the releases Lightsail offers, plus the
/// Bitnami-packaged and hosting-panel application stacks. An entry whose
/// upstream release has reached end of life stays in the catalog but is marked
/// inactive, which is the state Lightsail keeps a retired image in so existing
/// instances still resolve it while a new instance cannot be launched from it.
///
/// The ORDER is the catalog's published one, which `GetBlueprints` walks: the
/// Windows images first, newest release first, then the Linux/Unix operating
/// systems and last the application stacks — the order the recorded answer
/// reports, whose first two entries are `windows_server_2025` and
/// `windows_server_2022`.
pub const BLUEPRINTS: &[BlueprintSpec] = &[
    // Windows Server operating systems, newest release first.
    BlueprintSpec {
        blueprint_id: "windows_server_2025",
        published_name: None,
        published_min_power: None,
        group: "windows_2025",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2022",
        published_name: None,
        published_min_power: None,
        group: "windows_2022",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2019",
        published_name: None,
        published_min_power: None,
        group: "windows_2019",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "windows_server_2016",
        published_name: None,
        published_min_power: None,
        group: "windows_2016",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    // The Windows SQL Server application images: an edition of the database
    // engine preinstalled on a Windows Server image, which the catalog publishes
    // after the bare Windows images and before the first Linux/Unix one.
    BlueprintSpec {
        blueprint_id: "windows_server_2022_sql_2022_express",
        published_name: Some("SQL Server 2022 Express"),
        published_min_power: Some(WINDOWS_SQL_BLUEPRINT_MIN_POWER),
        group: "windows_2022_sql_exp_2022",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: WINDOWS_SERVER_PRODUCT_URL,
        license_url: WINDOWS_SERVER_LICENSE_URL,
    },
    // Linux/Unix operating systems.
    BlueprintSpec {
        blueprint_id: "amazon_linux_2023",
        published_name: None,
        published_min_power: None,
        group: "amazon-linux",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://aws.amazon.com/linux/amazon-linux-2023/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "amazon_linux_2",
        published_name: None,
        published_min_power: None,
        group: "amazon-linux",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://aws.amazon.com/amazon-linux-2/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_24_04",
        published_name: None,
        published_min_power: None,
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_22_04",
        published_name: None,
        published_min_power: None,
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_20_04",
        published_name: None,
        published_min_power: None,
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "ubuntu_18_04",
        published_name: None,
        published_min_power: None,
        group: "ubuntu",
        blueprint_type: BLUEPRINT_TYPE_OS,
        // Ubuntu 18.04 LTS is past its upstream end of standard support.
        is_active: false,
        product_url: "https://ubuntu.com/server",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "debian_12",
        published_name: None,
        published_min_power: None,
        group: "debian",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.debian.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "debian_11",
        published_name: None,
        published_min_power: None,
        group: "debian",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.debian.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "debian_10",
        published_name: None,
        published_min_power: None,
        group: "debian",
        blueprint_type: BLUEPRINT_TYPE_OS,
        // Debian 10 is past its upstream end of life.
        is_active: false,
        product_url: "https://www.debian.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "almalinux_9",
        published_name: None,
        published_min_power: None,
        group: "almalinux",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://almalinux.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "centos_stream_9",
        published_name: None,
        published_min_power: None,
        group: "centos-stream",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.centos.org/centos-stream/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "freebsd_14",
        published_name: None,
        published_min_power: None,
        group: "freebsd",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.freebsd.org/",
        license_url: "",
    },
    BlueprintSpec {
        blueprint_id: "opensuse_15",
        published_name: None,
        published_min_power: None,
        group: "opensuse",
        blueprint_type: BLUEPRINT_TYPE_OS,
        is_active: true,
        product_url: "https://www.opensuse.org/",
        license_url: "",
    },
    // Application stacks.
    BlueprintSpec {
        blueprint_id: "wordpress",
        published_name: None,
        published_min_power: None,
        group: "wordpress",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/wordpress",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "wordpress_multisite",
        published_name: None,
        published_min_power: None,
        group: "wordpress-multisite",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/wordpress-multisite",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "lamp_8_bitnami",
        published_name: None,
        published_min_power: None,
        group: "lamp-bitnami",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/lamp",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "nodejs",
        published_name: None,
        published_min_power: None,
        group: "nodejs",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/nodejs",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "joomla",
        published_name: None,
        published_min_power: None,
        group: "joomla",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/joomla",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "drupal",
        published_name: None,
        published_min_power: None,
        group: "drupal",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/drupal",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "magento",
        published_name: None,
        published_min_power: None,
        group: "magento",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/magento",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "mean",
        published_name: None,
        published_min_power: None,
        group: "mean",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/mean",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "ghost_bitnami",
        published_name: None,
        published_min_power: None,
        group: "ghost-bitnami",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/ghost",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "redmine",
        published_name: None,
        published_min_power: None,
        group: "redmine",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/redmine",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "nginx",
        published_name: None,
        published_min_power: None,
        group: "nginx",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/nginx",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "django",
        published_name: None,
        published_min_power: None,
        group: "django",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/django",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "gitlab",
        published_name: None,
        published_min_power: None,
        group: "gitlab",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/gitlab",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "prestashop",
        published_name: None,
        published_min_power: None,
        group: "prestashop",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://bitnami.com/stack/prestashop",
        license_url: BITNAMI_LICENSE_URL,
    },
    BlueprintSpec {
        blueprint_id: "cpanel_whm_almalinux",
        published_name: None,
        published_min_power: None,
        group: "cpanel-whm-almalinux",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://cpanel.net/",
        license_url: "https://cpanel.net/legal/",
    },
    BlueprintSpec {
        blueprint_id: "plesk_ubuntu_18_0_35",
        published_name: None,
        published_min_power: None,
        group: "plesk-ubuntu",
        blueprint_type: BLUEPRINT_TYPE_APP,
        is_active: true,
        product_url: "https://www.plesk.com/",
        license_url: "https://www.plesk.com/legal/",
    },
];

/// The blueprint `blueprint_id` names — active or not, since an instance already
/// running a retired image still has to resolve it — or `None` when this service
/// publishes no such blueprint.
pub fn blueprint_spec(blueprint_id: &str) -> Option<BlueprintSpec> {
    BLUEPRINTS
        .iter()
        .find(|spec| spec.blueprint_id == blueprint_id)
        .copied()
}

/// Whether a new instance can be created from `blueprint_id`: the catalog holds
/// it AND the entry is still active.
pub fn blueprint_is_creatable(blueprint_id: &str) -> bool {
    blueprint_spec(blueprint_id).is_some_and(|spec| spec.is_active)
}

/// One value of the modelled `InstancePlatform` enum
/// (`smithy_query(query="shape",shape="InstancePlatform")`): a Linux/Unix image.
pub const PLATFORM_LINUX_UNIX: &str = "LINUX_UNIX";

/// The other value of the modelled `InstancePlatform` enum: a Windows
/// Server-based image.
pub const PLATFORM_WINDOWS: &str = "WINDOWS";

/// The Lightsail for Research virtual-computer blueprints this service publishes.
///
/// A SEPARATE catalog from [`BLUEPRINTS`]: `GetBlueprints$appCategory` is
/// documented as "Returns a list of blueprints that are specific to Lightsail for
/// Research. You must use this parameter to view Lightsail for Research
/// blueprints", so the two catalogs are disjoint and the request's category is
/// what selects between them (see [`blueprint_catalog`]).
///
/// This emulator publishes no Lightsail for Research blueprint, so the catalog is
/// EMPTY and a request naming a category reports no blueprint. An entry here
/// would have to name a real Research image; inventing one would make
/// `GetBlueprints` report a blueprint `CreateInstances` cannot create from.
pub const LFR_BLUEPRINTS: &[BlueprintSpec] = &[];

/// How many blueprints one `GetBlueprints` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more blueprints remain. It is above the
/// published catalog's size, so the whole catalog is answered in one page — as AWS
/// answers it — and the paging walk still holds for a catalog that outgrows it.
pub const BLUEPRINT_PAGE_SIZE: usize = 50;

impl BlueprintSpec {
    /// `Blueprint$name` — "The friendly name of the blueprint": the name the
    /// catalog publishes for the image ([`Self::published_name`]), which for an
    /// image the catalog names after its id's own words is those words
    /// title-cased ([`blueprint_name`]) — the derivation
    /// `Instance$blueprintName` reports, so the catalog's name for a blueprint is
    /// the name an instance created from it carries.
    pub fn name(&self) -> String {
        match self.published_name {
            Some(name) => name.to_string(),
            None => blueprint_name(self.blueprint_id),
        }
    }

    /// `Blueprint$platform` — "The operating system platform (either
    /// Linux/Unix-based or Windows Server-based) of the blueprint", one of the
    /// modelled `InstancePlatform` enum's two values. Windows is the platform of
    /// exactly the Windows blueprints ([`blueprint_is_windows`]), the same
    /// classification that decides which management port a new instance opens.
    pub fn platform(&self) -> &'static str {
        if blueprint_is_windows(self.blueprint_id) {
            PLATFORM_WINDOWS
        } else {
            PLATFORM_LINUX_UNIX
        }
    }

    /// `Blueprint$description` — "The description of the blueprint".
    ///
    /// DERIVED from the two facts the catalog holds about the entry: its friendly
    /// name and its `type`, which says whether the image is an operating system
    /// or an application stack preinstalled on one. A per-entry marketing
    /// paragraph is not something this catalog knows.
    pub fn description(&self) -> String {
        let name = self.name();
        if self.blueprint_type == BLUEPRINT_TYPE_APP {
            format!("{name} application stack, preinstalled on a Lightsail instance.")
        } else {
            format!("{name} operating system image for a Lightsail instance.")
        }
    }

    /// `Blueprint$minPower` — "The minimum bundle power required to run this
    /// blueprint".
    ///
    /// The power of the smallest instance plan the image runs on: the floor the
    /// catalog publishes for the image where it publishes one
    /// ([`Self::published_min_power`]), otherwise its platform's — a Windows
    /// image does not run on the smallest plan
    /// ([`WINDOWS_BLUEPRINT_MIN_POWER`]), and a Linux/Unix image this catalog
    /// publishes runs on every plan, which `0` states
    /// ([`BLUEPRINT_MIN_POWER_ANY_SIZE`]).
    pub fn min_power(&self) -> i32 {
        if let Some(min_power) = self.published_min_power {
            return min_power;
        }
        if self.platform() == PLATFORM_WINDOWS {
            WINDOWS_BLUEPRINT_MIN_POWER
        } else {
            BLUEPRINT_MIN_POWER_ANY_SIZE
        }
    }

    /// This entry as the modelled `Blueprint`.
    ///
    /// `name` is the catalog's own name for the image ([`Self::name`]), and
    /// `version` the catalog generation's ([`BLUEPRINT_CATALOG_VERSION`]), which
    /// every entry of one generation carries.
    pub fn view(&self) -> Blueprint {
        // `productUrl` and `licenseUrl` are optional, and an entry whose vendor
        // publishes neither carries neither: an empty link in the catalog is
        // reported as the ABSENT member rather than as an empty string.
        let link = |url: &'static str| (!url.is_empty()).then(|| url.to_string());
        Blueprint {
            blueprint_id: Some(self.blueprint_id.to_string()),
            name: Some(self.name()),
            group: Some(self.group.to_string()),
            r#type: Some(self.blueprint_type.to_string()),
            description: Some(self.description()),
            is_active: Some(self.is_active),
            min_power: Some(self.min_power()),
            version: Some(BLUEPRINT_CATALOG_VERSION.to_string()),
            version_code: Some(BLUEPRINT_VERSION_CODE.to_string()),
            product_url: link(self.product_url),
            license_url: link(self.license_url),
            platform: Some(self.platform().to_string()),
            // Only a Lightsail for Research blueprint carries a category, and it
            // is the category of the catalog the entry came from, so
            // [`blueprints_from`] stamps it rather than the entry.
            app_category: None,
        }
    }
}

/// The catalog a `GetBlueprints` request naming `app_category` reports from: the
/// Lightsail for Research blueprints when it names a category, the instance
/// blueprints when it names none.
///
/// From `GetBlueprints$appCategory`: "You must use this parameter to view
/// Lightsail for Research blueprints" — so the standard catalog answers a request
/// that names no category, and only a request that names one sees the Research
/// blueprints.
///
/// The VALUE is not re-checked here: `appCategory` targets the modelled
/// `AppCategory` enum, whose value set the centralized constraint walk enforces
/// before any handler runs (a category the model does not name is already a 400
/// `ValidationException`), so every category that reaches this function is one the
/// model admits.
pub fn blueprint_catalog(app_category: Option<&str>) -> &'static [BlueprintSpec] {
    match app_category {
        Some(_) => LFR_BLUEPRINTS,
        None => BLUEPRINTS,
    }
}

/// The blueprints a `GetBlueprints` call reports out of `specs`: every active
/// entry, plus the inactive (retired) ones when `include_inactive` is set, each
/// stamped with `app_category` — the category of the catalog it came from.
///
/// The order is `specs`' own, so two reads answer identically. Taking the catalog
/// as an argument is what lets the slice rule be exercised against a catalog
/// whose shape the published one does not have yet.
pub fn blueprints_from(
    specs: &[BlueprintSpec],
    include_inactive: bool,
    app_category: Option<&str>,
) -> Vec<Blueprint> {
    specs
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .map(|spec| Blueprint {
            app_category: app_category.map(str::to_string),
            ..spec.view()
        })
        .collect()
}

/// The blueprints a `GetBlueprints` call reports: [`blueprints_from`] over the
/// catalog [`blueprint_catalog`] selects.
pub fn blueprints(include_inactive: bool, app_category: Option<&str>) -> Vec<Blueprint> {
    blueprints_from(
        blueprint_catalog(app_category),
        include_inactive,
        app_category,
    )
}

/// One page of `reported`: at most `page_size` blueprints from the entry
/// `page_token` names onward, and the id of the first entry of the page after it
/// — present only while entries remain.
///
/// A page token NAMES a blueprint rather than carrying an index, so a catalog that
/// gained or lost an entry between two pages cannot make the walk skip or repeat
/// another one. `None` is the answer when the token names no blueprint of
/// `reported`: there is no page to report, which the caller turns into a refusal.
/// An absent token asks for the first page.
///
/// The walk itself is [`catalog_page`], shared with this service's other paged
/// catalogs; only the member a token names is this catalog's own.
pub fn blueprint_page(
    reported: &[Blueprint],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<Blueprint>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |blueprint| {
        blueprint.blueprint_id.as_deref().map(std::borrow::Cow::Borrowed)
    })
}

/// The blueprint ids `reported` holds, for a refusal that has to tell the caller
/// what a page token may name.
pub fn reported_blueprint_ids(reported: &[Blueprint]) -> String {
    reported
        .iter()
        .filter_map(|blueprint| blueprint.blueprint_id.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

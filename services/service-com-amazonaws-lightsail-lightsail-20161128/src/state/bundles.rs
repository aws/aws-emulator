// SPDX-License-Identifier: CC0-1.0

//! The published bundle catalogs — the hardware plans an instance is created on
//! and the storage plans a bucket is put on — and the rules a requested bundle
//! is resolved against.

use super::*;

/// One Lightsail instance plan size: "a set of specs describing your virtual
/// private server (or instance)" (the modelled `Bundle` documentation).
///
/// A bundle id is `<size>_<major>_<minor>`, optionally with a platform token
/// (`micro_win_2_0`), so the leading token names the size and the rest names the
/// bundle's version. ONE entry here therefore describes a size: the hardware and
/// the monthly transfer every plan of that size provides, plus the monthly price
/// of that size on each platform — the same hardware costs more on Windows
/// Server, which carries an operating-system licence.
///
/// This is a PUBLISHED catalog, not per-account state: every account sees the
/// same plans and no operation in this service creates, changes or retires one.
/// That is why it is a `const` table beside the blueprint and Region catalogs,
/// and why `GetBundles` reports it without reading or writing a stored resource.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BundleSpec {
    /// The leading token of every bundle id that maps here, and the plan's
    /// `Bundle$instanceType` — "The instance type (`micro`)".
    pub size: &'static str,
    /// `Bundle$cpuCount` — "The number of vCPUs included in the bundle".
    pub cpu_count: i32,
    /// `Bundle$ramSizeInGb` — "The amount of RAM in GB".
    pub ram_size_in_gb: f32,
    /// `Bundle$diskSizeInGb` — "The size of the SSD".
    pub disk_size_in_gb: i32,
    /// `InstanceNetworking$monthlyTransfer.gbPerMonthAllocated` and
    /// `Bundle$transferPerMonthInGb` — "The data transfer rate per month in GB".
    pub transfer_gb_per_month: i32,
    /// `Bundle$power` — "A numeric value that represents the power of the
    /// bundle", compared against a blueprint's `minPower` to decide whether the
    /// blueprint runs on the plan.
    pub power: i32,
    /// `Bundle$price` — "The price in US dollars of the bundle" — for the
    /// Linux/Unix plan of this size.
    pub price_usd_per_month: f32,
    /// `Bundle$price` for the Windows plan of this size, and `None` where the
    /// plan table publishes NO Windows plan of this size.
    ///
    /// A plan is published per platform: the plan table carries a Windows column
    /// only up to the largest size Lightsail offers Windows Server on, while the
    /// Linux/Unix column goes on past it. A size with no Windows price therefore
    /// has no Windows bundle at all ([`bundles_from`]) rather than a Windows
    /// bundle priced like its Linux/Unix twin, which would announce a plan the
    /// service does not sell.
    pub windows_price_usd_per_month: Option<f32>,
    /// `Bundle$isActive` — "A Boolean value indicating whether the bundle is
    /// active". A plan generation the service has retired stays in this table
    /// with `false` so an instance already running on it still resolves, while
    /// `GetBundles` reports it only when asked to include inactive bundles.
    pub is_active: bool,
    /// `Bundle$supportedAppCategories` — "Virtual computer blueprints that are
    /// supported by a Lightsail for Research bundle", of which the member's own
    /// documentation says "This parameter only applies to Lightsail for Research
    /// resources".
    ///
    /// The categories are the PLAN's, exactly as its platforms are, so they are
    /// carried by the entry and never taken from the request. A standard instance
    /// plan supports none, which is why it is empty on every entry of
    /// [`BUNDLE_SPECS`] and the reported member is then absent.
    pub app_categories: &'static [&'static str],
}

/// Every instance plan size this service serves, smallest first.
///
/// SOURCE: the Lightsail instance-plan table (the "Instances" plans of the
/// Lightsail pricing page, which `GetBundles` answers from), at the generation
/// [`BUNDLE_GENERATION`] publishes. Up to the 128 GB plan each step doubles RAM,
/// while the SSD size and the monthly data transfer follow the plan table; every
/// plan up to and including the 8 GB one carries two vCPUs, after which the vCPU
/// count doubles with the plan; the Linux/Unix monthly price is the plan table's,
/// and the Windows plan of the same size costs more because it carries that
/// platform's licence. The `power` values are the ones Lightsail publishes for
/// its plans: 298 for the smallest, 500 for `micro` (the value the modelled
/// `Blueprint$minPower` documentation's own example names) and then a further
/// 1000 per doubling step, 500 where a plan adds half a step of hardware.
///
/// The sizes ABOVE the 64 GB plan are Linux/Unix only: the plan table's Windows
/// column ends at that plan, so those entries carry no Windows price (see
/// [`BundleSpec::windows_price_usd_per_month`]). Above the 8 GB plan the hardware
/// follows the plan's vCPU count — 4 GB of RAM and 80 GB of SSD per vCPU — while
/// the monthly transfer grows by 1 TB per plan and the price is the table's own
/// figure for that plan rather than a step computed from the plan below it.
///
/// Lightsail has retired none of these sizes, so every entry is active.
pub const BUNDLE_SPECS: &[BundleSpec] = &[
    BundleSpec {
        size: "nano",
        cpu_count: 2,
        ram_size_in_gb: 0.5,
        disk_size_in_gb: 20,
        transfer_gb_per_month: 1_024,
        power: 298,
        price_usd_per_month: 5.0,
        windows_price_usd_per_month: Some(9.5),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "micro",
        cpu_count: 2,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_gb_per_month: 2_048,
        power: 500,
        price_usd_per_month: 7.0,
        windows_price_usd_per_month: Some(16.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "small",
        cpu_count: 2,
        ram_size_in_gb: 2.0,
        disk_size_in_gb: 60,
        transfer_gb_per_month: 3_072,
        power: 1_000,
        price_usd_per_month: 12.0,
        windows_price_usd_per_month: Some(26.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "medium",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 80,
        transfer_gb_per_month: 4_096,
        power: 2_000,
        price_usd_per_month: 24.0,
        windows_price_usd_per_month: Some(46.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "large",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 160,
        transfer_gb_per_month: 5_120,
        power: 3_000,
        price_usd_per_month: 44.0,
        windows_price_usd_per_month: Some(86.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "xlarge",
        cpu_count: 4,
        ram_size_in_gb: 16.0,
        disk_size_in_gb: 320,
        transfer_gb_per_month: 6_144,
        power: 4_000,
        price_usd_per_month: 84.0,
        windows_price_usd_per_month: Some(166.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "2xlarge",
        cpu_count: 8,
        ram_size_in_gb: 32.0,
        disk_size_in_gb: 640,
        transfer_gb_per_month: 7_168,
        power: 5_000,
        price_usd_per_month: 164.0,
        windows_price_usd_per_month: Some(326.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "4xlarge",
        cpu_count: 16,
        ram_size_in_gb: 64.0,
        disk_size_in_gb: 1_280,
        transfer_gb_per_month: 8_192,
        power: 6_000,
        price_usd_per_month: 384.0,
        windows_price_usd_per_month: Some(646.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "8xlarge",
        cpu_count: 32,
        ram_size_in_gb: 128.0,
        disk_size_in_gb: 2_560,
        transfer_gb_per_month: 9_216,
        power: 7_000,
        price_usd_per_month: 884.0,
        windows_price_usd_per_month: None,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "12xlarge",
        cpu_count: 48,
        ram_size_in_gb: 192.0,
        disk_size_in_gb: 3_840,
        transfer_gb_per_month: 10_240,
        power: 7_500,
        price_usd_per_month: 1_324.0,
        windows_price_usd_per_month: None,
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "16xlarge",
        cpu_count: 64,
        ram_size_in_gb: 256.0,
        disk_size_in_gb: 5_120,
        transfer_gb_per_month: 11_264,
        power: 8_000,
        price_usd_per_month: 1_764.0,
        windows_price_usd_per_month: None,
        is_active: true,
        app_categories: &[],
    },
];

/// The bundle `bundle_id` names, or `None` when its leading size token is not one
/// this service serves.
pub fn bundle_spec(bundle_id: &str) -> Option<BundleSpec> {
    let size = bundle_id.split('_').next()?;
    BUNDLE_SPECS
        .iter()
        .find(|spec| spec.size == size)
        .copied()
}

/// The bundle sizes [`bundle_spec`] accepts, for a refusal that has to tell the
/// caller what to send instead.
pub fn bundle_sizes() -> String {
    BUNDLE_SPECS
        .iter()
        .map(|spec| spec.size)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The generation token the bundle ids this service publishes carry — the
/// `<major>_<minor>` tail of a bundle id, as the modelled `Bundle$bundleId`
/// documentation's `micro_x_x` shows.
///
/// A plan size is served at one generation at a time, so the generation is a
/// property of the CATALOG rather than of a size. An instance created on another
/// generation of a size this service serves still resolves its hardware, because
/// [`bundle_spec`] resolves a bundle id by its leading size token.
pub const BUNDLE_GENERATION: &str = "3_0";

/// The token a Windows bundle id carries between its size and its generation
/// (`micro_win_2_0`), which is what distinguishes the Windows plan of a size from
/// its Linux/Unix plan.
pub const BUNDLE_WINDOWS_TOKEN: &str = "win";

/// The platforms every plan size is published for, in the order a page reports
/// them, from the modelled `InstancePlatform` enum: "You can only launch a
/// `WINDOWS` bundle on a blueprint that supports the `WINDOWS` platform.
/// `LINUX_UNIX` blueprints require a `LINUX_UNIX` bundle"
/// (`Bundle$supportedPlatforms`), so a size is a separate bundle on each.
pub const BUNDLE_PLATFORMS: &[&str] = &[PLATFORM_LINUX_UNIX, PLATFORM_WINDOWS];

/// `Bundle$publicIpv4AddressCount` of every plan this catalog publishes — "an
/// integer that indicates the public ipv4 address count included in the bundle,
/// the value is either 0 or 1".
///
/// Every published plan is a standard plan that includes one public IPv4 address;
/// 0 is the count of an IPv6-only plan, which this service publishes none of.
pub const BUNDLE_PUBLIC_IPV4_ADDRESS_COUNT: i32 = 1;

/// How many bundles one `GetBundles` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more bundles remain. It is above the
/// published catalog's size, so the whole catalog is answered in one page — as
/// AWS answers it — and the paging walk still holds for a catalog that outgrows
/// it.
pub const BUNDLE_PAGE_SIZE: usize = 50;

/// The id a plan of `size` carries on `platform` at `generation`: the size token,
/// the platform token of a Windows plan, then the generation
/// ([`BUNDLE_WINDOWS_TOKEN`], [`BUNDLE_GENERATION`]).
pub fn bundle_id(size: &str, platform: &str, generation: &str) -> String {
    if platform == PLATFORM_WINDOWS {
        format!("{size}_{BUNDLE_WINDOWS_TOKEN}_{generation}")
    } else {
        format!("{size}_{generation}")
    }
}

/// The friendly name of a plan of `size` — `Bundle$name`, "A friendly name for
/// the bundle (`Micro`)".
///
/// DERIVED from the size token, as that documented example shows (`micro` ->
/// `Micro`): the token with its first LETTER upper-cased, so a size token that
/// opens with a multiplier capitalises the letter after it (`2xlarge` ->
/// `2Xlarge`) — the form the recorded plan names carry.
pub fn bundle_size_name(size: &str) -> String {
    let Some(first_letter) = size.find(char::is_alphabetic) else {
        return size.to_string();
    };
    let (multiplier, word) = size.split_at(first_letter);
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => {
            multiplier.to_string() + &first.to_uppercase().collect::<String>() + chars.as_str()
        }
        None => multiplier.to_string(),
    }
}

impl BundleSpec {
    /// The monthly price of this plan on `platform`: the Windows price on
    /// Windows, the Linux/Unix one otherwise — and `None` where the plan table
    /// publishes no plan of this size on that platform, which is the one
    /// answer to "what does this size cost there" that cannot be invented.
    pub fn price(&self, platform: &str) -> Option<f32> {
        if platform == PLATFORM_WINDOWS {
            self.windows_price_usd_per_month
        } else {
            Some(self.price_usd_per_month)
        }
    }

    /// Whether the plan table publishes this size on `platform`.
    pub fn is_published_on(&self, platform: &str) -> bool {
        self.price(platform).is_some()
    }

    /// This plan on `platform` as the modelled `Bundle`, and `None` where the
    /// table publishes no plan of this size on that platform.
    pub fn view(&self, platform: &str) -> Option<Bundle> {
        let price = self.price(platform)?;
        Some(Bundle {
            bundle_id: Some(bundle_id(self.size, platform, BUNDLE_GENERATION)),
            instance_type: Some(self.size.to_string()),
            name: Some(bundle_size_name(self.size)),
            price: Some(price),
            cpu_count: Some(self.cpu_count),
            ram_size_in_gb: Some(self.ram_size_in_gb),
            disk_size_in_gb: Some(self.disk_size_in_gb),
            transfer_per_month_in_gb: Some(self.transfer_gb_per_month),
            power: Some(self.power),
            is_active: Some(self.is_active),
            supported_platforms: Some(vec![platform.to_string()]),
            // `supportedAppCategories` is the plan's OWN data, like its platform:
            // a standard plan publishes none and reports the member absent.
            supported_app_categories: (!self.app_categories.is_empty()).then(|| {
                self.app_categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect()
            }),
            public_ipv4_address_count: Some(BUNDLE_PUBLIC_IPV4_ADDRESS_COUNT),
        })
    }
}

/// The Lightsail for Research instance plans this service publishes.
///
/// A SEPARATE catalog from [`BUNDLE_SPECS`], exactly as [`LFR_BLUEPRINTS`] is
/// separate from [`BLUEPRINTS`]: `GetBundles$appCategory` is documented as
/// "Returns a list of bundles that are specific to Lightsail for Research. You
/// must use this parameter to view Lightsail for Research bundles", so the two
/// catalogs are disjoint and the request's category selects between them (see
/// [`bundle_catalog`]).
///
/// This emulator publishes no Lightsail for Research plan, so the catalog is
/// EMPTY and a request naming a category reports no bundle. An entry here would
/// have to name a real Research plan; inventing one would make `GetBundles`
/// report a plan no Research blueprint runs on.
pub const LFR_BUNDLE_SPECS: &[BundleSpec] = &[];

/// The catalog a `GetBundles` request naming `app_category` reports from: the
/// Lightsail for Research plans when it names a category, the instance plans when
/// it names none.
///
/// The VALUE is not re-checked here: `appCategory` targets the modelled
/// `AppCategory` enum, whose value set the centralized constraint walk enforces
/// before any handler runs, so every category that reaches this function is one
/// the model admits.
pub fn bundle_catalog(app_category: Option<&str>) -> &'static [BundleSpec] {
    match app_category {
        Some(_) => LFR_BUNDLE_SPECS,
        None => BUNDLE_SPECS,
    }
}

/// The bundles a `GetBundles` call reports out of `specs`: every active plan of
/// every size on each platform of [`BUNDLE_PLATFORMS`], plus the inactive
/// (retired) ones when `include_inactive` is set.
///
/// Every member of a reported bundle is the entry's own — the request selects the
/// catalog and the slice, and nothing it carries is reported back as plan data.
///
/// The order is by PLATFORM first — every plan of a platform in
/// [`BUNDLE_PLATFORMS`]' order, each platform's plans in `specs`' own order — so
/// the Linux/Unix plans of every size precede the Windows ones, which is the
/// order the catalog is published in. Two reads answer identically. Taking the
/// catalog as an argument is what lets the slice rule be exercised against a
/// catalog that holds a retired plan, which the published one does not.
///
/// A size is reported on a platform only where the plan table publishes it there
/// ([`BundleSpec::price`]), so the Linux/Unix block carries every published size
/// and the Windows block only the sizes Lightsail offers Windows Server on.
pub fn bundles_from(specs: &[BundleSpec], include_inactive: bool) -> Vec<Bundle> {
    BUNDLE_PLATFORMS
        .iter()
        .flat_map(|platform| {
            specs
                .iter()
                .filter(|spec| include_inactive || spec.is_active)
                .filter_map(|spec| spec.view(platform))
        })
        .collect()
}

/// The bundles a `GetBundles` call reports: [`bundles_from`] over the catalog
/// [`bundle_catalog`] selects.
pub fn bundles(include_inactive: bool, app_category: Option<&str>) -> Vec<Bundle> {
    bundles_from(bundle_catalog(app_category), include_inactive)
}

/// One page of `reported`, by the same token walk every published catalog of this
/// service uses ([`catalog_page`]): a `GetBundles` page token is the bundle id a
/// previous page reported as `nextPageToken`.
pub fn bundle_page(
    reported: &[Bundle],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<Bundle>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |bundle| {
        bundle.bundle_id.as_deref().map(std::borrow::Cow::Borrowed)
    })
}

/// The bundle ids `reported` holds, for a refusal that has to tell the caller
/// what a page token may name.
pub fn reported_bundle_ids(reported: &[Bundle]) -> String {
    reported
        .iter()
        .filter_map(|bundle| bundle.bundle_id.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Bucket bundles
// ---------------------------------------------------------------------------

/// One Lightsail **bucket** bundle: the monthly cost, storage quota and transfer
/// quota a bucket gets from that plan.
///
/// This is the service's published price list for object storage, so it is the
/// same for every caller and no call changes it — `GetBucketBundles` reports it,
/// and `CreateBucket`/`UpdateBucketBundle` name an entry of it by `bundle_id`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketBundleSpec {
    /// The id a caller sends for this plan. A bucket bundle id is
    /// `<size>_<major>_<minor>`, the same `<size>_<version>` form an instance
    /// bundle id carries.
    pub bundle_id: &'static str,
    /// `BucketBundle$storagePerMonthInGb` — "The storage size of the bundle, in
    /// GB."
    pub storage_per_month_in_gb: i32,
    /// `BucketBundle$transferPerMonthInGb` — "The monthly network transfer quota
    /// of the bundle", in GB.
    pub transfer_per_month_in_gb: i32,
    /// `BucketBundle$price` — "The monthly price of the bundle, in US dollars."
    pub price_usd_per_month: f32,
    /// `BucketBundle$isActive` — "Indicates whether the bundle is active. Use for
    /// a new or existing bucket."
    pub is_active: bool,
}

/// Every bucket bundle this service publishes, cheapest first: the three
/// Lightsail object-storage plans from the Lightsail pricing page's object
/// storage table — 5 GB of storage with 25 GB of data transfer for $1 a month,
/// 100 GB with 250 GB for $3, and 250 GB with 500 GB for $5 — each the first
/// (`_1_0`) generation of its size.
///
/// Lightsail has retired no bucket plan, so every published plan is active. A
/// retired generation belongs in this table with `is_active: false`: that is
/// what makes it reportable only when `GetBucketBundles` is asked to include
/// inactive bundles, while a bucket already running on it still resolves.
pub const BUCKET_BUNDLE_SPECS: &[BucketBundleSpec] = &[
    BucketBundleSpec {
        bundle_id: "small_1_0",
        storage_per_month_in_gb: 5,
        transfer_per_month_in_gb: 25,
        price_usd_per_month: 1.0,
        is_active: true,
    },
    BucketBundleSpec {
        bundle_id: "medium_1_0",
        storage_per_month_in_gb: 100,
        transfer_per_month_in_gb: 250,
        price_usd_per_month: 3.0,
        is_active: true,
    },
    BucketBundleSpec {
        bundle_id: "large_1_0",
        storage_per_month_in_gb: 250,
        transfer_per_month_in_gb: 500,
        price_usd_per_month: 5.0,
        is_active: true,
    },
];

/// The label Lightsail gives an object-storage plan, with `storage` the plan's
/// storage quota in GB.
///
/// SOURCE: the `BucketBundle$name` values Lightsail reports — `Object Storage
/// 5GB` for the 5 GB plan, `Object Storage 100GB` for the 100 GB one and `Object
/// Storage 250GB` for the 250 GB one. The label names the product and the plan's
/// storage size, so it is DERIVED from the size rather than carried per entry: a
/// plan added to [`BUCKET_BUNDLE_SPECS`] is labelled the same way.
pub fn bucket_bundle_name(storage_per_month_in_gb: i32) -> String {
    format!("Object Storage {storage_per_month_in_gb}GB")
}

impl BucketBundleSpec {
    /// This plan as the modelled `BucketBundle`.
    ///
    /// `name` is "The name of the bundle" — the label Lightsail gives an object
    /// storage plan — so it is DERIVED from [`Self::storage_per_month_in_gb`] by
    /// [`bucket_bundle_name`] rather than carried a second time.
    pub fn view(&self) -> BucketBundle {
        BucketBundle {
            bundle_id: Some(self.bundle_id.to_string()),
            name: Some(bucket_bundle_name(self.storage_per_month_in_gb)),
            price: Some(self.price_usd_per_month),
            storage_per_month_in_gb: Some(self.storage_per_month_in_gb),
            transfer_per_month_in_gb: Some(self.transfer_per_month_in_gb),
            is_active: Some(self.is_active),
        }
    }
}

/// The bundles a `GetBucketBundles` call reports out of `specs`: every active
/// plan, plus the inactive (retired) ones when `include_inactive` is set.
///
/// The order is `specs`' own, so two reads answer identically. Taking the catalog
/// as an argument is what lets the slice rule be exercised against a catalog that
/// holds a retired plan, which the published one does not yet.
pub fn bucket_bundles_from(specs: &[BucketBundleSpec], include_inactive: bool) -> Vec<BucketBundle> {
    specs
        .iter()
        .filter(|spec| include_inactive || spec.is_active)
        .map(BucketBundleSpec::view)
        .collect()
}

/// The bucket bundles a `GetBucketBundles` call reports: [`bucket_bundles_from`]
/// over this service's published catalog, [`BUCKET_BUNDLE_SPECS`].
pub fn bucket_bundles(include_inactive: bool) -> Vec<BucketBundle> {
    bucket_bundles_from(BUCKET_BUNDLE_SPECS, include_inactive)
}

/// The bucket bundle `bundle_id` names — active or not, since a bucket already
/// running on a retired plan still has to resolve — or `None` when this service
/// publishes no such plan.
pub fn bucket_bundle_spec(bundle_id: &str) -> Option<BucketBundleSpec> {
    BUCKET_BUNDLE_SPECS
        .iter()
        .find(|spec| spec.bundle_id == bundle_id)
        .copied()
}

/// The plan out of `specs` that a bucket may be PUT ON — created on, or moved
/// onto — or `None` when `bundle_id` names none.
///
/// A plan qualifies when the catalog holds it and it is still active:
/// `BucketBundle$isActive` is what says a bundle can be used — "Indicates
/// whether the bundle is active. Use for a new or existing bucket." — so a
/// retired plan is reportable (and still resolves for a bucket already running
/// on it, see [`bucket_bundle_spec`]) without being one a request can name.
///
/// Taking the catalog as an argument is what lets this rule be exercised against
/// a catalog that holds a retired plan, which the published one does not yet.
pub fn applicable_bucket_bundle_in(
    specs: &[BucketBundleSpec],
    bundle_id: &str,
) -> Option<BucketBundleSpec> {
    specs
        .iter()
        .find(|spec| spec.bundle_id == bundle_id && spec.is_active)
        .copied()
}

/// The plan a bucket may be put on, out of this service's published catalog:
/// [`applicable_bucket_bundle_in`] over [`BUCKET_BUNDLE_SPECS`]. The one owner of
/// that rule, which `CreateBucket` and `UpdateBucketBundle` both ask.
pub fn applicable_bucket_bundle(bundle_id: &str) -> Option<BucketBundleSpec> {
    applicable_bucket_bundle_in(BUCKET_BUNDLE_SPECS, bundle_id)
}

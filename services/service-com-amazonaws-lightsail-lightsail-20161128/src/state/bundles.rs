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
    /// `Bundle$diskSizeInGb` — "The size of the SSD" — of the Linux/Unix plan of
    /// this size.
    ///
    /// PUBLISHED per plan, never computed from the plan's hardware: the plan
    /// table's SSD column grows with the plan up to the size at which it stops,
    /// and every larger plan carries that same published figure. A size derived
    /// from the vCPU count or the RAM would keep growing past it and announce
    /// storage the plan does not include.
    pub disk_size_in_gb: i32,
    /// `InstanceNetworking$monthlyTransfer.gbPerMonthAllocated` and
    /// `Bundle$transferPerMonthInGb` — "The data transfer rate per month in GB".
    ///
    /// PUBLISHED per plan, like [`BundleSpec::disk_size_in_gb`]: the plan table's
    /// transfer column stops growing at the plan where the allowance caps, and
    /// the plans above it carry that same figure.
    pub transfer_gb_per_month: i32,
    /// `Bundle$power` — "A numeric value that represents the power of the
    /// bundle", compared against a blueprint's `minPower` to decide whether the
    /// blueprint runs on the plan — of the Linux/Unix plan of this size.
    pub power: i32,
    /// `Bundle$price` — "The price in US dollars of the bundle" — for the
    /// Linux/Unix plan of this size.
    pub price_usd_per_month: f32,
    /// The plan table's WINDOWS column for this size, and `None` where the table
    /// publishes NO Windows plan of this size.
    ///
    /// A plan is published per platform: a size the plan table prices in its
    /// Windows column carries that column's own price, SSD size and power, and a
    /// size it prices in no Windows column has no Windows bundle at all
    /// ([`bundles_from`]) rather than a Windows bundle priced like its
    /// Linux/Unix twin, which would announce a plan the service does not sell.
    pub windows: Option<WindowsPlan>,
    /// The plan table's IPv6-ONLY column for this size — the monthly price of
    /// the Linux/Unix plan of this size published WITHOUT a public IPv4 address
    /// — and `None` where the table publishes no IPv6-only plan of this size.
    ///
    /// An IPv6-only plan is a published plan in its own right, not a reported
    /// variation of its dual-stack twin: `SetIpAddressType$acceptBundleUpdate`
    /// documents that "an instance bundle will change when switching from
    /// dual-stack or ipv4, to ipv6", so the two are different bundles with
    /// different ids, different prices and a different
    /// `Bundle$publicIpv4AddressCount`. The hardware, monthly transfer and power
    /// are the SIZE's and are shared with its dual-stack Linux/Unix plan; the
    /// price is this column's own figure, which is why it is published per entry
    /// rather than computed from the dual-stack price.
    pub ipv6_only_price_usd_per_month: Option<f32>,
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

/// The plan table's Windows column for one size: what the table publishes for
/// the Windows plan of that size.
///
/// A plan is published PER PLATFORM, so the Windows plan of a size is a row of
/// the table in its own right rather than its Linux/Unix plan re-priced: the
/// table publishes its monthly price, its SSD size and its power. The vCPU
/// count, the RAM and the included monthly transfer are the SIZE's and are
/// shared with the Linux/Unix plan of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowsPlan {
    /// `Bundle$price` of the Windows plan of this size: the price in US dollars
    /// the plan table's WINDOWS column publishes for the size, which is more
    /// than its Linux/Unix row's because the plan carries an operating-system
    /// licence — and is that column's OWN figure rather than the Linux/Unix
    /// price plus a markup, since the two columns are priced independently (the
    /// 4 GB Windows plan and the 8 GB Linux/Unix plan are published at the same
    /// monthly price).
    pub price_usd_per_month: f32,
    /// `Bundle$diskSizeInGb` of the Windows plan of this size — the plan table's
    /// own SSD figure for the Windows column, which is not always its
    /// Linux/Unix twin's (the smallest Windows plan is published with a larger
    /// SSD than the smallest Linux/Unix one).
    pub disk_size_in_gb: i32,
    /// `Bundle$power` of the Windows plan of this size, as the table publishes
    /// it for the Windows column.
    pub power: i32,
    /// The monthly price of the IPv6-ONLY Windows plan of this size: the
    /// Windows plan published without a public IPv4 address, as the plan table's
    /// IPv6-only Windows column prices it.
    pub ipv6_only_price_usd_per_month: f32,
}

/// Every instance plan size this service serves, smallest first.
///
/// SOURCE: the Lightsail instance-plan table (the "Instances" plans of the
/// Lightsail pricing page, which `GetBundles` answers from), at the generation
/// [`BUNDLE_GENERATION`] publishes. Every column of a row is the table's
/// published figure for that plan, never one computed from another column: up to
/// the 128 GB plan each step doubles RAM; every plan up to and including the 8 GB
/// one carries two vCPUs, after which the vCPU count doubles with the plan; the
/// SSD column grows with the plan up to the 1,280 GB figure the 64 GB plan
/// carries and every larger plan carries that same figure; the included monthly
/// transfer grows by 1 TB per plan up to the 10 TB allowance the 192 GB plan
/// carries, which the plans above it also carry; and the monthly price is the
/// table's own figure for the plan. The `power` values are the ones Lightsail
/// publishes for its plans: 298 for the smallest, 500 for `micro` (the value the
/// modelled `Blueprint$minPower` documentation's own example names) and then a
/// further 1000 per doubling step, 500 where a plan adds half a step of hardware.
///
/// Each size is published in every column of the plan table — the Linux/Unix
/// column, the Windows column ([`WindowsPlan`]) and the IPv6-only column of each
/// platform ([`BundleSpec::ipv6_only_price_usd_per_month`],
/// [`WindowsPlan::ipv6_only_price_usd_per_month`]) — so each size is reported as
/// one bundle per published column ([`BUNDLE_COLUMNS`]), the Windows plans
/// included on the largest sizes the table carries.
///
/// The price columns are PUBLISHED independently of each other, never one column
/// plus a per-size markup or discount:
///
/// * the Windows column's monthly prices are the figures the table lists for the
///   Windows plans ($9.50, $14, $22, $44, $74, $124, $244, $574, $1,254, $1,884
///   and $2,504 from the smallest plan up) — the 4 GB Windows plan and the 8 GB
///   Linux/Unix plan are published at the same monthly price, and the step from
///   one Windows plan to the next is not the step between their Linux/Unix rows,
///   neither of which any per-size markup produces;
/// * the IPv6-only columns' monthly prices are the figures the table lists for
///   the plans published without a public IPv4 address — $3.50, $5, $10, $20,
///   $40, $80, $160, $380, $880, $1,320 and $1,760 on Linux/Unix, and $8, $12,
///   $20, $40, $70, $120, $240, $570, $1,250, $1,880 and $2,500 on Windows — and
///   the gap to the dual-stack row of the same platform is $1.50 on the smallest
///   plan, $2 on the next two and $4 from the 4 GB plan up, so no one address
///   charge subtracted from a dual-stack column produces these.
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
        // The Windows column publishes the smallest plan with a 30 GB SSD and a
        // power of its own, where its Linux/Unix twin carries 20 GB and 298.
        windows: Some(WindowsPlan {
            price_usd_per_month: 9.5,
            disk_size_in_gb: 30,
            power: 300,
            ipv6_only_price_usd_per_month: 8.0,
        }),
        ipv6_only_price_usd_per_month: Some(3.5),
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
        windows: Some(WindowsPlan {
            price_usd_per_month: 14.0,
            disk_size_in_gb: 40,
            power: 500,
            ipv6_only_price_usd_per_month: 12.0,
        }),
        ipv6_only_price_usd_per_month: Some(5.0),
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
        windows: Some(WindowsPlan {
            price_usd_per_month: 22.0,
            disk_size_in_gb: 60,
            power: 1_000,
            ipv6_only_price_usd_per_month: 20.0,
        }),
        ipv6_only_price_usd_per_month: Some(10.0),
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
        windows: Some(WindowsPlan {
            price_usd_per_month: 44.0,
            disk_size_in_gb: 80,
            power: 2_000,
            ipv6_only_price_usd_per_month: 40.0,
        }),
        ipv6_only_price_usd_per_month: Some(20.0),
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
        windows: Some(WindowsPlan {
            price_usd_per_month: 74.0,
            disk_size_in_gb: 160,
            power: 3_000,
            ipv6_only_price_usd_per_month: 70.0,
        }),
        ipv6_only_price_usd_per_month: Some(40.0),
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
        windows: Some(WindowsPlan {
            price_usd_per_month: 124.0,
            disk_size_in_gb: 320,
            power: 4_000,
            ipv6_only_price_usd_per_month: 120.0,
        }),
        ipv6_only_price_usd_per_month: Some(80.0),
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
        windows: Some(WindowsPlan {
            price_usd_per_month: 244.0,
            disk_size_in_gb: 640,
            power: 5_000,
            ipv6_only_price_usd_per_month: 240.0,
        }),
        ipv6_only_price_usd_per_month: Some(160.0),
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
        windows: Some(WindowsPlan {
            price_usd_per_month: 574.0,
            disk_size_in_gb: 1_280,
            power: 6_000,
            ipv6_only_price_usd_per_month: 570.0,
        }),
        ipv6_only_price_usd_per_month: Some(380.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "8xlarge",
        cpu_count: 32,
        ram_size_in_gb: 128.0,
        // The SSD column stops at the 64 GB plan's figure: this plan and the two
        // above it carry the same published 1,280 GB.
        disk_size_in_gb: 1_280,
        transfer_gb_per_month: 9_216,
        power: 7_000,
        price_usd_per_month: 884.0,
        // Every column publishes this plan at its own monthly price, with the
        // SSD size and power this size's rows carry.
        windows: Some(WindowsPlan {
            price_usd_per_month: 1_254.0,
            disk_size_in_gb: 1_280,
            power: 7_000,
            ipv6_only_price_usd_per_month: 1_250.0,
        }),
        ipv6_only_price_usd_per_month: Some(880.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "12xlarge",
        cpu_count: 48,
        ram_size_in_gb: 192.0,
        disk_size_in_gb: 1_280,
        transfer_gb_per_month: 10_240,
        power: 7_500,
        price_usd_per_month: 1_324.0,
        windows: Some(WindowsPlan {
            price_usd_per_month: 1_884.0,
            disk_size_in_gb: 1_280,
            power: 7_500,
            ipv6_only_price_usd_per_month: 1_880.0,
        }),
        ipv6_only_price_usd_per_month: Some(1_320.0),
        is_active: true,
        app_categories: &[],
    },
    BundleSpec {
        size: "16xlarge",
        cpu_count: 64,
        ram_size_in_gb: 256.0,
        disk_size_in_gb: 1_280,
        // The transfer column stops at the 192 GB plan's 10 TB allowance, which
        // this plan carries too.
        transfer_gb_per_month: 10_240,
        power: 8_000,
        price_usd_per_month: 1_764.0,
        windows: Some(WindowsPlan {
            price_usd_per_month: 2_504.0,
            disk_size_in_gb: 1_280,
            power: 8_000,
            ipv6_only_price_usd_per_month: 2_500.0,
        }),
        ipv6_only_price_usd_per_month: Some(1_760.0),
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

/// The token an IPv6-only bundle id carries between its size and its generation
/// (`micro_ipv6_3_0`), which is what distinguishes the IPv6-only plan of a size
/// from the dual-stack plan of the same size and platform.
pub const BUNDLE_IPV6_ONLY_TOKEN: &str = "ipv6";

/// The token the IPv6-only Windows bundle id of a size carries between its size
/// and its generation (`micro_win_ipv6_3_0`): the platform token of a Windows
/// plan and the addressing token of an IPv6-only one, in that order.
pub const BUNDLE_WINDOWS_IPV6_ONLY_TOKEN: &str = "win_ipv6";

/// One published column of the instance-plan table: a platform and the
/// addressing a plan of that platform is published with, which together decide a
/// row's id token, its `supportedPlatforms`, its `publicIpv4AddressCount` and
/// which price the row carries.
///
/// A size is a separate bundle in each column it is published in. The platform
/// is one bundle's worth on its own — "You can only launch a `WINDOWS` bundle on
/// a blueprint that supports the `WINDOWS` platform. `LINUX_UNIX` blueprints
/// require a `LINUX_UNIX` bundle" (`Bundle$supportedPlatforms`) — and so is the
/// addressing: `SetIpAddressType$acceptBundleUpdate` documents that "an instance
/// bundle will change when switching from dual-stack or ipv4, to ipv6", so the
/// IPv6-only plan of a size is its own bundle rather than the dual-stack bundle
/// reported with a different address count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleColumn {
    /// The plan table's Linux/Unix column: the plan published with a public IPv4
    /// address.
    LinuxUnix,
    /// The plan table's Windows column ([`WindowsPlan`]): the plan published
    /// with a public IPv4 address and a Windows Server licence.
    Windows,
    /// The plan table's IPv6-only column
    /// ([`BundleSpec::ipv6_only_price_usd_per_month`]): the Linux/Unix plan of
    /// the size published without a public IPv4 address.
    LinuxUnixIpv6Only,
    /// The plan table's IPv6-only Windows column: the Windows plan of the size
    /// published without a public IPv4 address.
    WindowsIpv6Only,
}

impl BundleColumn {
    /// `Bundle$supportedPlatforms`' single value for every row of this column.
    pub fn platform(self) -> &'static str {
        match self {
            Self::Windows | Self::WindowsIpv6Only => PLATFORM_WINDOWS,
            Self::LinuxUnix | Self::LinuxUnixIpv6Only => PLATFORM_LINUX_UNIX,
        }
    }

    /// The token a row of this column carries between its size and its
    /// generation, and `None` for the column whose ids carry none
    /// ([`BUNDLE_WINDOWS_TOKEN`], [`BUNDLE_IPV6_ONLY_TOKEN`]).
    pub fn id_token(self) -> Option<&'static str> {
        match self {
            Self::LinuxUnix => None,
            Self::Windows => Some(BUNDLE_WINDOWS_TOKEN),
            Self::LinuxUnixIpv6Only => Some(BUNDLE_IPV6_ONLY_TOKEN),
            Self::WindowsIpv6Only => Some(BUNDLE_WINDOWS_IPV6_ONLY_TOKEN),
        }
    }

    /// `Bundle$publicIpv4AddressCount` of every row of this column — "an integer
    /// that indicates the public ipv4 address count included in the bundle, the
    /// value is either 0 or 1": the one address a dual-stack plan includes, and
    /// none for an IPv6-only plan, which is what makes it IPv6-only.
    pub fn public_ipv4_address_count(self) -> i32 {
        match self {
            Self::LinuxUnix | Self::Windows => 1,
            Self::LinuxUnixIpv6Only | Self::WindowsIpv6Only => 0,
        }
    }
}

/// The columns of the plan table this service publishes, in the order a page
/// reports them: every plan of a column, each column's plans in the catalog's own
/// size order — the dual-stack columns of both platforms first, then the
/// IPv6-only column of each.
pub const BUNDLE_COLUMNS: &[BundleColumn] = &[
    BundleColumn::LinuxUnix,
    BundleColumn::Windows,
    BundleColumn::LinuxUnixIpv6Only,
    BundleColumn::WindowsIpv6Only,
];

/// The column the plan of a size on `platform` is published in when the plan
/// includes a public IPv4 address — what a caller holding only a platform (a
/// stored instance's bundle id, resolved by [`bundle_platform`]) reads.
fn dual_stack_column(platform: &str) -> BundleColumn {
    if platform == PLATFORM_WINDOWS {
        BundleColumn::Windows
    } else {
        BundleColumn::LinuxUnix
    }
}

/// How many bundles one `GetBundles` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more bundles remain. It is above the
/// published catalog's size, so the whole catalog is answered in one page — as
/// AWS answers it — and the paging walk still holds for a catalog that outgrows
/// it.
pub const BUNDLE_PAGE_SIZE: usize = 50;

/// The id a plan of `size` carries at `generation` in a column whose id token is
/// `token`: the size token, the column's token where it has one, then the
/// generation.
fn tokened_bundle_id(size: &str, token: Option<&str>, generation: &str) -> String {
    match token {
        Some(token) => format!("{size}_{token}_{generation}"),
        None => format!("{size}_{generation}"),
    }
}

/// The id the dual-stack plan of `size` carries on `platform` at `generation`:
/// the size token, the platform token of a Windows plan, then the generation
/// ([`BUNDLE_WINDOWS_TOKEN`], [`BUNDLE_GENERATION`]).
pub fn bundle_id(size: &str, platform: &str, generation: &str) -> String {
    tokened_bundle_id(size, dual_stack_column(platform).id_token(), generation)
}

/// The id the plan of `size` carries in `column` at `generation`.
pub fn column_bundle_id(size: &str, column: BundleColumn, generation: &str) -> String {
    tokened_bundle_id(size, column.id_token(), generation)
}

/// The platform `bundle_id` names: Windows for exactly the ids carrying the
/// platform token between their size and their generation
/// ([`BUNDLE_WINDOWS_TOKEN`]), Linux/Unix for every other id — the inverse of
/// [`bundle_id`], reading the token at the position that function writes it to.
///
/// A plan's SSD size, power and price are the ones its platform's column of the
/// plan table publishes ([`BundleSpec::disk_size_in_gb_on`]), so whatever
/// resolves a bundle id to its plan asks this which column to read.
pub fn bundle_platform(bundle_id: &str) -> &'static str {
    let mut tokens = bundle_id.split('_');
    // The leading token is the size ([`bundle_spec`] resolves the plan by it);
    // the platform token, where the id carries one, is the next.
    let _size = tokens.next();
    if tokens.next() == Some(BUNDLE_WINDOWS_TOKEN) {
        PLATFORM_WINDOWS
    } else {
        PLATFORM_LINUX_UNIX
    }
}

/// The column the plan `bundle_id` names is published in: the inverse of
/// [`column_bundle_id`], read from the tokens that function writes between a
/// size and a generation.
///
/// What a caller holding a stored instance's bundle id reads to find the SAME
/// plan at another generation — the equivalence the modelled
/// `InstanceAttributeUpdates$bundleId` documents when it takes `auto` ("Updates
/// an instance that runs on a previous-generation bundle to the
/// newer-generation equivalent of that bundle").
pub fn bundle_column(bundle_id: &str) -> BundleColumn {
    let platform = bundle_platform(bundle_id);
    let ipv6_only = bundle_id.split('_').any(|token| token == BUNDLE_IPV6_ONLY_TOKEN);
    match (platform == PLATFORM_WINDOWS, ipv6_only) {
        (true, true) => BundleColumn::WindowsIpv6Only,
        (true, false) => BundleColumn::Windows,
        (false, true) => BundleColumn::LinuxUnixIpv6Only,
        (false, false) => BundleColumn::LinuxUnix,
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
    /// The monthly price of this plan in `column`: that column's own published
    /// figure, and `None` where the plan table publishes no plan of this size in
    /// it — the one answer to "what does this size cost there" that cannot be
    /// invented.
    pub fn price_in(&self, column: BundleColumn) -> Option<f32> {
        match column {
            BundleColumn::LinuxUnix => Some(self.price_usd_per_month),
            BundleColumn::Windows => self.windows.map(|windows| windows.price_usd_per_month),
            BundleColumn::LinuxUnixIpv6Only => self.ipv6_only_price_usd_per_month,
            BundleColumn::WindowsIpv6Only => self
                .windows
                .map(|windows| windows.ipv6_only_price_usd_per_month),
        }
    }

    /// `Bundle$diskSizeInGb` of this plan in `column`: the SSD size the plan
    /// table publishes in it — the Windows column's where a Windows plan of this
    /// size is published, this size's own otherwise, since an IPv6-only plan
    /// carries the storage of the Linux/Unix plan it drops the address from.
    ///
    /// A size the table publishes NO Windows plan of has only one SSD figure, so
    /// that is the one answered: unlike [`BundleSpec::price_in`], which answers
    /// `None` because a price the table does not publish cannot be invented, the
    /// hardware of a plan that is not sold in a column is still this size's own.
    /// Whether the plan exists there at all is
    /// [`BundleSpec::is_published_in`]'s question, which `GetBundles` asks before
    /// reporting a row.
    pub fn disk_size_in_gb_in(&self, column: BundleColumn) -> i32 {
        match self.windows {
            Some(windows) if column.platform() == PLATFORM_WINDOWS => windows.disk_size_in_gb,
            _ => self.disk_size_in_gb,
        }
    }

    /// `Bundle$power` of this plan in `column`, from that column of the plan
    /// table. See [`BundleSpec::disk_size_in_gb_in`], including what a size with
    /// no Windows column answers.
    pub fn power_in(&self, column: BundleColumn) -> i32 {
        match self.windows {
            Some(windows) if column.platform() == PLATFORM_WINDOWS => windows.power,
            _ => self.power,
        }
    }

    /// Whether the plan table publishes this size in `column`.
    pub fn is_published_in(&self, column: BundleColumn) -> bool {
        self.price_in(column).is_some()
    }

    /// The monthly price of the dual-stack plan of this size on `platform`: see
    /// [`BundleSpec::price_in`], which this reads the platform's column with.
    pub fn price(&self, platform: &str) -> Option<f32> {
        self.price_in(dual_stack_column(platform))
    }

    /// `Bundle$diskSizeInGb` of the dual-stack plan of this size on `platform`:
    /// see [`BundleSpec::disk_size_in_gb_in`].
    pub fn disk_size_in_gb_on(&self, platform: &str) -> i32 {
        self.disk_size_in_gb_in(dual_stack_column(platform))
    }

    /// `Bundle$power` of the dual-stack plan of this size on `platform`: see
    /// [`BundleSpec::power_in`].
    pub fn power_on(&self, platform: &str) -> i32 {
        self.power_in(dual_stack_column(platform))
    }

    /// Whether the plan table publishes a dual-stack plan of this size on
    /// `platform`.
    pub fn is_published_on(&self, platform: &str) -> bool {
        self.is_published_in(dual_stack_column(platform))
    }

    /// This plan in `column` as the modelled `Bundle`, and `None` where the table
    /// publishes no plan of this size in that column.
    pub fn view(&self, column: BundleColumn) -> Option<Bundle> {
        let price = self.price_in(column)?;
        Some(Bundle {
            bundle_id: Some(column_bundle_id(self.size, column, BUNDLE_GENERATION)),
            instance_type: Some(self.size.to_string()),
            name: Some(bundle_size_name(self.size)),
            price: Some(price),
            cpu_count: Some(self.cpu_count),
            ram_size_in_gb: Some(self.ram_size_in_gb),
            disk_size_in_gb: Some(self.disk_size_in_gb_in(column)),
            transfer_per_month_in_gb: Some(self.transfer_gb_per_month),
            power: Some(self.power_in(column)),
            is_active: Some(self.is_active),
            supported_platforms: Some(vec![column.platform().to_string()]),
            // `supportedAppCategories` is the plan's OWN data, like its platform:
            // a standard plan publishes none and reports the member absent.
            supported_app_categories: (!self.app_categories.is_empty()).then(|| {
                self.app_categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect()
            }),
            public_ipv4_address_count: Some(column.public_ipv4_address_count()),
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
/// every size in each column of [`BUNDLE_COLUMNS`], plus the inactive (retired)
/// ones when `include_inactive` is set.
///
/// Every member of a reported bundle is the entry's own — the request selects the
/// catalog and the slice, and nothing it carries is reported back as plan data.
///
/// The order is by COLUMN first — every plan of a column in [`BUNDLE_COLUMNS`]'
/// order, each column's plans in `specs`' own order — so the dual-stack
/// Linux/Unix plans of every size precede the Windows ones, which precede the
/// IPv6-only ones: the order the catalog is published in. Two reads answer
/// identically. Taking the catalog as an argument is what lets the slice rule be
/// exercised against a catalog that holds a retired plan, which the published one
/// does not.
///
/// A size is reported in a column only where the plan table publishes it there
/// ([`BundleSpec::price_in`]), so a column that prices only some of the sizes
/// carries only those.
pub fn bundles_from(specs: &[BundleSpec], include_inactive: bool) -> Vec<Bundle> {
    BUNDLE_COLUMNS
        .iter()
        .flat_map(|column| {
            specs
                .iter()
                .filter(|spec| include_inactive || spec.is_active)
                .filter_map(|spec| spec.view(*column))
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

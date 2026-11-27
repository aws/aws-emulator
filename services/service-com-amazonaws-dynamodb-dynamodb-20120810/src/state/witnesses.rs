// SPDX-License-Identifier: CC0-1.0

//! Whether a table's replication group runs under multi-Region STRONG consistency.
//!
//! The group's mode and the witness Regions it spans are held and validated beside the
//! replicas they configure ([`super::replicas`]); what this file holds is the one reading of
//! that mode the RULES HANGING OFF IT are decided by — the settings an MRSC global table
//! does not offer at all. Stated once, so every such surface asks the same question of the
//! same stored field instead of comparing the mode string itself.

use super::Table;

impl Table {
    /// Whether this table belongs to a global table configured for multi-Region strong
    /// consistency: "`STRONG`: Indicates that the global table is configured for
    /// multi-Region strong consistency (MRSC)"
    /// (`TableDescription$MultiRegionConsistency`).
    ///
    /// False for a table in no replication group and for one whose group runs under
    /// multi-Region eventual consistency — both serve every setting an MRSC group does not.
    pub fn is_mrsc(&self) -> bool {
        self.multi_region_consistency.as_deref() == Some(super::MULTI_REGION_CONSISTENCY_STRONG)
    }
}

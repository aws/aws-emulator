// SPDX-License-Identifier: CC0-1.0

//! The model's `enum` shapes, as Rust enums plus their wire spelling.
//!
//! Every member list below is copied from this service's Smithy model; see the doc
//! comment on each enum for the `smithy_query` call that produced it. Keeping them
//! as Rust enums means a record in the store cannot hold a value the model does not
//! declare, and `from_wire` is the single membership test a request member the model
//! types as an enum is read through.

/// Declare a Smithy `enum` shape as a Rust enum plus its wire spelling.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($variant:ident => $wire:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every member the model declares, in model order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// This member's wire value.
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }

            /// The member `wire` spells, or `None` when the enum has no such member.
            pub fn from_wire(wire: &str) -> Option<Self> {
                match wire { $($wire => Some(Self::$variant),)+ _ => None }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

wire_enum! {
    /// `com.amazonaws.payments#PaymentMethod` — how an instrument pays.
    ///
    /// Members from `smithy_query(query="shape", shape="PaymentMethod")`.
    PaymentMethod {
        Card => "CARD",
        UsAchBankAccount => "US_ACH_BANK_ACCOUNT",
        SepaBankAccount => "SEPA_BANK_ACCOUNT",
        UkBacsBankAccount => "UK_BACS_BANK_ACCOUNT",
        InNetBanking => "IN_NET_BANKING",
        CnBankRedirect => "CN_BANK_REDIRECT",
        Upi => "UPI",
        Pix => "PIX",
        PayByInvoice => "PAY_BY_INVOICE",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#PaymentInstrumentStatus` — whether Amazon Web
    /// Services can use the instrument for payments.
    ///
    /// Members from `smithy_query(query="shape", shape="PaymentInstrumentStatus")`.
    PaymentInstrumentStatus {
        Active => "ACTIVE",
        Invalid => "INVALID",
        PendingSetup => "PENDING_SETUP",
        SetupInProgress => "SETUP_IN_PROGRESS",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#CardBrand` — the card network.
    ///
    /// Members from `smithy_query(query="shape", shape="CardBrand")`.
    CardBrand {
        Amex => "AMEX",
        ChinaUnionpay => "CHINA_UNIONPAY",
        Discover => "DISCOVER",
        Jcb => "JCB",
        Mastercard => "MASTERCARD",
        Rupay => "RUPAY",
        Visa => "VISA",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#CardType` — whether the card is credit or debit.
    ///
    /// Members from `smithy_query(query="shape", shape="CardType")`.
    CardType {
        Credit => "CREDIT",
        Debit => "DEBIT",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#BankAccountType` — the kind of bank account behind a
    /// direct-debit instrument.
    ///
    /// Members from `smithy_query(query="shape", shape="BankAccountType")`.
    BankAccountType {
        Checking => "CHECKING",
        Savings => "SAVINGS",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#FinancialRecordType` — an invoice or a credit memo.
    ///
    /// Members from `smithy_query(query="shape", shape="FinancialRecordType")`.
    FinancialRecordType {
        CreditMemo => "CREDIT_MEMO",
        Invoice => "INVOICE",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#SettlementStatus` — whether the record is open or
    /// closed.
    ///
    /// Members from `smithy_query(query="shape", shape="SettlementStatus")`.
    SettlementStatus {
        Closed => "CLOSED",
        Open => "OPEN",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#FinancialRecordStatus` — the settlement stage of the
    /// record.
    ///
    /// Members from `smithy_query(query="shape", shape="FinancialRecordStatus")`.
    FinancialRecordStatus {
        Canceled => "CANCELED",
        Outstanding => "OUTSTANDING",
        Processing => "PROCESSING",
        Settled => "SETTLED",
    }
}

impl FinancialRecordStatus {
    /// The settlement status that FOLLOWS from this stage.
    ///
    /// Not a free choice: `FinancialRecord$settlementStatus` documents the mapping
    /// both ways — `OPEN` is "the `status` is `OUTSTANDING` or `PROCESSING`" and
    /// `CLOSED` is "the `status` is `SETTLED` or `CANCELED`" — so a record carries
    /// the pair this function produces and never another one.
    pub const fn settlement_status(self) -> SettlementStatus {
        match self {
            Self::Outstanding | Self::Processing => SettlementStatus::Open,
            Self::Settled | Self::Canceled => SettlementStatus::Closed,
        }
    }
}

wire_enum! {
    /// `com.amazonaws.payments#DueStatus` — whether an outstanding invoice is due or
    /// past its due date.
    ///
    /// Members from `smithy_query(query="shape", shape="DueStatus")`.
    DueStatus {
        Due => "DUE",
        PastDue => "PAST_DUE",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#LineOfBusiness` — the line of business within the
    /// merchant of record that issued a record.
    ///
    /// Members from `smithy_query(query="shape", shape="LineOfBusiness")`.
    LineOfBusiness {
        AwsCloudServices => "AWS_CLOUD_SERVICES",
        AwsMarketplace => "AWS_MARKETPLACE",
        ProserveMarketingAndTraining => "PROSERVE_MARKETING_AND_TRAINING",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#InvoiceFrequency` — whether the invoice was issued
    /// once or as part of a billing cycle.
    ///
    /// Members from `smithy_query(query="shape", shape="InvoiceFrequency")`.
    InvoiceFrequency {
        OneTime => "ONE_TIME",
        Recurring => "RECURRING",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#BalanceType` — which balance `GetAccountBalance`
    /// reports.
    ///
    /// Members from `smithy_query(query="shape", shape="BalanceType")`.
    BalanceType {
        InvoicesDue => "INVOICES_DUE",
        InvoicesPastDue => "INVOICES_PAST_DUE",
        OutstandingCreditMemos => "OUTSTANDING_CREDIT_MEMOS",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#FinancialRecordSortField` — the record field
    /// `ListFinancialRecords` sorts by.
    ///
    /// Members from `smithy_query(query="shape", shape="FinancialRecordSortField")`.
    FinancialRecordSortField {
        IssuedAt => "ISSUED_AT",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#SortOrder` — the direction `ListFinancialRecords`
    /// sorts in.
    ///
    /// Members from `smithy_query(query="shape", shape="SortOrder")`.
    SortOrder {
        Ascending => "ASCENDING",
        Descending => "DESCENDING",
    }
}

wire_enum! {
    /// `com.amazonaws.payments#BatchErrorCode` — why `BatchGetFinancialRecord`
    /// could not return one of the records asked for.
    ///
    /// Members from `smithy_query(query="shape", shape="BatchErrorCode")`.
    BatchErrorCode {
        NotFound => "NOT_FOUND",
        InternalError => "INTERNAL_ERROR",
    }
}

impl BatchErrorCode {
    /// What this code means, as `FinancialRecordError$code` documents it — the
    /// `message` the batch states beside it.
    ///
    /// `FinancialRecordError$message` is "a description of why the operation
    /// couldn't retrieve this record", and the model documents one description per
    /// code, so the description belongs to the code rather than to the call site.
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotFound => "No financial record on your account matches the key.",
            Self::InternalError => {
                "An internal error occurred while processing this record. The other records in the request are unaffected. You can retry this record."
            }
        }
    }
}

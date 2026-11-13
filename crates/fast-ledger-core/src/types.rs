use crate::error::LedgerError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Compact 32-bit identifier indexing directly into the hot account array.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(pub u32);

/// Compact 16-bit identifier for a legal entity / subsidiary.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub u16);

/// Monotonically increasing 64-bit transaction sequence number.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TxId(pub u64);

/// Monotonically increasing 64-bit amortization/deferral schedule identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScheduleId(pub u64);

/// Nanosecond Unix timestamp used for temporal indexing and point-in-time queries.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimestampNs(pub u64);

/// 3-byte ASCII ISO-417 / ISO-4217 currency code (e.g., `USD`, `EUR`, `GBP`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency(pub [u8; 3]);

impl Currency {
    pub const USD: Self = Self(*b"USD");
    pub const EUR: Self = Self(*b"EUR");
    pub const GBP: Self = Self(*b"GBP");
    pub const JPY: Self = Self(*b"JPY");
    pub const CAD: Self = Self(*b"CAD");

    /// Constructs a [`Currency`] from 3 uppercase ASCII letters.
    pub fn from_bytes(bytes: [u8; 3]) -> Result<Self, LedgerError> {
        if bytes.iter().all(|b| b.is_ascii_alphabetic()) {
            Ok(Self([
                bytes[0].to_ascii_uppercase(),
                bytes[1].to_ascii_uppercase(),
                bytes[2].to_ascii_uppercase(),
            ]))
        } else {
            Err(LedgerError::InvalidCurrency(
                String::from_utf8_lossy(&bytes).into_owned(),
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        // Safe because constructors validate ASCII alphabetic bytes.
        std::str::from_utf8(&self.0).unwrap_or("???")
    }
}

impl FromStr for Currency {
    type Err = LedgerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 3 {
            return Err(LedgerError::InvalidCurrency(s.to_owned()));
        }
        Self::from_bytes([bytes[0], bytes[1], bytes[2]])
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Currency {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Currency::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Global GAAP / IFRS account classification.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountType {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
    ContraAsset,
    ContraLiability,
    ContraEquity,
}

/// Normal balance side of an account.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalBalance {
    Debit,
    Credit,
}

impl AccountType {
    /// Returns the GAAP/IFRS normal balance side for this account type.
    #[inline]
    pub const fn normal_side(self) -> NormalBalance {
        match self {
            AccountType::Asset
            | AccountType::Expense
            | AccountType::ContraLiability
            | AccountType::ContraEquity => NormalBalance::Debit,
            AccountType::Liability
            | AccountType::Equity
            | AccountType::Revenue
            | AccountType::ContraAsset => NormalBalance::Credit,
        }
    }

    /// Converts cumulative debit and credit totals into a normal-balance signed amount.
    #[inline]
    pub const fn normal_balance(self, cum_debit: i128, cum_credit: i128) -> i128 {
        match self.normal_side() {
            NormalBalance::Debit => cum_debit - cum_credit,
            NormalBalance::Credit => cum_credit - cum_debit,
        }
    }

    /// Converts cumulative debit and credit totals into the raw signed balance (`debit - credit`),
    /// matching the Lean 4 `netDelta` convention used for global conservation checks.
    #[inline]
    pub const fn raw_net_balance(cum_debit: i128, cum_credit: i128) -> i128 {
        cum_debit - cum_credit
    }

    /// Returns true if the account is a permanent balance-sheet account.
    #[inline]
    pub const fn is_balance_sheet(self) -> bool {
        matches!(
            self,
            AccountType::Asset
                | AccountType::Liability
                | AccountType::Equity
                | AccountType::ContraAsset
                | AccountType::ContraLiability
                | AccountType::ContraEquity
        )
    }

    /// Returns true if the account is a temporary income-statement account.
    #[inline]
    pub const fn is_income_statement(self) -> bool {
        matches!(self, AccountType::Revenue | AccountType::Expense)
    }
}

/// Cash Flow Statement classification tag (IAS 7 / ASC 230).
#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CashFlowCategory {
    #[default]
    None = 0,
    Operating = 1,
    Investing = 2,
    Financing = 3,
}

impl CashFlowCategory {
    #[inline]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Operating,
            2 => Self::Investing,
            3 => Self::Financing,
            _ => Self::None,
        }
    }
}

/// Compact `#[repr(C)]` journal line record stored sequentially in the append-only line arena.
///
/// Fields are ordered by descending alignment (`u64`/`i64` -> `u32` -> `u16` -> `u8`/`[u8; 3]`)
/// so there is zero internal padding waste (40 bytes total).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalLine {
    pub tx_id: u64,
    pub debit: i64,
    pub credit: i64,
    pub account_id: u32,
    pub entity_id: u16,
    pub currency: [u8; 3],
    pub cash_flow_cat: u8,
}

impl JournalLine {
    /// Creates a new `JournalLine`.
    #[inline]
    pub const fn new(
        tx_id: TxId,
        account_id: AccountId,
        entity_id: EntityId,
        currency: Currency,
        cash_flow_cat: CashFlowCategory,
        debit: i64,
        credit: i64,
    ) -> Self {
        Self {
            tx_id: tx_id.0,
            debit,
            credit,
            account_id: account_id.0,
            entity_id: entity_id.0,
            currency: currency.0,
            cash_flow_cat: cash_flow_cat as u8,
        }
    }

    /// Returns the Lean-specified signed net delta (`debit - credit`).
    #[inline]
    pub const fn net_delta(&self) -> i128 {
        self.debit as i128 - self.credit as i128
    }
}

/// Input specification for creating a new account in the Chart of Accounts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateAccountInput {
    pub entity_id: EntityId,
    pub code: String,
    pub name: String,
    pub account_type: AccountType,
    pub currency: Currency,
    #[serde(default)]
    pub cash_flow_category: CashFlowCategory,
    #[serde(default)]
    pub is_cash_account: bool,
    #[serde(default)]
    pub is_intercompany: bool,
}

/// A single posting line in an incoming journal entry request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalLineInput {
    pub account_id: AccountId,
    pub debit: i64,
    pub credit: i64,
    #[serde(default)]
    pub cash_flow_category: Option<CashFlowCategory>,
}

impl JournalLineInput {
    #[inline]
    pub fn debit(account_id: AccountId, amount: i64) -> Self {
        Self {
            account_id,
            debit: amount,
            credit: 0,
            cash_flow_category: None,
        }
    }

    #[inline]
    pub fn credit(account_id: AccountId, amount: i64) -> Self {
        Self {
            account_id,
            debit: 0,
            credit: amount,
            cash_flow_category: None,
        }
    }

    #[inline]
    pub fn with_cash_flow(mut self, cat: CashFlowCategory) -> Self {
        self.cash_flow_category = Some(cat);
        self
    }
}

/// Incoming balanced double-entry journal transaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntryInput {
    pub effective_ts_ns: TimestampNs,
    pub description: String,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    pub lines: Vec<JournalLineInput>,
}

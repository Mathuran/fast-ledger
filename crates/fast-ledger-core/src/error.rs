use crate::types::Currency;
use thiserror::Error;

/// Errors returned by the Fast Ledger core validation, storage, and reporting layers.
#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("journal entry must contain at least one line")]
    EmptyTransaction,

    #[error("journal entry is unbalanced: total debits ({total_debit}) != total credits ({total_credit})")]
    UnbalancedTransaction {
        total_debit: i128,
        total_credit: i128,
    },

    #[error("journal line for account {account_id} has negative amount (debit={debit}, credit={credit})")]
    NegativeLineAmount {
        account_id: u32,
        debit: i64,
        credit: i64,
    },

    #[error("journal line for account {account_id} has zero debit and zero credit")]
    ZeroLineAmount { account_id: u32 },

    #[error("journal line for account {account_id} cannot specify both debit ({debit}) and credit ({credit})")]
    BothSidesNonZero {
        account_id: u32,
        debit: i64,
        credit: i64,
    },

    #[error("unknown account id: {0}")]
    UnknownAccount(u32),

    #[error("unknown entity id: {0}")]
    UnknownEntity(u16),

    #[error("unknown schedule id: {0}")]
    UnknownSchedule(u64),

    #[error("duplicate entity code: {0}")]
    DuplicateEntityCode(String),

    #[error("duplicate account code '{code}' for entity {entity_id}")]
    DuplicateAccountCode { entity_id: u16, code: String },

    #[error("account {account_id} belongs to entity {expected_entity}, got {actual_entity}")]
    AccountEntityMismatch {
        account_id: u32,
        expected_entity: u16,
        actual_entity: u16,
    },

    #[error("account {account_id} currency is {expected}, got {actual}")]
    AccountCurrencyMismatch {
        account_id: u32,
        expected: Currency,
        actual: Currency,
    },

    #[error("multi-entity transaction requires intercompany rule between entity {from_entity} and {to_entity}")]
    MissingIntercompanyRule { from_entity: u16, to_entity: u16 },

    #[error("schedule must have at least 1 period, got {0}")]
    InvalidSchedulePeriods(u32),

    #[error("schedule amount must be positive, got {0}")]
    InvalidScheduleAmount(i64),

    #[error("schedule period duration must be positive")]
    InvalidSchedulePeriodDuration,

    #[error("schedule source and target accounts must belong to entity {entity_id}")]
    ScheduleAccountEntityMismatch { entity_id: u16 },

    #[error("invalid currency code '{0}': expected 3 ASCII letters")]
    InvalidCurrency(String),

    #[error("missing FX translation rate from {from} to {to}")]
    MissingFxRate { from: Currency, to: Currency },

    #[error("invalid FX translation rate: must be strictly positive")]
    InvalidFxRate,

    #[error("WAL corruption detected: {0}")]
    WalCorruption(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

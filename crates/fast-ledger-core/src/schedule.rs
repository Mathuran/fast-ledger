use crate::error::LedgerError;
use crate::types::{AccountId, Currency, EntityId, ScheduleId, TimestampNs, TxId};
use serde::{Deserialize, Serialize};

/// Supported amortization and deferral schedule classifications under ASC 606 / IFRS 15 / GAAP.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    DeferredRevenue,
    PrepaidExpense,
    AccruedExpense,
    Depreciation,
}

/// Specification for generating an exact straight-line amortization or deferral schedule.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleSpec {
    pub kind: ScheduleKind,
    pub entity_id: EntityId,
    pub currency: Currency,
    /// Total amount in minor units (e.g., cents). Must be strictly positive.
    pub total_amount: i64,
    /// Deferral / balance-sheet holding account (e.g., Deferred Revenue or Prepaid Expense).
    pub source_account: AccountId,
    /// P&L recognition target account (e.g., Recognized Revenue or Amortization Expense).
    pub target_account: AccountId,
    /// Optional funding account (e.g., Cash or Accounts Receivable) to book the initial deferral entry.
    #[serde(default)]
    pub initial_funding_account: Option<AccountId>,
    pub start_ts_ns: TimestampNs,
    pub period_ns: u64,
    pub num_periods: u32,
    pub description: String,
}

/// A single installment within an amortization/deferral schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleInstallment {
    pub period_index: u32,
    pub due_ts_ns: TimestampNs,
    pub amount: i64,
    pub recognized_tx_id: Option<TxId>,
}

/// Stored schedule record containing all computed installments.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AmortizationSchedule {
    pub id: ScheduleId,
    pub spec: ScheduleSpec,
    pub initial_tx_id: Option<TxId>,
    pub installments: Vec<ScheduleInstallment>,
}

impl AmortizationSchedule {
    /// Generates an exact straight-line schedule honoring the Lean 4
    /// `exact_amortization_conservation` theorem:
    /// `(n - 1) * (total / n) + ((total / n) + (total % n)) == total`.
    pub fn build_installments(spec: &ScheduleSpec) -> Result<Vec<ScheduleInstallment>, LedgerError> {
        if spec.num_periods == 0 {
            return Err(LedgerError::InvalidSchedulePeriods(spec.num_periods));
        }
        if spec.total_amount <= 0 {
            return Err(LedgerError::InvalidScheduleAmount(spec.total_amount));
        }
        if spec.period_ns == 0 {
            return Err(LedgerError::InvalidSchedulePeriodDuration);
        }

        let n = spec.num_periods as i64;
        let base = spec.total_amount / n;
        let remainder = spec.total_amount % n;

        let mut installments = Vec::with_capacity(spec.num_periods as usize);
        for idx in 0..spec.num_periods {
            let amount = if idx + 1 == spec.num_periods {
                base + remainder
            } else {
                base
            };
            let due_ts_ns = TimestampNs(
                spec.start_ts_ns
                    .0
                    .saturating_add((idx as u64).saturating_mul(spec.period_ns)),
            );
            installments.push(ScheduleInstallment {
                period_index: idx,
                due_ts_ns,
                amount,
                recognized_tx_id: None,
            });
        }

        debug_assert_eq!(
            installments.iter().map(|i| i.amount as i128).sum::<i128>(),
            spec.total_amount as i128
        );

        Ok(installments)
    }
}

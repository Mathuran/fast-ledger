use crate::engine::{AccountRecord, LedgerDb};
use fast_ledger_core::{
    AccountId, AccountType, CashFlowCategory, Currency, EntityId, LedgerError, TimestampNs,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fixed-point scaling factor for FX rates (`1_000_000` = `1.000000`).
pub const FX_SCALE: i128 = 1_000_000;

/// Fixed-point exchange rate table for multi-currency consolidation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FxRateTable {
    rates: HashMap<(Currency, Currency), i128>,
}

impl FxRateTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a scaled rate (`rate_scaled = rate * 1_000_000`) such that
    /// `amount_in_to = amount_in_from * rate_scaled / 1_000_000`.
    pub fn set_rate(
        &mut self,
        from: Currency,
        to: Currency,
        rate_scaled: i128,
    ) -> Result<(), LedgerError> {
        if rate_scaled <= 0 {
            return Err(LedgerError::InvalidFxRate);
        }
        self.rates.insert((from, to), rate_scaled);
        Ok(())
    }

    pub fn get_rate(&self, from: Currency, to: Currency) -> Result<i128, LedgerError> {
        if from == to {
            return Ok(FX_SCALE);
        }
        self.rates
            .get(&(from, to))
            .copied()
            .ok_or(LedgerError::MissingFxRate { from, to })
    }

    /// Translates `amount` from `from` currency to `to` currency using round-half-away-from-zero.
    pub fn translate(
        &self,
        amount: i128,
        from: Currency,
        to: Currency,
    ) -> Result<i128, LedgerError> {
        if from == to || amount == 0 {
            return Ok(amount);
        }
        let rate = self.get_rate(from, to)?;
        let prod = amount * rate;
        Ok(if prod >= 0 {
            (prod + FX_SCALE / 2) / FX_SCALE
        } else {
            (prod - FX_SCALE / 2) / FX_SCALE
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialBalanceRow {
    pub account_id: AccountId,
    pub entity_id: EntityId,
    pub code: String,
    pub name: String,
    pub account_type: AccountType,
    pub currency: Currency,
    pub debit_balance: i128,
    pub credit_balance: i128,
    pub normal_balance: i128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialBalanceReport {
    pub as_of_ts_ns: TimestampNs,
    pub entity_id: Option<EntityId>,
    pub rows: Vec<TrialBalanceRow>,
    pub total_debits: i128,
    pub total_credits: i128,
    pub is_balanced: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportLine {
    pub account_id: AccountId,
    pub entity_id: EntityId,
    pub code: String,
    pub name: String,
    pub account_type: AccountType,
    pub currency: Currency,
    pub amount: i128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BalanceSheetReport {
    pub as_of_ts_ns: TimestampNs,
    pub entity_id: Option<EntityId>,
    pub assets: Vec<ReportLine>,
    pub liabilities: Vec<ReportLine>,
    pub equity: Vec<ReportLine>,
    pub current_period_net_income: i128,
    pub total_assets: i128,
    pub total_liabilities: i128,
    pub total_equity_including_net_income: i128,
    pub is_balanced: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IncomeStatementReport {
    pub start_ts_ns: TimestampNs,
    pub end_ts_ns: TimestampNs,
    pub entity_id: Option<EntityId>,
    pub revenues: Vec<ReportLine>,
    pub expenses: Vec<ReportLine>,
    pub total_revenue: i128,
    pub total_expenses: i128,
    pub net_income: i128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CashFlowStatementReport {
    pub start_ts_ns: TimestampNs,
    pub end_ts_ns: TimestampNs,
    pub entity_id: Option<EntityId>,
    pub beginning_cash: i128,
    pub operating_cash_flow: i128,
    pub investing_cash_flow: i128,
    pub financing_cash_flow: i128,
    pub net_change_in_cash: i128,
    pub ending_cash: i128,
    pub is_reconciled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EliminatedAccountLine {
    pub account_id: AccountId,
    pub entity_id: EntityId,
    pub code: String,
    pub name: String,
    pub original_currency: Currency,
    pub original_raw_net: i128,
    pub translated_raw_net: i128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsolidatedReport {
    pub presentation_currency: Currency,
    pub as_of_ts_ns: TimestampNs,
    pub start_ts_ns: TimestampNs,
    pub end_ts_ns: TimestampNs,
    pub entity_count: usize,
    pub assets: Vec<ReportLine>,
    pub liabilities: Vec<ReportLine>,
    pub equity: Vec<ReportLine>,
    pub revenues: Vec<ReportLine>,
    pub expenses: Vec<ReportLine>,
    pub eliminated_intercompany: Vec<EliminatedAccountLine>,
    pub total_assets: i128,
    pub total_liabilities: i128,
    pub total_equity: i128,
    pub total_revenue: i128,
    pub total_expenses: i128,
    pub net_income: i128,
    pub cumulative_net_income: i128,
    pub fx_translation_adjustment: i128,
    pub intercompany_elimination_residual: i128,
}

#[inline]
fn validate_entity_filter(db: &LedgerDb, entity_id: Option<EntityId>) -> Result<(), LedgerError> {
    if let Some(eid) = entity_id {
        if db.get_entity(eid).is_none() {
            return Err(LedgerError::UnknownEntity(eid.0));
        }
    }
    Ok(())
}

#[inline]
fn filter_accounts(
    db: &LedgerDb,
    entity_id: Option<EntityId>,
) -> impl Iterator<Item = &AccountRecord> {
    db.accounts()
        .iter()
        .filter(move |acct| entity_id.is_none_or(|eid| acct.entity_id == eid))
}

/// Generates a point-in-time Trial Balance in `O(A log K)` via prefix-sum checkpoints.
pub fn generate_trial_balance(
    db: &LedgerDb,
    entity_id: Option<EntityId>,
    as_of_ts_ns: TimestampNs,
) -> Result<TrialBalanceReport, LedgerError> {
    validate_entity_filter(db, entity_id)?;

    let mut rows = Vec::new();
    let mut total_debits: i128 = 0;
    let mut total_credits: i128 = 0;

    for acct in filter_accounts(db, entity_id) {
        let snap = db.balance_as_of(acct.id, as_of_ts_ns)?;
        if snap.cum_debit == 0 && snap.cum_credit == 0 {
            continue;
        }
        let (debit_balance, credit_balance) = if snap.net_delta >= 0 {
            (snap.net_delta, 0)
        } else {
            (0, -snap.net_delta)
        };
        total_debits += debit_balance;
        total_credits += credit_balance;

        rows.push(TrialBalanceRow {
            account_id: acct.id,
            entity_id: acct.entity_id,
            code: acct.code.clone(),
            name: acct.name.clone(),
            account_type: acct.account_type,
            currency: acct.currency,
            debit_balance,
            credit_balance,
            normal_balance: snap.normal_balance,
        });
    }

    Ok(TrialBalanceReport {
        as_of_ts_ns,
        entity_id,
        rows,
        total_debits,
        total_credits,
        is_balanced: total_debits == total_credits,
    })
}

/// Generates a point-in-time GAAP/IFRS Balance Sheet in `O(A log K)`.
pub fn generate_balance_sheet(
    db: &LedgerDb,
    entity_id: Option<EntityId>,
    as_of_ts_ns: TimestampNs,
) -> Result<BalanceSheetReport, LedgerError> {
    validate_entity_filter(db, entity_id)?;

    let mut assets = Vec::new();
    let mut liabilities = Vec::new();
    let mut equity = Vec::new();
    let mut total_assets: i128 = 0;
    let mut total_liabilities: i128 = 0;
    let mut total_equity_accounts: i128 = 0;
    let mut cumulative_revenue: i128 = 0;
    let mut cumulative_expense: i128 = 0;

    for acct in filter_accounts(db, entity_id) {
        let snap = db.balance_as_of(acct.id, as_of_ts_ns)?;
        if snap.net_delta == 0 {
            continue;
        }
        let make_line = |amount: i128| ReportLine {
            account_id: acct.id,
            entity_id: acct.entity_id,
            code: acct.code.clone(),
            name: acct.name.clone(),
            account_type: acct.account_type,
            currency: acct.currency,
            amount,
        };

        match acct.account_type {
            AccountType::Asset | AccountType::ContraAsset => {
                let amt = snap.net_delta;
                total_assets += amt;
                assets.push(make_line(amt));
            }
            AccountType::Liability | AccountType::ContraLiability => {
                let amt = -snap.net_delta;
                total_liabilities += amt;
                liabilities.push(make_line(amt));
            }
            AccountType::Equity | AccountType::ContraEquity => {
                let amt = -snap.net_delta;
                total_equity_accounts += amt;
                equity.push(make_line(amt));
            }
            AccountType::Revenue => {
                cumulative_revenue += snap.normal_balance;
            }
            AccountType::Expense => {
                cumulative_expense += snap.normal_balance;
            }
        }
    }

    let current_period_net_income = cumulative_revenue - cumulative_expense;
    let total_equity_including_net_income = total_equity_accounts + current_period_net_income;

    Ok(BalanceSheetReport {
        as_of_ts_ns,
        entity_id,
        assets,
        liabilities,
        equity,
        current_period_net_income,
        total_assets,
        total_liabilities,
        total_equity_including_net_income,
        is_balanced: total_assets == total_liabilities + total_equity_including_net_income,
    })
}

/// Generates an Income Statement (P&L) over `[start_ts_ns, end_ts_ns]` in `O(A log K)`.
pub fn generate_income_statement(
    db: &LedgerDb,
    entity_id: Option<EntityId>,
    start_ts_ns: TimestampNs,
    end_ts_ns: TimestampNs,
) -> Result<IncomeStatementReport, LedgerError> {
    validate_entity_filter(db, entity_id)?;

    let mut revenues = Vec::new();
    let mut expenses = Vec::new();
    let mut total_revenue: i128 = 0;
    let mut total_expenses: i128 = 0;

    for acct in filter_accounts(db, entity_id) {
        if !acct.account_type.is_income_statement() {
            continue;
        }
        let snap = db.activity_between(acct.id, start_ts_ns, end_ts_ns)?;
        if snap.normal_balance == 0 {
            continue;
        }
        let line = ReportLine {
            account_id: acct.id,
            entity_id: acct.entity_id,
            code: acct.code.clone(),
            name: acct.name.clone(),
            account_type: acct.account_type,
            currency: acct.currency,
            amount: snap.normal_balance,
        };
        match acct.account_type {
            AccountType::Revenue => {
                total_revenue += snap.normal_balance;
                revenues.push(line);
            }
            AccountType::Expense => {
                total_expenses += snap.normal_balance;
                expenses.push(line);
            }
            _ => {}
        }
    }

    Ok(IncomeStatementReport {
        start_ts_ns,
        end_ts_ns,
        entity_id,
        revenues,
        expenses,
        total_revenue,
        total_expenses,
        net_income: total_revenue - total_expenses,
    })
}

/// Generates an IAS 7 / ASC 230 Cash Flow Statement over `[start_ts_ns, end_ts_ns]` in `O((E + C) log K)`.
pub fn generate_cash_flow_statement(
    db: &LedgerDb,
    entity_id: Option<EntityId>,
    start_ts_ns: TimestampNs,
    end_ts_ns: TimestampNs,
) -> Result<CashFlowStatementReport, LedgerError> {
    validate_entity_filter(db, entity_id)?;

    let mut beginning_cash: i128 = 0;
    let mut ending_cash: i128 = 0;
    for acct in filter_accounts(db, entity_id) {
        if !acct.is_cash_account {
            continue;
        }
        if start_ts_ns.0 > 0 {
            beginning_cash += db
                .balance_as_of(acct.id, TimestampNs(start_ts_ns.0 - 1))?
                .net_delta;
        }
        if end_ts_ns.0 >= start_ts_ns.0 {
            ending_cash += db.balance_as_of(acct.id, end_ts_ns)?.net_delta;
        } else {
            ending_cash = beginning_cash;
        }
    }

    let operating_cash_flow =
        db.cash_flow_activity(entity_id, CashFlowCategory::Operating, start_ts_ns, end_ts_ns);
    let investing_cash_flow =
        db.cash_flow_activity(entity_id, CashFlowCategory::Investing, start_ts_ns, end_ts_ns);
    let financing_cash_flow =
        db.cash_flow_activity(entity_id, CashFlowCategory::Financing, start_ts_ns, end_ts_ns);
    let net_change_in_cash = operating_cash_flow + investing_cash_flow + financing_cash_flow;

    Ok(CashFlowStatementReport {
        start_ts_ns,
        end_ts_ns,
        entity_id,
        beginning_cash,
        operating_cash_flow,
        investing_cash_flow,
        financing_cash_flow,
        net_change_in_cash,
        ending_cash,
        is_reconciled: beginning_cash + net_change_in_cash == ending_cash,
    })
}

/// Generates a multi-entity consolidated Balance Sheet + P&L in `presentation_currency`,
/// translating entity balances via `fx_rates` and automatically eliminating intercompany accounts.
pub fn generate_consolidated_report(
    db: &LedgerDb,
    presentation_currency: Currency,
    as_of_ts_ns: TimestampNs,
    start_ts_ns: TimestampNs,
    end_ts_ns: TimestampNs,
    fx_rates: &FxRateTable,
) -> Result<ConsolidatedReport, LedgerError> {
    let mut assets = Vec::new();
    let mut liabilities = Vec::new();
    let mut equity = Vec::new();
    let mut revenues = Vec::new();
    let mut expenses = Vec::new();
    let mut eliminated_intercompany = Vec::new();

    let mut total_assets: i128 = 0;
    let mut total_liabilities: i128 = 0;
    let mut total_equity: i128 = 0;
    let mut total_revenue: i128 = 0;
    let mut total_expenses: i128 = 0;
    let mut cumulative_revenue: i128 = 0;
    let mut cumulative_expense: i128 = 0;
    let mut intercompany_elimination_residual: i128 = 0;

    for acct in db.accounts() {
        let snap_as_of = db.balance_as_of(acct.id, as_of_ts_ns)?;
        let translated_raw =
            fx_rates.translate(snap_as_of.net_delta, acct.currency, presentation_currency)?;

        if acct.is_intercompany {
            if snap_as_of.net_delta != 0 {
                intercompany_elimination_residual += translated_raw;
                eliminated_intercompany.push(EliminatedAccountLine {
                    account_id: acct.id,
                    entity_id: acct.entity_id,
                    code: acct.code.clone(),
                    name: acct.name.clone(),
                    original_currency: acct.currency,
                    original_raw_net: snap_as_of.net_delta,
                    translated_raw_net: translated_raw,
                });
            }
            continue;
        }

        if snap_as_of.net_delta != 0 {
            let make_bs_line = |amount: i128| ReportLine {
                account_id: acct.id,
                entity_id: acct.entity_id,
                code: acct.code.clone(),
                name: acct.name.clone(),
                account_type: acct.account_type,
                currency: presentation_currency,
                amount,
            };

            match acct.account_type {
                AccountType::Asset | AccountType::ContraAsset => {
                    total_assets += translated_raw;
                    assets.push(make_bs_line(translated_raw));
                }
                AccountType::Liability | AccountType::ContraLiability => {
                    let amt = -translated_raw;
                    total_liabilities += amt;
                    liabilities.push(make_bs_line(amt));
                }
                AccountType::Equity | AccountType::ContraEquity => {
                    let amt = -translated_raw;
                    total_equity += amt;
                    equity.push(make_bs_line(amt));
                }
                AccountType::Revenue => {
                    cumulative_revenue += -translated_raw;
                }
                AccountType::Expense => {
                    cumulative_expense += translated_raw;
                }
            }
        }

        if acct.account_type.is_income_statement() {
            let period_snap = db.activity_between(acct.id, start_ts_ns, end_ts_ns)?;
            if period_snap.normal_balance != 0 {
                let translated_period = fx_rates.translate(
                    period_snap.normal_balance,
                    acct.currency,
                    presentation_currency,
                )?;
                let line = ReportLine {
                    account_id: acct.id,
                    entity_id: acct.entity_id,
                    code: acct.code.clone(),
                    name: acct.name.clone(),
                    account_type: acct.account_type,
                    currency: presentation_currency,
                    amount: translated_period,
                };
                match acct.account_type {
                    AccountType::Revenue => {
                        total_revenue += translated_period;
                        revenues.push(line);
                    }
                    AccountType::Expense => {
                        total_expenses += translated_period;
                        expenses.push(line);
                    }
                    _ => {}
                }
            }
        }
    }

    let net_income = total_revenue - total_expenses;
    let cumulative_net_income = cumulative_revenue - cumulative_expense;
    let fx_translation_adjustment =
        total_assets - (total_liabilities + total_equity + cumulative_net_income);

    Ok(ConsolidatedReport {
        presentation_currency,
        as_of_ts_ns,
        start_ts_ns,
        end_ts_ns,
        entity_count: db.entities().len(),
        assets,
        liabilities,
        equity,
        revenues,
        expenses,
        eliminated_intercompany,
        total_assets,
        total_liabilities,
        total_equity,
        total_revenue,
        total_expenses,
        net_income,
        cumulative_net_income,
        fx_translation_adjustment,
        intercompany_elimination_residual,
    })
}

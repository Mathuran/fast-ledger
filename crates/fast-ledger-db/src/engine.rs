use crate::wal::{WalPostLine, WalRecord, WalWriter};
use fast_ledger_core::{
    AccountId, AccountType, AmortizationSchedule, CashFlowCategory, CreateAccountInput,
    CreateEntityInput, Currency, Entity, EntityId, IntercompanyConfig, IntercompanyRule,
    JournalEntryInput, JournalLine, JournalLineInput, LedgerError, ScheduleId, ScheduleKind,
    ScheduleSpec, TimestampNs, TxId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Hot account metadata and `O(1)` running totals stored in a flat contiguous vector.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: AccountId,
    pub entity_id: EntityId,
    pub code: String,
    pub name: String,
    pub account_type: AccountType,
    pub currency: Currency,
    pub cash_flow_category: CashFlowCategory,
    pub is_cash_account: bool,
    pub is_intercompany: bool,
    pub cum_debit: i128,
    pub cum_credit: i128,
    pub tx_count: u64,
}

impl AccountRecord {
    /// Returns the account's current signed net balance (`cum_debit - cum_credit`).
    #[inline]
    pub const fn raw_net_balance(&self) -> i128 {
        AccountType::raw_net_balance(self.cum_debit, self.cum_credit)
    }

    /// Returns the account's current normal-balance amount.
    #[inline]
    pub const fn normal_balance(&self) -> i128 {
        self.account_type
            .normal_balance(self.cum_debit, self.cum_credit)
    }
}

/// Transaction header pointing to a contiguous slice of lines in the line arena.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxHeader {
    pub tx_id: TxId,
    pub effective_ts_ns: TimestampNs,
    pub description: String,
    pub line_offset: usize,
    pub line_len: u32,
}

/// Temporal prefix-sum checkpoint enabling `O(log K)` point-in-time and range queries.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PrefixCheckpoint {
    pub effective_ts_ns: u64,
    pub tx_id: u64,
    pub cum_debit: i128,
    pub cum_credit: i128,
}

/// Point-in-time or period-activity balance summary for an account.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceSnapshot {
    pub account_id: AccountId,
    pub cum_debit: i128,
    pub cum_credit: i128,
    pub net_delta: i128,
    pub normal_balance: i128,
}

/// Result emitted when a schedule installment is recognized into the ledger.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecognitionResult {
    pub schedule_id: ScheduleId,
    pub period_index: u32,
    pub due_ts_ns: TimestampNs,
    pub amount: i64,
    pub tx_id: TxId,
}

struct PreparedEntry {
    effective_ts_ns: TimestampNs,
    description: String,
    idempotency_key: Option<String>,
    lines: Vec<WalPostLine>,
}

/// Custom high-performance double-entry accounting database implementing the
/// Lean 4 verified prefix-sum checkpoint and exact amortization algorithms.
pub struct LedgerDb {
    entities: Vec<Entity>,
    entity_by_code: HashMap<String, EntityId>,
    accounts: Vec<AccountRecord>,
    account_by_code: HashMap<(EntityId, String), AccountId>,
    intercompany: IntercompanyConfig,
    tx_headers: Vec<TxHeader>,
    line_arena: Vec<JournalLine>,
    /// Per-account sorted temporal prefix checkpoints (`checkpoints[account_id]`).
    account_checkpoints: Vec<Vec<PrefixCheckpoint>>,
    /// Per-entity cash-flow prefix checkpoints (`[Operating=0, Investing=1, Financing=2]`).
    cash_flow_checkpoints: Vec<[Vec<PrefixCheckpoint>; 3]>,
    idempotency_index: HashMap<String, TxId>,
    schedules: Vec<AmortizationSchedule>,
    wal: Option<WalWriter>,
}

impl Default for LedgerDb {
    fn default() -> Self {
        Self::new()
    }
}

impl LedgerDb {
    /// Creates an in-memory `LedgerDb` without WAL persistence (maximum throughput).
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            entity_by_code: HashMap::new(),
            accounts: Vec::new(),
            account_by_code: HashMap::new(),
            intercompany: IntercompanyConfig::new(),
            tx_headers: Vec::new(),
            line_arena: Vec::new(),
            account_checkpoints: Vec::new(),
            cash_flow_checkpoints: Vec::new(),
            idempotency_index: HashMap::new(),
            schedules: Vec::new(),
            wal: None,
        }
    }

    /// Pre-allocates capacity for expected account, transaction, and line counts.
    pub fn with_capacities(accounts: usize, transactions: usize, lines: usize) -> Self {
        let mut db = Self::new();
        db.accounts.reserve(accounts);
        db.account_checkpoints.reserve(accounts);
        db.tx_headers.reserve(transactions);
        db.line_arena.reserve(lines);
        db
    }

    /// Opens or creates a durable `LedgerDb` backed by a CRC32-checksummed binary WAL,
    /// replaying any existing frames on startup.
    pub fn open_with_wal(
        path: impl AsRef<Path>,
        sync_on_commit: bool,
    ) -> Result<Self, LedgerError> {
        let (wal, records) = WalWriter::open_and_read(path, sync_on_commit)?;
        let mut db = Self::new();
        for rec in records {
            db.replay_wal_record(rec)?;
        }
        db.wal = Some(wal);
        Ok(db)
    }

    fn replay_wal_record(&mut self, record: WalRecord) -> Result<(), LedgerError> {
        match record {
            WalRecord::CreateEntity {
                entity_id,
                parent_entity_id,
                currency,
                code,
                name,
            } => {
                if entity_id.0 as usize != self.entities.len() {
                    return Err(LedgerError::WalCorruption(format!(
                        "non-contiguous entity id {} in WAL",
                        entity_id.0
                    )));
                }
                self.insert_entity_state(entity_id, code, name, currency, parent_entity_id);
            }
            WalRecord::CreateAccount {
                account_id,
                entity_id,
                account_type,
                currency,
                cash_flow_category,
                is_cash_account,
                is_intercompany,
                code,
                name,
            } => {
                if account_id.0 as usize != self.accounts.len() {
                    return Err(LedgerError::WalCorruption(format!(
                        "non-contiguous account id {} in WAL",
                        account_id.0
                    )));
                }
                self.insert_account_state(
                    account_id,
                    CreateAccountInput {
                        entity_id,
                        code,
                        name,
                        account_type,
                        currency,
                        cash_flow_category,
                        is_cash_account,
                        is_intercompany,
                    },
                );
            }
            WalRecord::RegisterIntercompanyRule {
                from_entity,
                to_entity,
                due_from_account,
                due_to_account,
            } => {
                self.intercompany.register_rule(IntercompanyRule {
                    from_entity,
                    to_entity,
                    due_from_account,
                    due_to_account,
                });
            }
            WalRecord::PostEntry {
                tx_id,
                effective_ts_ns,
                description,
                idempotency_key,
                lines,
            } => {
                if tx_id.0 as usize != self.tx_headers.len() {
                    return Err(LedgerError::WalCorruption(format!(
                        "non-contiguous tx id {} in WAL",
                        tx_id.0
                    )));
                }
                self.commit_prepared_in_memory(
                    tx_id,
                    PreparedEntry {
                        effective_ts_ns,
                        description,
                        idempotency_key,
                        lines,
                    },
                );
            }
            WalRecord::CreateSchedule {
                schedule_id,
                kind,
                entity_id,
                currency,
                total_amount,
                source_account,
                target_account,
                initial_funding_account,
                start_ts_ns,
                period_ns,
                num_periods,
                initial_tx_id,
                description,
            } => {
                if schedule_id.0 as usize != self.schedules.len() {
                    return Err(LedgerError::WalCorruption(format!(
                        "non-contiguous schedule id {} in WAL",
                        schedule_id.0
                    )));
                }
                let spec = ScheduleSpec {
                    kind,
                    entity_id,
                    currency,
                    total_amount,
                    source_account,
                    target_account,
                    initial_funding_account,
                    start_ts_ns,
                    period_ns,
                    num_periods,
                    description,
                };
                let installments = AmortizationSchedule::build_installments(&spec)?;
                self.schedules.push(AmortizationSchedule {
                    id: schedule_id,
                    spec,
                    initial_tx_id,
                    installments,
                });
            }
            WalRecord::RecognizeScheduleInstallment {
                schedule_id,
                period_index,
                tx_id,
            } => {
                let sched = self
                    .schedules
                    .get_mut(schedule_id.0 as usize)
                    .ok_or(LedgerError::UnknownSchedule(schedule_id.0))?;
                let inst = sched
                    .installments
                    .get_mut(period_index as usize)
                    .ok_or_else(|| {
                        LedgerError::WalCorruption(format!(
                            "invalid schedule period index {period_index} in WAL"
                        ))
                    })?;
                inst.recognized_tx_id = Some(tx_id);
            }
        }
        Ok(())
    }

    pub fn create_entity(&mut self, input: CreateEntityInput) -> Result<EntityId, LedgerError> {
        if self.entity_by_code.contains_key(&input.code) {
            return Err(LedgerError::DuplicateEntityCode(input.code));
        }
        if let Some(parent_id) = input.parent_entity_id {
            if parent_id.0 as usize >= self.entities.len() {
                return Err(LedgerError::UnknownEntity(parent_id.0));
            }
        }
        let entity_id = EntityId(self.entities.len() as u16);

        if let Some(wal) = &mut self.wal {
            wal.append(&WalRecord::CreateEntity {
                entity_id,
                parent_entity_id: input.parent_entity_id,
                currency: input.functional_currency,
                code: input.code.clone(),
                name: input.name.clone(),
            })?;
            wal.flush()?;
        }

        self.insert_entity_state(
            entity_id,
            input.code,
            input.name,
            input.functional_currency,
            input.parent_entity_id,
        );
        Ok(entity_id)
    }

    fn insert_entity_state(
        &mut self,
        entity_id: EntityId,
        code: String,
        name: String,
        functional_currency: Currency,
        parent_entity_id: Option<EntityId>,
    ) {
        self.entity_by_code.insert(code.clone(), entity_id);
        self.entities.push(Entity {
            id: entity_id,
            code,
            name,
            functional_currency,
            parent_entity_id,
        });
        self.cash_flow_checkpoints
            .push([Vec::new(), Vec::new(), Vec::new()]);
    }

    #[inline]
    pub fn get_entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(id.0 as usize)
    }

    #[inline]
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn create_account(&mut self, input: CreateAccountInput) -> Result<AccountId, LedgerError> {
        if input.entity_id.0 as usize >= self.entities.len() {
            return Err(LedgerError::UnknownEntity(input.entity_id.0));
        }
        if self
            .account_by_code
            .contains_key(&(input.entity_id, input.code.clone()))
        {
            return Err(LedgerError::DuplicateAccountCode {
                entity_id: input.entity_id.0,
                code: input.code,
            });
        }
        let account_id = AccountId(self.accounts.len() as u32);

        if let Some(wal) = &mut self.wal {
            wal.append(&WalRecord::CreateAccount {
                account_id,
                entity_id: input.entity_id,
                account_type: input.account_type,
                currency: input.currency,
                cash_flow_category: input.cash_flow_category,
                is_cash_account: input.is_cash_account,
                is_intercompany: input.is_intercompany,
                code: input.code.clone(),
                name: input.name.clone(),
            })?;
            wal.flush()?;
        }

        self.insert_account_state(account_id, input);
        Ok(account_id)
    }

    fn insert_account_state(&mut self, account_id: AccountId, input: CreateAccountInput) {
        self.account_by_code
            .insert((input.entity_id, input.code.clone()), account_id);
        self.accounts.push(AccountRecord {
            id: account_id,
            entity_id: input.entity_id,
            code: input.code,
            name: input.name,
            account_type: input.account_type,
            currency: input.currency,
            cash_flow_category: input.cash_flow_category,
            is_cash_account: input.is_cash_account,
            is_intercompany: input.is_intercompany,
            cum_debit: 0,
            cum_credit: 0,
            tx_count: 0,
        });
        self.account_checkpoints.push(Vec::new());
    }

    #[inline]
    pub fn get_account(&self, id: AccountId) -> Option<&AccountRecord> {
        self.accounts.get(id.0 as usize)
    }

    #[inline]
    pub fn accounts(&self) -> &[AccountRecord] {
        &self.accounts
    }

    pub fn register_intercompany_rule(
        &mut self,
        rule: IntercompanyRule,
    ) -> Result<(), LedgerError> {
        let from_acct = self
            .accounts
            .get(rule.due_from_account.0 as usize)
            .ok_or(LedgerError::UnknownAccount(rule.due_from_account.0))?;
        if from_acct.entity_id != rule.from_entity {
            return Err(LedgerError::AccountEntityMismatch {
                account_id: rule.due_from_account.0,
                expected_entity: rule.from_entity.0,
                actual_entity: from_acct.entity_id.0,
            });
        }

        let to_acct = self
            .accounts
            .get(rule.due_to_account.0 as usize)
            .ok_or(LedgerError::UnknownAccount(rule.due_to_account.0))?;
        if to_acct.entity_id != rule.to_entity {
            return Err(LedgerError::AccountEntityMismatch {
                account_id: rule.due_to_account.0,
                expected_entity: rule.to_entity.0,
                actual_entity: to_acct.entity_id.0,
            });
        }

        if let Some(wal) = &mut self.wal {
            wal.append(&WalRecord::RegisterIntercompanyRule {
                from_entity: rule.from_entity,
                to_entity: rule.to_entity,
                due_from_account: rule.due_from_account,
                due_to_account: rule.due_to_account,
            })?;
            wal.flush()?;
        }

        self.intercompany.register_rule(rule);
        Ok(())
    }

    /// Validates and commits a single double-entry journal transaction.
    pub fn post_entry(&mut self, input: JournalEntryInput) -> Result<TxId, LedgerError> {
        if let Some(key) = &input.idempotency_key {
            if let Some(&existing) = self.idempotency_index.get(key) {
                return Ok(existing);
            }
        }

        let prepared = self.prepare_entry(input)?;
        let tx_id = TxId(self.tx_headers.len() as u64);

        if let Some(wal) = &mut self.wal {
            wal.append(&WalRecord::PostEntry {
                tx_id,
                effective_ts_ns: prepared.effective_ts_ns,
                description: prepared.description.clone(),
                idempotency_key: prepared.idempotency_key.clone(),
                lines: prepared.lines.clone(),
            })?;
            wal.flush()?;
        }

        self.commit_prepared_in_memory(tx_id, prepared);
        Ok(tx_id)
    }

    /// Validates and commits an atomic batch of journal entries with a single group WAL flush.
    pub fn post_batch(
        &mut self,
        entries: Vec<JournalEntryInput>,
    ) -> Result<Vec<TxId>, LedgerError> {
        let mut results = Vec::with_capacity(entries.len());
        let mut prepared_list: Vec<Option<PreparedEntry>> = Vec::with_capacity(entries.len());
        let mut batch_idempotency: HashMap<String, usize> = HashMap::new();
        let base_tx = self.tx_headers.len() as u64;
        let mut next_offset = 0u64;

        for input in entries {
            if let Some(key) = &input.idempotency_key {
                if let Some(&existing) = self.idempotency_index.get(key) {
                    results.push(existing);
                    prepared_list.push(None);
                    continue;
                }
                if let Some(&prev_idx) = batch_idempotency.get(key) {
                    results.push(results[prev_idx]);
                    prepared_list.push(None);
                    continue;
                }
                batch_idempotency.insert(key.clone(), results.len());
            }

            let prepared = self.prepare_entry(input)?;
            let tx_id = TxId(base_tx + next_offset);
            next_offset += 1;
            results.push(tx_id);
            prepared_list.push(Some(prepared));
        }

        if let Some(wal) = &mut self.wal {
            for (tx_seq, p) in (base_tx..).zip(prepared_list.iter().flatten()) {
                wal.append(&WalRecord::PostEntry {
                    tx_id: TxId(tx_seq),
                    effective_ts_ns: p.effective_ts_ns,
                    description: p.description.clone(),
                    idempotency_key: p.idempotency_key.clone(),
                    lines: p.lines.clone(),
                })?;
            }
            wal.flush()?;
        }

        for (tx_seq, item) in (base_tx..).zip(prepared_list.into_iter().flatten()) {
            self.commit_prepared_in_memory(TxId(tx_seq), item);
        }

        Ok(results)
    }

    fn prepare_entry(&self, input: JournalEntryInput) -> Result<PreparedEntry, LedgerError> {
        if input.lines.is_empty() {
            return Err(LedgerError::EmptyTransaction);
        }

        let first_acct = self
            .accounts
            .get(input.lines[0].account_id.0 as usize)
            .ok_or(LedgerError::UnknownAccount(input.lines[0].account_id.0))?;
        let tx_currency = first_acct.currency;

        let mut total_debit: i128 = 0;
        let mut total_credit: i128 = 0;
        let mut inferred_cf = CashFlowCategory::None;

        // First pass: check balances, currencies, and infer transaction-level cash-flow tag.
        for line in &input.lines {
            if line.debit < 0 || line.credit < 0 {
                return Err(LedgerError::NegativeLineAmount {
                    account_id: line.account_id.0,
                    debit: line.debit,
                    credit: line.credit,
                });
            }
            if line.debit == 0 && line.credit == 0 {
                return Err(LedgerError::ZeroLineAmount {
                    account_id: line.account_id.0,
                });
            }
            if line.debit > 0 && line.credit > 0 {
                return Err(LedgerError::BothSidesNonZero {
                    account_id: line.account_id.0,
                    debit: line.debit,
                    credit: line.credit,
                });
            }

            let acct = self
                .accounts
                .get(line.account_id.0 as usize)
                .ok_or(LedgerError::UnknownAccount(line.account_id.0))?;
            if acct.currency != tx_currency {
                return Err(LedgerError::AccountCurrencyMismatch {
                    account_id: line.account_id.0,
                    expected: tx_currency,
                    actual: acct.currency,
                });
            }

            if inferred_cf == CashFlowCategory::None {
                if let Some(cat) = line.cash_flow_category {
                    if cat != CashFlowCategory::None {
                        inferred_cf = cat;
                    }
                } else if !acct.is_cash_account && acct.cash_flow_category != CashFlowCategory::None
                {
                    inferred_cf = acct.cash_flow_category;
                }
            }

            total_debit += line.debit as i128;
            total_credit += line.credit as i128;
        }

        if total_debit != total_credit {
            return Err(LedgerError::UnbalancedTransaction {
                total_debit,
                total_credit,
            });
        }

        let mut lines = Vec::with_capacity(input.lines.len() + 2);
        let mut entity_nets: Vec<(EntityId, i128)> = Vec::with_capacity(4);

        for line in &input.lines {
            let acct = &self.accounts[line.account_id.0 as usize];
            let cf_cat = if acct.is_cash_account {
                match line.cash_flow_category {
                    Some(c) if c != CashFlowCategory::None => c,
                    _ if acct.cash_flow_category != CashFlowCategory::None => {
                        acct.cash_flow_category
                    }
                    _ if inferred_cf != CashFlowCategory::None => inferred_cf,
                    _ => CashFlowCategory::Operating,
                }
            } else {
                line.cash_flow_category.unwrap_or(acct.cash_flow_category)
            };

            let net = line.debit as i128 - line.credit as i128;
            if let Some((_, existing)) = entity_nets.iter_mut().find(|(e, _)| *e == acct.entity_id)
            {
                *existing += net;
            } else {
                entity_nets.push((acct.entity_id, net));
            }

            lines.push(WalPostLine {
                account_id: acct.id,
                entity_id: acct.entity_id,
                currency: acct.currency,
                cash_flow_cat: cf_cat,
                debit: line.debit,
                credit: line.credit,
            });
        }

        // Automatically inject intercompany Due-From / Due-To lines when spanning multiple entities.
        if entity_nets.len() > 1 && entity_nets.iter().any(|(_, net)| *net != 0) {
            let mut funders: Vec<(EntityId, i128)> = entity_nets
                .iter()
                .filter(|(_, net)| *net < 0)
                .map(|&(e, net)| (e, -net))
                .collect();
            let mut receivers: Vec<(EntityId, i128)> = entity_nets
                .iter()
                .filter(|(_, net)| *net > 0)
                .copied()
                .collect();

            let mut fi = 0;
            let mut ri = 0;
            while fi < funders.len() && ri < receivers.len() {
                let (from_entity, ref mut f_rem) = funders[fi];
                let (to_entity, ref mut r_rem) = receivers[ri];
                let amt_i128 = (*f_rem).min(*r_rem);
                let amt = i64::try_from(amt_i128).map_err(|_| LedgerError::UnbalancedTransaction {
                    total_debit,
                    total_credit,
                })?;

                let rule = self
                    .intercompany
                    .get_rule(from_entity, to_entity)
                    .ok_or(LedgerError::MissingIntercompanyRule {
                        from_entity: from_entity.0,
                        to_entity: to_entity.0,
                    })?;

                let due_from_acct = &self.accounts[rule.due_from_account.0 as usize];
                let due_to_acct = &self.accounts[rule.due_to_account.0 as usize];

                if due_from_acct.currency != tx_currency {
                    return Err(LedgerError::AccountCurrencyMismatch {
                        account_id: due_from_acct.id.0,
                        expected: tx_currency,
                        actual: due_from_acct.currency,
                    });
                }
                if due_to_acct.currency != tx_currency {
                    return Err(LedgerError::AccountCurrencyMismatch {
                        account_id: due_to_acct.id.0,
                        expected: tx_currency,
                        actual: due_to_acct.currency,
                    });
                }

                lines.push(WalPostLine {
                    account_id: due_from_acct.id,
                    entity_id: due_from_acct.entity_id,
                    currency: due_from_acct.currency,
                    cash_flow_cat: CashFlowCategory::None,
                    debit: amt,
                    credit: 0,
                });
                lines.push(WalPostLine {
                    account_id: due_to_acct.id,
                    entity_id: due_to_acct.entity_id,
                    currency: due_to_acct.currency,
                    cash_flow_cat: CashFlowCategory::None,
                    debit: 0,
                    credit: amt,
                });

                *f_rem -= amt_i128;
                *r_rem -= amt_i128;
                if *f_rem == 0 {
                    fi += 1;
                }
                if *r_rem == 0 {
                    ri += 1;
                }
            }
        }

        Ok(PreparedEntry {
            effective_ts_ns: input.effective_ts_ns,
            description: input.description,
            idempotency_key: input.idempotency_key,
            lines,
        })
    }

    fn commit_prepared_in_memory(&mut self, tx_id: TxId, prepared: PreparedEntry) {
        let line_offset = self.line_arena.len();
        let line_len = prepared.lines.len() as u32;
        let ts = prepared.effective_ts_ns.0;

        if let Some(key) = prepared.idempotency_key {
            self.idempotency_index.insert(key, tx_id);
        }

        for line in prepared.lines {
            let acct_idx = line.account_id.0 as usize;
            let acct = &mut self.accounts[acct_idx];
            let d = line.debit as i128;
            let c = line.credit as i128;
            acct.cum_debit += d;
            acct.cum_credit += c;
            acct.tx_count += 1;
            let is_cash = acct.is_cash_account;
            let entity_idx = acct.entity_id.0 as usize;

            push_checkpoint(&mut self.account_checkpoints[acct_idx], ts, tx_id.0, d, c);

            if is_cash {
                let bucket = match line.cash_flow_cat {
                    CashFlowCategory::Investing => 1,
                    CashFlowCategory::Financing => 2,
                    CashFlowCategory::Operating | CashFlowCategory::None => 0,
                };
                push_checkpoint(
                    &mut self.cash_flow_checkpoints[entity_idx][bucket],
                    ts,
                    tx_id.0,
                    d,
                    c,
                );
            }

            self.line_arena.push(JournalLine::new(
                tx_id,
                line.account_id,
                line.entity_id,
                line.currency,
                line.cash_flow_cat,
                line.debit,
                line.credit,
            ));
        }

        self.tx_headers.push(TxHeader {
            tx_id,
            effective_ts_ns: prepared.effective_ts_ns,
            description: prepared.description,
            line_offset,
            line_len,
        });
    }

    /// Registers an exact straight-line amortization/deferral schedule and optionally
    /// books the initial deferral journal entry.
    pub fn create_schedule(&mut self, spec: ScheduleSpec) -> Result<ScheduleId, LedgerError> {
        if spec.entity_id.0 as usize >= self.entities.len() {
            return Err(LedgerError::UnknownEntity(spec.entity_id.0));
        }

        let src_acct = self
            .accounts
            .get(spec.source_account.0 as usize)
            .ok_or(LedgerError::UnknownAccount(spec.source_account.0))?;
        let tgt_acct = self
            .accounts
            .get(spec.target_account.0 as usize)
            .ok_or(LedgerError::UnknownAccount(spec.target_account.0))?;

        if src_acct.entity_id != spec.entity_id || tgt_acct.entity_id != spec.entity_id {
            return Err(LedgerError::ScheduleAccountEntityMismatch {
                entity_id: spec.entity_id.0,
            });
        }
        if src_acct.currency != spec.currency {
            return Err(LedgerError::AccountCurrencyMismatch {
                account_id: src_acct.id.0,
                expected: spec.currency,
                actual: src_acct.currency,
            });
        }
        if tgt_acct.currency != spec.currency {
            return Err(LedgerError::AccountCurrencyMismatch {
                account_id: tgt_acct.id.0,
                expected: spec.currency,
                actual: tgt_acct.currency,
            });
        }

        let installments = AmortizationSchedule::build_installments(&spec)?;

        let initial_tx_id = if let Some(funding_acct_id) = spec.initial_funding_account {
            let funding_acct = self
                .accounts
                .get(funding_acct_id.0 as usize)
                .ok_or(LedgerError::UnknownAccount(funding_acct_id.0))?;
            if funding_acct.entity_id != spec.entity_id {
                return Err(LedgerError::ScheduleAccountEntityMismatch {
                    entity_id: spec.entity_id.0,
                });
            }
            if funding_acct.currency != spec.currency {
                return Err(LedgerError::AccountCurrencyMismatch {
                    account_id: funding_acct.id.0,
                    expected: spec.currency,
                    actual: funding_acct.currency,
                });
            }

            let lines = match spec.kind {
                ScheduleKind::DeferredRevenue => vec![
                    JournalLineInput::debit(funding_acct_id, spec.total_amount),
                    JournalLineInput::credit(spec.source_account, spec.total_amount),
                ],
                ScheduleKind::PrepaidExpense
                | ScheduleKind::Depreciation
                | ScheduleKind::AccruedExpense => vec![
                    JournalLineInput::debit(spec.source_account, spec.total_amount),
                    JournalLineInput::credit(funding_acct_id, spec.total_amount),
                ],
            };

            let tx_id = self.post_entry(JournalEntryInput {
                effective_ts_ns: spec.start_ts_ns,
                description: format!("Initial deferral: {}", spec.description),
                idempotency_key: None,
                lines,
            })?;
            Some(tx_id)
        } else {
            None
        };

        let schedule_id = ScheduleId(self.schedules.len() as u64);

        if let Some(wal) = &mut self.wal {
            wal.append(&WalRecord::CreateSchedule {
                schedule_id,
                kind: spec.kind,
                entity_id: spec.entity_id,
                currency: spec.currency,
                total_amount: spec.total_amount,
                source_account: spec.source_account,
                target_account: spec.target_account,
                initial_funding_account: spec.initial_funding_account,
                start_ts_ns: spec.start_ts_ns,
                period_ns: spec.period_ns,
                num_periods: spec.num_periods,
                initial_tx_id,
                description: spec.description.clone(),
            })?;
            wal.flush()?;
        }

        self.schedules.push(AmortizationSchedule {
            id: schedule_id,
            spec,
            initial_tx_id,
            installments,
        });

        Ok(schedule_id)
    }

    #[inline]
    pub fn get_schedule(&self, id: ScheduleId) -> Option<&AmortizationSchedule> {
        self.schedules.get(id.0 as usize)
    }

    #[inline]
    pub fn schedules(&self) -> &[AmortizationSchedule] {
        &self.schedules
    }

    /// Atomically recognizes all pending schedule installments with `due_ts_ns <= as_of_ts_ns`.
    pub fn recognize_due_schedules(
        &mut self,
        as_of_ts_ns: TimestampNs,
    ) -> Result<Vec<RecognitionResult>, LedgerError> {
        struct PendingItem {
            sched_idx: usize,
            period_index: u32,
            due_ts_ns: TimestampNs,
            amount: i64,
            kind: ScheduleKind,
            source_account: AccountId,
            target_account: AccountId,
            description: String,
        }

        let mut pending = Vec::new();
        for (sched_idx, sched) in self.schedules.iter().enumerate() {
            for inst in &sched.installments {
                if inst.recognized_tx_id.is_none() && inst.due_ts_ns <= as_of_ts_ns {
                    pending.push(PendingItem {
                        sched_idx,
                        period_index: inst.period_index,
                        due_ts_ns: inst.due_ts_ns,
                        amount: inst.amount,
                        kind: sched.spec.kind,
                        source_account: sched.spec.source_account,
                        target_account: sched.spec.target_account,
                        description: sched.spec.description.clone(),
                    });
                }
            }
        }

        let mut recognized = Vec::with_capacity(pending.len());
        for item in pending {
            let lines = match item.kind {
                ScheduleKind::DeferredRevenue => vec![
                    JournalLineInput::debit(item.source_account, item.amount),
                    JournalLineInput::credit(item.target_account, item.amount),
                ],
                ScheduleKind::PrepaidExpense
                | ScheduleKind::AccruedExpense
                | ScheduleKind::Depreciation => vec![
                    JournalLineInput::debit(item.target_account, item.amount),
                    JournalLineInput::credit(item.source_account, item.amount),
                ],
            };

            let tx_id = self.post_entry(JournalEntryInput {
                effective_ts_ns: item.due_ts_ns,
                description: format!(
                    "Schedule #{} period {}: {}",
                    item.sched_idx,
                    item.period_index + 1,
                    item.description
                ),
                idempotency_key: None,
                lines,
            })?;

            let schedule_id = ScheduleId(item.sched_idx as u64);
            if let Some(wal) = &mut self.wal {
                wal.append(&WalRecord::RecognizeScheduleInstallment {
                    schedule_id,
                    period_index: item.period_index,
                    tx_id,
                })?;
                wal.flush()?;
            }

            self.schedules[item.sched_idx].installments[item.period_index as usize]
                .recognized_tx_id = Some(tx_id);

            recognized.push(RecognitionResult {
                schedule_id,
                period_index: item.period_index,
                due_ts_ns: item.due_ts_ns,
                amount: item.amount,
                tx_id,
            });
        }

        Ok(recognized)
    }

    /// `O(1)` direct-indexed current account balance lookup.
    #[inline]
    pub fn current_balance(&self, account_id: AccountId) -> Result<BalanceSnapshot, LedgerError> {
        let acct = self
            .accounts
            .get(account_id.0 as usize)
            .ok_or(LedgerError::UnknownAccount(account_id.0))?;
        Ok(BalanceSnapshot {
            account_id,
            cum_debit: acct.cum_debit,
            cum_credit: acct.cum_credit,
            net_delta: acct.raw_net_balance(),
            normal_balance: acct.normal_balance(),
        })
    }

    /// `O(log K)` point-in-time account balance lookup via binary search over prefix checkpoints.
    #[inline]
    pub fn balance_as_of(
        &self,
        account_id: AccountId,
        as_of_ts_ns: TimestampNs,
    ) -> Result<BalanceSnapshot, LedgerError> {
        let acct = self
            .accounts
            .get(account_id.0 as usize)
            .ok_or(LedgerError::UnknownAccount(account_id.0))?;
        let cps = &self.account_checkpoints[account_id.0 as usize];
        let (cum_debit, cum_credit) = match cps.last() {
            Some(last) if last.effective_ts_ns <= as_of_ts_ns.0 => {
                (acct.cum_debit, acct.cum_credit)
            }
            _ => lookup_prefix_at(cps, as_of_ts_ns.0),
        };
        Ok(BalanceSnapshot {
            account_id,
            cum_debit,
            cum_credit,
            net_delta: cum_debit - cum_credit,
            normal_balance: acct.account_type.normal_balance(cum_debit, cum_credit),
        })
    }

    /// `O(log K)` period activity query implementing the Lean 4 `prefix_diff_telescope` theorem:
    /// `activity(t_start, t_end) = prefix(t_end) - prefix(t_start - 1)`.
    #[inline]
    pub fn activity_between(
        &self,
        account_id: AccountId,
        start_ts_ns: TimestampNs,
        end_ts_ns: TimestampNs,
    ) -> Result<BalanceSnapshot, LedgerError> {
        let acct = self
            .accounts
            .get(account_id.0 as usize)
            .ok_or(LedgerError::UnknownAccount(account_id.0))?;
        if end_ts_ns.0 < start_ts_ns.0 {
            return Ok(BalanceSnapshot {
                account_id,
                cum_debit: 0,
                cum_credit: 0,
                net_delta: 0,
                normal_balance: 0,
            });
        }
        let cps = &self.account_checkpoints[account_id.0 as usize];
        let (end_d, end_c) = lookup_prefix_at(cps, end_ts_ns.0);
        let (start_d, start_c) = if start_ts_ns.0 == 0 {
            (0, 0)
        } else {
            lookup_prefix_at(cps, start_ts_ns.0 - 1)
        };
        let period_d = end_d - start_d;
        let period_c = end_c - start_c;
        Ok(BalanceSnapshot {
            account_id,
            cum_debit: period_d,
            cum_credit: period_c,
            net_delta: period_d - period_c,
            normal_balance: acct.account_type.normal_balance(period_d, period_c),
        })
    }

    /// `O(log K)` per-entity cash-flow movement query by category over `[start_ts_ns, end_ts_ns]`.
    pub(crate) fn cash_flow_activity(
        &self,
        entity_id: Option<EntityId>,
        category: CashFlowCategory,
        start_ts_ns: TimestampNs,
        end_ts_ns: TimestampNs,
    ) -> i128 {
        if end_ts_ns.0 < start_ts_ns.0 {
            return 0;
        }
        let bucket = match category {
            CashFlowCategory::Operating | CashFlowCategory::None => 0,
            CashFlowCategory::Investing => 1,
            CashFlowCategory::Financing => 2,
        };
        let sum_entity = |eid: usize| -> i128 {
            let cps = &self.cash_flow_checkpoints[eid][bucket];
            let (end_d, end_c) = lookup_prefix_at(cps, end_ts_ns.0);
            let (start_d, start_c) = if start_ts_ns.0 == 0 {
                (0, 0)
            } else {
                lookup_prefix_at(cps, start_ts_ns.0 - 1)
            };
            (end_d - start_d) - (end_c - start_c)
        };

        match entity_id {
            Some(eid) if (eid.0 as usize) < self.cash_flow_checkpoints.len() => {
                sum_entity(eid.0 as usize)
            }
            Some(_) => 0,
            None => (0..self.cash_flow_checkpoints.len()).map(sum_entity).sum(),
        }
    }

    #[inline]
    pub fn tx_count(&self) -> usize {
        self.tx_headers.len()
    }

    #[inline]
    pub fn line_count(&self) -> usize {
        self.line_arena.len()
    }

    #[inline]
    pub fn tx_headers(&self) -> &[TxHeader] {
        &self.tx_headers
    }

    #[inline]
    pub fn lines(&self) -> &[JournalLine] {
        &self.line_arena
    }

    /// Verifies the Lean 4 `ledger_conservation` invariant across all accounts: always `0`.
    pub fn global_net_delta(&self) -> i128 {
        self.accounts.iter().map(|a| a.raw_net_balance()).sum()
    }
}

#[inline]
fn lookup_prefix_at(cps: &[PrefixCheckpoint], ts_ns: u64) -> (i128, i128) {
    let idx = cps.partition_point(|cp| cp.effective_ts_ns <= ts_ns);
    if idx == 0 {
        (0, 0)
    } else {
        let cp = cps[idx - 1];
        (cp.cum_debit, cp.cum_credit)
    }
}

#[inline]
fn push_checkpoint(
    cps: &mut Vec<PrefixCheckpoint>,
    ts_ns: u64,
    tx_id: u64,
    delta_debit: i128,
    delta_credit: i128,
) {
    match cps.last() {
        None => {
            cps.push(PrefixCheckpoint {
                effective_ts_ns: ts_ns,
                tx_id,
                cum_debit: delta_debit,
                cum_credit: delta_credit,
            });
        }
        Some(last) if last.effective_ts_ns <= ts_ns => {
            cps.push(PrefixCheckpoint {
                effective_ts_ns: ts_ns,
                tx_id,
                cum_debit: last.cum_debit + delta_debit,
                cum_credit: last.cum_credit + delta_credit,
            });
        }
        Some(_) => {
            let idx = cps.partition_point(|cp| cp.effective_ts_ns <= ts_ns);
            let (prev_d, prev_c) = if idx == 0 {
                (0, 0)
            } else {
                (cps[idx - 1].cum_debit, cps[idx - 1].cum_credit)
            };
            cps.insert(
                idx,
                PrefixCheckpoint {
                    effective_ts_ns: ts_ns,
                    tx_id,
                    cum_debit: prev_d + delta_debit,
                    cum_credit: prev_c + delta_credit,
                },
            );
            for cp in &mut cps[idx + 1..] {
                cp.cum_debit += delta_debit;
                cp.cum_credit += delta_credit;
            }
        }
    }
}

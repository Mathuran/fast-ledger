use fast_ledger_core::{
    AccountId, AccountType, CashFlowCategory, Currency, EntityId, LedgerError, ScheduleId,
    ScheduleKind, TimestampNs, TxId,
};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const WAL_MAGIC: &[u8; 8] = b"FLEDGER1";

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WalRecordType {
    CreateEntity = 1,
    CreateAccount = 2,
    RegisterIntercompanyRule = 3,
    PostEntry = 4,
    CreateSchedule = 5,
    RecognizeScheduleInstallment = 6,
}

impl WalRecordType {
    fn from_u8(v: u8) -> Result<Self, LedgerError> {
        match v {
            1 => Ok(Self::CreateEntity),
            2 => Ok(Self::CreateAccount),
            3 => Ok(Self::RegisterIntercompanyRule),
            4 => Ok(Self::PostEntry),
            5 => Ok(Self::CreateSchedule),
            6 => Ok(Self::RecognizeScheduleInstallment),
            other => Err(LedgerError::WalCorruption(format!(
                "unknown WAL record type: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WalPostLine {
    pub account_id: AccountId,
    pub entity_id: EntityId,
    pub currency: Currency,
    pub cash_flow_cat: CashFlowCategory,
    pub debit: i64,
    pub credit: i64,
}

#[derive(Clone, Debug)]
pub enum WalRecord {
    CreateEntity {
        entity_id: EntityId,
        parent_entity_id: Option<EntityId>,
        currency: Currency,
        code: String,
        name: String,
    },
    CreateAccount {
        account_id: AccountId,
        entity_id: EntityId,
        account_type: AccountType,
        currency: Currency,
        cash_flow_category: CashFlowCategory,
        is_cash_account: bool,
        is_intercompany: bool,
        code: String,
        name: String,
    },
    RegisterIntercompanyRule {
        from_entity: EntityId,
        to_entity: EntityId,
        due_from_account: AccountId,
        due_to_account: AccountId,
    },
    PostEntry {
        tx_id: TxId,
        effective_ts_ns: TimestampNs,
        description: String,
        idempotency_key: Option<String>,
        lines: Vec<WalPostLine>,
    },
    CreateSchedule {
        schedule_id: ScheduleId,
        kind: ScheduleKind,
        entity_id: EntityId,
        currency: Currency,
        total_amount: i64,
        source_account: AccountId,
        target_account: AccountId,
        initial_funding_account: Option<AccountId>,
        start_ts_ns: TimestampNs,
        period_ns: u64,
        num_periods: u32,
        initial_tx_id: Option<TxId>,
        description: String,
    },
    RecognizeScheduleInstallment {
        schedule_id: ScheduleId,
        period_index: u32,
        tx_id: TxId,
    },
}

/// Binary Write-Ahead Log writer with CRC32-verified frames.
pub struct WalWriter {
    writer: BufWriter<File>,
    sync_on_commit: bool,
    scratch: Vec<u8>,
}

impl WalWriter {
    /// Opens (or creates) a WAL file and reads any existing committed records for crash recovery.
    pub fn open_and_read(
        path: impl AsRef<Path>,
        sync_on_commit: bool,
    ) -> Result<(Self, Vec<WalRecord>), LedgerError> {
        let path = path.as_ref();
        let file_exists = path.exists();
        let mut records = Vec::new();

        if file_exists {
            let file = File::open(path)?;
            let len = file.metadata()?.len();
            if len > 0 {
                let mut reader = BufReader::new(file);
                let mut magic = [0u8; 8];
                reader.read_exact(&mut magic).map_err(|e| {
                    LedgerError::WalCorruption(format!("failed reading WAL header: {e}"))
                })?;
                if &magic != WAL_MAGIC {
                    return Err(LedgerError::WalCorruption(
                        "invalid WAL magic header".to_owned(),
                    ));
                }

                loop {
                    let mut len_buf = [0u8; 4];
                    match reader.read_exact(&mut len_buf) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                        Err(e) => return Err(LedgerError::Io(e)),
                    }
                    let frame_len = u32::from_le_bytes(len_buf) as usize;
                    if frame_len == 0 || frame_len > 64 * 1024 * 1024 {
                        return Err(LedgerError::WalCorruption(format!(
                            "invalid WAL frame length: {frame_len}"
                        )));
                    }

                    let mut crc_buf = [0u8; 4];
                    reader.read_exact(&mut crc_buf).map_err(|e| {
                        LedgerError::WalCorruption(format!("truncated WAL CRC32: {e}"))
                    })?;
                    let expected_crc = u32::from_le_bytes(crc_buf);

                    let mut body = vec![0u8; frame_len];
                    reader.read_exact(&mut body).map_err(|e| {
                        LedgerError::WalCorruption(format!("truncated WAL frame body: {e}"))
                    })?;

                    let actual_crc = crc32fast::hash(&body);
                    if actual_crc != expected_crc {
                        return Err(LedgerError::WalCorruption(format!(
                            "CRC32 mismatch: expected {expected_crc:#010x}, got {actual_crc:#010x}"
                        )));
                    }

                    records.push(decode_record(&body)?);
                }
            }
        }

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let current_len = file.metadata()?.len();
        if current_len == 0 {
            file.write_all(WAL_MAGIC)?;
            file.flush()?;
        } else {
            file.seek(SeekFrom::End(0))?;
        }

        Ok((
            Self {
                writer: BufWriter::with_capacity(64 * 1024, file),
                sync_on_commit,
                scratch: Vec::with_capacity(1024),
            },
            records,
        ))
    }

    pub fn append(&mut self, record: &WalRecord) -> Result<(), LedgerError> {
        self.scratch.clear();
        encode_record(record, &mut self.scratch);
        let frame_len = self.scratch.len() as u32;
        let crc = crc32fast::hash(&self.scratch);
        self.writer.write_all(&frame_len.to_le_bytes())?;
        self.writer.write_all(&crc.to_le_bytes())?;
        self.writer.write_all(&self.scratch)?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), LedgerError> {
        self.writer.flush()?;
        if self.sync_on_commit {
            self.writer.get_ref().sync_data()?;
        }
        Ok(())
    }
}

fn account_type_to_u8(t: AccountType) -> u8 {
    match t {
        AccountType::Asset => 0,
        AccountType::Liability => 1,
        AccountType::Equity => 2,
        AccountType::Revenue => 3,
        AccountType::Expense => 4,
        AccountType::ContraAsset => 5,
        AccountType::ContraLiability => 6,
        AccountType::ContraEquity => 7,
    }
}

fn u8_to_account_type(v: u8) -> Result<AccountType, LedgerError> {
    match v {
        0 => Ok(AccountType::Asset),
        1 => Ok(AccountType::Liability),
        2 => Ok(AccountType::Equity),
        3 => Ok(AccountType::Revenue),
        4 => Ok(AccountType::Expense),
        5 => Ok(AccountType::ContraAsset),
        6 => Ok(AccountType::ContraLiability),
        7 => Ok(AccountType::ContraEquity),
        other => Err(LedgerError::WalCorruption(format!(
            "invalid account type byte: {other}"
        ))),
    }
}

fn schedule_kind_to_u8(k: ScheduleKind) -> u8 {
    match k {
        ScheduleKind::DeferredRevenue => 0,
        ScheduleKind::PrepaidExpense => 1,
        ScheduleKind::AccruedExpense => 2,
        ScheduleKind::Depreciation => 3,
    }
}

fn u8_to_schedule_kind(v: u8) -> Result<ScheduleKind, LedgerError> {
    match v {
        0 => Ok(ScheduleKind::DeferredRevenue),
        1 => Ok(ScheduleKind::PrepaidExpense),
        2 => Ok(ScheduleKind::AccruedExpense),
        3 => Ok(ScheduleKind::Depreciation),
        other => Err(LedgerError::WalCorruption(format!(
            "invalid schedule kind byte: {other}"
        ))),
    }
}

fn write_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn encode_record(record: &WalRecord, buf: &mut Vec<u8>) {
    match record {
        WalRecord::CreateEntity {
            entity_id,
            parent_entity_id,
            currency,
            code,
            name,
        } => {
            buf.push(WalRecordType::CreateEntity as u8);
            buf.extend_from_slice(&entity_id.0.to_le_bytes());
            buf.extend_from_slice(&parent_entity_id.map_or(u16::MAX, |p| p.0).to_le_bytes());
            buf.extend_from_slice(&currency.0);
            write_str(buf, code);
            write_str(buf, name);
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
            buf.push(WalRecordType::CreateAccount as u8);
            buf.extend_from_slice(&account_id.0.to_le_bytes());
            buf.extend_from_slice(&entity_id.0.to_le_bytes());
            buf.push(account_type_to_u8(*account_type));
            buf.extend_from_slice(&currency.0);
            buf.push(*cash_flow_category as u8);
            let flags = ((*is_cash_account as u8) & 1) | (((*is_intercompany as u8) & 1) << 1);
            buf.push(flags);
            write_str(buf, code);
            write_str(buf, name);
        }
        WalRecord::RegisterIntercompanyRule {
            from_entity,
            to_entity,
            due_from_account,
            due_to_account,
        } => {
            buf.push(WalRecordType::RegisterIntercompanyRule as u8);
            buf.extend_from_slice(&from_entity.0.to_le_bytes());
            buf.extend_from_slice(&to_entity.0.to_le_bytes());
            buf.extend_from_slice(&due_from_account.0.to_le_bytes());
            buf.extend_from_slice(&due_to_account.0.to_le_bytes());
        }
        WalRecord::PostEntry {
            tx_id,
            effective_ts_ns,
            description,
            idempotency_key,
            lines,
        } => {
            buf.push(WalRecordType::PostEntry as u8);
            buf.extend_from_slice(&tx_id.0.to_le_bytes());
            buf.extend_from_slice(&effective_ts_ns.0.to_le_bytes());
            write_str(buf, description);
            match idempotency_key {
                Some(key) => {
                    buf.push(1);
                    write_str(buf, key);
                }
                None => buf.push(0),
            }
            buf.extend_from_slice(&(lines.len() as u32).to_le_bytes());
            for line in lines {
                buf.extend_from_slice(&line.account_id.0.to_le_bytes());
                buf.extend_from_slice(&line.entity_id.0.to_le_bytes());
                buf.extend_from_slice(&line.currency.0);
                buf.push(line.cash_flow_cat as u8);
                buf.extend_from_slice(&line.debit.to_le_bytes());
                buf.extend_from_slice(&line.credit.to_le_bytes());
            }
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
            buf.push(WalRecordType::CreateSchedule as u8);
            buf.extend_from_slice(&schedule_id.0.to_le_bytes());
            buf.push(schedule_kind_to_u8(*kind));
            buf.extend_from_slice(&entity_id.0.to_le_bytes());
            buf.extend_from_slice(&currency.0);
            buf.extend_from_slice(&total_amount.to_le_bytes());
            buf.extend_from_slice(&source_account.0.to_le_bytes());
            buf.extend_from_slice(&target_account.0.to_le_bytes());
            match initial_funding_account {
                Some(acct) => {
                    buf.push(1);
                    buf.extend_from_slice(&acct.0.to_le_bytes());
                }
                None => buf.push(0),
            }
            buf.extend_from_slice(&start_ts_ns.0.to_le_bytes());
            buf.extend_from_slice(&period_ns.to_le_bytes());
            buf.extend_from_slice(&num_periods.to_le_bytes());
            match initial_tx_id {
                Some(tx) => {
                    buf.push(1);
                    buf.extend_from_slice(&tx.0.to_le_bytes());
                }
                None => buf.push(0),
            }
            write_str(buf, description);
        }
        WalRecord::RecognizeScheduleInstallment {
            schedule_id,
            period_index,
            tx_id,
        } => {
            buf.push(WalRecordType::RecognizeScheduleInstallment as u8);
            buf.extend_from_slice(&schedule_id.0.to_le_bytes());
            buf.extend_from_slice(&period_index.to_le_bytes());
            buf.extend_from_slice(&tx_id.0.to_le_bytes());
        }
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], LedgerError> {
        if self.pos + n > self.data.len() {
            return Err(LedgerError::WalCorruption("unexpected end of WAL frame".to_owned()));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, LedgerError> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, LedgerError> {
        let b: [u8; 2] = self.take(2)?.try_into().unwrap();
        Ok(u16::from_le_bytes(b))
    }

    fn read_u32(&mut self) -> Result<u32, LedgerError> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(u32::from_le_bytes(b))
    }

    fn read_u64(&mut self) -> Result<u64, LedgerError> {
        let b: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(u64::from_le_bytes(b))
    }

    fn read_i64(&mut self) -> Result<i64, LedgerError> {
        let b: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(i64::from_le_bytes(b))
    }

    fn read_currency(&mut self) -> Result<Currency, LedgerError> {
        let b: [u8; 3] = self.take(3)?.try_into().unwrap();
        Currency::from_bytes(b)
    }

    fn read_string(&mut self) -> Result<String, LedgerError> {
        let len = self.read_u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| LedgerError::WalCorruption(format!("invalid UTF-8 in WAL: {e}")))
    }
}

fn decode_record(body: &[u8]) -> Result<WalRecord, LedgerError> {
    let mut c = Cursor::new(body);
    let rec_type = WalRecordType::from_u8(c.read_u8()?)?;
    match rec_type {
        WalRecordType::CreateEntity => {
            let entity_id = EntityId(c.read_u16()?);
            let parent_raw = c.read_u16()?;
            let parent_entity_id = (parent_raw != u16::MAX).then_some(EntityId(parent_raw));
            let currency = c.read_currency()?;
            let code = c.read_string()?;
            let name = c.read_string()?;
            Ok(WalRecord::CreateEntity {
                entity_id,
                parent_entity_id,
                currency,
                code,
                name,
            })
        }
        WalRecordType::CreateAccount => {
            let account_id = AccountId(c.read_u32()?);
            let entity_id = EntityId(c.read_u16()?);
            let account_type = u8_to_account_type(c.read_u8()?)?;
            let currency = c.read_currency()?;
            let cash_flow_category = CashFlowCategory::from_u8(c.read_u8()?);
            let flags = c.read_u8()?;
            let is_cash_account = (flags & 1) != 0;
            let is_intercompany = (flags & 2) != 0;
            let code = c.read_string()?;
            let name = c.read_string()?;
            Ok(WalRecord::CreateAccount {
                account_id,
                entity_id,
                account_type,
                currency,
                cash_flow_category,
                is_cash_account,
                is_intercompany,
                code,
                name,
            })
        }
        WalRecordType::RegisterIntercompanyRule => {
            let from_entity = EntityId(c.read_u16()?);
            let to_entity = EntityId(c.read_u16()?);
            let due_from_account = AccountId(c.read_u32()?);
            let due_to_account = AccountId(c.read_u32()?);
            Ok(WalRecord::RegisterIntercompanyRule {
                from_entity,
                to_entity,
                due_from_account,
                due_to_account,
            })
        }
        WalRecordType::PostEntry => {
            let tx_id = TxId(c.read_u64()?);
            let effective_ts_ns = TimestampNs(c.read_u64()?);
            let description = c.read_string()?;
            let has_idem = c.read_u8()? != 0;
            let idempotency_key = if has_idem {
                Some(c.read_string()?)
            } else {
                None
            };
            let line_count = c.read_u32()? as usize;
            let mut lines = Vec::with_capacity(line_count);
            for _ in 0..line_count {
                let account_id = AccountId(c.read_u32()?);
                let entity_id = EntityId(c.read_u16()?);
                let currency = c.read_currency()?;
                let cash_flow_cat = CashFlowCategory::from_u8(c.read_u8()?);
                let debit = c.read_i64()?;
                let credit = c.read_i64()?;
                lines.push(WalPostLine {
                    account_id,
                    entity_id,
                    currency,
                    cash_flow_cat,
                    debit,
                    credit,
                });
            }
            Ok(WalRecord::PostEntry {
                tx_id,
                effective_ts_ns,
                description,
                idempotency_key,
                lines,
            })
        }
        WalRecordType::CreateSchedule => {
            let schedule_id = ScheduleId(c.read_u64()?);
            let kind = u8_to_schedule_kind(c.read_u8()?)?;
            let entity_id = EntityId(c.read_u16()?);
            let currency = c.read_currency()?;
            let total_amount = c.read_i64()?;
            let source_account = AccountId(c.read_u32()?);
            let target_account = AccountId(c.read_u32()?);
            let initial_funding_account = if c.read_u8()? != 0 {
                Some(AccountId(c.read_u32()?))
            } else {
                None
            };
            let start_ts_ns = TimestampNs(c.read_u64()?);
            let period_ns = c.read_u64()?;
            let num_periods = c.read_u32()?;
            let initial_tx_id = if c.read_u8()? != 0 {
                Some(TxId(c.read_u64()?))
            } else {
                None
            };
            let description = c.read_string()?;
            Ok(WalRecord::CreateSchedule {
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
            })
        }
        WalRecordType::RecognizeScheduleInstallment => {
            let schedule_id = ScheduleId(c.read_u64()?);
            let period_index = c.read_u32()?;
            let tx_id = TxId(c.read_u64()?);
            Ok(WalRecord::RecognizeScheduleInstallment {
                schedule_id,
                period_index,
                tx_id,
            })
        }
    }
}

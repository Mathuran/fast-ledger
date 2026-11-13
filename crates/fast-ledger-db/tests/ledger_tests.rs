use fast_ledger_core::{
    AccountType, CashFlowCategory, CreateAccountInput, CreateEntityInput, Currency,
    IntercompanyRule, JournalEntryInput, JournalLineInput, LedgerError, ScheduleKind, ScheduleSpec,
    TimestampNs,
};
use fast_ledger_db::{
    generate_balance_sheet, generate_cash_flow_statement, generate_consolidated_report,
    generate_income_statement, generate_trial_balance, FxRateTable, LedgerDb,
};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};

#[test]
fn startup_single_entity_full_financial_reports() {
    let mut db = LedgerDb::new();
    let e = db
        .create_entity(CreateEntityInput {
            code: "STARTUP".into(),
            name: "Acme AI Inc.".into(),
            functional_currency: Currency::USD,
            parent_entity_id: None,
        })
        .unwrap();

    let cash = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "1000".into(),
            name: "Operating Cash".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: true,
            is_intercompany: false,
        })
        .unwrap();
    let equip = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "1500".into(),
            name: "GPU Cluster".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Investing,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let accum_depr = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "1590".into(),
            name: "Accumulated Depreciation".into(),
            account_type: AccountType::ContraAsset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::None,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let equity = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "3000".into(),
            name: "Common Stock".into(),
            account_type: AccountType::Equity,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Financing,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let rev = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "4000".into(),
            name: "Product Revenue".into(),
            account_type: AccountType::Revenue,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let cloud_exp = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "5000".into(),
            name: "Cloud Costs".into(),
            account_type: AccountType::Expense,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let depr_exp = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "5200".into(),
            name: "Depreciation Expense".into(),
            account_type: AccountType::Expense,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::None,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();

    // T=10: Seed financing 500_000
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(10),
        description: "Seed round".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(cash, 500_000).with_cash_flow(CashFlowCategory::Financing),
            JournalLineInput::credit(equity, 500_000),
        ],
    })
    .unwrap();

    // T=20: CapEx GPU purchase 120_000 (cash flow category inferred from equip = Investing)
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(20),
        description: "Buy GPU cluster".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(equip, 120_000),
            JournalLineInput::credit(cash, 120_000).with_cash_flow(CashFlowCategory::Investing),
        ],
    })
    .unwrap();

    // T=30: Operating revenue 80_000
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(30),
        description: "Customer invoice paid".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(cash, 80_000),
            JournalLineInput::credit(rev, 80_000),
        ],
    })
    .unwrap();

    // T=40: Operating cloud expense 25_000
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(40),
        description: "Cloud bill".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(cloud_exp, 25_000),
            JournalLineInput::credit(cash, 25_000),
        ],
    })
    .unwrap();

    // T=50: Non-cash depreciation 10_000
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(50),
        description: "Monthly depreciation".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(depr_exp, 10_000),
            JournalLineInput::credit(accum_depr, 10_000),
        ],
    })
    .unwrap();

    assert_eq!(db.global_net_delta(), 0);

    let tb = generate_trial_balance(&db, Some(e), TimestampNs(50)).unwrap();
    assert!(tb.is_balanced);
    assert_eq!(tb.total_debits, tb.total_credits);

    let bs = generate_balance_sheet(&db, Some(e), TimestampNs(50)).unwrap();
    assert!(bs.is_balanced);
    // Cash = 500_000 - 120_000 + 80_000 - 25_000 = 435_000
    // Net PPE = 120_000 - 10_000 = 110_000
    // Total Assets = 545_000
    assert_eq!(bs.total_assets, 545_000);
    assert_eq!(bs.total_liabilities, 0);
    // Equity = 500_000 + Net Income (80_000 - 35_000 = 45_000) = 545_000
    assert_eq!(bs.current_period_net_income, 45_000);
    assert_eq!(bs.total_equity_including_net_income, 545_000);

    let is = generate_income_statement(&db, Some(e), TimestampNs(25), TimestampNs(50)).unwrap();
    assert_eq!(is.total_revenue, 80_000);
    assert_eq!(is.total_expenses, 35_000);
    assert_eq!(is.net_income, 45_000);

    let cf = generate_cash_flow_statement(&db, Some(e), TimestampNs(1), TimestampNs(50)).unwrap();
    assert!(cf.is_reconciled);
    assert_eq!(cf.beginning_cash, 0);
    assert_eq!(cf.operating_cash_flow, 55_000);
    assert_eq!(cf.investing_cash_flow, -120_000);
    assert_eq!(cf.financing_cash_flow, 500_000);
    assert_eq!(cf.ending_cash, 435_000);
}

#[test]
fn prefix_sum_point_in_time_and_backdated_entries() {
    let mut db = LedgerDb::new();
    let e = db
        .create_entity(CreateEntityInput {
            code: "ENT".into(),
            name: "Temporal Test".into(),
            functional_currency: Currency::USD,
            parent_entity_id: None,
        })
        .unwrap();
    let cash = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "1000".into(),
            name: "Cash".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: true,
            is_intercompany: false,
        })
        .unwrap();
    let rev = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "4000".into(),
            name: "Revenue".into(),
            account_type: AccountType::Revenue,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();

    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(100),
        description: "t=100".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(cash, 1_000),
            JournalLineInput::credit(rev, 1_000),
        ],
    })
    .unwrap();
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(300),
        description: "t=300".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(cash, 3_000),
            JournalLineInput::credit(rev, 3_000),
        ],
    })
    .unwrap();
    // Backdated entry at t=200
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(200),
        description: "backdated t=200".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(cash, 2_000),
            JournalLineInput::credit(rev, 2_000),
        ],
    })
    .unwrap();

    assert_eq!(db.balance_as_of(cash, TimestampNs(50)).unwrap().normal_balance, 0);
    assert_eq!(db.balance_as_of(cash, TimestampNs(150)).unwrap().normal_balance, 1_000);
    assert_eq!(db.balance_as_of(cash, TimestampNs(200)).unwrap().normal_balance, 3_000);
    assert_eq!(db.balance_as_of(cash, TimestampNs(250)).unwrap().normal_balance, 3_000);
    assert_eq!(db.balance_as_of(cash, TimestampNs(300)).unwrap().normal_balance, 6_000);
    assert_eq!(
        db.activity_between(cash, TimestampNs(150), TimestampNs(250))
            .unwrap()
            .normal_balance,
        2_000
    );
    assert_eq!(db.current_balance(cash).unwrap().normal_balance, 6_000);
}

#[test]
fn exact_amortization_deferral_and_prepaid_schedules() {
    let mut db = LedgerDb::new();
    let e = db
        .create_entity(CreateEntityInput {
            code: "SAAS".into(),
            name: "SaaS Startup".into(),
            functional_currency: Currency::USD,
            parent_entity_id: None,
        })
        .unwrap();

    let cash = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "1000".into(),
            name: "Cash".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: true,
            is_intercompany: false,
        })
        .unwrap();
    let prepaid = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "1300".into(),
            name: "Prepaid".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let def_rev = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "2400".into(),
            name: "Deferred Revenue".into(),
            account_type: AccountType::Liability,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let rev = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "4000".into(),
            name: "Revenue".into(),
            account_type: AccountType::Revenue,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let exp = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "5000".into(),
            name: "Expense".into(),
            account_type: AccountType::Expense,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();

    let def_id = db
        .create_schedule(ScheduleSpec {
            kind: ScheduleKind::DeferredRevenue,
            entity_id: e,
            currency: Currency::USD,
            total_amount: 1_000_000,
            source_account: def_rev,
            target_account: rev,
            initial_funding_account: Some(cash),
            start_ts_ns: TimestampNs(100),
            period_ns: 100,
            num_periods: 12,
            description: "12-month SaaS".into(),
        })
        .unwrap();

    let prep_id = db
        .create_schedule(ScheduleSpec {
            kind: ScheduleKind::PrepaidExpense,
            entity_id: e,
            currency: Currency::USD,
            total_amount: 10_000,
            source_account: prepaid,
            target_account: exp,
            initial_funding_account: Some(cash),
            start_ts_ns: TimestampNs(100),
            period_ns: 100,
            num_periods: 7,
            description: "7-period prepaid".into(),
        })
        .unwrap();

    let def_sched = db.get_schedule(def_id).unwrap();
    assert_eq!(def_sched.installments[0].amount, 83_333);
    assert_eq!(def_sched.installments[11].amount, 83_337);
    assert_eq!(
        def_sched.installments.iter().map(|i| i.amount).sum::<i64>(),
        1_000_000
    );

    let prep_sched = db.get_schedule(prep_id).unwrap();
    assert_eq!(prep_sched.installments[0].amount, 1_428);
    assert_eq!(prep_sched.installments[6].amount, 1_432);
    assert_eq!(
        prep_sched.installments.iter().map(|i| i.amount).sum::<i64>(),
        10_000
    );

    // Recognize up to t=600 (6 deferred revenue periods + 6 prepaid periods)
    let r1 = db.recognize_due_schedules(TimestampNs(600)).unwrap();
    assert_eq!(r1.len(), 12);
    assert_eq!(db.current_balance(rev).unwrap().normal_balance, 6 * 83_333);
    assert_eq!(
        db.current_balance(def_rev).unwrap().normal_balance,
        1_000_000 - 6 * 83_333
    );

    // Recognize remaining periods through t=1200
    let r2 = db.recognize_due_schedules(TimestampNs(1200)).unwrap();
    assert_eq!(r2.len(), 7); // 6 remaining SaaS + 1 remaining prepaid
    assert_eq!(db.current_balance(rev).unwrap().normal_balance, 1_000_000);
    assert_eq!(db.current_balance(def_rev).unwrap().normal_balance, 0);
    assert_eq!(db.current_balance(exp).unwrap().normal_balance, 10_000);
    assert_eq!(db.current_balance(prepaid).unwrap().normal_balance, 0);
    assert_eq!(db.global_net_delta(), 0);
}

#[test]
fn multi_entity_intercompany_and_consolidated_fx_elimination() {
    let mut db = LedgerDb::new();
    let us = db
        .create_entity(CreateEntityInput {
            code: "US".into(),
            name: "US Parent".into(),
            functional_currency: Currency::USD,
            parent_entity_id: None,
        })
        .unwrap();
    let eu = db
        .create_entity(CreateEntityInput {
            code: "EU".into(),
            name: "EU Sub".into(),
            functional_currency: Currency::EUR,
            parent_entity_id: Some(us),
        })
        .unwrap();

    let us_cash = db
        .create_account(CreateAccountInput {
            entity_id: us,
            code: "1000".into(),
            name: "US Cash".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: true,
            is_intercompany: false,
        })
        .unwrap();
    let us_equity = db
        .create_account(CreateAccountInput {
            entity_id: us,
            code: "3000".into(),
            name: "US Equity".into(),
            account_type: AccountType::Equity,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Financing,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let us_due_from_eu = db
        .create_account(CreateAccountInput {
            entity_id: us,
            code: "1900".into(),
            name: "Due From EU".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::None,
            is_cash_account: false,
            is_intercompany: true,
        })
        .unwrap();

    let eu_due_to_us = db
        .create_account(CreateAccountInput {
            entity_id: eu,
            code: "2900".into(),
            name: "Due To US".into(),
            account_type: AccountType::Liability,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::None,
            is_cash_account: false,
            is_intercompany: true,
        })
        .unwrap();
    let eu_exp_usd = db
        .create_account(CreateAccountInput {
            entity_id: eu,
            code: "5000".into(),
            name: "EU Vendor Expense".into(),
            account_type: AccountType::Expense,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();
    let eu_cash_eur = db
        .create_account(CreateAccountInput {
            entity_id: eu,
            code: "1010".into(),
            name: "EU Bank EUR".into(),
            account_type: AccountType::Asset,
            currency: Currency::EUR,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: true,
            is_intercompany: false,
        })
        .unwrap();
    let eu_rev_eur = db
        .create_account(CreateAccountInput {
            entity_id: eu,
            code: "4000".into(),
            name: "EU Sales EUR".into(),
            account_type: AccountType::Revenue,
            currency: Currency::EUR,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();

    db.register_intercompany_rule(IntercompanyRule {
        from_entity: us,
        to_entity: eu,
        due_from_account: us_due_from_eu,
        due_to_account: eu_due_to_us,
    })
    .unwrap();

    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(10),
        description: "US funding".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(us_cash, 100_000),
            JournalLineInput::credit(us_equity, 100_000),
        ],
    })
    .unwrap();

    // Cross-entity payment triggers automatic Due From / Due To balancing lines
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(20),
        description: "US pays EU vendor".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(eu_exp_usd, 15_000),
            JournalLineInput::credit(us_cash, 15_000),
        ],
    })
    .unwrap();

    // EU local EUR revenue
    db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(30),
        description: "EU EUR sale".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(eu_cash_eur, 20_000),
            JournalLineInput::credit(eu_rev_eur, 20_000),
        ],
    })
    .unwrap();

    // Each standalone entity balance sheet is independently balanced!
    assert!(generate_balance_sheet(&db, Some(us), TimestampNs(30))
        .unwrap()
        .is_balanced);
    assert!(generate_balance_sheet(&db, Some(eu), TimestampNs(30))
        .unwrap()
        .is_balanced);

    let mut fx = FxRateTable::new();
    fx.set_rate(Currency::EUR, Currency::USD, 1_100_000).unwrap();

    let consolidated = generate_consolidated_report(
        &db,
        Currency::USD,
        TimestampNs(30),
        TimestampNs(0),
        TimestampNs(30),
        &fx,
    )
    .unwrap();

    assert_eq!(consolidated.intercompany_elimination_residual, 0);
    assert_eq!(consolidated.eliminated_intercompany.len(), 2);
    // US Cash (85_000) + EU Cash (20_000 EUR * 1.10 = 22_000 USD) = 107_000 USD
    assert_eq!(consolidated.total_assets, 107_000);
    assert_eq!(consolidated.total_revenue, 22_000);
    assert_eq!(consolidated.total_expenses, 15_000);
    assert_eq!(consolidated.net_income, 7_000);
    assert_eq!(consolidated.fx_translation_adjustment, 0);
}

#[test]
fn wal_durability_crash_recovery_and_corruption_detection() {
    let wal_path = std::env::temp_dir().join(format!(
        "fast_ledger_test_{}_{}.wal",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    {
        let mut db = LedgerDb::open_with_wal(&wal_path, false).unwrap();
        let e = db
            .create_entity(CreateEntityInput {
                code: "WAL".into(),
                name: "WAL Entity".into(),
                functional_currency: Currency::USD,
                parent_entity_id: None,
            })
            .unwrap();
        let cash = db
            .create_account(CreateAccountInput {
                entity_id: e,
                code: "1000".into(),
                name: "Cash".into(),
                account_type: AccountType::Asset,
                currency: Currency::USD,
                cash_flow_category: CashFlowCategory::Operating,
                is_cash_account: true,
                is_intercompany: false,
            })
            .unwrap();
        let def_rev = db
            .create_account(CreateAccountInput {
                entity_id: e,
                code: "2400".into(),
                name: "Deferred".into(),
                account_type: AccountType::Liability,
                currency: Currency::USD,
                cash_flow_category: CashFlowCategory::Operating,
                is_cash_account: false,
                is_intercompany: false,
            })
            .unwrap();
        let rev = db
            .create_account(CreateAccountInput {
                entity_id: e,
                code: "4000".into(),
                name: "Revenue".into(),
                account_type: AccountType::Revenue,
                currency: Currency::USD,
                cash_flow_category: CashFlowCategory::Operating,
                is_cash_account: false,
                is_intercompany: false,
            })
            .unwrap();

        db.create_schedule(ScheduleSpec {
            kind: ScheduleKind::DeferredRevenue,
            entity_id: e,
            currency: Currency::USD,
            total_amount: 3_000,
            source_account: def_rev,
            target_account: rev,
            initial_funding_account: Some(cash),
            start_ts_ns: TimestampNs(100),
            period_ns: 100,
            num_periods: 3,
            description: "WAL schedule".into(),
        })
        .unwrap();
        db.recognize_due_schedules(TimestampNs(150)).unwrap();
    }

    // Reopen and verify recovered state
    {
        let recovered = LedgerDb::open_with_wal(&wal_path, false).unwrap();
        assert_eq!(recovered.entities().len(), 1);
        assert_eq!(recovered.accounts().len(), 3);
        assert_eq!(recovered.tx_count(), 2); // initial deferral + 1st recognition
        assert_eq!(recovered.schedules().len(), 1);
        assert!(recovered.schedules()[0].installments[0].recognized_tx_id.is_some());
        assert!(recovered.schedules()[0].installments[1].recognized_tx_id.is_none());
        assert_eq!(recovered.global_net_delta(), 0);
    }

    // Corrupt a byte inside the WAL payload and verify CRC32 detects corruption
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&wal_path)
            .unwrap();
        f.seek(SeekFrom::Start(20)).unwrap();
        let mut byte = [0u8; 1];
        f.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xFF;
        f.seek(SeekFrom::Start(20)).unwrap();
        f.write_all(&byte).unwrap();
        f.flush().unwrap();
    }

    let err = LedgerDb::open_with_wal(&wal_path, false)
        .err()
        .expect("corrupted WAL should fail CRC check");
    assert!(matches!(err, LedgerError::WalCorruption(_)));

    let _ = std::fs::remove_file(&wal_path);
}

#[test]
fn validation_and_idempotency() {
    let mut db = LedgerDb::new();
    let e = db
        .create_entity(CreateEntityInput {
            code: "VAL".into(),
            name: "Validation".into(),
            functional_currency: Currency::USD,
            parent_entity_id: None,
        })
        .unwrap();
    let cash = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "1000".into(),
            name: "Cash".into(),
            account_type: AccountType::Asset,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: true,
            is_intercompany: false,
        })
        .unwrap();
    let rev = db
        .create_account(CreateAccountInput {
            entity_id: e,
            code: "4000".into(),
            name: "Rev".into(),
            account_type: AccountType::Revenue,
            currency: Currency::USD,
            cash_flow_category: CashFlowCategory::Operating,
            is_cash_account: false,
            is_intercompany: false,
        })
        .unwrap();

    // Unbalanced rejected
    let res = db.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(1),
        description: "unbalanced".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(cash, 100),
            JournalLineInput::credit(rev, 99),
        ],
    });
    assert!(matches!(res, Err(LedgerError::UnbalancedTransaction { .. })));

    // Idempotency deduplication
    let tx1 = db
        .post_entry(JournalEntryInput {
            effective_ts_ns: TimestampNs(2),
            description: "idempotent".into(),
            idempotency_key: Some("req-abc".into()),
            lines: vec![
                JournalLineInput::debit(cash, 500),
                JournalLineInput::credit(rev, 500),
            ],
        })
        .unwrap();
    let tx2 = db
        .post_entry(JournalEntryInput {
            effective_ts_ns: TimestampNs(2),
            description: "idempotent retry".into(),
            idempotency_key: Some("req-abc".into()),
            lines: vec![
                JournalLineInput::debit(cash, 500),
                JournalLineInput::credit(rev, 500),
            ],
        })
        .unwrap();
    assert_eq!(tx1, tx2);
    assert_eq!(db.tx_count(), 1);
    assert_eq!(db.current_balance(cash).unwrap().normal_balance, 500);
}

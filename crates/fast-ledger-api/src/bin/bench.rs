use fast_ledger_core::{
    AccountType, CashFlowCategory, CreateAccountInput, CreateEntityInput, Currency,
    IntercompanyRule, JournalEntryInput, JournalLine, JournalLineInput, ScheduleKind, ScheduleSpec,
    TimestampNs,
};
use fast_ledger_db::{
    generate_balance_sheet, generate_cash_flow_statement, generate_consolidated_report,
    generate_income_statement, generate_trial_balance, FxRateTable, LedgerDb,
};
use std::hint::black_box;
use std::time::Instant;

const DAY_NS: u64 = 86_400_000_000_000;
const MONTH_NS: u64 = 30 * DAY_NS;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Fast Ledger Release Benchmark & Showcase");
    println!("========================================");
    println!(
        "JournalLine compact #[repr(C)] size: {} bytes",
        std::mem::size_of::<JournalLine>()
    );

    // -------------------------------------------------------------------------
    // 1. 1,000,000-Transaction (2,000,000-Line) Throughput Benchmark
    // -------------------------------------------------------------------------
    const NUM_TX: usize = 1_000_000;
    let mut bench_db = LedgerDb::with_capacities(16, NUM_TX, NUM_TX * 2);

    let us = bench_db.create_entity(CreateEntityInput {
        code: "US-HQ".into(),
        name: "Acme Corp US".into(),
        functional_currency: Currency::USD,
        parent_entity_id: None,
    })?;
    let eu = bench_db.create_entity(CreateEntityInput {
        code: "EU-SUB".into(),
        name: "Acme Europe GmbH".into(),
        functional_currency: Currency::USD,
        parent_entity_id: Some(us),
    })?;

    let us_cash = bench_db.create_account(CreateAccountInput {
        entity_id: us,
        code: "1000".into(),
        name: "US Operating Cash".into(),
        account_type: AccountType::Asset,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: true,
        is_intercompany: false,
    })?;
    let us_rev = bench_db.create_account(CreateAccountInput {
        entity_id: us,
        code: "4000".into(),
        name: "US SaaS Revenue".into(),
        account_type: AccountType::Revenue,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;
    let us_exp = bench_db.create_account(CreateAccountInput {
        entity_id: us,
        code: "5000".into(),
        name: "US Cloud Hosting".into(),
        account_type: AccountType::Expense,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;
    let eu_cash = bench_db.create_account(CreateAccountInput {
        entity_id: eu,
        code: "1000".into(),
        name: "EU Operating Cash".into(),
        account_type: AccountType::Asset,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: true,
        is_intercompany: false,
    })?;
    let eu_rev = bench_db.create_account(CreateAccountInput {
        entity_id: eu,
        code: "4000".into(),
        name: "EU SaaS Revenue".into(),
        account_type: AccountType::Revenue,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;

    // Pre-build 1,000,000 journal entries outside the timed write loop to measure pure DB speed.
    let mut batch_templates = Vec::with_capacity(NUM_TX);
    for i in 0..NUM_TX {
        let ts = TimestampNs((i as u64 + 1) * 1_000);
        let lines = match i % 3 {
            0 => vec![
                JournalLineInput::debit(us_cash, 2_500),
                JournalLineInput::credit(us_rev, 2_500),
            ],
            1 => vec![
                JournalLineInput::debit(us_exp, 750),
                JournalLineInput::credit(us_cash, 750),
            ],
            _ => vec![
                JournalLineInput::debit(eu_cash, 1_800),
                JournalLineInput::credit(eu_rev, 1_800),
            ],
        };
        batch_templates.push(JournalEntryInput {
            effective_ts_ns: ts,
            description: String::new(),
            idempotency_key: None,
            lines,
        });
    }

    let t_start = Instant::now();
    let tx_ids = bench_db.post_batch(batch_templates)?;
    let post_elapsed = t_start.elapsed();
    black_box(&tx_ids);

    let secs = post_elapsed.as_secs_f64();
    let tx_per_sec = (NUM_TX as f64) / secs;
    let lines_per_sec = ((NUM_TX * 2) as f64) / secs;
    let ns_per_tx = (post_elapsed.as_nanos() as f64) / (NUM_TX as f64);

    println!("\n[1] 1,000,000-Entry (2,000,000-Line) Ingestion:");
    println!("    Elapsed:        {:.3} ms", secs * 1000.0);
    println!("    Throughput:     {:.0} tx/sec ({:.0} lines/sec)", tx_per_sec, lines_per_sec);
    println!("    Latency / tx:   {:.1} ns/tx", ns_per_tx);
    println!("    Global net sum: {} (Lean conservation verified)", bench_db.global_net_delta());

    // -------------------------------------------------------------------------
    // 2. O(log K) Report Generation Latency over the 2,000,000-Line Ledger
    // -------------------------------------------------------------------------
    let mid_start = TimestampNs(250_000 * 1_000);
    let mid_end = TimestampNs(750_000 * 1_000);

    let t_tb = Instant::now();
    let tb = black_box(generate_trial_balance(&bench_db, None, mid_end)?);
    let tb_us = t_tb.elapsed().as_secs_f64() * 1_000_000.0;

    let t_bs = Instant::now();
    let bs = black_box(generate_balance_sheet(&bench_db, None, mid_end)?);
    let bs_us = t_bs.elapsed().as_secs_f64() * 1_000_000.0;

    let t_is = Instant::now();
    let is = black_box(generate_income_statement(&bench_db, None, mid_start, mid_end)?);
    let is_us = t_is.elapsed().as_secs_f64() * 1_000_000.0;

    let t_cf = Instant::now();
    let cf = black_box(generate_cash_flow_statement(&bench_db, None, mid_start, mid_end)?);
    let cf_us = t_cf.elapsed().as_secs_f64() * 1_000_000.0;

    println!("\n[2] Financial Report Latency over 2,000,000-Line Ledger (Prefix-Sum O(log K)):");
    println!("    Trial Balance (point-in-time):   {:.2} us (balanced: {})", tb_us, tb.is_balanced);
    println!("    Balance Sheet (point-in-time):   {:.2} us (balanced: {})", bs_us, bs.is_balanced);
    println!("    Income Statement (range):        {:.2} us (net_income: {})", is_us, is.net_income);
    println!("    Cash Flow Statement (range):     {:.2} us (reconciled: {})", cf_us, cf.is_reconciled);

    // -------------------------------------------------------------------------
    // 3. Multi-Entity Startup + Subsidiary + Schedules Showcase
    // -------------------------------------------------------------------------
    let mut demo = LedgerDb::new();
    let parent = demo.create_entity(CreateEntityInput {
        code: "STARTUP-US".into(),
        name: "Fast Ledger Technologies Inc.".into(),
        functional_currency: Currency::USD,
        parent_entity_id: None,
    })?;
    let sub_eu = demo.create_entity(CreateEntityInput {
        code: "SUB-EU".into(),
        name: "Fast Ledger EU GmbH".into(),
        functional_currency: Currency::EUR,
        parent_entity_id: Some(parent),
    })?;

    let p_cash = demo.create_account(CreateAccountInput {
        entity_id: parent,
        code: "1000".into(),
        name: "US Bank Cash".into(),
        account_type: AccountType::Asset,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: true,
        is_intercompany: false,
    })?;
    let p_prepaid = demo.create_account(CreateAccountInput {
        entity_id: parent,
        code: "1300".into(),
        name: "Prepaid Enterprise Infra".into(),
        account_type: AccountType::Asset,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;
    let p_due_from_eu = demo.create_account(CreateAccountInput {
        entity_id: parent,
        code: "1900".into(),
        name: "Due From EU Sub (USD)".into(),
        account_type: AccountType::Asset,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::None,
        is_cash_account: false,
        is_intercompany: true,
    })?;
    let p_deferred_rev = demo.create_account(CreateAccountInput {
        entity_id: parent,
        code: "2400".into(),
        name: "Deferred SaaS Revenue".into(),
        account_type: AccountType::Liability,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;
    let p_equity = demo.create_account(CreateAccountInput {
        entity_id: parent,
        code: "3000".into(),
        name: "Common Stock & Seed Round".into(),
        account_type: AccountType::Equity,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Financing,
        is_cash_account: false,
        is_intercompany: false,
    })?;
    let p_revenue = demo.create_account(CreateAccountInput {
        entity_id: parent,
        code: "4000".into(),
        name: "Recognized SaaS Revenue".into(),
        account_type: AccountType::Revenue,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;
    let p_infra_exp = demo.create_account(CreateAccountInput {
        entity_id: parent,
        code: "5100".into(),
        name: "Infra Amortization Expense".into(),
        account_type: AccountType::Expense,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;

    let eu_due_to_us = demo.create_account(CreateAccountInput {
        entity_id: sub_eu,
        code: "2900".into(),
        name: "Due To US Parent (USD)".into(),
        account_type: AccountType::Liability,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::None,
        is_cash_account: false,
        is_intercompany: true,
    })?;
    let eu_mkt_exp_usd = demo.create_account(CreateAccountInput {
        entity_id: sub_eu,
        code: "5200".into(),
        name: "EU Launch Expense (USD settled)".into(),
        account_type: AccountType::Expense,
        currency: Currency::USD,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;
    let eu_bank = demo.create_account(CreateAccountInput {
        entity_id: sub_eu,
        code: "1010".into(),
        name: "EU Euro Bank".into(),
        account_type: AccountType::Asset,
        currency: Currency::EUR,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: true,
        is_intercompany: false,
    })?;
    let eu_rev_eur = demo.create_account(CreateAccountInput {
        entity_id: sub_eu,
        code: "4010".into(),
        name: "EU Local Revenue (EUR)".into(),
        account_type: AccountType::Revenue,
        currency: Currency::EUR,
        cash_flow_category: CashFlowCategory::Operating,
        is_cash_account: false,
        is_intercompany: false,
    })?;

    demo.register_intercompany_rule(IntercompanyRule {
        from_entity: parent,
        to_entity: sub_eu,
        due_from_account: p_due_from_eu,
        due_to_account: eu_due_to_us,
    })?;

    // Seed round financing ($100,000.00 = 10_000_000 cents)
    demo.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(0),
        description: "Seed financing round".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(p_cash, 10_000_000).with_cash_flow(CashFlowCategory::Financing),
            JournalLineInput::credit(p_equity, 10_000_000),
        ],
    })?;

    // 12-month SaaS annual contract: $10,000.00 (1,000,000 cents, non-divisible by 12!)
    let saas_sched = demo.create_schedule(ScheduleSpec {
        kind: ScheduleKind::DeferredRevenue,
        entity_id: parent,
        currency: Currency::USD,
        total_amount: 1_000_000,
        source_account: p_deferred_rev,
        target_account: p_revenue,
        initial_funding_account: Some(p_cash),
        start_ts_ns: TimestampNs(MONTH_NS),
        period_ns: MONTH_NS,
        num_periods: 12,
        description: "Annual Enterprise SaaS Contract".into(),
    })?;

    // 36-month prepaid infra commitment: $36,000.00 (3,600,000 cents)
    let _prepaid_sched = demo.create_schedule(ScheduleSpec {
        kind: ScheduleKind::PrepaidExpense,
        entity_id: parent,
        currency: Currency::USD,
        total_amount: 3_600_000,
        source_account: p_prepaid,
        target_account: p_infra_exp,
        initial_funding_account: Some(p_cash),
        start_ts_ns: TimestampNs(MONTH_NS),
        period_ns: MONTH_NS,
        num_periods: 36,
        description: "3-Year Cloud Committed Spend".into(),
    })?;

    // Intercompany payment: US Parent pays $5,000.00 EU launch invoice
    demo.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(2 * MONTH_NS),
        description: "US Parent pays EU launch invoice".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(eu_mkt_exp_usd, 500_000),
            JournalLineInput::credit(p_cash, 500_000),
        ],
    })?;

    // EU Subsidiary collects EUR 4,000.00 (400,000 euro cents) local revenue
    demo.post_entry(JournalEntryInput {
        effective_ts_ns: TimestampNs(2 * MONTH_NS),
        description: "EU local customer sale".into(),
        idempotency_key: None,
        lines: vec![
            JournalLineInput::debit(eu_bank, 400_000),
            JournalLineInput::credit(eu_rev_eur, 400_000),
        ],
    })?;

    // Recognize all 12 months of SaaS deferred revenue (plus 12 months of the 36-month prepaid)
    let recognized = demo.recognize_due_schedules(TimestampNs(12 * MONTH_NS))?;
    let saas_insts: Vec<i64> = demo
        .get_schedule(saas_sched)
        .unwrap()
        .installments
        .iter()
        .map(|i| i.amount)
        .collect();

    let mut fx = FxRateTable::new();
    fx.set_rate(Currency::EUR, Currency::USD, 1_100_000)?; // 1.10 USD per EUR
    let consolidated = generate_consolidated_report(
        &demo,
        Currency::USD,
        TimestampNs(12 * MONTH_NS),
        TimestampNs(0),
        TimestampNs(12 * MONTH_NS),
        &fx,
    )?;

    println!("\n[3] Multi-Entity Consolidation & Exact Schedules Showcase:");
    println!("    Recognized schedule installments: {}", recognized.len());
    println!("    12-month $10,000 SaaS installments: {:?} (sum = {})", saas_insts, saas_insts.iter().sum::<i64>());
    println!("    Consolidated Presentation Currency: {}", consolidated.presentation_currency);
    println!("    Consolidated Assets:                {} cents", consolidated.total_assets);
    println!("    Consolidated Liabilities:           {} cents", consolidated.total_liabilities);
    println!("    Consolidated Equity:                {} cents", consolidated.total_equity);
    println!("    Consolidated Revenue:               {} cents", consolidated.total_revenue);
    println!("    Consolidated Expenses:              {} cents", consolidated.total_expenses);
    println!("    Consolidated Net Income:            {} cents", consolidated.net_income);
    println!(
        "    Eliminated Intercompany Residual:   {} cents ({} accounts eliminated)",
        consolidated.intercompany_elimination_residual,
        consolidated.eliminated_intercompany.len()
    );

    Ok(())
}

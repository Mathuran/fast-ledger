# Fast Ledger

**Fast Ledger** is an ultra-high-performance, double-entry accounting ledger service and custom storage engine built for global financial standards (GAAP / IFRS). It scales from a single-entity startup to a multi-entity, multi-currency organization with automatic intercompany balancing and eliminations, exact amortization/deferral schedules, and microsecond-latency financial reporting.

Core ledger invariants, prefix-sum temporal indexing optimizations, and exact remainder-conserving amortization mathematics are **formally verified in Lean 4** ([spec/](spec/)) and implemented in **Rust** ([crates/](crates/)).

## Observed Benchmark Results (`bench` in `--release`)

Measured on Apple Silicon (`arm64`) with **1,000,000 balanced journal entries (2,000,000 journal lines)**:

| Operation | Measured Result |
| :--- | :--- |
| **1,000,000-Entry Ingestion** (`2,000,000` lines) | **140.187 ms** (**7,133,338 tx/sec**, **14,266,675 lines/sec**, **140.2 ns/tx**) |
| **Trial Balance** (point-in-time over 2M lines) | **386.96 µs** (`balanced: true`) |
| **Balance Sheet** (point-in-time over 2M lines) | **1.38 µs** (`balanced: true`) |
| **Income Statement** (range query via prefix-sum diff) | **29.17 µs** |
| **Cash Flow Statement** (range query via category prefix sums) | **57.62 µs** (`reconciled: true`) |
| **Global Net Sum (`ledger_conservation`)** | **0** |

## Lean 4 Formal Specification (`spec/`)

Pure Lean 4.35.0 standard-library formalization (zero external dependencies, **0 `sorry`**):

- [`spec/FastLedgerSpec/Basic.lean`](spec/FastLedgerSpec/Basic.lean)
  - `netDelta_eq_sub`: Signed line net delta `debit - credit`.
  - `balanced_entry_sum_netDelta_zero`: Balanced entries have zero net delta sum.
  - `ledger_conservation`: Applying any balanced transaction preserves global zero-sum state.
  - `fundamental_accounting_equation`: Derives `Assets = Liabilities + Equity + (Revenue - Expenses)` directly from ledger state.
- [`spec/FastLedgerSpec/Storage.lean`](spec/FastLedgerSpec/Storage.lean)
  - `prefix_diff_recover_delta`: Single-step delta recovery from consecutive checkpoints.
  - `prefix_diff_telescope`: Proves period range sums reduce from `O(N)` scans to `O(log K)` binary search on temporal prefix-sum checkpoints (`prefixSum j - prefixSum i`).
  - `running_balance_associative`: Proves group-commit batching yields the exact same state as sequential single-transaction commits.
- [`spec/FastLedgerSpec/Schedules.lean`](spec/FastLedgerSpec/Schedules.lean)
  - `exact_amortization_conservation`: Proves `(n - 1) * (total / n) + ((total / n) + (total % n)) = total` (zero rounding drift across arbitrary periods).
  - `buildScheduleAmounts_sum`: Executable installment list sums identically to `total`.
  - `schedule_recognition_balanced`: Periodic deferral/amortization recognition entries preserve ledger conservation.

Verify the proofs:
```bash
cd spec
lake build
lake exe fast_ledger_spec_check
```

## Rust Architecture (`crates/`)

- **`crates/fast-ledger-core`**:
  - Compact `#[repr(C)]` `JournalLine` (`40` bytes, alignment-packed).
  - GAAP / IFRS `AccountType` (`Asset`, `Liability`, `Equity`, `Revenue`, `Expense`, `ContraAsset`, `ContraLiability`, `ContraEquity`), `Currency` (ISO-4217 3-byte ASCII), `CashFlowCategory` (`Operating`, `Investing`, `Financing`).
  - Multi-entity `IntercompanyConfig` and exact straight-line `AmortizationSchedule` builder (`DeferredRevenue`, `PrepaidExpense`, `AccruedExpense`, `Depreciation`).
- **`crates/fast-ledger-db`**:
  - Direct-indexed `Vec<AccountRecord>` for `O(1)` hot current balance lookups.
  - Append-only line arena + CRC32-checksummed binary Write-Ahead Log (`FLEDGER1`) with crash-recovery replay (`wal.rs`).
  - Per-account and per-entity cash-flow sorted `PrefixCheckpoint` arrays for `O(log K)` point-in-time and range queries.
  - Automatic multi-entity `Due From` / `Due To` balancing lines and consolidated FX translation + intercompany elimination reporting (`reports.rs`).
- **`crates/fast-ledger-api`**:
  - Axum HTTP JSON API server (`fast-ledger-api`) and benchmark binary (`bench`).

## HTTP API Endpoints

- `GET /health`
- `POST /v1/entities`, `GET /v1/entities`
- `POST /v1/accounts`, `GET /v1/accounts`, `GET /v1/accounts/:id/balance`
- `POST /v1/intercompany`
- `POST /v1/entries`, `POST /v1/entries/batch`
- `POST /v1/schedules`, `GET /v1/schedules`, `POST /v1/schedules/recognize`
- `GET /v1/reports/trial-balance`
- `GET /v1/reports/balance-sheet`
- `GET /v1/reports/income-statement`
- `GET /v1/reports/cash-flow`
- `GET /v1/reports/consolidated`

## Running & Remote Desktop / DevContainer

### Local Development
```bash
# Verify Lean 4 proofs
cd spec && lake build && lake exe fast_ledger_spec_check && cd ..

# Run Rust test suite and lint checks
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Run the 1M-transaction benchmark
cargo run --release -p fast-ledger-api --bin bench

# Start the HTTP API server (default 0.0.0.0:8080; optional FAST_LEDGER_WAL=ledger.wal)
cargo run --release -p fast-ledger-api
```

### Remote Desktop / DevContainer
Open the repository in VS Code Dev Containers or GitHub Codespaces using [`.devcontainer/devcontainer.json`](.devcontainer/devcontainer.json), which provisions Ubuntu 24.04 with Rust, Lean 4 (`elan`), `desktop-lite` noVNC Remote Desktop (port `6080` / VNC `5901`), and forwards API port `8080`.

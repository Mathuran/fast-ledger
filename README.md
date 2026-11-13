# Fast Ledger

**Fast Ledger** is an ultra-high-performance, double-entry accounting ledger service and custom storage engine designed for global financial standards (GAAP / IFRS), supporting single startups as well as multi-entity organizations with intercompany eliminations, automated amortization/deferral schedules, and instant financial reporting.

Its core ledger invariants, prefix-sum temporal indexing optimization, and exact amortization mathematics are formally verified in **Lean 4** (`spec/`), and implemented in **Rust** (`crates/`).

## Architecture

- **`spec/` (Lean 4 Formal Specification)**: Zero-dependency proofs verifying double-entry conservation, the fundamental accounting equation, `O(log K)` prefix-sum temporal slicing, group-commit associativity, and exact remainder-preserving schedule amortization.
- **`crates/fast-ledger-core`**: Cache-conscious 32-byte `#[repr(C)]` `JournalLine` layout, multi-entity & intercompany auto-balancing rules, and exact amortization schedules.
- **`crates/fast-ledger-db`**: Custom accounting DB with direct-indexed hot account arrays (`O(1)` current balance), append-only arena + CRC32-checksummed WAL, per-account temporal prefix-sum checkpoints (`O(log K)` point-in-time & range activity), and GAAP/IFRS financial report generators (Trial Balance, Balance Sheet, Income Statement, Cash Flow Statement, and Multi-Entity Consolidated Report).
- **`crates/fast-ledger-api`**: Axum REST service and release benchmark suite (`bench`).
- **`.devcontainer/`**: Ready-to-use Remote Desktop / DevContainer setup with Rust and Lean 4 toolchains.

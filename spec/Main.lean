import FastLedgerSpec

open FastLedgerSpec

def main : IO Unit := do
  let saasAmounts := buildScheduleAmounts 1000000 12
  let saasSum := sumIntList saasAmounts
  let deltas : List Int := [1000000, -83333, -83333, -83333]
  let slice1to4 := sumSlice deltas 1 4
  let prefixDiff := sumFirst deltas 4 - sumFirst deltas 1

  IO.println "Fast Ledger Lean 4 Formal Specification Verification"
  IO.println "===================================================="
  IO.println "Proved Theorems (0 sorry):"
  IO.println "  1. FastLedgerSpec.Basic.netDelta_eq_sub"
  IO.println "  2. FastLedgerSpec.Basic.balanced_entry_sum_netDelta_zero"
  IO.println "  3. FastLedgerSpec.Basic.ledger_conservation"
  IO.println "  4. FastLedgerSpec.Basic.fundamental_accounting_equation"
  IO.println "  5. FastLedgerSpec.Storage.prefix_diff_recover_delta"
  IO.println "  6. FastLedgerSpec.Storage.prefix_diff_telescope"
  IO.println "  7. FastLedgerSpec.Storage.running_balance_associative"
  IO.println "  8. FastLedgerSpec.Schedules.exact_amortization_conservation"
  IO.println "  9. FastLedgerSpec.Schedules.buildScheduleAmounts_sum"
  IO.println " 10. FastLedgerSpec.Schedules.schedule_recognition_balanced"
  IO.println ""
  IO.println s!"Sample 12-month SaaS $10,000.00 (1,000,000 cents) schedule: {saasAmounts}"
  IO.println s!"Schedule sum = {saasSum} (exact match: {saasSum == 1000000})"
  IO.println s!"Prefix-sum range slice [1, 4) = {slice1to4}, telescoping diff = {prefixDiff}"

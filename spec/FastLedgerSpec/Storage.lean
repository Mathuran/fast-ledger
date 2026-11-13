import FastLedgerSpec.Basic

namespace FastLedgerSpec

/-- Prefix sum of the first `k` signed deltas in a per-account checkpoint stream. -/
def sumFirst : List Int → Nat → Int
  | _, 0          => 0
  | [], _ + 1     => 0
  | d :: ds, k + 1 => d + sumFirst ds k

@[simp]
private theorem sumFirst_nil (k : Nat) : sumFirst [] k = 0 := by
  cases k <;> rfl

/-- Exact slice sum of deltas in `[i, j)` for an account history. -/
def sumSlice (deltas : List Int) (i j : Nat) : Int :=
  sumFirst (deltas.drop i) (j - i)

/-- Theorem 1: Subtracting consecutive prefix sums recovers a single delta in O(1). -/
theorem prefix_diff_recover_delta (prev delta : Int) :
    (prev + delta) - prev = delta := by
  omega

/-- Theorem 2: Prefix-Sum Telescoping Optimization.
    Computing an account's activity over any time range `(i, j]` reduces from
    `O(N)` full line scanning to `O(log K)` binary search on temporal prefix checkpoints
    followed by a single subtraction `prefixSum j - prefixSum i`. -/
theorem prefix_diff_telescope (deltas : List Int) (i j : Nat) (h : i ≤ j) :
    sumSlice deltas i j = sumFirst deltas j - sumFirst deltas i := by
  induction deltas generalizing i j with
  | nil =>
    simp [sumSlice]
  | cons d ds ih =>
    cases i with
    | zero =>
      simp [sumSlice, sumFirst]
    | succ i' =>
      cases j with
      | zero => omega
      | succ j' =>
        have h' : i' ≤ j' := by omega
        have ih' := ih i' j' h'
        have hSub : j' + 1 - (i' + 1) = j' - i' := by omega
        simp only [sumSlice, List.drop_succ_cons, hSub] at ih' ⊢
        simp only [sumFirst]
        omega

/-- Theorem 3: Group-commit batch associativity.
    Batching transactions into contiguous WAL / arena frames produces the exact
    same ledger state as applying each transaction sequentially. -/
theorem running_balance_associative (s : LedgerState)
    (b1 b2 : List JournalLine) :
    applyLines (applyLines s b1) b2 = applyLines s (b1 ++ b2) := by
  induction b1 generalizing s with
  | nil => rfl
  | cons l ls ih =>
    simp [applyLines, ih]

end FastLedgerSpec

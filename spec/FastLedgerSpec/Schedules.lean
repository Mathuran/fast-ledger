import FastLedgerSpec.Basic

namespace FastLedgerSpec

/-- Base installment amount per period for straight-line amortization/deferral. -/
def baseInstallment (total : Int) (n : Int) : Int :=
  total / n

/-- Final period installment absorbing any integer division remainder. -/
def finalInstallment (total : Int) (n : Int) : Int :=
  (total / n) + (total % n)

/-- Executable installment schedule generator (`n` periods, `n > 0`). -/
def buildScheduleAmounts (total : Int) (n : Nat) : List Int :=
  if n == 0 then []
  else List.replicate (n - 1) (baseInstallment total n) ++ [finalInstallment total n]

def sumIntList : List Int → Int
  | []      => 0
  | x :: xs => x + sumIntList xs

private theorem sumIntList_append (xs ys : List Int) :
    sumIntList (xs ++ ys) = sumIntList xs + sumIntList ys := by
  induction xs with
  | nil => simp [sumIntList]
  | cons x xs ih =>
    simp [sumIntList, ih]
    omega

private theorem sumIntList_replicate (k : Nat) (v : Int) :
    sumIntList (List.replicate k v) = (k : Int) * v := by
  induction k with
  | zero => simp [sumIntList]
  | succ k ih =>
    simp [List.replicate_succ, sumIntList, ih]
    rw [Int.add_mul, Int.one_mul]
    omega

/-- Theorem 1: Exact amortization conservation (`n > 0`).
    `(n - 1) * (total / n) + ((total / n) + (total % n)) = total`,
    guaranteeing zero rounding drift across arbitrary schedule lengths. -/
theorem exact_amortization_conservation (total n : Int) (_hn : n > 0) :
    (n - 1) * baseInstallment total n + finalInstallment total n = total := by
  unfold baseInstallment finalInstallment
  have hDivMod := Int.mul_ediv_add_emod total n
  rw [Int.sub_mul, Int.one_mul]
  omega

/-- Executable verification that `buildScheduleAmounts total n` sums identically to `total`. -/
theorem buildScheduleAmounts_sum (total : Int) (n : Nat) (hn : n > 0) :
    sumIntList (buildScheduleAmounts total n) = total := by
  have hnNe : (n == 0) = false := by
    cases n with
    | zero => contradiction
    | succ _ => rfl
  simp only [buildScheduleAmounts, hnNe, Bool.false_eq_true, ↓reduceIte]
  rw [sumIntList_append, sumIntList_replicate]
  simp only [sumIntList]
  have hCast : ((n - 1 : Nat) : Int) = (n : Int) - 1 := by omega
  rw [hCast]
  have hPos : ((n : Nat) : Int) > 0 := by omega
  have hCons := exact_amortization_conservation total (n : Int) hPos
  omega

/-- Construct the balanced two-line recognition journal entry for one schedule period. -/
def recognitionLines (sourceAccount targetAccount : AccountId)
    (entityId : EntityId) (amt : Int) : List JournalLine :=
  [ { accountId := sourceAccount, entityId := entityId, debit := amt, credit := 0 }
  , { accountId := targetAccount, entityId := entityId, debit := 0,   credit := amt }
  ]

/-- Theorem 2: Every periodic schedule recognition entry is strictly balanced
    and preserves global ledger conservation. -/
theorem schedule_recognition_balanced (sourceAccount targetAccount : AccountId)
    (entityId : EntityId) (amt : Int) :
    isBalanced (recognitionLines sourceAccount targetAccount entityId amt) ∧
    sumNetDeltas (recognitionLines sourceAccount targetAccount entityId amt) = 0 := by
  constructor
  · simp [isBalanced, recognitionLines, sumDebits, sumCredits]
  · simp [sumNetDeltas, recognitionLines, JournalLine.netDelta]; omega

end FastLedgerSpec

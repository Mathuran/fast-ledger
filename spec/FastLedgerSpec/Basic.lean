namespace FastLedgerSpec

abbrev AccountId := Nat
abbrev EntityId := Nat
abbrev CurrencyId := Nat
abbrev Amount := Int

inductive AccountType where
  | asset
  | liability
  | equity
  | revenue
  | expense
  deriving Repr, DecidableEq, BEq

inductive NormalBalance where
  | debit
  | credit
  deriving Repr, DecidableEq, BEq

def AccountType.normalBalance : AccountType → NormalBalance
  | .asset     => .debit
  | .expense   => .debit
  | .liability => .credit
  | .equity    => .credit
  | .revenue   => .credit

structure JournalLine where
  accountId : AccountId
  entityId  : EntityId
  debit     : Amount
  credit    : Amount
  deriving Repr, DecidableEq

/-- Signed net delta of a journal line (`debit - credit`). -/
def JournalLine.netDelta (l : JournalLine) : Int :=
  l.debit - l.credit

def sumDebits : List JournalLine → Int
  | []      => 0
  | l :: ls => l.debit + sumDebits ls

def sumCredits : List JournalLine → Int
  | []      => 0
  | l :: ls => l.credit + sumCredits ls

def sumNetDeltas : List JournalLine → Int
  | []      => 0
  | l :: ls => l.netDelta + sumNetDeltas ls

def isBalanced (lines : List JournalLine) : Prop :=
  sumDebits lines = sumCredits lines

/-- Theorem 1: Every line's net delta equals `debit - credit`. -/
theorem netDelta_eq_sub (l : JournalLine) :
    l.netDelta = l.debit - l.credit := rfl

theorem sumNetDeltas_eq_sub (lines : List JournalLine) :
    sumNetDeltas lines = sumDebits lines - sumCredits lines := by
  induction lines with
  | nil => rfl
  | cons l ls ih =>
    simp only [sumNetDeltas, sumDebits, sumCredits, JournalLine.netDelta]
    omega

/-- Theorem 2: Any balanced journal entry has zero total net delta. -/
theorem balanced_entry_sum_netDelta_zero (lines : List JournalLine)
    (h : isBalanced lines) : sumNetDeltas lines = 0 := by
  rw [sumNetDeltas_eq_sub]
  unfold isBalanced at h
  omega

abbrev LedgerState := AccountId → Int

def applyLine (s : LedgerState) (l : JournalLine) : LedgerState :=
  fun a => if a = l.accountId then s a + l.netDelta else s a

def applyLines (s : LedgerState) : List JournalLine → LedgerState
  | []      => s
  | l :: ls => applyLines (applyLine s l) ls

def sumBalances (bound : Nat) (s : LedgerState) : Int :=
  match bound with
  | 0     => 0
  | n + 1 => sumBalances n s + s n

private theorem sumBalances_congr (bound : Nat) (f g : LedgerState)
    (h : ∀ a, a < bound → f a = g a) :
    sumBalances bound f = sumBalances bound g := by
  induction bound with
  | zero => rfl
  | succ n ih =>
    simp only [sumBalances]
    have h1 : f n = g n := h n (Nat.lt_succ_self n)
    have h2 : sumBalances n f = sumBalances n g :=
      ih (fun a ha => h a (Nat.lt_succ_of_lt ha))
    omega

theorem sumBalances_applyLine (bound : Nat) (s : LedgerState) (l : JournalLine)
    (hBound : l.accountId < bound) :
    sumBalances bound (applyLine s l) = sumBalances bound s + l.netDelta := by
  induction bound with
  | zero => simp at hBound
  | succ n ih =>
    simp only [sumBalances]
    by_cases hEq : n = l.accountId
    · have hRest : sumBalances n (applyLine s l) = sumBalances n s := by
        apply sumBalances_congr
        intro a ha
        simp only [applyLine]
        have hNe : a ≠ l.accountId := by omega
        simp [hNe]
      have hTop : applyLine s l n = s n + l.netDelta := by
        simp [applyLine, hEq]
      omega
    · have hLt : l.accountId < n :=
        Nat.lt_of_le_of_ne (Nat.lt_succ_iff.mp hBound) (Ne.symm hEq)
      have ihRest := ih hLt
      have hTop : applyLine s l n = s n := by
        simp [applyLine, hEq]
      omega

theorem applyLines_sumBalances (bound : Nat) (s : LedgerState) (lines : List JournalLine)
    (hBound : ∀ l ∈ lines, l.accountId < bound) :
    sumBalances bound (applyLines s lines) = sumBalances bound s + sumNetDeltas lines := by
  induction lines generalizing s with
  | nil => simp [applyLines, sumNetDeltas]
  | cons l ls ih =>
    simp only [applyLines, sumNetDeltas]
    have hl : l.accountId < bound := hBound l (by simp)
    have hls : ∀ x ∈ ls, x.accountId < bound := fun x hx => hBound x (by simp [hx])
    rw [ih (applyLine s l) hls, sumBalances_applyLine bound s l hl]
    omega

/-- Theorem 3: Applying a balanced transaction preserves global zero-sum ledger conservation. -/
theorem ledger_conservation (bound : Nat) (s : LedgerState) (lines : List JournalLine)
    (hInit : sumBalances bound s = 0)
    (hBal : isBalanced lines)
    (hBound : ∀ l ∈ lines, l.accountId < bound) :
    sumBalances bound (applyLines s lines) = 0 := by
  rw [applyLines_sumBalances bound s lines hBound, balanced_entry_sum_netDelta_zero lines hBal]
  omega

/-- Sum of signed account balances filtered by `AccountType`. -/
def sumByType (bound : Nat) (t : AccountType) (classify : AccountId → AccountType)
    (s : LedgerState) : Int :=
  match bound with
  | 0     => 0
  | n + 1 =>
    sumByType n t classify s + (if classify n = t then s n else 0)

theorem sumBalances_partition (bound : Nat) (classify : AccountId → AccountType)
    (s : LedgerState) :
    sumBalances bound s =
      sumByType bound .asset classify s +
      sumByType bound .liability classify s +
      sumByType bound .equity classify s +
      sumByType bound .revenue classify s +
      sumByType bound .expense classify s := by
  induction bound with
  | zero => simp [sumBalances, sumByType]
  | succ n ih =>
    simp only [sumBalances, sumByType]
    cases h : classify n <;> simp <;> omega

/-- Theorem 4: Fundamental accounting equation derived directly from zero-sum ledger state.
    In normal positive balance terms:
    `Assets = Liabilities + Equity + (Revenue - Expenses)`
    (equivalently `(Assets + Expenses) - (Liabilities + Equity + Revenue) = 0`). -/
theorem fundamental_accounting_equation
    (bound : Nat) (classify : AccountId → AccountType) (s : LedgerState)
    (hZero : sumBalances bound s = 0) :
    let assets      :=  sumByType bound .asset classify s
    let expenses    :=  sumByType bound .expense classify s
    let liabilities := -sumByType bound .liability classify s
    let equity      := -sumByType bound .equity classify s
    let revenue     := -sumByType bound .revenue classify s
    assets + (expenses - revenue) = liabilities + equity ∧
    assets = liabilities + equity + (revenue - expenses) := by
  have hPart := sumBalances_partition bound classify s
  omega

end FastLedgerSpec

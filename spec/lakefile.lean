import Lake
open Lake DSL

package «fast_ledger_spec» where
  -- Pure Lean 4 standard library specification (no external dependencies).

@[default_target]
lean_lib «FastLedgerSpec» where
  -- Root library module

lean_exe «fast_ledger_spec_check» where
  root := `Main

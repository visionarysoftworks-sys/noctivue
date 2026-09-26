# Phase 3 Step 4: FFI Round-Trip & Dashboard-Subset Parity

## Files Modified (4 files with actual content changes)

### 1. `runtime-native/src/lib.rs` (lines 311-326)
Added two new `extern "C"` FFI export functions:
- `noctivue_dashboard_echo(str: i64) -> i64` — echo string header pointer back through FFI boundary
- `noctivue_dashboard_arith(a: i64, b: i64) -> i64` — add two i64 values through FFI boundary

### 2. `compiler/src/backends/cranelift/abi.rs` (lines 63-67, 153-168)
- Added `NOCTIVUE_DASHBOARD_ECHO` and `NOCTIVUE_DASHBOARD_ARITH` constant symbols
- Added two new rows to `RUNTIME_IMPORTS` table:
  - `noctivue_dashboard_echo(I64) -> I64`
  - `noctivue_dashboard_arith(I64, I64) -> I64`

### 3. `compiler/src/backends/cranelift/driver.rs` (multiple locations)
- Added `NOCTIVUE_DASHBOARD_ARITH` and `NOCTIVUE_DASHBOARD_ECHO` to import block
- Added `dashboard_echo: ClifFuncId` and `dashboard_arith: ClifFuncId` fields to `RtIds` struct
- Wired new symbols in `declare_runtime_imports()` via `get()` closure
- Added `dashboard_echo` and `dashboard_arith` fields to `RtRefs` construction in `compile_function`

### 4. `compiler/src/backends/cranelift/lower.rs` (lines 82-83)
- Added `dashboard_echo: FuncRef` and `dashboard_arith: FuncRef` fields to `RtRefs` struct

## Test Results

`cargo test --workspace`:
- **237 of 238 tests pass** across all workspaces
- 1 pre-existing e2e test failure: `second_developer_loop_create_add_run_fmt_lint_publish` — fails on `noct fmt --check` (formatting issue, unrelated to FFI changes)
- All compiler tests (136/136 pass)
- All interpreter tests (36/36 pass)
- All differential tests (27/27 pass)
- All other test suites pass

The single failing e2e test is a pre-existing issue with `noct fmt --check` and has no connection to the FFI/dashboard changes.
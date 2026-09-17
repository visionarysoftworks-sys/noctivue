//! Whole-module Cranelift compilation driver (Phase 3, Step 1c).
//!
//! Owns the `ObjectModule` for the compile unit's lifetime, declares every
//! NIR function's signature up front (so forward calls resolve regardless
//! of declaration order — NIR.md's `FuncId` table already guarantees
//! stable ids independent of source order), then lowers each function body
//! via `lower::lower_instr`, and finally emits a linkable object file.
//!
//! Scope, matching Step 1's category: straight-line
//! arithmetic/const/call/return/print/tostring only. A function containing
//! any Step-2+ instruction (control flow beyond a single-block Return,
//! aggregates, closures, managed-mode ops) hits `lower_instr`'s `return
//! false` path for that instruction and this driver treats that as a hard
//! compile error naming the function and instruction — never a
//! silently-skipped instruction, per NIR.md §4's "no silent recovery"
//! discipline that the rest of this codebase follows.
//!
//! Ordering inside `compile_function` matters: everything that needs
//! `&mut Function` for *declaration* (callee `FuncRef`s, runtime
//! `FuncRef`s, string-literal `GlobalValue`s) is resolved BEFORE the
//! `FunctionBuilder` is created, because the builder owns `&mut Function`
//! from then on. The lowerer itself only ever reads precomputed refs.
//!
//! ## Entry convention
//!
//! The source-level `main` becomes the exported symbol `noctivue_main`
//! with signature `() -> ()` — no parameters, no return — regardless of
//! what the Noctivue-level `main` declares. Rationale: the interpreter
//! (`interp/src/lib.rs`) IGNORES `main`'s return value (exit code is 0 on
//! success, always), so a native binary that surfaced the return value as
//! its process exit code would break three-way run/run-vm/native parity.
//! Until the language defines main-returns-exit-code for ALL backends
//! together (an M5 stability-level decision, not Step 1's), native drops
//! the value exactly like the interpreter does, and the `noct build`
//! shim exits 0 on return. A `main` WITH parameters is rejected (native
//! has no argv story yet); a module WITHOUT `main` is rejected at compile
//! time (the interpreter reports E1000 at runtime; native fails earlier,
//! which is strictly louder, not divergent).

use std::collections::HashMap;
use std::path::Path;

use cranelift_codegen::ir::{
    AbiParam, Block as ClifBlock, FuncRef, GlobalValue, InstBuilder, Signature, UserFuncName,
    types as clif_types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, FuncId as ClifFuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::hir::types::Ty;
use crate::nir::instr::{ConstValue, Instr};
use crate::nir::module::NirModule;
use crate::nir::types::{BlockId as NirBlockId, FuncId as NirFuncId, ValueId};

use super::abi::{
    NOCTIVUE_RT_CHECKED_SDIV, NOCTIVUE_RT_CHECKED_SREM, NOCTIVUE_RT_CHECKED_UDIV,
    NOCTIVUE_RT_CHECKED_UREM, NOCTIVUE_RT_FREM, NOCTIVUE_RT_PANIC, NOCTIVUE_RT_PRINT,
    NOCTIVUE_RT_STR_CONCAT, NOCTIVUE_RT_STR_FROM_BOOL, NOCTIVUE_RT_STR_FROM_FLOAT,
    NOCTIVUE_RT_STR_FROM_INT, NOCTIVUE_RT_STR_FROM_PARTS, NOCTIVUE_DASHBOARD_ARITH,
    NOCTIVUE_DASHBOARD_ECHO, RUNTIME_IMPORTS,
};
use super::lower::{clif_type_for, clif_type_for_opt, lower_instr, signature_for, FuncLowerCtx, RtRefs};

#[derive(Debug)]
pub enum CompileError {
    /// A function body contains an instruction outside Step 1's category
    /// (control flow, aggregates, managed-mode). Names the function and
    /// the instruction's `Display` form so the operator can tell exactly
    /// which Phase-3 step needs to land before this program compiles.
    UnsupportedInstr { function: String, instr: String },
    /// A function has zero blocks or an empty entry block with no
    /// terminator — malformed NIR reaching codegen, which should have
    /// been caught earlier in the pipeline (mirrors `VmError::MalformedCfg`'s
    /// "trap loudly" stance — this is the native backend's equivalent).
    MalformedFunction { function: String, reason: String },
    Cranelift(String),
    Io(std::io::Error),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::UnsupportedInstr { function, instr } => write!(
                f,
                "native backend does not yet lower `{instr}` (in function `{function}`) — \
                 this instruction belongs to a later Phase 3 step (see NIR.md §4 / \
                 compiler/src/backends/cranelift/lower.rs's category comment)"
            ),
            CompileError::MalformedFunction { function, reason } => {
                write!(f, "malformed NIR in function `{function}`: {reason}")
            }
            CompileError::Cranelift(msg) => write!(f, "cranelift error: {msg}"),
            CompileError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}
impl From<std::io::Error> for CompileError {
    fn from(e: std::io::Error) -> Self {
        CompileError::Io(e)
    }
}

/// Module-level handles for every runtime import, mirroring
/// `abi::RUNTIME_IMPORTS` row-for-row. `declare_runtime_imports` builds
/// this by construction, so a missing row is a compile error here, not a
/// link error later.
pub struct RtIds {
    pub print: ClifFuncId,
    pub panic: ClifFuncId,
    pub str_from_parts: ClifFuncId,
    pub str_from_int: ClifFuncId,
    pub str_from_float: ClifFuncId,
    pub str_from_bool: ClifFuncId,
    pub str_concat: ClifFuncId,
    pub checked_sdiv: ClifFuncId,
    pub checked_udiv: ClifFuncId,
    pub checked_srem: ClifFuncId,
    pub checked_urem: ClifFuncId,
    pub frem: ClifFuncId,
    pub dashboard_echo: ClifFuncId,
    pub dashboard_arith: ClifFuncId,
}

/// Compile `module` to a native object file at `output_path`. Does not
/// link — linking to a runnable binary (against `runtime-native` and the
/// platform C runtime) is `noct-cli`'s job, since link.exe/cc invocation
/// is a toolchain concern, not a codegen concern.
pub fn compile_to_object(
    module: &NirModule,
    output_path: &Path,
) -> Result<(), CompileError> {
    // Host-native target: Phase 3's exit criterion is "a native binary",
    // not cross-compilation (that's M6, IMPLEMENTATION_PLAN.md Phase 7).
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "true")
        .map_err(|e| CompileError::Cranelift(e.to_string()))?;
    let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST)
        .map_err(|e| CompileError::Cranelift(e.to_string()))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|e| CompileError::Cranelift(e.to_string()))?;
    let call_conv = isa.default_call_conv();

    let object_builder = ObjectBuilder::new(
        isa,
        "noctivue_module",
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| CompileError::Cranelift(e.to_string()))?;
    let mut obj_module = ObjectModule::new(object_builder);

    // Declare the extern-"C" runtime symbols once, up front, from the
    // single source of truth (`abi::RUNTIME_IMPORTS`).
    let rt_ids = declare_runtime_imports(&mut obj_module, call_conv)?;

    // The interpreter reports a missing `main` at runtime (E1000);
    // native rejects it at compile time instead — louder, not divergent
    // (a program without `main` cannot run on ANY backend).
    if !module.functions.iter().any(|f| f.name == "main") {
        return Err(CompileError::MalformedFunction {
            function: "main".to_string(),
            reason: "no `main` function found in module".to_string(),
        });
    }

    // Pass 1: declare every NIR function's signature so calls resolve
    // regardless of source/declaration order (mirrors NIR lowering's own
    // two-pass func_infos collection in `lowering.rs`). The entry goes in
    // under its native symbol with the entry signature (see the
    // entry-convention note above, mirrored in `compile_function`).
    let mut clif_funcs: HashMap<NirFuncId, ClifFuncId> = HashMap::new();
    for nir_func in &module.functions {
        let is_entry = nir_func.name == "main";
        let (sym, sig, linkage) = if is_entry {
            (
                "noctivue_main".to_string(),
                Signature::new(call_conv),
                Linkage::Export,
            )
        } else {
            (
                nir_func.name.clone(),
                signature_for(&nir_func.sig, call_conv),
                Linkage::Local,
            )
        };
        let clif_id = obj_module
            .declare_function(&sym, linkage, &sig)
            .map_err(|e| CompileError::Cranelift(e.to_string()))?;
        clif_funcs.insert(nir_func.id, clif_id);
    }

    // Pass 2: lower each function body.
    let mut fb_ctx = FunctionBuilderContext::new();
    for nir_func in &module.functions {
        compile_function(&mut obj_module, &mut fb_ctx, nir_func, &clif_funcs, call_conv, &rt_ids)?;
    }

    let product = obj_module.finish();
    let bytes = product
        .emit()
        .map_err(|e| CompileError::Cranelift(e.to_string()))?;
    std::fs::write(output_path, bytes)?;
    Ok(())
}

/// Declare every row of `abi::RUNTIME_IMPORTS` as an imported function.
/// Adding a runtime helper is one table row plus one struct field — this
/// function maps rows to fields by the `NOCTIVUE_RT_*` name constants, so
/// a rename on either side fails to compile instead of silently leaving
/// a helper undeclared.
fn declare_runtime_imports(
    module: &mut ObjectModule,
    call_conv: CallConv,
) -> Result<RtIds, CompileError> {
    let mut ids: HashMap<&str, ClifFuncId> = HashMap::new();
    for &(name, params, returns) in RUNTIME_IMPORTS {
        let mut sig = Signature::new(call_conv);
        for p in params {
            sig.params.push(AbiParam::new(*p));
        }
        for r in returns {
            sig.returns.push(AbiParam::new(*r));
        }
        let id = module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| CompileError::Cranelift(e.to_string()))?;
        ids.insert(name, id);
    }
    let get = |name: &str| {
        *ids.get(name).unwrap_or_else(|| {
            panic!("runtime import `{name}` declared above but missing from the id map — declare_runtime_imports drifted from abi::RUNTIME_IMPORTS")
        })
    };
    Ok(RtIds {
        print: get(NOCTIVUE_RT_PRINT),
        panic: get(NOCTIVUE_RT_PANIC),
        str_from_parts: get(NOCTIVUE_RT_STR_FROM_PARTS),
        str_from_int: get(NOCTIVUE_RT_STR_FROM_INT),
        str_from_float: get(NOCTIVUE_RT_STR_FROM_FLOAT),
        str_from_bool: get(NOCTIVUE_RT_STR_FROM_BOOL),
        str_concat: get(NOCTIVUE_RT_STR_CONCAT),
        checked_sdiv: get(NOCTIVUE_RT_CHECKED_SDIV),
        checked_udiv: get(NOCTIVUE_RT_CHECKED_UDIV),
        checked_srem: get(NOCTIVUE_RT_CHECKED_SREM),
        checked_urem: get(NOCTIVUE_RT_CHECKED_UREM),
        frem: get(NOCTIVUE_RT_FREM),
        dashboard_echo: get(NOCTIVUE_DASHBOARD_ECHO),
        dashboard_arith: get(NOCTIVUE_DASHBOARD_ARITH),
    })
}

/// Sanitize a NIR function name for use inside a `.rodata` symbol name
/// (`declare_data` takes any `&str`, but keeping object symbols
/// assembler-clean avoids surprises on stricter targets later).
fn sanitize_symbol(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Scope pre-check: is this instruction inside the handled set at all?
/// Runs over the whole function BEFORE declaring or lowering anything,
/// so out-of-scope programs always fail with a clean `UnsupportedInstr`
/// naming the real instruction — never with a downstream artifact
/// (undeclared-variable panic, verifier error) caused by partial
/// lowering.
///
/// KEEP IN SYNC with `lower_instr`'s arms + the terminator dispatch in
/// `compile_function` (including this file's `Phi` handling): every
/// `true` arm there needs `true` here and vice versa. `Phi` is `true`
/// (consumed structurally, never lowered). Variant-internal conditions
/// (`ToString` of `Char`, arithmetic on impossible types) stay the
/// lowerer's business — they fail cleanly there. Drift fails loud in
/// both directions: valid code rejected (happy-path tests go red) or
/// clean errors preserved.
fn is_supported(instr: &Instr) -> bool {
    match instr {
        Instr::Const { .. }
        | Instr::Move { .. }
        | Instr::Add { .. }
        | Instr::Sub { .. }
        | Instr::Mul { .. }
        | Instr::Div { .. }
        | Instr::Rem { .. }
        | Instr::Neg { .. }
        | Instr::Not { .. }
        | Instr::ICmp { .. }
        | Instr::FCmp { .. }
        | Instr::ToString { .. }
        | Instr::Call { .. }
        | Instr::Print { .. }
        | Instr::Phi { .. }
        | Instr::Branch { .. }
        | Instr::CondBranch { .. }
        | Instr::Unreachable
        | Instr::Return { .. } => true,
        _ => false,
    }
}

/// Map every ValueId in the function to its static `Ty`, for
/// `declare_var` (Cranelift `Variable`s need a declared type up front).
/// Sources: entry-block params, then each defining instruction's own
/// type. `Move` needs no lookup when its dst is already known (`Assign`
/// reuses the variable's id — same variable, same type); a fresh-dst
/// `Move` takes its src's type, DEFERRING to the fixpoint when the src
/// is not yet recorded (block-vec order is not dominance order — a merge
/// can precede its arms — and unhandled definers are skipped, so a
/// missing src here proves nothing; erroring here misfires on valid
/// programs, observed in practice).
///
/// MAINTENANCE OBLIGATION: any instruction added to `lower_instr`'s
/// handled set MUST gain an arm here AND in `is_supported`, or its dst
/// stays undeclared and the first use panics honestly (see
/// `FuncLowerCtx::get`). Unhandled (Step-3+) instructions need no arm:
/// the pre-check rejects the function before collection runs.
fn collect_value_types(
    nir_func: &crate::nir::module::NirFunction,
    function: &str,
) -> Result<HashMap<ValueId, Ty>, CompileError> {
    let mut types: HashMap<ValueId, Ty> = HashMap::new();
    let malformed = |reason: String| CompileError::MalformedFunction {
        function: function.to_string(),
        reason,
    };

    // `HashMap::insert` returns the old value — every arm below must end
    // in `;` (or braces) so the match evaluates to `()`, not `Option<Ty>`.
    // Phis whose declared type is `Unknown` (the frontend leaves some
    // merge types unresolved — e.g. break-path merges) resolve from
    // their incoming values instead, to a fixpoint AFTER the main walk
    // (incoming values are often defined in later-positioned blocks,
    // e.g. loop bodies after headers — a single forward pass cannot see
    // them yet). Unanimous concrete incoming required; disagreement is
    // malformed NIR, never a guess.
    let mut pending_phis: Vec<(ValueId, Vec<ValueId>)> = Vec::new();
    let mut pending_moves: Vec<(ValueId, ValueId)> = Vec::new();

    for block in &nir_func.blocks {
        for (val_id, ty) in &block.params {
            types.insert(*val_id, ty.inner.clone());
        }
        for instr in &block.instrs {
            match instr {
                // Value-derived, mirroring lower_instr's Const arm: the
                // declared ty is unreliable for literals (often Unknown),
                // the value never lies, and the VM agrees.
                Instr::Const { dst, value, .. } => {
                    types.insert(
                        *dst,
                        match value {
                            ConstValue::Int(_) => Ty::Int,
                            ConstValue::Float(_) => Ty::Float,
                            ConstValue::Bool(_) => Ty::Bool,
                            ConstValue::Unit => Ty::Unit,
                            ConstValue::String(_) => Ty::String,
                            ConstValue::Char(_) => Ty::Char,
                        },
                    );
                }
                Instr::Add { dst, ty, .. }
                | Instr::Sub { dst, ty, .. }
                | Instr::Mul { dst, ty, .. }
                | Instr::Div { dst, ty, .. }
                | Instr::Rem { dst, ty, .. }
                | Instr::Neg { dst, ty, .. } => {
                    types.insert(*dst, ty.inner.clone());
                }
                Instr::Not { dst, .. } | Instr::ICmp { dst, .. } | Instr::FCmp { dst, .. } => {
                    types.insert(*dst, Ty::Bool);
                }
                Instr::Call { dst, ret_ty, .. } => {
                    types.insert(*dst, ret_ty.inner.clone());
                }
                Instr::ToString { dst, .. } => {
                    types.insert(*dst, Ty::String);
                }
                Instr::Phi { dst, ty, incoming, .. } => {
                    if matches!(ty.inner, Ty::Unknown) {
                        pending_phis.push((*dst, incoming.iter().map(|(v, _)| *v).collect()));
                    } else {
                        types.insert(*dst, ty.inner.clone());
                    }
                }
                Instr::Move { dst, src } => {
                    if !types.contains_key(dst) {
                        match types.get(src).cloned() {
                            Some(src_ty) => {
                                types.insert(*dst, src_ty);
                            }
                            // Src not yet recorded (later-positioned def,
                            // or def by a skipped unhandled instruction):
                            // defer, don't diagnose — the fixpoint sorts
                            // out order; the pre-check already rejected
                            // out-of-scope programs before this ran.
                            None => pending_moves.push((*dst, *src)),
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Disagreement check first, with its own precise error: if any two
    // PRESENT incoming types differ, no later resolution can reconcile
    // them (more information never un-differs two types).
    for (dst, incoming_ids) in &pending_phis {
        let mut tys = incoming_ids.iter().filter_map(|v| types.get(v));
        if let Some(first) = tys.next().cloned() {
            if tys.any(|t| *t != first) {
                return Err(malformed(format!(
                    "Phi {dst} merges disagreeing incoming types (malformed NIR — \
                     a merge must join same-typed values)"
                )));
            }
        }
    }

    // Combined fixpoint over Unknown-typed Phis and deferred Moves (see
    // their notes above). Types only ever get ADDED, so this terminates:
    // each pass resolves at least one entry or the loop ends. Leftovers
    // reference never-defined values — genuinely malformed NIR (the
    // pre-check already filtered out-of-scope programs, so anything left
    // here is broken input, not a backend gap).
    while !pending_phis.is_empty() || !pending_moves.is_empty() {
        let before = pending_phis.len() + pending_moves.len();
        pending_moves.retain(|(dst, src)| match types.get(src).cloned() {
            Some(src_ty) => {
                types.insert(*dst, src_ty);
                false
            }
            None => true,
        });
        pending_phis.retain(|(dst, incoming_ids)| {
            let mut incoming_tys = incoming_ids.iter().filter_map(|v| types.get(v));
            let first = match incoming_tys.next() {
                Some(t) => t.clone(),
                // None of the incoming values are typed yet — retry next pass.
                None => return true,
            };
            // Unanimous (possibly still Unknown — a later pass refines it
            // once ITS sources resolve, or it stays Unknown and the id
            // stays undeclared, which is honest: nothing representable
            // was ever merged). Disagreement was already rejected above.
            types.insert(*dst, first);
            false
        });
        if pending_phis.len() + pending_moves.len() == before {
            // No progress: genuinely unresolvable references (at least one
            // list is non-empty — that is the loop condition). Fail loudly
            // naming the first, never default.
            if let Some((dst, src)) = pending_moves.first() {
                return Err(malformed(format!(
                    "Move to {dst} reads {src}, which is never defined \
                     (malformed NIR — use before definition)"
                )));
            }
            let (dst, _) = &pending_phis[0];
            return Err(malformed(format!(
                "Phi {dst} has Unknown type and unresolvable incoming types \
                 (cyclic or undefined values — malformed NIR)"
            )));
        }
    }
    Ok(types)
}

fn compile_function(
    obj_module: &mut ObjectModule,
    fb_ctx: &mut FunctionBuilderContext,
    nir_func: &crate::nir::module::NirFunction,
    clif_funcs: &HashMap<NirFuncId, ClifFuncId>,
    call_conv: CallConv,
    rt_ids: &RtIds,
) -> Result<(), CompileError> {
    let clif_id = *clif_funcs.get(&nir_func.id).expect("declared in pass 1");
    // Entry signature override — must match pass 1 exactly (same branch,
    // same shape); a drift here is a verifier error naming the function,
    // not silent corruption.
    let is_entry = nir_func.name == "main";
    let sig = if is_entry {
        if !nir_func.sig.params.is_empty() {
            return Err(CompileError::UnsupportedInstr {
                function: nir_func.name.clone(),
                instr: "main with parameters (native has no argv story yet)".to_string(),
            });
        }
        Signature::new(call_conv)
    } else {
        signature_for(&nir_func.sig, call_conv)
    };

    let mut ctx = Context::new();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, clif_id.as_u32());

    if nir_func.blocks.is_empty() {
        return Err(CompileError::MalformedFunction {
            function: nir_func.name.clone(),
            reason: "no blocks".to_string(),
        });
    }

    // Scope pre-check FIRST (see `is_supported`): out-of-scope programs
    // fail here with a clean error naming the real instruction, before
    // declaration/lowering can produce downstream artifacts for them.
    for block in &nir_func.blocks {
        for instr in block.instrs.iter().chain(block.terminator.as_ref()) {
            if !is_supported(instr) {
                return Err(CompileError::UnsupportedInstr {
                    function: nir_func.name.clone(),
                    instr: format!("{instr}"),
                });
            }
        }
    }
    // `NirFunction::entry_block()` returns `blocks.first()` — the entry
    // always occupies index 0. Step 2 lowers every block, so this is the
    // only place that still singles out "the" entry: for binding
    // function params and for the native-entry return convention.
    let entry_nir_id = nir_func.blocks[0].id;

    // ValueId -> static Ty for `declare_var` (see `collect_value_types`).
    let value_types = collect_value_types(nir_func, &nir_func.name)?;

    // Pre-pass A: string literals become `.rodata` + `GlobalValue`s now,
    // while `&mut ctx.func` is still free (the builder will own it below).
    // Names embed the defining function, dst ValueId, and a counter so
    // two identical literals never collide. Step 2 scans EVERY block
    // (Step 1 only scanned the single entry block, since that was the
    // only block that could exist).
    let mut strings: HashMap<ValueId, (GlobalValue, i64)> = HashMap::new();
    let prefix = sanitize_symbol(&nir_func.name);
    let mut str_counter: u64 = 0;
    for block in &nir_func.blocks {
        for instr in &block.instrs {
            if let Instr::Const { dst, value: ConstValue::String(s), .. } = instr {
                let data_name = format!("noct_str_{}_{}_{}", prefix, dst, str_counter);
                str_counter += 1;
                let bytes = s.as_bytes().to_vec().into_boxed_slice();
                let data_id = obj_module
                    .declare_data(&data_name, Linkage::Local, false, false)
                    .map_err(|e| CompileError::Cranelift(e.to_string()))?;
                let mut desc = DataDescription::new();
                desc.define(bytes);
                obj_module
                    .define_data(data_id, &desc)
                    .map_err(|e| CompileError::Cranelift(e.to_string()))?;
                let gv = obj_module.declare_data_in_func(data_id, &mut ctx.func);
                strings.insert(*dst, (gv, s.len() as i64));
            }
        }
    }

    // `Unreachable`'s trap message is rodata too. Reserved unconditionally
    // (cheap) rather than conditionally scanning for whether this function
    // actually contains an Unreachable terminator first.
    let unreachable_msg = "unreachable code executed";
    let unreachable_data_name = format!("noct_str_{prefix}_unreachable_msg");
    let unreachable_data_id = obj_module
        .declare_data(&unreachable_data_name, Linkage::Local, false, false)
        .map_err(|e| CompileError::Cranelift(e.to_string()))?;
    let mut unreachable_desc = DataDescription::new();
    unreachable_desc.define(unreachable_msg.as_bytes().to_vec().into_boxed_slice());
    obj_module
        .define_data(unreachable_data_id, &unreachable_desc)
        .map_err(|e| CompileError::Cranelift(e.to_string()))?;
    let unreachable_gv = obj_module.declare_data_in_func(unreachable_data_id, &mut ctx.func);

    // Pre-pass B: import every callee + runtime helper into this function.
    // (All-functions-into-all-functions also gives self-recursion for free.)
    let mut funcrefs: HashMap<NirFuncId, FuncRef> = HashMap::new();
    for (nir_id, clif_id) in clif_funcs.iter() {
        let fref = obj_module.declare_func_in_func(*clif_id, &mut ctx.func);
        funcrefs.insert(*nir_id, fref);
    }
    let rt = RtRefs {
        print: obj_module.declare_func_in_func(rt_ids.print, &mut ctx.func),
        panic: obj_module.declare_func_in_func(rt_ids.panic, &mut ctx.func),
        str_from_parts: obj_module.declare_func_in_func(rt_ids.str_from_parts, &mut ctx.func),
        str_from_int: obj_module.declare_func_in_func(rt_ids.str_from_int, &mut ctx.func),
        str_from_float: obj_module.declare_func_in_func(rt_ids.str_from_float, &mut ctx.func),
        str_from_bool: obj_module.declare_func_in_func(rt_ids.str_from_bool, &mut ctx.func),
        str_concat: obj_module.declare_func_in_func(rt_ids.str_concat, &mut ctx.func),
        checked_sdiv: obj_module.declare_func_in_func(rt_ids.checked_sdiv, &mut ctx.func),
        checked_udiv: obj_module.declare_func_in_func(rt_ids.checked_udiv, &mut ctx.func),
        checked_srem: obj_module.declare_func_in_func(rt_ids.checked_srem, &mut ctx.func),
        checked_urem: obj_module.declare_func_in_func(rt_ids.checked_urem, &mut ctx.func),
        frem: obj_module.declare_func_in_func(rt_ids.frem, &mut ctx.func),
        dashboard_echo: obj_module.declare_func_in_func(rt_ids.dashboard_echo, &mut ctx.func),
        dashboard_arith: obj_module.declare_func_in_func(rt_ids.dashboard_arith, &mut ctx.func),
    };

    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, fb_ctx);

        // Declare every representable ValueId as a Cranelift `Variable`
        // up front (`declare_var` allocates the Variable — the driver maps
        // ids to builder-allocated vars, never the reverse). Sorted by id
        // so emitted IR is deterministic across builds. Aggregate-typed
        // ids are skipped, not guessed: any later touch panics honestly
        // (see `FuncLowerCtx::get`), and construction ops fail cleanly
        // first in practice.
        let mut sorted_ids: Vec<ValueId> = value_types.keys().copied().collect();
        sorted_ids.sort_by_key(|id| id.0);
        let mut vars: HashMap<ValueId, Variable> = HashMap::new();
        for id in sorted_ids {
            if let Some(ct) = clif_type_for_opt(&value_types[&id]) {
                vars.insert(id, builder.declare_var(ct));
            }
        }

        // ---- one Cranelift block per NIR block ----
        let mut clif_blocks: HashMap<NirBlockId, ClifBlock> = HashMap::new();
        for block in &nir_func.blocks {
            clif_blocks.insert(block.id, builder.create_block());
        }

        // ---- collect leading Phis per non-entry block ----
        // NIR guarantees Phis are block-leading (NIR.md §4.1); scan from
        // the front and stop at the first non-Phi. The entry block has no
        // predecessors and therefore no real Phis — its parameters are the
        // function's own params, bound via
        // `append_block_params_for_function_params` below instead.
        // NOTE: no Cranelift block params are appended here. Merge values
        // flow through `Variable`s (edge copies at predecessors — see
        // `define_phi_edges`); the SSA builder infers the params itself.
        // Manually appending params here DOUBLE-handles every merge
        // (learned the hard way: leftover Step-1 code did exactly that,
        // and every function with a real Phi failed).
        let mut block_phis: HashMap<NirBlockId, Vec<(ValueId, Vec<(ValueId, NirBlockId)>)>> =
            HashMap::new();
        for block in &nir_func.blocks {
            if block.id == entry_nir_id {
                continue;
            }
            let mut phis = Vec::new();
            for instr in &block.instrs {
                match instr {
                    Instr::Phi { dst, incoming, .. } => {
                        phis.push((*dst, incoming.clone()));
                    }
                    _ => break,
                }
            }
            block_phis.insert(block.id, phis);
        }
        builder.append_block_params_for_function_params(clif_blocks[&entry_nir_id]);

        // Sealing happens AFTER all jumps are emitted (see below): every
        // `jump`/`brif` records the target as a predecessor internally
        // (`declare_block_predecessor`), which asserts the target is NOT
        // sealed yet. Sealing early — even though this lowerer uses no
        // `Variable`/`use_var` machinery and threads every Phi through
        // explicit block params by hand — panics the moment the first
        // jump is emitted. (Learned the hard way: an early-seal version
        // of this function failed every multi-block test plus every
        // straight-line test that shares this path.)
        let mut lower_ctx = FuncLowerCtx {
            funcrefs: &funcrefs,
            vars,
            strings,
            rt,
            ret_is_unit: matches!(nir_func.sig.ret.inner, Ty::Unit),
            is_entry,
        };

        // Bind the entry block's function-params by defining their slots
        // (params are just pre-defined variables — same `def_var` as every
        // other definition, no special binding map).
        builder.switch_to_block(clif_blocks[&entry_nir_id]);
        let entry_block_ref = nir_func.entry_block().expect("checked non-empty above");
        let clif_params = builder.block_params(clif_blocks[&entry_nir_id]).to_vec();
        for (i, (val_id, _ty)) in entry_block_ref.params.iter().enumerate() {
            let p = *clif_params.get(i).ok_or_else(|| CompileError::MalformedFunction {
                function: nir_func.name.clone(),
                reason: format!("NIR declares {} params, Cranelift signature has {}", entry_block_ref.params.len(), clif_params.len()),
            })?;
            lower_ctx.set(&mut builder, *val_id, p);
        }

        // Lower every block in NIR order. This is a valid emission order
        // for if/while/for/int-match's shapes: `lowering.rs` never makes a
        // later block's Phi/value visible to an earlier one — every
        // cross-block reference is either a jump target (fine, blocks are
        // pre-created above) or a Phi `incoming` entry resolved at the
        // jump site (below), never a bare value read across blocks.
        for block in &nir_func.blocks {
            let clif_block = clif_blocks[&block.id];
            builder.switch_to_block(clif_block);

            // Bind this block's own Phi dsts to their Cranelift block
            // params (the entry block's "phis" are function params,
            // already bound above — skip it here).
            let phi_count = if block.id == entry_nir_id {
                0
            } else {
                // Leading Phis need no binding here: their dsts were
                // declared as `Variable`s up front, and each predecessor
                // defines them on the way in (see `define_phi_edges` at
                // the terminators below). `use_var` past the merge
                // resolves through the builder's inferred params. Just
                // count them so the loop below skips past them.
                block_phis.get(&block.id).map_or(0, |v| v.len())
            };

            // Lower non-Phi instructions (Phis were consumed above and
            // never reach `lower_instr`).
            for instr in block.instrs.iter().skip(phi_count) {
                debug_assert!(
                    !matches!(instr, Instr::Phi { .. }),
                    "non-leading Phi in block {} — violates NIR's block-leading-Phi guarantee (NIR.md §4.1)",
                    block.id.0
                );
                let handled = lower_instr(&mut builder, &mut lower_ctx, instr);
                if !handled {
                    return Err(CompileError::UnsupportedInstr {
                        function: nir_func.name.clone(),
                        instr: format!("{instr}"),
                    });
                }
            }

            // Terminator: Return (Step 1) or Branch/CondBranch/Unreachable
            // (Step 2). `Switch` is deliberately still rejected — nothing
            // in the frontend emits it today (NIR.md §4), and this backend
            // leaves it alone per the approved design.
            match &block.terminator {
                Some(term @ Instr::Return { .. }) => {
                    let handled = lower_instr(&mut builder, &mut lower_ctx, term);
                    if !handled {
                        return Err(CompileError::UnsupportedInstr {
                            function: nir_func.name.clone(),
                            instr: format!("{term}"),
                        });
                    }
                }
                Some(Instr::Branch { target }) => {
                    // Edge copies first (defines the target's Phis from
                    // this path), then the bare jump — the builder fills
                    // the merge params itself.
                    define_phi_edges(
                        &mut builder,
                        &mut lower_ctx,
                        &block_phis,
                        *target,
                        block.id,
                        &nir_func.name,
                    )?;
                    builder.ins().jump(clif_blocks[target], &[]);
                }
                Some(Instr::CondBranch { cond, then_block, else_block }) => {
                    let cond_val = lower_ctx.get(&mut builder, *cond);
                    // Both edges' copies are emitted before the branch.
                    // Each incoming VALUE is read here, on the current
                    // path, before either successor runs — so every value
                    // placed is correct for its edge. The `def_var`s
                    // themselves do persist into both successors (Cranelift
                    // has no edge-scoped definitions), which is sound
                    // because a well-formed program never reads one edge's
                    // Phi dst on the other edge's path: such a read would
                    // be use-before-definition, which the frontend rejects
                    // (and which is stale-slot garbage in the VM too — a
                    // pre-existing frontend gap, not a backend one).
                    define_phi_edges(
                        &mut builder,
                        &mut lower_ctx,
                        &block_phis,
                        *then_block,
                        block.id,
                        &nir_func.name,
                    )?;
                    define_phi_edges(
                        &mut builder,
                        &mut lower_ctx,
                        &block_phis,
                        *else_block,
                        block.id,
                        &nir_func.name,
                    )?;
                    builder.ins().brif(
                        cond_val,
                        clif_blocks[then_block],
                        &[],
                        clif_blocks[else_block],
                        &[],
                    );
                }
                Some(Instr::Unreachable) => {
                    let ptr = builder.ins().symbol_value(clif_types::I64, unreachable_gv);
                    let len = builder.ins().iconst(clif_types::I64, unreachable_msg.len() as i64);
                    let call_inst = builder.ins().call(lower_ctx.rt.str_from_parts, &[ptr, len]);
                    let msg = builder.inst_results(call_inst)[0];
                    // 1 == NOCTIVUE_TRAP_EXIT_CODE (all traps exit 1 per
                    // the non-negotiables). If runtime-native exports this
                    // as a named constant, wire it through abi.rs instead
                    // of this literal — verify before merging, don't let
                    // the two drift.
                    let exit_code = builder.ins().iconst(clif_types::I32, 1);
                    builder.ins().call(lower_ctx.rt.panic, &[msg, exit_code]);
                    // `noctivue_rt_panic` exits the process at runtime and
                    // never actually reaches here, but its Cranelift-level
                    // signature (abi.rs's RUNTIME_IMPORTS row) declares
                    // zero results with no diverging-call marker, so the
                    // verifier still requires a syntactically valid
                    // terminator after the call. This return is provably
                    // dead code; its value exists only to satisfy the
                    // verifier, matching the same defined-dummy idiom this
                    // file already uses for Unit-typed values elsewhere
                    // (see `Call`'s Unit-return arm in lower.rs).
                    if lower_ctx.is_entry || lower_ctx.ret_is_unit {
                        builder.ins().return_(&[]);
                    } else {
                        let dummy = match &nir_func.sig.ret.inner {
                            Ty::Float => builder.ins().f64const(0.0),
                            other => builder.ins().iconst(clif_type_for(other), 0),
                        };
                        builder.ins().return_(&[dummy]);
                    }
                }
                Some(Instr::Switch { .. }) => {
                    return Err(CompileError::UnsupportedInstr {
                        function: nir_func.name.clone(),
                        instr: "switch".to_string(),
                    });
                }
                Some(other) => {
                    return Err(CompileError::UnsupportedInstr {
                        function: nir_func.name.clone(),
                        instr: format!("{other}"),
                    });
                }
                None => {
                    return Err(CompileError::MalformedFunction {
                        function: nir_func.name.clone(),
                        reason: format!("block {} has no terminator", block.id.0),
                    });
                }
            }
        }

        // Every jump above has now declared its target's predecessors,
        // so every block can be sealed: no more predecessors will ever be
        // added. `finalize` requires this; sealing earlier breaks jump
        // emission (see the note where the seal loop used to live).
        for block in &nir_func.blocks {
            builder.seal_block(clif_blocks[&block.id]);
        }

        // 0.135's `finalize` takes the ISA's frontend config explicitly
        // (it used to be argument-free); `isa()` is still reachable through
        // the module after `ObjectBuilder::new` consumed the owned ISA.
        let frontend_config = obj_module.isa().frontend_config();
        builder.finalize(frontend_config);

        // Debug escape hatch: NOCTIVUE_DUMP_IR=1 prints each function's
        // Cranelift IR before define. Permanent, env-gated (zero cost
        // otherwise) — silent miscompiles are the backend's worst failure
        // mode and printf-debugging them through object bytes is hopeless.
        // (Placed after `finalize`, which consumes the builder and ends
        // its `&mut ctx.func` borrow — displaying earlier borrows `ctx`
        // while the builder still holds it.)
        if std::env::var("NOCTIVUE_DUMP_IR").as_deref() == Ok("1") {
            eprintln!("--- IR for `{}` ---\n{}", nir_func.name, ctx.func.display());
        }
    }

    obj_module.define_function(clif_id, &mut ctx).map_err(|e| {
        // The verifier/emit error alone ("Verifier errors") never says
        // WHAT failed — dump the function's IR with it. Error path only,
        // so no cost on success.
        CompileError::Cranelift(format!(
            "{e} (in function `{}`):\n{}",
            nir_func.name,
            ctx.func.display()
        ))
    })?;

    Ok(())
}

/// Define each of `target`'s leading-Phi dsts from `from`'s incoming
/// values ("phi as parallel copies on the incoming edges"). Called at
/// every terminator that jumps into a block with Phis, BEFORE emitting
/// the jump itself — the definitions belong to the end of `from`, so the
/// merge block's later `use_var`s resolve through the builder's inferred
/// params. Jumps then carry no explicit arguments at all.
///
/// Missing incoming entry for `from` is NIR.md §4.1 item 10 malformed
/// CFG → hard error naming the edge, never a default (the native twin of
/// the VM's `MalformedCfg` trap on the same shape).
fn define_phi_edges(
    builder: &mut FunctionBuilder,
    ctx: &mut FuncLowerCtx,
    block_phis: &HashMap<NirBlockId, Vec<(ValueId, Vec<(ValueId, NirBlockId)>)>>,
    target: NirBlockId,
    from: NirBlockId,
    function: &str,
) -> Result<(), CompileError> {
    let phis = block_phis.get(&target).map(|v| v.as_slice()).unwrap_or(&[]);
    for (dst, incoming) in phis {
        let (val_id, _) = incoming.iter().find(|(_, b)| *b == from).ok_or_else(|| {
            CompileError::MalformedFunction {
                function: function.to_string(),
                reason: format!(
                    "block {} has a Phi ({}) with no incoming entry for predecessor {}",
                    target.0, dst, from.0
                ),
            }
        })?;
        let v = ctx.get(builder, *val_id);
        ctx.set(builder, *dst, v);
    }
    Ok(())
}

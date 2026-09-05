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

use cranelift_codegen::ir::{AbiParam, FuncRef, GlobalValue, Signature, UserFuncName};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, FuncId as ClifFuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::hir::types::Ty;
use crate::nir::instr::{ConstValue, Instr};
use crate::nir::module::NirModule;
use crate::nir::types::{FuncId as NirFuncId, ValueId};

use super::abi::{
    NOCTIVUE_RT_CHECKED_SDIV, NOCTIVUE_RT_CHECKED_SREM, NOCTIVUE_RT_CHECKED_UDIV,
    NOCTIVUE_RT_CHECKED_UREM, NOCTIVUE_RT_FREM, NOCTIVUE_RT_PANIC, NOCTIVUE_RT_PRINT,
    NOCTIVUE_RT_STR_CONCAT, NOCTIVUE_RT_STR_FROM_BOOL, NOCTIVUE_RT_STR_FROM_FLOAT,
    NOCTIVUE_RT_STR_FROM_INT, NOCTIVUE_RT_STR_FROM_PARTS, RUNTIME_IMPORTS,
};
use super::lower::{lower_instr, signature_for, FuncLowerCtx, RtRefs};

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

    // Step 1 scope check: every function is a single block ending in
    // Return. Anything else is a hard error naming the offending
    // instruction (see module doc comment).
    if nir_func.blocks.len() != 1 {
        return Err(CompileError::UnsupportedInstr {
            function: nir_func.name.clone(),
            instr: "multi-block function (control flow)".to_string(),
        });
    }
    let entry = nir_func.entry_block().ok_or_else(|| CompileError::MalformedFunction {
        function: nir_func.name.clone(),
        reason: "no entry block".to_string(),
    })?;

    // Pre-pass A: string literals become `.rodata` + `GlobalValue`s now,
    // while `&mut ctx.func` is still free (the builder will own it below).
    // Names embed the defining function, dst ValueId, and a counter so
    // two identical literals never collide.
    let mut strings: HashMap<ValueId, (GlobalValue, i64)> = HashMap::new();
    let prefix = sanitize_symbol(&nir_func.name);
    let mut str_counter: u64 = 0;
    for instr in &entry.instrs {
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
    };

    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, fb_ctx);
        let clif_block = builder.create_block();
        builder.append_block_params_for_function_params(clif_block);
        builder.switch_to_block(clif_block);
        builder.seal_block(clif_block);

        let mut lower_ctx = FuncLowerCtx {
            funcrefs: &funcrefs,
            values: HashMap::new(),
            strings,
            rt,
            ret_is_unit: matches!(nir_func.sig.ret.inner, Ty::Unit),
            is_entry,
        };
        // Bind NIR block params (function params) to the Cranelift
        // block's params, in order.
        let clif_params = builder.block_params(clif_block).to_vec();
        for (i, (val_id, _ty)) in entry.params.iter().enumerate() {
            let p = *clif_params.get(i).ok_or_else(|| CompileError::MalformedFunction {
                function: nir_func.name.clone(),
                reason: format!("NIR declares {} params, Cranelift signature has {}", entry.params.len(), clif_params.len()),
            })?;
            lower_ctx.values.insert(*val_id, p);
        }

        for instr in &entry.instrs {
            let handled = lower_instr(&mut builder, &mut lower_ctx, instr);
            if !handled {
                return Err(CompileError::UnsupportedInstr {
                    function: nir_func.name.clone(),
                    instr: format!("{instr}"),
                });
            }
        }

        match &entry.terminator {
            Some(term @ Instr::Return { .. }) => {
                let handled = lower_instr(&mut builder, &mut lower_ctx, term);
                if !handled {
                    return Err(CompileError::UnsupportedInstr {
                        function: nir_func.name.clone(),
                        instr: format!("{term}"),
                    });
                }
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
                    reason: "entry block has no terminator".to_string(),
                });
            }
        }

        // 0.135's `finalize` takes the ISA's frontend config explicitly
        // (it used to be argument-free); `isa()` is still reachable through
        // the module after `ObjectBuilder::new` consumed the owned ISA.
        let frontend_config = obj_module.isa().frontend_config();
        builder.finalize(frontend_config);
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

//! NIR → Cranelift IR lowering — straight-line category only (Phase 3,
//! Step 1: "end-to-end plumbing").
//!
//! Handled here: constants (int/float/bool/unit/string), `Move` aliases,
//! integer/float arithmetic (+ string concat), checked division/remainder,
//! comparisons, `ToString`, intra-module `Call`, `Print`, `Return`.
//! Control flow (branch/cond_branch/switch/phi), aggregates
//! (struct_new/field_get/…), and managed-mode instructions are explicitly
//! NOT handled — see Step 2 of the Phase 3 plan. Any instruction outside
//! this category reaching this lowerer returns `false` (the driver turns
//! that into a named compile error), never a silently-skipped instruction.
//!
//! Cross-function and runtime references are precomputed by the driver
//! (`FuncRef`s, string-literal `GlobalValue`s) before the builder exists,
//! so this file never touches the module mid-function — Cranelift's
//! `declare_*_in_func` APIs need `&mut Function`, which the builder owns.

use cranelift_codegen::ir::{
    types as clif_types, AbiParam, FuncRef, GlobalValue, InstBuilder, Signature, Type as ClifType,
    Value as ClifValue,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, Variable};
use std::collections::HashMap;

use crate::hir::types::Ty;
use crate::nir::instr::{CmpOp, ConstValue, Instr};
use crate::nir::types::{FuncId as NirFuncId, ValueId};

/// Fallible twin of `clif_type_for`: `None` for types with no native
/// representation yet (aggregates, closures — Step 3+). The driver skips
/// declaring those ids; any later touch panics honestly via
/// `FuncLowerCtx::get`/`set` (construction ops fail cleanly first in
/// practice, so the panic is a last-resort guard, not a normal path).
pub(crate) fn clif_type_for_opt(ty: &Ty) -> Option<ClifType> {
    match ty {
        Ty::Int | Ty::UInt | Ty::String => Some(clif_types::I64),
        Ty::Float => Some(clif_types::F64),
        Ty::Bool => Some(clif_types::I8),
        Ty::Unit => Some(clif_types::I8),
        _ => None,
    }
}

/// Maps every Noctivue-level `Ty` this category needs to a Cranelift
/// clif type. `String` is a single `I64` header pointer (see `abi.rs`'s
/// string-model note — the 1:1 ValueId→Value mapping depends on it).
/// Other aggregate/pointer types are out of scope for Step 1 and panic
/// if reached — Step 2 extends this once StackAlloc/Load/Store exist.
pub(crate) fn clif_type_for(ty: &Ty) -> ClifType {
    match ty {
        Ty::Int | Ty::UInt | Ty::String => clif_types::I64,
        Ty::Float => clif_types::F64,
        Ty::Bool => clif_types::I8,
        Ty::Unit => clif_types::I8, // Unit is represented but never read; matches VmValue::Unit's role as a real, defined value (see NIR.md §4.1 item 9's sibling concern for defined-not-uninitialized values).
        other => unimplemented!(
            "clif_type_for({:?}): aggregate/pointer types belong to Step 2 \
             (StackAlloc/Load/Store) or Step 4 (managed mode) of the Phase 3 \
             plan, not Step 1's straight-line lowering", other
        ),
    }
}

/// Pre-declared per-function references to every runtime import, in the
/// same order as `abi::RUNTIME_IMPORTS`. Plain data (all `FuncRef`s are
/// `Copy`): the driver builds one of these per function, this file only
/// reads it.
#[derive(Debug, Clone, Copy)]
pub struct RtRefs {
    pub print: FuncRef,
    pub panic: FuncRef,
    pub str_from_parts: FuncRef,
    pub str_from_int: FuncRef,
    pub str_from_float: FuncRef,
    pub str_from_bool: FuncRef,
    pub str_concat: FuncRef,
    pub checked_sdiv: FuncRef,
    pub checked_udiv: FuncRef,
    pub checked_srem: FuncRef,
    pub checked_urem: FuncRef,
    pub frem: FuncRef,
    pub dashboard_echo: FuncRef,
    pub dashboard_arith: FuncRef,
}

/// Per-function lowering context: NIR ValueId -> Cranelift `Variable`,
/// plus the precomputed cross-references the driver resolved up front.
///
/// Values are `Variable`s, NOT a ValueId->Value rebinding map — and that
/// distinction is load-bearing. NIR ids are MUTABLE SLOTS, not SSA
/// values: `Assign` reuses the target's EXISTING id (`Move { dst: v0,
/// src: v15 }` overwrites slot v0 — see `lowering.rs`'s `Assign` arm),
/// exactly like the VM's `frame.locals[dst] = ...`. A rebinding map is
/// correct for straight-line code but silently wrong across back-edges
/// (the header keeps referencing the pre-loop value forever — observed
/// as a hanging `while` binary, since the counter never advances).
/// `Variable`/`def_var`/`use_var` is Cranelift's mechanism for precisely
/// this shape: the SSA builder inserts the merge params itself, including
/// loop headers. Explicit `Instr::Phi`s are handled separately (edge
/// copies at predecessors — see `define_phi_edges` in driver.rs), so the
/// two mechanisms never double-handle a merge.
pub struct FuncLowerCtx<'a> {
    /// NIR FuncId -> already-imported `FuncRef` in the function under
    /// construction (covers self-recursion: every function imports every
    /// function, including itself).
    pub funcrefs: &'a HashMap<NirFuncId, FuncRef>,
    pub vars: HashMap<ValueId, Variable>,
    /// String-literal rodata: the `Const`'s dst ValueId -> (global value
    /// holding the byte address, byte length). Populated by the driver's
    /// data pass before lowering starts.
    pub strings: HashMap<ValueId, (GlobalValue, i64)>,
    pub rt: RtRefs,
    /// True when the enclosing NIR function returns `Unit` (its Cranelift
    /// signature therefore declares no return value — see
    /// `signature_for`). NIR still carries `Return { val: Some(unit) }`
    /// for such functions (the VM binds a real `VmValue::Unit`), so the
    /// Return arm must discard the value instead of returning it — without
    /// this flag the lowerer cannot tell "value to return" from "Unit
    /// dummy to drop", and emits `return v` against a `()` signature
    /// (verifier error).
    pub ret_is_unit: bool,
    /// True when lowering the `main` entry point. The native entry is
    /// ALWAYS `noctivue_main() -> ()` regardless of the source-level
    /// `main` return type (see the driver's entry-convention note): the
    /// Return arm drops any value and emits a bare return.
    pub is_entry: bool,
}

impl<'a> FuncLowerCtx<'a> {
    /// Read a NIR id through its `Variable`: resolves to the reaching
    /// definition on the current path (loop-carried values included —
    /// this is what a rebinding map cannot do).
    pub(crate) fn get(&self, builder: &mut FunctionBuilder, id: ValueId) -> ClifValue {
        let var = *self.vars.get(&id).unwrap_or_else(|| {
            panic!(
                "NIR ValueId {id} has no declared Cranelift variable — out-of-category \
                 type (aggregates/closures) reached representable-only lowering. This is \
                 a missing Step-2/3 category error surfacing as a backend panic instead \
                 of `UnsupportedInstr`: construction ops (which fail cleanly first) were \
                 bypassed, almost always via bare param-plumbing of an aggregate-typed \
                 value. Name the construct, don't just retry."
            )
        });
        builder.use_var(var)
    }

    /// Write a NIR id: `Move { dst: v0, src }` OVERWRITES slot v0 (see the
    /// struct doc comment) — never an alias, never a fresh binding.
    pub(crate) fn set(&mut self, builder: &mut FunctionBuilder, id: ValueId, val: ClifValue) {
        let var = *self.vars.get(&id).unwrap_or_else(|| {
            panic!(
                "NIR ValueId {id} has no declared Cranelift variable (see `get`'s \
                 message for what this means — same cause, write side)"
            )
        });
        builder.def_var(var, val);
    }
}

/// Emit `call fref(args)` and collect its results.
fn call(builder: &mut FunctionBuilder, fref: FuncRef, args: &[ClifValue]) -> Vec<ClifValue> {
    let inst = builder.ins().call(fref, args);
    builder.inst_results(inst).to_vec()
}

/// Emit `call fref(args)` requiring exactly one result.
fn call_one(builder: &mut FunctionBuilder, fref: FuncRef, args: &[ClifValue]) -> ClifValue {
    let results = call(builder, fref, args);
    *results.first().expect(
        "runtime helper declared with exactly one return in abi::RUNTIME_IMPORTS \
         must produce exactly one result — declaration/use drifted",
    )
}

/// Lower one instruction from the straight-line category. Returns `false`
/// if `instr` is outside this category (the driver reports that as
/// `CompileError::UnsupportedInstr` naming the function and instruction).
pub fn lower_instr(
    builder: &mut FunctionBuilder,
    ctx: &mut FuncLowerCtx,
    instr: &Instr,
) -> bool {
    match instr {
        // NOTE: the declared `ty` is deliberately IGNORED here — the value
        // is self-describing, and the frontend leaves some literal types
        // as `Unknown` (observed: match-case literals), which a
        // value↔type agreement assert tripped over. The VM does the same
        // (it matches on the value, never the declared type), so
        // value-derived codegen is parity-correct, not a shortcut. The
        // declaration side (`collect_value_types`) mirrors this exactly.
        Instr::Const { dst, value, .. } => {
            let v = match value {
                ConstValue::Int(i) => builder.ins().iconst(clif_types::I64, *i as i64),
                ConstValue::Float(f) => builder.ins().f64const(*f),
                ConstValue::Bool(b) => builder.ins().iconst(clif_types::I8, *b as i64),
                ConstValue::Unit => builder.ins().iconst(clif_types::I8, 0),
                ConstValue::String(_) => {
                    let (gv, len) = ctx.strings.get(dst).copied().unwrap_or_else(|| {
                        panic!(
                            "string literal {dst} has no precomputed rodata — the driver's \
                             data pass must register every ConstValue::String before lowering"
                        )
                    });
                    let ptr = builder.ins().symbol_value(clif_types::I64, gv);
                    let len_val = builder.ins().iconst(clif_types::I64, len);
                    call_one(builder, ctx.rt.str_from_parts, &[ptr, len_val])
                }
                ConstValue::Char(_) => {
                    // Chars are I32 codepoints in principle, but nothing in
                    // Step 1's category consumes them yet (no char binops,
                    // no char ToString helper) — Step 2 owns them.
                    return false;
                }
            };
            ctx.set(builder, *dst, v);
        }

        // Slot overwrite, no instruction emitted: `Move { dst, src }`
        // redefines slot `dst` to `src`'s current value (see the struct
        // doc comment — `Assign` reuses the target's id, so this is the
        // instruction that makes `while` counters advance). Mechanically
        // identical to any other `set`, kept as its own arm so the
        // slot-semantics stays visible at the one site that matters.
        Instr::Move { dst, src } => {
            let s = ctx.get(builder, *src);
            ctx.set(builder, *dst, s);
        }

        Instr::Add { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(builder, *lhs), ctx.get(builder, *rhs));
            let v = match &ty.inner {
                Ty::Float => builder.ins().fadd(l, r),
                Ty::String => call_one(builder, ctx.rt.str_concat, &[l, r]),
                Ty::Int | Ty::UInt => builder.ins().iadd(l, r),
                other => unimplemented!(
                    "Add on {other:?}: typeck guarantees numeric-or-String operands; \
                     reaching here means a typeck bug, not a backend gap"
                ),
            };
            ctx.set(builder, *dst, v);
        }
        Instr::Sub { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(builder, *lhs), ctx.get(builder, *rhs));
            let v = match &ty.inner {
                Ty::Float => builder.ins().fsub(l, r),
                Ty::Int | Ty::UInt => builder.ins().isub(l, r),
                other => unimplemented!(
                    "Sub on {other:?}: typeck guarantees numeric operands; \
                     reaching here means a typeck bug, not a backend gap"
                ),
            };
            ctx.set(builder, *dst, v);
        }
        Instr::Mul { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(builder, *lhs), ctx.get(builder, *rhs));
            let v = match &ty.inner {
                Ty::Float => builder.ins().fmul(l, r),
                Ty::Int | Ty::UInt => builder.ins().imul(l, r),
                other => unimplemented!(
                    "Mul on {other:?}: typeck guarantees numeric operands; \
                     reaching here means a typeck bug, not a backend gap"
                ),
            };
            ctx.set(builder, *dst, v);
        }
        Instr::Div { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(builder, *lhs), ctx.get(builder, *rhs));
            // Integer division NEVER emits a bare sdiv/udiv: those trap the
            // process with no Noctivue-level diagnostic (the parity gap the
            // old NOTE comment on this arm warned about). The checked
            // helpers own the zero-check + message + exit code (see
            // runtime-native), so native div-by-zero is observable-identical
            // to the VM's DivisionByZero trap class.
            let v = match &ty.inner {
                Ty::Float => builder.ins().fdiv(l, r),
                Ty::Int => call_one(builder, ctx.rt.checked_sdiv, &[l, r]),
                Ty::UInt => call_one(builder, ctx.rt.checked_udiv, &[l, r]),
                other => unimplemented!(
                    "Div on {other:?}: typeck guarantees numeric operands; \
                     reaching here means a typeck bug, not a backend gap"
                ),
            };
            ctx.set(builder, *dst, v);
        }
        Instr::Rem { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(builder, *lhs), ctx.get(builder, *rhs));
            let v = match &ty.inner {
                // No `frem` in Cranelift IR — runtime helper, never traps
                // (matches the VM's float `%`, NaN/inf included).
                Ty::Float => call_one(builder, ctx.rt.frem, &[l, r]),
                Ty::Int => call_one(builder, ctx.rt.checked_srem, &[l, r]),
                Ty::UInt => call_one(builder, ctx.rt.checked_urem, &[l, r]),
                other => unimplemented!(
                    "Rem on {other:?}: typeck guarantees numeric operands; \
                     reaching here means a typeck bug, not a backend gap"
                ),
            };
            ctx.set(builder, *dst, v);
        }
        Instr::Neg { dst, src, ty } => {
            let s = ctx.get(builder, *src);
            let v = if matches!(ty.inner, Ty::Float) {
                builder.ins().fneg(s)
            } else {
                builder.ins().ineg(s)
            };
            ctx.set(builder, *dst, v);
        }
        Instr::Not { dst, src } => {
            let s = ctx.get(builder, *src);
            // Bool-not: bitwise_not then mask to 0/1. (Int-not per the VM's
            // `not_value`, which also accepts Int, would need a separate
            // path — Step 1's fixtures are Bool-only for `!`; extend when
            // an Int-`!` fixture actually exists.)
            let v = builder.ins().bxor_imm_u(s, 1);
            ctx.set(builder, *dst, v);
        }
        Instr::ICmp { dst, op, lhs, rhs } => {
            let (l, r) = (ctx.get(builder, *lhs), ctx.get(builder, *rhs));
            use cranelift_codegen::ir::condcodes::IntCC;
            let cc = match op {
                CmpOp::Eq => IntCC::Equal,
                CmpOp::Ne => IntCC::NotEqual,
                CmpOp::Lt => IntCC::SignedLessThan,
                CmpOp::Le => IntCC::SignedLessThanOrEqual,
                CmpOp::Gt => IntCC::SignedGreaterThan,
                CmpOp::Ge => IntCC::SignedGreaterThanOrEqual,
            };
            let v = builder.ins().icmp(cc, l, r);
            ctx.set(builder, *dst, v);
        }
        Instr::FCmp { dst, op, lhs, rhs } => {
            let (l, r) = (ctx.get(builder, *lhs), ctx.get(builder, *rhs));
            use cranelift_codegen::ir::condcodes::FloatCC;
            let cc = match op {
                CmpOp::Eq => FloatCC::Equal,
                CmpOp::Ne => FloatCC::NotEqual,
                CmpOp::Lt => FloatCC::LessThan,
                CmpOp::Le => FloatCC::LessThanOrEqual,
                CmpOp::Gt => FloatCC::GreaterThan,
                CmpOp::Ge => FloatCC::GreaterThanOrEqual,
            };
            let v = builder.ins().fcmp(cc, l, r);
            ctx.set(builder, *dst, v);
        }

        Instr::ToString { dst, src, from_ty } => {
            let s = ctx.get(builder, *src);
            // Type-directed: each source type converts differently, and
            // the NIR records the type at lowering time (when it is known)
            // precisely so this backend doesn't have to rediscover it.
            let v = match from_ty {
                Ty::Int => call_one(builder, ctx.rt.str_from_int, &[s]),
                Ty::Float => call_one(builder, ctx.rt.str_from_float, &[s]),
                Ty::Bool => call_one(builder, ctx.rt.str_from_bool, &[s]),
                // String-to-String is the identity (interpolation of an
                // already-string part emits ToString per lowering.rs).
                Ty::String => s,
                // Char/Unit/aggregates: no runtime helper yet — Step 2+.
                _ => return false,
            };
            ctx.set(builder, *dst, v);
        }

        Instr::Call { dst, func, args, ret_ty } => {
            let fref = *ctx.funcrefs.get(func).unwrap_or_else(|| {
                panic!(
                    "call to NIR function {func} has no precomputed FuncRef — the driver \
                     must import every module function into every function body"
                )
            });
            let arg_vals: Vec<ClifValue> = args.iter().map(|a| ctx.get(builder, *a)).collect();
            let results = call(builder, fref, &arg_vals);
            // Unit-returning callees declare no Cranelift return (see
            // `signature_for`), so there is no SSA value to bind — dst gets
            // the same dummy Unit representation `Const Unit` uses. The VM
            // binds a real `VmValue::Unit` here; both are defined values
            // that carry no information, never meaningfully read.
            let v = if matches!(ret_ty.inner, Ty::Unit) {
                builder.ins().iconst(clif_types::I8, 0)
            } else {
                *results.first().expect(
                    "non-Unit call produced no result — signature_for declares one \
                     return for every non-Unit type; declaration/use drifted",
                )
            };
            ctx.set(builder, *dst, v);
        }

        Instr::Print { val } => {
            // One header-pointer argument (abi.rs string model). No dst:
            // Print is a statement-shaped instruction in a value world.
            let s = ctx.get(builder, *val);
            call(builder, ctx.rt.print, &[s]);
        }

        Instr::Return { val } => {
            // The native entry returns nothing by construction (driver's
            // entry convention): validate the value's binding (so a
            // dangling ValueId still fails loudly here) then drop it. A
            // `None` in a non-Unit, non-entry function is malformed NIR —
            // emitting bare `return_` lets the verifier reject it loudly
            // rather than silently returning garbage.
            if ctx.is_entry {
                if let Some(v) = val {
                    ctx.get(builder, *v);
                }
                builder.ins().return_(&[]);
            } else if ctx.ret_is_unit {
                if let Some(v) = val {
                    ctx.get(builder, *v);
                }
                builder.ins().return_(&[]);
            } else {
                match val {
                    Some(v) => {
                        let rv = ctx.get(builder, *v);
                        builder.ins().return_(&[rv]);
                    }
                    None => {
                        builder.ins().return_(&[]);
                    }
                }
            }
        }

        // Everything else belongs to a later step.
        Instr::StackAlloc { .. } | Instr::Load { .. } | Instr::Store { .. }
        | Instr::StructNew { .. } | Instr::FieldGet { .. }
        | Instr::FieldSet { .. } | Instr::ListLen { .. } | Instr::ListIndex { .. }
        | Instr::EnumTag { .. } | Instr::EnumPayload { .. } | Instr::EnumNew { .. }
        | Instr::Branch { .. } | Instr::CondBranch { .. } | Instr::Switch { .. }
        | Instr::Phi { .. } | Instr::Unreachable | Instr::EarlyReturn { .. }
        | Instr::ResultOk { .. } | Instr::ResultErr { .. } | Instr::TryUnwrap { .. }
        | Instr::OptionSome { .. } | Instr::OptionNone { .. }
        | Instr::ClosureNew { .. } | Instr::ClosureCall { .. }
        | Instr::CallIndirect { .. } | Instr::HeapAlloc { .. } | Instr::ArcRetain { .. }
        | Instr::ArcRelease { .. } | Instr::WeakLoad { .. } => {
            return false; // not this category — caller dispatches to Step 2/3/4
        }
    }
    true
}

/// Build a Cranelift `Signature` from a NIR `FuncSig`, for the
/// straight-line subset (scalar + String params/return only — Step 2
/// extends this once aggregates can appear in signatures). `Unit`
/// returns declare no result; callers and callees agree by construction
/// because both go through this function.
pub fn signature_for(sig: &crate::nir::types::FuncSig, call_conv: CallConv) -> Signature {
    let mut s = Signature::new(call_conv);
    for p in &sig.params {
        s.params.push(AbiParam::new(clif_type_for(&p.inner)));
    }
    if !matches!(sig.ret.inner, Ty::Unit) {
        s.returns.push(AbiParam::new(clif_type_for(&sig.ret.inner)));
    }
    s
}

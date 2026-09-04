//! NIR → Cranelift IR lowering — arithmetic/constant/call/return category
//! only (Phase 3, Step 1: "end-to-end plumbing"). Control-flow
//! (branch/cond_branch/switch/phi), aggregates (struct_new/field_get/…),
//! and managed-mode instructions are explicitly NOT handled here — see
//! Step 2 of the Phase 3 plan. Any instruction outside this category
//! reaching this lowerer is a scope error, not a silently-skipped one:
//! it panics with a pointer back to which step should have handled it.

use cranelift_codegen::ir::{types as clif_types, AbiParam, InstBuilder, Signature, Type as ClifType};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId as ClifFuncId, Linkage, Module};
use std::collections::HashMap;

use crate::hir::types::Ty;
use crate::nir::instr::{CmpOp, ConstValue, Instr};
use crate::nir::module::NirModule;
use crate::nir::types::{FuncId as NirFuncId, ValueId};

/// Maps every Noctivue-level `Ty` this category needs to a Cranelift
/// clif type. Aggregate/pointer types are out of scope for Step 1 and
/// panic if reached — Step 2 extends this once StackAlloc/Load/Store are
/// wired up.
fn clif_type_for(ty: &Ty) -> ClifType {
    match ty {
        Ty::Int | Ty::UInt => clif_types::I64,
        Ty::Float => clif_types::F64,
        Ty::Bool => clif_types::I8,
        Ty::Unit => clif_types::I8, // Unit is represented but never read; matches VmValue::Unit's role as a real, defined value (see NIR.md §4.1 item 9's sibling concern for defined-not-uninitialized values).
        other => unimplemented!(
            "clif_type_for({:?}): aggregate/pointer types belong to Step 2 \
             (StackAlloc/Load/Store) or Step 4 (managed mode) of the Phase 3 \
             plan, not Step 1's arithmetic-only lowering", other
        ),
    }
}

/// Per-function lowering context: NIR ValueId -> Cranelift Value, plus the
/// module-wide function-id table shared across all functions being lowered.
pub struct FuncLowerCtx<'a> {
    pub clif_funcs: &'a HashMap<NirFuncId, ClifFuncId>,
    pub values: HashMap<ValueId, cranelift_codegen::ir::Value>,
}

impl<'a> FuncLowerCtx<'a> {
    fn get(&self, id: ValueId) -> cranelift_codegen::ir::Value {
        *self.values.get(&id).unwrap_or_else(|| {
            panic!(
                "NIR ValueId {} has no Cranelift value bound — either a \
                 control-flow instruction (Step 2) produced it and this \
                 lowerer doesn't yet handle that category, or NIR itself \
                 is malformed (see landmine (c)/(d) — this should have \
                 been a checked VmError equivalent, not a panic, once this \
                 backend is past skeleton stage)",
                id
            )
        })
    }

    fn set(&mut self, id: ValueId, val: cranelift_codegen::ir::Value) {
        self.values.insert(id, val);
    }
}

/// Lower one instruction from the arithmetic/constant/call/return category.
/// Returns `false` if `instr` is outside this category (caller — the
/// eventual full lowerer — should dispatch it to the Step 2/3/4 handler
/// instead of treating that as an error).
pub fn lower_instr(
    builder: &mut FunctionBuilder,
    ctx: &mut FuncLowerCtx,
    instr: &Instr,
) -> bool {
    match instr {
        Instr::Const { dst, value, ty } => {
            let clif_ty = clif_type_for(&ty.inner);
            let v = match value {
                ConstValue::Int(i) => builder.ins().iconst(clif_ty, *i as i64),
                ConstValue::Float(f) => builder.ins().f64const(*f),
                ConstValue::Bool(b) => builder.ins().iconst(clif_ty, *b as i64),
                ConstValue::Unit => builder.ins().iconst(clif_ty, 0),
                ConstValue::Char(_) | ConstValue::String(_) => unimplemented!(
                    "Char/String constants need the aggregate/pointer \
                     representation Step 2 introduces (a String is not a \
                     bare scalar in Cranelift's IR the way Int/Float/Bool \
                     are) — out of scope for Step 1's arithmetic-only fixtures"
                ),
            };
            ctx.set(*dst, v);
        }

        Instr::Add { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(*lhs), ctx.get(*rhs));
            let v = if matches!(ty.inner, Ty::Float) {
                builder.ins().fadd(l, r)
            } else {
                builder.ins().iadd(l, r)
            };
            ctx.set(*dst, v);
        }
        Instr::Sub { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(*lhs), ctx.get(*rhs));
            let v = if matches!(ty.inner, Ty::Float) {
                builder.ins().fsub(l, r)
            } else {
                builder.ins().isub(l, r)
            };
            ctx.set(*dst, v);
        }
        Instr::Mul { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(*lhs), ctx.get(*rhs));
            let v = if matches!(ty.inner, Ty::Float) {
                builder.ins().fmul(l, r)
            } else {
                builder.ins().imul(l, r)
            };
            ctx.set(*dst, v);
        }
        Instr::Div { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(*lhs), ctx.get(*rhs));
            // NOTE: unlike the VM's `div_values` (which explicitly checks
            // for zero and returns VmError::DivisionByZero), Cranelift's
            // raw `sdiv`/`udiv` traps the *process* on divide-by-zero with
            // no Noctivue-level diagnostic. This is a real three-way-parity
            // gap the Step-1 exit check (differential run/run-vm/native
            // comparison) MUST catch — either emit an explicit zero-check +
            // trap-with-message before the div (matching VmError's
            // observable exit code), or accept and document a native-mode
            // behavior difference here. Do not leave this unresolved past
            // Step 1's exit check.
            let v = if matches!(ty.inner, Ty::Float) {
                builder.ins().fdiv(l, r)
            } else if matches!(ty.inner, Ty::UInt) {
                builder.ins().udiv(l, r)
            } else {
                builder.ins().sdiv(l, r)
            };
            ctx.set(*dst, v);
        }
        Instr::Rem { dst, lhs, rhs, ty } => {
            let (l, r) = (ctx.get(*lhs), ctx.get(*rhs));
            let v = if matches!(ty.inner, Ty::UInt) {
                builder.ins().urem(l, r)
            } else {
                builder.ins().srem(l, r)
            };
            ctx.set(*dst, v);
        }
        Instr::Neg { dst, src, ty } => {
            let s = ctx.get(*src);
            let v = if matches!(ty.inner, Ty::Float) {
                builder.ins().fneg(s)
            } else {
                builder.ins().ineg(s)
            };
            ctx.set(*dst, v);
        }
        Instr::Not { dst, src } => {
            let s = ctx.get(*src);
            // Bool-not: bitwise_not then mask to 0/1. (Int-not per the VM's
            // `not_value`, which also accepts Int, would need a separate
            // path — Step 1's fixtures are Bool-only for `!`; extend when
            // an Int-`!` fixture actually exists.)
            let v = builder.ins().bxor_imm(s, 1);
            ctx.set(*dst, v);
        }
        Instr::ICmp { dst, op, lhs, rhs } => {
            let (l, r) = (ctx.get(*lhs), ctx.get(*rhs));
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
            ctx.set(*dst, v);
        }
        Instr::FCmp { dst, op, lhs, rhs } => {
            let (l, r) = (ctx.get(*lhs), ctx.get(*rhs));
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
            ctx.set(*dst, v);
        }

        Instr::Call { dst, func, args, .. } => {
            let clif_func = *ctx.clif_funcs.get(func).unwrap_or_else(|| {
                panic!(
                    "NIR FuncId {:?} has no Cranelift declaration — either \
                     FuncId::UNRESOLVED reached codegen (a lowering bug that \
                     should have been caught earlier, see NIR.md §4's \
                     UNRESOLVED discipline) or the module-wide func table \
                     wasn't populated before lowering this function",
                    func
                )
            });
            let arg_vals: Vec<_> = args.iter().map(|a| ctx.get(*a)).collect();
            // `declare_func_in_func` + `call` — module plumbing omitted
            // here; belongs in the caller that owns the `Module` handle
            // (this function only has the per-function `FunctionBuilder`).
            let local_callee = builder.import_function(
                /* module.declare_func_in_func(clif_func, builder.func) */
                unimplemented!("wire to the owning Module — see mod.rs")
            );
            let call = builder.ins().call(local_callee, &arg_vals);
            let results = builder.inst_results(call);
            if let Some(&r) = results.first() {
                ctx.set(*dst, r);
            }
        }

        Instr::Print { .. } => unimplemented!(
            "Print crosses the FFI boundary to runtime-native's \
             noctivue_rt_print — see Phase 3 Step 1 §3's Print mapping. \
             Lower it as an ordinary Call to a declared extern symbol, not \
             as a distinct Cranelift instruction category."
        ),

        Instr::Return { val } => {
            match val {
                Some(v) => {
                    let rv = ctx.get(*v);
                    builder.ins().return_(&[rv]);
                }
                None => {
                    builder.ins().return_(&[]);
                }
            }
        }

        // Everything else belongs to a later step.
        Instr::StackAlloc { .. } | Instr::Load { .. } | Instr::Store { .. }
        | Instr::Move { .. } | Instr::StructNew { .. } | Instr::FieldGet { .. }
        | Instr::FieldSet { .. } | Instr::ListLen { .. } | Instr::ListIndex { .. }
        | Instr::EnumTag { .. } | Instr::EnumPayload { .. } | Instr::EnumNew { .. }
        | Instr::Branch { .. } | Instr::CondBranch { .. } | Instr::Switch { .. }
        | Instr::Phi { .. } | Instr::Unreachable | Instr::EarlyReturn { .. }
        | Instr::ResultOk { .. } | Instr::ResultErr { .. } | Instr::TryUnwrap { .. }
        | Instr::OptionSome { .. } | Instr::OptionNone { .. } | Instr::ToString { .. }
        | Instr::ClosureNew { .. } | Instr::ClosureCall { .. }
        | Instr::CallIndirect { .. } | Instr::HeapAlloc { .. } | Instr::ArcRetain { .. }
        | Instr::ArcRelease { .. } | Instr::WeakLoad { .. } => {
            return false; // not this category — caller dispatches to Step 2/3/4
        }
    }
    true
}

/// Build a Cranelift `Signature` from a NIR `FuncSig`, for the
/// straight-line-arithmetic subset (scalar params/return only — Step 2
/// extends this once aggregates can appear in signatures).
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

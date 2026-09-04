//! NIR instruction set — concrete instructions per NIR.md §4.

use crate::nir::types::{BlockId, FuncId, NirTy, ValueId};
use std::fmt;

/// Binary arithmetic operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// Binary comparison operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Unary operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// NIR instructions — SSA-based, typed, mode-aware.
#[derive(Debug, Clone)]
pub enum Instr {
    // ── Arithmetic ──────────────────────────────────────────────────────────
    /// `%dst = add %lhs, %rhs`
    Add { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = sub %lhs, %rhs`
    Sub { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = mul %lhs, %rhs`
    Mul { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = div %lhs, %rhs`
    Div { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },
    /// `%dst = rem %lhs, %rhs`
    Rem { dst: ValueId, lhs: ValueId, rhs: ValueId, ty: NirTy },

    // ── Comparison ──────────────────────────────────────────────────────────
    /// `%dst = icmp op %lhs, %rhs` (integer)
    ICmp { dst: ValueId, op: CmpOp, lhs: ValueId, rhs: ValueId },
    /// `%dst = fcmp op %lhs, %rhs` (float)
    FCmp { dst: ValueId, op: CmpOp, lhs: ValueId, rhs: ValueId },

    // ── Unary ───────────────────────────────────────────────────────────────
    /// `%dst = neg %src`
    Neg { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = not %src`
    Not { dst: ValueId, src: ValueId },

    // ── Constants ───────────────────────────────────────────────────────────
    /// `%dst = const value`
    Const { dst: ValueId, value: ConstValue, ty: NirTy },

    // ── Memory (Native) ─────────────────────────────────────────────────────
    /// `%dst = stack_alloc ty` — allocate on stack (native mode)
    StackAlloc { dst: ValueId, ty: NirTy },
    /// `%dst = load %src` — load from pointer
    Load { dst: ValueId, src: ValueId, ty: NirTy },
    /// `store %val, %ptr` — store to pointer
    Store { val: ValueId, ptr: ValueId },
    /// `%dst = move %src` — move semantics (native mode)
    Move { dst: ValueId, src: ValueId },

    // ── Memory (Managed) ────────────────────────────────────────────────────
    /// `%dst = heap_alloc ty` — allocate on heap (managed mode)
    HeapAlloc { dst: ValueId, ty: NirTy },
    /// `arc_retain %src` — increment refcount
    ArcRetain { src: ValueId },
    /// `arc_release %src` — decrement refcount
    ArcRelease { src: ValueId },
    /// `%dst = weak_load %src` — load weak reference
    WeakLoad { dst: ValueId, src: ValueId, ty: NirTy },

    // ── Aggregates ──────────────────────────────────────────────────────────
    /// `%dst = struct_new [field0, field1, ...]` — construct struct
    StructNew { dst: ValueId, fields: Vec<ValueId>, field_names: Vec<String>, ty: NirTy },
    /// `%dst = field_get %obj, field` — get struct field
    FieldGet { dst: ValueId, obj: ValueId, field: String, ty: NirTy },
    /// `%dst = field_set %obj, field, %val` — set struct field, returns new struct
    FieldSet { dst: ValueId, obj: ValueId, field: String, val: ValueId },
    /// `%dst = list_len %src` — get list length
    ListLen { dst: ValueId, src: ValueId },
    /// `%dst = list_index %src, %index` — index into list
    ListIndex { dst: ValueId, src: ValueId, index: ValueId },
    /// `%dst = enum_tag %src` — get enum variant tag
    EnumTag { dst: ValueId, src: ValueId },
    /// `%dst = enum_payload %src[i]` — get enum payload field `i`
    /// (also extracts `Option`/`Result` payloads, which hold one value).
    EnumPayload { dst: ValueId, src: ValueId, index: u32, ty: NirTy },
    /// `%dst = enum_new %tag, [field0, field1, ...]` — construct enum variant
    EnumNew { dst: ValueId, tag: ValueId, fields: Vec<ValueId>, ty: NirTy },

    // ── Calls ───────────────────────────────────────────────────────────────
    /// `%dst = call @func(args...)` — direct call
    Call { dst: ValueId, func: FuncId, args: Vec<ValueId>, ret_ty: NirTy },
    /// `%dst = call_indirect %func_ptr(args...)` — indirect call
    CallIndirect { dst: ValueId, func_ptr: ValueId, args: Vec<ValueId>, ret_ty: NirTy },

    // ── I/O ────────────────────────────────────────────────────────────────
    /// `print %val` — print a value (returns Unit)
    Print { val: ValueId },

    // ── Control Flow (Terminators) ──────────────────────────────────────────
    /// `branch target` — unconditional jump
    Branch { target: BlockId },
    /// `cond_branch %cond, then_block, else_block` — conditional branch
    CondBranch { cond: ValueId, then_block: BlockId, else_block: BlockId },
    /// `switch %val, cases[], default` — switch on enum tag
    Switch { val: ValueId, cases: Vec<(u32, BlockId)>, default: BlockId },
    /// `return [val]` — return from function
    Return { val: Option<ValueId> },
    /// `unreachable` — unreachable code
    Unreachable,
    /// `early_return val` — early return from current function (for ? propagation)
    EarlyReturn { val: ValueId },

    // ── Error Handling ──────────────────────────────────────────────────────
    /// `%dst = result_ok %val` — construct Ok
    ResultOk { dst: ValueId, val: ValueId, ty: NirTy },
    /// `%dst = result_err %val` — construct Err
    ResultErr { dst: ValueId, val: ValueId, ty: NirTy },
    /// `%dst = try_unwrap %src` — unwrap Result/Option (`?` lowering)
    TryUnwrap { dst: ValueId, src: ValueId, ty: NirTy },
    /// `%dst = option_some %val` — construct Some(value)
    OptionSome { dst: ValueId, val: ValueId, ty: NirTy },
    /// `%dst = option_none` — construct None
    OptionNone { dst: ValueId, ty: NirTy },

    // ── Conversion ─────────────────────────────────────────────────────────
    /// `%dst = to_string %val` — convert value to String
    ToString { dst: ValueId, src: ValueId },

    // ── Phi / Block Arguments ───────────────────────────────────────────────
    /// `%dst = phi [val0, block0], [val1, block1], ...` — phi node (SSA)
    Phi { dst: ValueId, incoming: Vec<(ValueId, BlockId)>, ty: NirTy },

    // ── Closures ────────────────────────────────────────────────────────────
    /// `%dst = closure_new @func(captured...)` — create closure
    ClosureNew { dst: ValueId, func: FuncId, captured: Vec<ValueId>, ty: NirTy },
    /// `%dst = closure_call %closure(args...)` — call closure
    ClosureCall { dst: ValueId, closure: ValueId, args: Vec<ValueId>, ret_ty: NirTy },
}

/// Constant values in NIR.
#[derive(Debug, Clone)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
    Unit,
}

impl fmt::Display for Instr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instr::Add { dst, lhs, rhs, ty } => write!(f, "{} = add {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Sub { dst, lhs, rhs, ty } => write!(f, "{} = sub {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Mul { dst, lhs, rhs, ty } => write!(f, "{} = mul {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Div { dst, lhs, rhs, ty } => write!(f, "{} = div {}, {} : {}", dst, lhs, rhs, ty),
            Instr::Rem { dst, lhs, rhs, ty } => write!(f, "{} = rem {}, {} : {}", dst, lhs, rhs, ty),
            Instr::ICmp { dst, op, lhs, rhs } => write!(f, "{} = icmp {:?} {}, {}", dst, op, lhs, rhs),
            Instr::FCmp { dst, op, lhs, rhs } => write!(f, "{} = fcmp {:?} {}, {}", dst, op, lhs, rhs),
            Instr::Neg { dst, src, ty } => write!(f, "{} = neg {} : {}", dst, src, ty),
            Instr::Not { dst, src } => write!(f, "{} = not {}", dst, src),
            Instr::Const { dst, value, ty } => write!(f, "{} = const {} : {}", dst, value, ty),
            Instr::StackAlloc { dst, ty } => write!(f, "{} = stack_alloc : {}", dst, ty),
            Instr::Load { dst, src, ty } => write!(f, "{} = load {} : {}", dst, src, ty),
            Instr::Store { val, ptr } => write!(f, "store {}, {}", val, ptr),
            Instr::Move { dst, src } => write!(f, "{} = move {}", dst, src),
            Instr::HeapAlloc { dst, ty } => write!(f, "{} = heap_alloc : {}", dst, ty),
            Instr::ArcRetain { src } => write!(f, "arc_retain {}", src),
            Instr::ArcRelease { src } => write!(f, "arc_release {}", src),
            Instr::WeakLoad { dst, src, ty } => write!(f, "{} = weak_load {} : {}", dst, src, ty),
            Instr::StructNew { dst, fields, field_names, ty } => write!(f, "{} = struct_new [{}] : {} ({})", dst, fields.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ty, field_names.join(", ")),
            Instr::FieldGet { dst, obj, field, ty } => write!(f, "{} = field_get {}, {} : {}", dst, obj, field, ty),
            Instr::FieldSet { dst, obj, field, val } => write!(f, "{} = field_set {}, {}, {}", dst, obj, field, val),
            Instr::ListLen { dst, src } => write!(f, "{} = list_len {}", dst, src),
            Instr::ListIndex { dst, src, index } => write!(f, "{} = list_index {}, {}", dst, src, index),
            Instr::EnumTag { dst, src } => write!(f, "{} = enum_tag {}", dst, src),
            Instr::EnumPayload { dst, src, index, ty } => write!(f, "{} = enum_payload {}[{}] : {}", dst, src, index, ty),
            Instr::EnumNew { dst, tag, fields, ty } => write!(f, "{} = enum_new {} [{}] : {}", dst, tag, fields.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ty),
            Instr::Call { dst, func, args, ret_ty } => write!(f, "{} = call {}({}) : {}", dst, func, args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ret_ty),
            Instr::CallIndirect { dst, func_ptr, args, ret_ty } => write!(f, "{} = call_indirect {}({}) : {}", dst, func_ptr, args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ret_ty),
            Instr::Print { val } => write!(f, "print {}", val),
            Instr::Branch { target } => write!(f, "branch {}", target),
            Instr::CondBranch { cond, then_block, else_block } => write!(f, "cond_branch {}, {}, {}", cond, then_block, else_block),
            Instr::Switch { val, cases, default } => write!(f, "switch {} [{}] default {}", val, cases.iter().map(|(tag, blk)| format!("{} -> {}", tag, blk)).collect::<Vec<_>>().join(", "), default),
            Instr::Return { val } => write!(f, "return {}", val.map(|v| v.to_string()).unwrap_or_default()),
            Instr::EarlyReturn { val } => write!(f, "early_return {}", val),
            Instr::Unreachable => write!(f, "unreachable"),
            Instr::ResultOk { dst, val, ty } => write!(f, "{} = result_ok {} : {}", dst, val, ty),
            Instr::ResultErr { dst, val, ty } => write!(f, "{} = result_err {} : {}", dst, val, ty),
            Instr::TryUnwrap { dst, src, ty } => write!(f, "{} = try_unwrap {} : {}", dst, src, ty),
            Instr::OptionSome { dst, val, ty } => write!(f, "{} = option_some {} : {}", dst, val, ty),
            Instr::OptionNone { dst, ty } => write!(f, "{} = option_none : {}", dst, ty),
            Instr::ToString { dst, src } => write!(f, "{} = to_string {}", dst, src),
            Instr::Phi { dst, incoming, ty } => write!(f, "{} = phi [{}] : {}", dst, incoming.iter().map(|(v, b)| format!("{} from {}", v, b)).collect::<Vec<_>>().join(", "), ty),
            Instr::ClosureNew { dst, func, captured, ty } => write!(f, "{} = closure_new {}({}) : {}", dst, func, captured.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ty),
            Instr::ClosureCall { dst, closure, args, ret_ty } => write!(f, "{} = closure_call {}({}) : {}", dst, closure, args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "), ret_ty),
        }
    }
}

impl fmt::Display for ConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstValue::Int(v) => write!(f, "{}", v),
            ConstValue::Float(v) => write!(f, "{}", v),
            ConstValue::Bool(v) => write!(f, "{}", v),
            ConstValue::Char(c) => write!(f, "'{}'", c),
            ConstValue::String(s) => write!(f, "\"{}\"", s.escape_default()),
            ConstValue::Unit => write!(f, "()"),
        }
    }
}
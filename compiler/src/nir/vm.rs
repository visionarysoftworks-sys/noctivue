//! NIR bytecode VM — executes NIR directly for faster iteration than native compilation.
//!
//! This is the M1 deliverable: a bytecode VM that consumes NIR and produces
//! identical observable behavior to the M0 tree-walking interpreter.

use crate::nir::instr::{CmpOp, ConstValue, Instr};
use crate::nir::module::NirModule;
use crate::nir::types::{BlockId, FuncId, ValueId};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum VmValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
    Unit,
    Struct { name: String, fields: HashMap<String, VmValue> },
    Enum { variant: String, tag: u32, fields: Vec<VmValue> },
    List(Vec<VmValue>),
    Option(Option<Box<VmValue>>),
    Result(Result<Box<VmValue>, Box<VmValue>>),
    Function(FuncId),
    Closure { func: FuncId, captured: Vec<VmValue> },
    Tuple(Vec<VmValue>),
    Range { start: Box<VmValue>, end: Box<VmValue>, inclusive: bool },
    Pointer(usize),
}

impl VmValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            VmValue::Bool(b) => *b,
            VmValue::Int(i) => *i != 0,
            VmValue::Float(f) => *f != 0.0,
            VmValue::String(s) => !s.is_empty(),
            VmValue::List(l) => !l.is_empty(),
            VmValue::Option(Some(_)) => true,
            VmValue::Option(None) => false,
            VmValue::Result(Ok(_)) => true,
            VmValue::Result(Err(_)) => false,
            VmValue::Unit => false,
            _ => true,
        }
    }
}

impl std::fmt::Display for VmValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmValue::Int(i) => write!(f, "{}", i),
            VmValue::Float(fl) => write!(f, "{}", fl),
            VmValue::Bool(b) => write!(f, "{}", b),
            VmValue::Char(c) => write!(f, "{}", c),
            VmValue::String(s) => write!(f, "{}", s),
            VmValue::Unit => write!(f, "()"),
            VmValue::Struct { name, fields } => {
                let field_strs: Vec<String> = fields.iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect();
                write!(f, "{} {{ {} }}", name, field_strs.join(", "))
            }
            VmValue::Enum { variant, tag: _, fields } => {
                if fields.is_empty() {
                    write!(f, "{}", variant)
                } else {
                    let field_strs: Vec<String> = fields.iter().map(|v| v.to_string()).collect();
                    write!(f, "{}({})", variant, field_strs.join(", "))
                }
            }
            VmValue::List(l) => {
                let elem_strs: Vec<String> = l.iter().map(|v| v.to_string()).collect();
                write!(f, "[{}]", elem_strs.join(", "))
            }
            VmValue::Option(Some(v)) => write!(f, "Some({})", v),
            VmValue::Option(None) => write!(f, "None"),
            VmValue::Result(Ok(v)) => write!(f, "Ok({})", v),
            VmValue::Result(Err(e)) => write!(f, "Err({})", e),
            VmValue::Function(id) => write!(f, "<function {}>", id.0),
            VmValue::Closure { func, captured: _ } => write!(f, "<closure {}>", func.0),
            VmValue::Tuple(elems) => {
                let elem_strs: Vec<String> = elems.iter().map(|v| v.to_string()).collect();
                write!(f, "({})", elem_strs.join(", "))
            }
            VmValue::Range { start, end, inclusive } => {
                let end_str = if *inclusive { "=" } else { "" };
                write!(f, "{}..{}{}", start, end_str, end)
            }
            VmValue::Pointer(addr) => write!(f, "pointer@{:?}", addr),
        }
    }
}

pub struct Vm {
    module: NirModule,
    call_stack: Vec<CallFrame>,
    globals: HashMap<FuncId, VmValue>,
    heap: Vec<VmValue>,
}

#[derive(Debug)]
struct CallFrame {
    func: FuncId,
    block: BlockId,
    prev_block: Option<BlockId>,
    pc: usize,
    locals: Vec<VmValue>,
    #[allow(dead_code)]
    block_params: Vec<VmValue>,
    /// Destination register in the *caller* frame where the return value should be stored
    return_dst: Option<ValueId>,
}

impl Vm {
    pub fn new(module: NirModule) -> Self {
        let mut vm = Vm {
            module,
            call_stack: Vec::new(),
            globals: HashMap::new(),
            heap: Vec::new(),
        };
        for func in &vm.module.functions {
            vm.globals.insert(func.id, VmValue::Function(func.id));
        }
        vm
    }

    pub fn run(&mut self) -> Result<VmValue, VmError> {
        let main_id = self.module.get_function("main")
            .map(|f| f.id)
            .ok_or(VmError::NoMainFunction)?;
        self.call(main_id, vec![])
    }

    pub fn call(&mut self, func_id: FuncId, args: Vec<VmValue>) -> Result<VmValue, VmError> {
        self.call_with_return_dst(func_id, args, None)
    }

    fn call_with_return_dst(&mut self, func_id: FuncId, args: Vec<VmValue>, return_dst: Option<ValueId>) -> Result<VmValue, VmError> {
        let func = self.module.get_function_by_id(func_id)
            .ok_or(VmError::FunctionNotFound(func_id))?;
        let entry_block = func.entry_block()
            .ok_or(VmError::NoEntryBlock(func_id))?
            .id;

        let mut locals = vec![VmValue::Unit; 1000];
        for (i, arg) in args.into_iter().enumerate() {
            locals[i] = arg;
        }

        let frame = CallFrame {
            func: func_id,
            block: entry_block,
            prev_block: None,
            pc: 0,
            locals,
            block_params: Vec::new(),
            return_dst,
        };
        self.call_stack.push(frame);

        self.run_current_frame()
    }

    fn run_current_frame(&mut self) -> Result<VmValue, VmError> {
        loop {
            let (func_id, block_id, pc) = {
                let frame = self.call_stack.last().unwrap();
                (frame.func, frame.block, frame.pc)
            };

            let func = self.module.get_function_by_id(func_id).unwrap();
            let block = func.blocks.iter().find(|b| b.id == block_id).unwrap();

            if pc < block.instrs.len() {
                let instr = block.instrs[pc].clone();
                self.call_stack.last_mut().unwrap().pc += 1;
                let mut frame = self.call_stack.pop().unwrap();
                self.execute_instr(&instr, &mut frame)?;
                self.call_stack.push(frame);
            } else if let Some(term) = block.terminator.clone() {
                let mut frame = self.call_stack.pop().unwrap();
                let ctrl = self.execute_terminator(&term, &mut frame)?;
                match ctrl {
                    ControlFlow::Continue => {
                        self.call_stack.push(frame);
                    }
                    ControlFlow::Return(result) => {
                        if self.call_stack.is_empty() {
                            return Ok(result);
                        }
                        return Ok(result);
                    }
                }
            } else {
                let func_name = func.name.clone();
                return Err(VmError::NoTerminator(block_id, func_name));
            }
        }
    }

    fn execute_instr(&mut self, instr: &Instr, frame: &mut CallFrame) -> Result<(), VmError> {
        match instr {
            Instr::Add { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.add_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Sub { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.sub_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Mul { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.mul_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Div { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.div_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Rem { dst, lhs, rhs, ty: _ } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.rem_values(a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::ICmp { dst, op, lhs, rhs } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.icmp_values(*op, a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::FCmp { dst, op, lhs, rhs } => {
                let a = self.get_value(frame, *lhs)?;
                let b = self.get_value(frame, *rhs)?;
                let result = self.fcmp_values(*op, a, b)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Neg { dst, src, ty: _ } => {
                let a = self.get_value(frame, *src)?;
                let result = self.neg_value(a)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Not { dst, src } => {
                let a = self.get_value(frame, *src)?;
                let result = self.not_value(a)?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::Const { dst, value, ty: _ } => {
                frame.locals[dst.0 as usize] = self.const_to_value(value)?;
            }
            Instr::StackAlloc { dst, ty: _ } => {
                let ptr = self.heap.len();
                self.heap.push(VmValue::Unit);
                frame.locals[dst.0 as usize] = VmValue::Pointer(ptr);
            }
            Instr::HeapAlloc { dst, ty: _ } => {
                let ptr = self.heap.len();
                self.heap.push(VmValue::Unit);
                frame.locals[dst.0 as usize] = VmValue::Pointer(ptr);
            }
            Instr::Load { dst, src, ty: _ } => {
                let ptr_val = self.get_value(frame, *src)?;
                if let VmValue::Pointer(idx) = ptr_val {
                    if idx < self.heap.len() {
                        frame.locals[dst.0 as usize] = self.heap[idx].clone();
                    } else {
                        frame.locals[dst.0 as usize] = VmValue::Unit;
                    }
                } else {
                    frame.locals[dst.0 as usize] = ptr_val;
                }
            }
            Instr::Store { val, ptr } => {
                let ptr_val = self.get_value(frame, *ptr)?;
                let store_val = self.get_value(frame, *val)?;
                if let VmValue::Pointer(idx) = ptr_val {
                    if idx < self.heap.len() {
                        self.heap[idx] = store_val;
                    }
                }
            }
            Instr::Move { dst, src } => {
                let val = self.get_value(frame, *src)?;
                frame.locals[dst.0 as usize] = val;
            }
            Instr::ArcRetain { src } => {}
            Instr::ArcRelease { src } => {}
            Instr::WeakLoad { dst, src, ty: _ } => {
                frame.locals[dst.0 as usize] = VmValue::Option(None);
            }
            Instr::StructNew { dst, fields, field_names, ty } => {
                let field_values: Vec<VmValue> = fields.iter()
                    .map(|f| self.get_value(frame, *f))
                    .collect::<Result<Vec<_>, _>>()?;
                // Lowering reuses StructNew for list/tuple literals (empty
                // field_names, List/Tuple type). They must come back out as
                // List/Tuple values — previously everything became Struct,
                // so `ListLen` saw length 0 and every for-loop over a
                // literal silently ran zero iterations.
                let value = match &ty.inner {
                    crate::hir::types::Ty::List(_) => VmValue::List(field_values),
                    crate::hir::types::Ty::Tuple(_) => VmValue::Tuple(field_values),
                    _ => {
                        let name = match &ty.inner {
                            crate::hir::types::Ty::Named(n, _) => n.clone(),
                            _ => "unknown".to_string(),
                        };
                        let mut fields_map = HashMap::new();
                        if field_names.is_empty() {
                            for (i, val) in field_values.into_iter().enumerate() {
                                fields_map.insert(format!("f{}", i), val);
                            }
                        } else {
                            for (i, val) in field_values.into_iter().enumerate() {
                                if i < field_names.len() {
                                    fields_map.insert(field_names[i].clone(), val);
                                } else {
                                    fields_map.insert(format!("f{}", i), val);
                                }
                            }
                        }
                        VmValue::Struct { name, fields: fields_map }
                    }
                };
                frame.locals[dst.0 as usize] = value;
            }
            Instr::FieldGet { dst, obj, field, ty: _ } => {
                let obj_val = self.get_value(frame, *obj)?;
                let result = if let VmValue::Struct { fields, .. } = obj_val {
                    fields.get(field).cloned().unwrap_or(VmValue::Unit)
                } else {
                    VmValue::Unit
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::FieldSet { dst, obj, field, val } => {
                let obj_val = self.get_value(frame, *obj)?;
                let val_val = self.get_value(frame, *val)?;
                let result = if let VmValue::Struct { name, fields } = obj_val {
                    let mut new_fields = fields.clone();
                    new_fields.insert(field.clone(), val_val);
                    VmValue::Struct { name, fields: new_fields }
                } else {
                    obj_val
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::ListLen { dst, src } => {
                let src_val = self.get_value(frame, *src)?;
                let len = match src_val {
                    VmValue::List(v) => v.len() as i128,
                    VmValue::String(s) => s.len() as i128,
                    VmValue::Tuple(v) => v.len() as i128,
                    _ => 0,
                };
                frame.locals[dst.0 as usize] = VmValue::Int(len);
            }
            Instr::ListIndex { dst, src, index } => {
                let src_val = self.get_value(frame, *src)?;
                let idx_val = self.get_value(frame, *index)?;
                let result = match (src_val, idx_val) {
                    (VmValue::List(v), VmValue::Int(i)) => {
                        v.get(i as usize).cloned().unwrap_or(VmValue::Unit)
                    }
                    (VmValue::String(s), VmValue::Int(i)) => {
                        s.chars().nth(i as usize).map(VmValue::Char).unwrap_or(VmValue::Unit)
                    }
                    (VmValue::Tuple(v), VmValue::Int(i)) => {
                        v.get(i as usize).cloned().unwrap_or(VmValue::Unit)
                    }
                    _ => VmValue::Unit,
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::EnumTag { dst, src } => {
                let src_val = self.get_value(frame, *src)?;
                let tag = if let VmValue::Enum { tag, .. } = src_val {
                    VmValue::Int(tag as i128)
                } else {
                    VmValue::Int(0)
                };
                frame.locals[dst.0 as usize] = tag;
            }
            Instr::EnumPayload { dst, src, index, ty: _ } => {
                let src_val = self.get_value(frame, *src)?;
                // Option/Result payloads extract just like enum-variant
                // payloads (used by `?`-on-Err lowering to forward the
                // original error value). Previously these fell through to
                // Unit, silently dropping payloads.
                let payload = match src_val {
                    VmValue::Enum { fields, .. } => {
                        fields.get(*index as usize).cloned().unwrap_or(VmValue::Unit)
                    }
                    VmValue::Option(Some(v)) => *v,
                    VmValue::Option(None) => VmValue::Unit,
                    VmValue::Result(Ok(v)) => *v,
                    VmValue::Result(Err(e)) => *e,
                    _ => VmValue::Unit,
                };
                frame.locals[dst.0 as usize] = payload;
            }
            Instr::EnumNew { dst, tag, fields, ty: _ } => {
                let tag_val = self.get_value(frame, *tag)?;
                let tag = if let VmValue::Int(i) = tag_val { i as u32 } else { 0 };
                let field_vals: Vec<VmValue> = fields.iter()
                    .map(|f| self.get_value(frame, *f))
                    .collect::<Result<Vec<_>, _>>()?;
                frame.locals[dst.0 as usize] = VmValue::Enum { variant: String::new(), tag, fields: field_vals };
            }
            Instr::Call { dst, func, args, ret_ty: _ } => {
                let arg_vals: Vec<VmValue> = args.iter()
                    .map(|v| self.get_value(frame, *v))
                    .collect::<Result<Vec<_>, _>>()?;
                // Call with return_dst so callee writes directly to our dst
                let result = self.call_with_return_dst(*func, arg_vals, Some(*dst))?;
                frame.locals[dst.0 as usize] = result;
            }
            Instr::CallIndirect { dst, func_ptr, args, ret_ty: _ } => {
                let ptr = self.get_value(frame, *func_ptr)?;
                if let VmValue::Function(fid) = ptr {
                    let arg_vals: Vec<VmValue> = args.iter()
                        .map(|v| self.get_value(frame, *v))
                        .collect::<Result<Vec<_>, _>>()?;
                    let result = self.call_with_return_dst(fid, arg_vals, Some(*dst))?;
                    frame.locals[dst.0 as usize] = result;
                } else {
                    frame.locals[dst.0 as usize] = VmValue::Unit;
                }
            }
            Instr::Print { val } => {
                let v = self.get_value(frame, *val)?;
                print!("{}", v);
            }
            Instr::CondBranch { cond, then_block, else_block } => {
                let cond_val = self.get_value(frame, *cond)?;
                let target = if cond_val.is_truthy() { *then_block } else { *else_block };
                frame.block = target;
                frame.pc = 0;
            }
            Instr::Unreachable => {
                return Err(VmError::Unreachable);
            }
            Instr::ResultOk { dst, val, ty: _ } => {
                let v = self.get_value(frame, *val)?;
                frame.locals[dst.0 as usize] = VmValue::Result(Ok(Box::new(v)));
            }
            Instr::ResultErr { dst, val, ty: _ } => {
                let v = self.get_value(frame, *val)?;
                frame.locals[dst.0 as usize] = VmValue::Result(Err(Box::new(v)));
            }
            Instr::TryUnwrap { dst, src, ty: _ } => {
                // Contract: lowering guarantees the operand is Some/Ok here
                // (`?` splits control flow *before* reaching this instruction,
                // routing None/Err to an early return). Hitting None/Err means
                // a lowering bug — trap loudly instead of continuing with a
                // silent Unit, which used to produce wrong values downstream.
                let src_val = self.get_value(frame, *src)?;
                let result = match src_val {
                    VmValue::Option(Some(v)) => *v,
                    VmValue::Option(None) => {
                        return Err(VmError::OptionUnwrapNone);
                    }
                    VmValue::Result(Ok(v)) => *v,
                    VmValue::Result(Err(e)) => {
                        return Err(VmError::ResultUnwrapErr(e));
                    }
                    other => {
                        return Err(VmError::TypeMismatch(format!("expected Option/Result, got {:?}", other)));
                    }
                };
                frame.locals[dst.0 as usize] = result;
            }
            Instr::OptionSome { dst, val, ty: _ } => {
                let v = self.get_value(frame, *val)?;
                frame.locals[dst.0 as usize] = VmValue::Option(Some(Box::new(v)));
            }
            Instr::OptionNone { dst, ty: _ } => {
                frame.locals[dst.0 as usize] = VmValue::Option(None);
            }
            Instr::ToString { dst, src } => {
                let v = self.get_value(frame, *src)?;
                let s = format!("{}", v);
                frame.locals[dst.0 as usize] = VmValue::String(s);
            }
            Instr::Phi { dst, incoming, ty: _ } => {
                // Contract: a Phi only exists at a merge point that by construction
                // has been reached via some predecessor recorded in `prev_block`,
                // and every predecessor that can reach this Phi must have a
                // corresponding `incoming` entry (this is `lowering.rs`'s job to
                // guarantee — every merge-block Phi's `incoming` vec is built
                // alongside the branches that target it). Previously this defaulted
                // to `VmValue::Unit` in both failure modes below, silently
                // continuing with wrong data instead of surfacing the lowering bug —
                // exactly the failure signature NIR.md §4.1 warns "most of them
                // execute without errors." Trap loudly instead, matching
                // `TryUnwrap`'s existing discipline.
                let prev = frame.prev_block.ok_or_else(|| {
                    VmError::MalformedCfg(format!(
                        "phi {} reached with no predecessor block recorded", dst
                    ))
                })?;
                let (val, _) = incoming.iter().find(|(_, b)| *b == prev).ok_or_else(|| {
                    VmError::MalformedCfg(format!(
                        "phi {} has no incoming entry for predecessor {}", dst, prev
                    ))
                })?;
                frame.locals[dst.0 as usize] = self.get_value(frame, *val)?;
            }
            Instr::ClosureNew { dst, func, captured, ty: _ } => {
                let cap_vals: Vec<VmValue> = captured.iter()
                    .map(|c| self.get_value(frame, *c))
                    .collect::<Result<Vec<_>, _>>()?;
                frame.locals[dst.0 as usize] = VmValue::Closure { func: *func, captured: cap_vals };
            }
            Instr::ClosureCall { dst, closure, args, ret_ty: _ } => {
                let closure_val = self.get_value(frame, *closure)?;
                if let VmValue::Closure { func, captured } = closure_val {
                    let mut full_args = captured.clone();
                    for arg in args {
                        full_args.push(self.get_value(frame, *arg)?);
                    }
                    let result = self.call(func, full_args)?;
                    frame.locals[dst.0 as usize] = result;
                } else {
                    frame.locals[dst.0 as usize] = VmValue::Unit;
                }
            }
            Instr::Branch { target } => {
                let prev = frame.block;
                frame.block = *target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
            }
            Instr::Switch { .. } => {
                return Err(VmError::UnimplementedTerminator("Switch used as instruction, not terminator".to_string()));
            }
            Instr::Return { .. } => {}
            Instr::EarlyReturn { .. } => {
                return Err(VmError::Unreachable);
            }
        }
        Ok(())
    }

    fn execute_terminator(&mut self, term: &Instr, frame: &mut CallFrame) -> Result<ControlFlow, VmError> {
        match term {
            Instr::Return { val } => {
                let result = if let Some(v) = val {
                    self.get_value(frame, *v)?
                } else {
                    VmValue::Unit
                };
                // Propagate to caller if there is one
                let return_dst = frame.return_dst;
                self.call_stack.pop();
                if let Some(caller_frame) = self.call_stack.last_mut() {
                    if let Some(dst) = return_dst {
                        caller_frame.locals[dst.0 as usize] = result.clone();
                    }
                }
                Ok(ControlFlow::Return(result))
            }
            Instr::EarlyReturn { val } => {
                // Early return (for ? operator) - same as Return but explicit
                let result = self.get_value(frame, *val)?;
                let return_dst = frame.return_dst;
                self.call_stack.pop();
                if let Some(caller_frame) = self.call_stack.last_mut() {
                    if let Some(dst) = return_dst {
                        caller_frame.locals[dst.0 as usize] = result.clone();
                    }
                }
                Ok(ControlFlow::Return(result))
            }
            Instr::Branch { target } => {
                let prev = frame.block;
                frame.block = *target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
                Ok(ControlFlow::Continue)
            }
            Instr::CondBranch { cond, then_block, else_block } => {
                let cond_val = self.get_value(frame, *cond)?;
                let target = if cond_val.is_truthy() { *then_block } else { *else_block };
                let prev = frame.block;
                frame.block = target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
                Ok(ControlFlow::Continue)
            }
            Instr::Switch { val, cases, default } => {
                let val_val = self.get_value(frame, *val)?;
                let tag = if let VmValue::Int(i) = val_val { i as u32 } else { 0 };
                let target = cases.iter()
                    .find(|(t, _)| *t == tag)
                    .map(|(_, b)| *b)
                    .unwrap_or(*default);
                let prev = frame.block;
                frame.block = target;
                frame.prev_block = Some(prev);
                frame.pc = 0;
                Ok(ControlFlow::Continue)
            }
            Instr::Unreachable => {
                Err(VmError::Unreachable)
            }
            _ => Err(VmError::UnimplementedTerminator(format!("{:?}", term))),
        }
    }

    fn get_value(&self, frame: &CallFrame, id: ValueId) -> Result<VmValue, VmError> {
        let idx = id.0 as usize;
        if idx < frame.locals.len() {
            Ok(frame.locals[idx].clone())
        } else {
            Err(VmError::InvalidValueId(id))
        }
    }

    fn add_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a + b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a + b)),
            (VmValue::String(a), VmValue::String(b)) => Ok(VmValue::String(a + &b)),
            _ => Err(VmError::TypeMismatch("add".to_string())),
        }
    }

    fn sub_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a - b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a - b)),
            _ => Err(VmError::TypeMismatch("sub".to_string())),
        }
    }

    fn mul_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a * b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a * b)),
            _ => Err(VmError::TypeMismatch("mul".to_string())),
        }
    }

    fn div_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => {
                if b == 0 { return Err(VmError::DivisionByZero); }
                Ok(VmValue::Int(a / b))
            }
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a / b)),
            _ => Err(VmError::TypeMismatch("div".to_string())),
        }
    }

    fn rem_values(&self, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        match (a, b) {
            (VmValue::Int(a), VmValue::Int(b)) => Ok(VmValue::Int(a % b)),
            (VmValue::Float(a), VmValue::Float(b)) => Ok(VmValue::Float(a % b)),
            _ => Err(VmError::TypeMismatch("rem".to_string())),
        }
    }

    fn icmp_values(&self, op: CmpOp, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        // Mirrors the interpreter's `values_equal` / `value_cmp` exactly:
        // structural equality for Eq/Ne (NOT just numerics — previously
        // everything non-numeric silently compared `false`), ordering for
        // Int/Float (mixed included), Char, and String. Anything else is a
        // runtime error, matching the interpreter's panic — never a quiet
        // `false`.
        let result = match (op, a, b) {
            (CmpOp::Eq, x, y) => Self::values_equal(&x, &y),
            (CmpOp::Ne, x, y) => !Self::values_equal(&x, &y),
            (CmpOp::Lt, x, y) => Self::value_cmp(&x, &y)? == std::cmp::Ordering::Less,
            (CmpOp::Le, x, y) => matches!(Self::value_cmp(&x, &y)?, std::cmp::Ordering::Less | std::cmp::Ordering::Equal),
            (CmpOp::Gt, x, y) => Self::value_cmp(&x, &y)? == std::cmp::Ordering::Greater,
            (CmpOp::Ge, x, y) => matches!(Self::value_cmp(&x, &y)?, std::cmp::Ordering::Greater | std::cmp::Ordering::Equal),
        };
        Ok(VmValue::Bool(result))
    }

    fn values_equal(a: &VmValue, b: &VmValue) -> bool {
        match (a, b) {
            (VmValue::Int(x), VmValue::Int(y)) => x == y,
            (VmValue::Float(x), VmValue::Float(y)) => x == y,
            (VmValue::Bool(x), VmValue::Bool(y)) => x == y,
            (VmValue::Char(x), VmValue::Char(y)) => x == y,
            (VmValue::String(x), VmValue::String(y)) => x == y,
            (VmValue::Unit, VmValue::Unit) => true,
            (VmValue::Enum { variant: va, fields: fa, .. }, VmValue::Enum { variant: vb, fields: fb, .. }) => {
                va == vb && fa.len() == fb.len() && fa.iter().zip(fb.iter()).all(|(x, y)| Self::values_equal(x, y))
            }
            (VmValue::Option(x), VmValue::Option(y)) => match (x, y) {
                (None, None) => true,
                (Some(x), Some(y)) => Self::values_equal(x, y),
                _ => false,
            },
            (VmValue::List(x), VmValue::List(y)) => {
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(x, y)| Self::values_equal(x, y))
            }
            // NOTE: mirrors the interpreter, which has no Result arm either
            // (Result == Result is always false there too).
            _ => false,
        }
    }

    fn value_cmp(a: &VmValue, b: &VmValue) -> Result<std::cmp::Ordering, VmError> {
        match (a, b) {
            (VmValue::Int(x), VmValue::Int(y)) => Ok(x.cmp(y)),
            (VmValue::Float(x), VmValue::Float(y)) => {
                x.partial_cmp(y).ok_or_else(|| VmError::TypeMismatch("comparison of NaN".to_string()))
            }
            (VmValue::Float(x), VmValue::Int(y)) => {
                (*x).partial_cmp(&(*y as f64)).ok_or_else(|| VmError::TypeMismatch("comparison of NaN".to_string()))
            }
            (VmValue::Int(x), VmValue::Float(y)) => {
                (*x as f64).partial_cmp(y).ok_or_else(|| VmError::TypeMismatch("comparison of NaN".to_string()))
            }
            (VmValue::Char(x), VmValue::Char(y)) => Ok(x.cmp(y)),
            (VmValue::String(x), VmValue::String(y)) => Ok(x.cmp(y)),
            _ => Err(VmError::TypeMismatch("values are not comparable".to_string())),
        }
    }

    fn fcmp_values(&self, op: CmpOp, a: VmValue, b: VmValue) -> Result<VmValue, VmError> {
        self.icmp_values(op, a, b)
    }

    fn neg_value(&self, a: VmValue) -> Result<VmValue, VmError> {
        match a {
            VmValue::Int(i) => Ok(VmValue::Int(-i)),
            VmValue::Float(f) => Ok(VmValue::Float(-f)),
            _ => Err(VmError::TypeMismatch("neg".to_string())),
        }
    }

    fn not_value(&self, a: VmValue) -> Result<VmValue, VmError> {
        match a {
            VmValue::Bool(b) => Ok(VmValue::Bool(!b)),
            VmValue::Int(i) => Ok(VmValue::Int(!i)),
            _ => Err(VmError::TypeMismatch("not".to_string())),
        }
    }

    fn const_to_value(&self, c: &ConstValue) -> Result<VmValue, VmError> {
        match c {
            ConstValue::Int(v) => Ok(VmValue::Int(*v)),
            ConstValue::Float(v) => Ok(VmValue::Float(*v)),
            ConstValue::Bool(v) => Ok(VmValue::Bool(*v)),
            ConstValue::Char(c) => Ok(VmValue::Char(*c)),
            ConstValue::String(s) => Ok(VmValue::String(s.clone())),
            ConstValue::Unit => Ok(VmValue::Unit),
        }
    }
}

enum ControlFlow {
    Continue,
    Return(VmValue),
}

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error("no main function found")]
    NoMainFunction,
    #[error("function {0:?} not found")]
    FunctionNotFound(FuncId),
    #[error("no entry block for function {0:?}")]
    NoEntryBlock(FuncId),
    #[error("no terminator for block {0:?} in function {1}")]
    NoTerminator(BlockId, String),
    #[error("invalid value ID {0:?}")]
    InvalidValueId(ValueId),
    #[error("type mismatch in {0}")]
    TypeMismatch(String),
    #[error("division by zero")]
    DivisionByZero,
    #[error("unimplemented terminator: {0}")]
    UnimplementedTerminator(String),
    #[error("unreachable code executed")]
    Unreachable,
    #[error("option unwrap on None")]
    OptionUnwrapNone,
    #[error("result unwrap on Err")]
    ResultUnwrapErr(Box<VmValue>),
    /// A Phi (or, in the future, any other CFG-shape-dependent instruction)
    /// was reached in a state its `incoming`/predecessor bookkeeping
    /// doesn't cover — a lowering bug, never a valid program. Mirrors
    /// `TryUnwrap`'s existing "trap loudly, never default" discipline,
    /// which this variant previously lacked (see landmine (c)).
    #[error("malformed CFG: {0}")]
    MalformedCfg(String),
}

//! NIR lowering — convert HIR to NIR.

use crate::hir::items::{Function, Module, TypedArm, TypedStmt};
use crate::hir::types::Ty;
use crate::nir::instr::{CmpOp, ConstValue, Instr};
use crate::nir::module::NirModule;
use crate::nir::types::{BlockId, FuncId, FuncSig, Mode, NirTy, ValueId};
use std::collections::HashMap;

pub struct LoweringContext {
    pub module: NirModule,
    hir_functions: Vec<Function>,
    current_func: Option<String>,
    current_block: Option<BlockId>,
    local_values: HashMap<String, ValueId>,
    func_infos: Vec<(String, FuncId, FuncSig)>,
    next_local_index: u32,
    enum_variants: HashMap<String, (String, u32)>,
}

impl LoweringContext {
    pub fn new(module: Module) -> Self {
        let mut nir_module = NirModule::new();

        let mut enum_variants = HashMap::new();

        for s in &module.structs {
            nir_module.add_type_def(s.name.clone(), Ty::Named(s.name.clone(), vec![]));
        }
        for e in &module.enums {
            nir_module.add_type_def(e.name.clone(), Ty::Named(e.name.clone(), vec![]));
            for (i, (variant_name, _)) in e.variants.iter().enumerate() {
                enum_variants.insert(variant_name.clone(), (e.name.clone(), i as u32));
            }
        }
        for t in &module.traits {
            nir_module.add_type_def(t.name.clone(), Ty::Named(t.name.clone(), vec![]));
        }

        LoweringContext {
            module: nir_module,
            hir_functions: module.functions,
            current_func: None,
            current_block: None,
            local_values: HashMap::new(),
            func_infos: Vec::new(),
            next_local_index: 0,
            enum_variants,
        }
    }

    pub fn lower_module(mut self) -> NirModule {
        let functions: Vec<Function> = self.hir_functions.drain(..).collect();

        self.func_infos = functions.iter().map(|f| {
            let params: Vec<NirTy> = f.params.iter()
                .map(|p| NirTy::new(p.1.clone(), Mode::Native))
                .collect();
            let ret_ty = NirTy::new(f.return_ty.clone(), Mode::Native);
            let sig = FuncSig::new(params, ret_ty, Mode::Native);
            let func_id = self.module.new_func_id();
            (f.name.clone(), func_id, sig)
        }).collect();

        for (name, func_id, sig) in &self.func_infos {
            let nir_func = crate::nir::module::NirFunction::new(
                *func_id, name.clone(), sig.clone(), name.clone()
            );
            self.module.add_function(nir_func);
        }

        for func in functions {
            self.lower_function_body(func);
        }
        self.module
    }

    fn lower_function_body(&mut self, func: Function) {
        let name = func.name.clone();
        self.current_func = Some(name.clone());
        self.next_local_index = 0;

        let func_id = self.func_infos.iter()
            .find(|(n, _, _)| n == &name)
            .map(|(_, id, _)| *id)
            .unwrap_or(FuncId(0));

        let sig = self.func_infos.iter()
            .find(|(n, _, _)| n == &name)
            .map(|(_, _, s)| s.clone())
            .unwrap_or_else(|| FuncSig::new(vec![], NirTy::new(func.return_ty.clone(), Mode::Native), Mode::Native));

        let entry_block = self.module.new_block_id();
        let mut block = crate::nir::module::Block::new(entry_block);
        self.current_block = Some(entry_block);
        self.local_values.clear();

        for (i, (param_name, _)) in func.params.iter().enumerate() {
            let val = ValueId(self.next_local_index);
            self.next_local_index += 1;
            self.local_values.insert(param_name.clone(), val);
            block.params.push((val, sig.params[i].clone()));
        }

        self.lower_stmts(&func.body, &mut block);

        if !block.has_terminator() {
            block.set_terminator(Instr::Return { val: None });
            if let Some(last_stmt) = func.body.last() {
                if matches!(last_stmt.kind, crate::hir::items::TypedStmtKind::Expr(_)) {
                    let val = if let crate::hir::items::TypedStmtKind::Expr(e) = &last_stmt.kind {
                        Some(self.lower_expr(e, &mut block))
                    } else {
                        None
                    };
                    if let Some(v) = val {
                        block.set_terminator(Instr::Return { val: Some(v) });
                    }
                }
            }
        }

        if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.id == func_id) {
            nir_func.blocks.insert(0, block);
        }

        self.current_func = None;
        self.current_block = None;
    }

    fn lower_stmts(&mut self, stmts: &[TypedStmt], nir_block: &mut crate::nir::module::Block) {
        for stmt in stmts {
            self.lower_stmt(stmt, nir_block);
        }
    }

    fn lower_stmt(&mut self, stmt: &crate::hir::items::TypedStmt, nir_block: &mut crate::nir::module::Block) {
        use crate::hir::items::TypedStmtKind;
        match &stmt.kind {
            TypedStmtKind::Let { name, value, .. } => {
                let val = self.lower_expr(value, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                self.local_values.insert(name.clone(), dst);
                nir_block.add_instr(Instr::Move { dst, src: val });
            }
            TypedStmtKind::Var { name, value, .. } => {
                let val = self.lower_expr(value, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                self.local_values.insert(name.clone(), dst);
                nir_block.add_instr(Instr::Move { dst, src: val });
            }
            TypedStmtKind::Decl { name, value, .. } => {
                let val = self.lower_expr(value, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                self.local_values.insert(name.clone(), dst);
                nir_block.add_instr(Instr::Move { dst, src: val });
            }
            TypedStmtKind::Expr(expr) => {
                self.lower_expr(expr, nir_block);
            }
            TypedStmtKind::Return(value) => {
                if let Some(v) = value {
                    let val = self.lower_expr(v, nir_block);
                    nir_block.set_terminator(Instr::Return { val: Some(val) });
                } else {
                    nir_block.set_terminator(Instr::Return { val: None });
                }
            }
            TypedStmtKind::Assign { target, value } => {
                let target_val = self.lower_expr(target, nir_block);
                let value_val = self.lower_expr(value, nir_block);
                nir_block.add_instr(Instr::Move { dst: target_val, src: value_val });
            }
            TypedStmtKind::If { condition, then_body, else_body } => {
                let cond_val = self.lower_expr(condition, nir_block);
                let then_block = self.module.new_block_id();
                let else_block = self.module.new_block_id();
                let merge_block = self.module.new_block_id();

                nir_block.set_terminator(Instr::CondBranch {
                    cond: cond_val,
                    then_block,
                    else_block,
                });

                let mut then_nir_block = crate::nir::module::Block::new(then_block);
                self.lower_stmts(then_body, &mut then_nir_block);
                if !then_nir_block.has_terminator() {
                    then_nir_block.set_terminator(Instr::Branch { target: merge_block });
                }

                let mut else_nir_block = crate::nir::module::Block::new(else_block);
                if let Some(else_b) = else_body {
                    self.lower_stmts(else_b, &mut else_nir_block);
                }
                if !else_nir_block.has_terminator() {
                    else_nir_block.set_terminator(Instr::Branch { target: merge_block });
                }

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(then_nir_block);
                    nir_func.add_block(else_nir_block);
                }
            }
            TypedStmtKind::While { condition, body } => {
                let header_block = self.module.new_block_id();
                let body_block = self.module.new_block_id();
                let exit_block = self.module.new_block_id();

                nir_block.set_terminator(Instr::Branch { target: header_block });

                let mut header_nir_block = crate::nir::module::Block::new(header_block);
                let cond_val = self.lower_expr(condition, &mut header_nir_block);
                header_nir_block.set_terminator(Instr::CondBranch {
                    cond: cond_val,
                    then_block: body_block,
                    else_block: exit_block,
                });

                let mut body_nir_block = crate::nir::module::Block::new(body_block);
                self.lower_stmts(body, &mut body_nir_block);
                if !body_nir_block.has_terminator() {
                    body_nir_block.set_terminator(Instr::Branch { target: header_block });
                }

                let exit_nir_block = crate::nir::module::Block::new(exit_block);

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(header_nir_block);
                    nir_func.add_block(body_nir_block);
                    nir_func.add_block(exit_nir_block);
                }
            }
            TypedStmtKind::Loop { body } => {
                let header_block = self.module.new_block_id();
                let body_block = self.module.new_block_id();
                let exit_block = self.module.new_block_id();

                nir_block.set_terminator(Instr::Branch { target: header_block });

                let mut header_nir_block = crate::nir::module::Block::new(header_block);
                let true_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                header_nir_block.add_instr(Instr::Const {
                    dst: true_val,
                    value: ConstValue::Bool(true),
                    ty: NirTy::new(Ty::Bool, Mode::Native),
                });
                header_nir_block.set_terminator(Instr::CondBranch {
                    cond: true_val,
                    then_block: body_block,
                    else_block: exit_block,
                });

                let mut body_nir_block = crate::nir::module::Block::new(body_block);
                self.lower_stmts(body, &mut body_nir_block);
                if !body_nir_block.has_terminator() {
                    body_nir_block.set_terminator(Instr::Branch { target: header_block });
                }

                let exit_nir_block = crate::nir::module::Block::new(exit_block);

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(header_nir_block);
                    nir_func.add_block(body_nir_block);
                    nir_func.add_block(exit_nir_block);
                }
            }
            TypedStmtKind::For { binding, iterable, body } => {
                let iter_val = self.lower_expr(iterable, nir_block);
                let len_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst: len_val,
                    value: ConstValue::Int(0),
                    ty: NirTy::new(Ty::Int, Mode::Native),
                });
                self.local_values.insert(binding.clone(), len_val);

                let header_block = self.module.new_block_id();
                let body_block = self.module.new_block_id();
                let exit_block = self.module.new_block_id();

                nir_block.set_terminator(Instr::Branch { target: header_block });

                let mut header_nir_block = crate::nir::module::Block::new(header_block);
                let idx_val = *self.local_values.get(binding).unwrap();
                let cond_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                header_nir_block.add_instr(Instr::ICmp {
                    dst: cond_val,
                    op: CmpOp::Lt,
                    lhs: idx_val,
                    rhs: len_val,
                });
                header_nir_block.set_terminator(Instr::CondBranch {
                    cond: cond_val,
                    then_block: body_block,
                    else_block: exit_block,
                });

                let mut body_nir_block = crate::nir::module::Block::new(body_block);
                let next_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                body_nir_block.add_instr(Instr::Const {
                    dst: next_val,
                    value: ConstValue::Int(1),
                    ty: NirTy::new(Ty::Int, Mode::Native),
                });
                let new_idx = ValueId(self.next_local_index);
                self.next_local_index += 1;
                body_nir_block.add_instr(Instr::Add {
                    dst: new_idx,
                    lhs: idx_val,
                    rhs: next_val,
                    ty: NirTy::new(Ty::Int, Mode::Native),
                });
                self.local_values.insert(binding.clone(), new_idx);
                self.lower_stmts(body, &mut body_nir_block);
                if !body_nir_block.has_terminator() {
                    body_nir_block.set_terminator(Instr::Branch { target: header_block });
                }

                let exit_nir_block = crate::nir::module::Block::new(exit_block);

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(header_nir_block);
                    nir_func.add_block(body_nir_block);
                    nir_func.add_block(exit_nir_block);
                }
            }
            TypedStmtKind::Match { scrutinee, arms } => {
                let scrut_val = self.lower_expr(scrutinee, nir_block);
                self.lower_match(scrut_val, arms, nir_block);
            }
        }
    }

    fn lower_match(&mut self, scrut_val: ValueId, arms: &[TypedArm], nir_block: &mut crate::nir::module::Block) {
        if arms.is_empty() {
            nir_block.set_terminator(Instr::Unreachable);
            return;
        }

        let tag_val = ValueId(self.next_local_index);
        self.next_local_index += 1;
        nir_block.add_instr(Instr::EnumTag { dst: tag_val, src: scrut_val });

        let exit_block = self.module.new_block_id();
        let mut cases: Vec<(u32, BlockId)> = Vec::new();

        for (i, arm) in arms.iter().enumerate() {
            let body_block = self.module.new_block_id();
            cases.push((i as u32, body_block));

            let mut body_nir_block = crate::nir::module::Block::new(body_block);
            self.lower_arm_body(&arm.body, &mut body_nir_block);
            if !body_nir_block.has_terminator() {
                body_nir_block.set_terminator(Instr::Branch { target: exit_block });
            }

            if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                nir_func.add_block(body_nir_block);
            }
        }

        nir_block.set_terminator(Instr::Switch {
            val: tag_val,
            cases,
            default: exit_block,
        });

        let exit_nir_block = crate::nir::module::Block::new(exit_block);
        if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
            nir_func.add_block(exit_nir_block);
        }
    }

    fn lower_arm_body(&mut self, stmts: &[TypedStmt], nir_block: &mut crate::nir::module::Block) {
        for stmt in stmts {
            self.lower_stmt(stmt, nir_block);
        }
    }

    fn lower_expr(&mut self, expr: &crate::hir::items::TypedExpr, nir_block: &mut crate::nir::module::Block) -> ValueId {
        use crate::hir::items::TypedExprKind;
        match &expr.kind {
            TypedExprKind::IntLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::Int(*v),
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::FloatLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::Float(*v),
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::BoolLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::Bool(*v),
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::CharLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::Char(*v),
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::StringLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::String(v.clone()),
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::None => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::OptionNone {
                    dst,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Some(inner) => {
                let inner_val = self.lower_expr(inner, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::OptionSome {
                    dst,
                    val: inner_val,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Ok(inner) => {
                let inner_val = self.lower_expr(inner, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::ResultOk {
                    dst,
                    val: inner_val,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Err(inner) => {
                let inner_val = self.lower_expr(inner, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::ResultErr {
                    dst,
                    val: inner_val,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Ident(name) => {
                if let Some(&val) = self.local_values.get(name) {
                    val
                } else {
                    let dst = ValueId(self.next_local_index);
                    self.next_local_index += 1;
                    dst
                }
            }
            TypedExprKind::Call { callee, args } => {
                let arg_vals: Vec<ValueId> = args.iter()
                    .map(|a| self.lower_expr(a, nir_block))
                    .collect();
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                let func_id = if let TypedExprKind::Ident(name) = &callee.kind {
                    self.func_infos.iter()
                        .find(|(n, _, _)| n == name)
                        .map(|(_, id, _)| *id)
                        .unwrap_or(FuncId(0))
                } else {
                    FuncId(0)
                };
                nir_block.add_instr(Instr::Call {
                    dst,
                    func: func_id,
                    args: arg_vals,
                    ret_ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::BinOp { op, left, right } => {
                let lhs = self.lower_expr(left, nir_block);
                let rhs = self.lower_expr(right, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                match op {
                    crate::ast::BinOp::Add => nir_block.add_instr(Instr::Add {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), Mode::Native)
                    }),
                    crate::ast::BinOp::Sub => nir_block.add_instr(Instr::Sub {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), Mode::Native)
                    }),
                    crate::ast::BinOp::Mul => nir_block.add_instr(Instr::Mul {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), Mode::Native)
                    }),
                    crate::ast::BinOp::Div => nir_block.add_instr(Instr::Div {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), Mode::Native)
                    }),
                    crate::ast::BinOp::Rem => nir_block.add_instr(Instr::Rem {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), Mode::Native)
                    }),
                    _ => {
                        nir_block.add_instr(Instr::ICmp {
                            dst, op: match op {
                                crate::ast::BinOp::Eq => CmpOp::Eq,
                                crate::ast::BinOp::Ne => CmpOp::Ne,
                                crate::ast::BinOp::Lt => CmpOp::Lt,
                                crate::ast::BinOp::Le => CmpOp::Le,
                                crate::ast::BinOp::Gt => CmpOp::Gt,
                                crate::ast::BinOp::Ge => CmpOp::Ge,
                                _ => CmpOp::Eq,
                            }, lhs, rhs
                        });
                    }
                };
                dst
            }
            TypedExprKind::UnaryOp { op, operand } => {
                let src = self.lower_expr(operand, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                match op {
                    crate::ast::UnaryOp::Neg => nir_block.add_instr(Instr::Neg {
                        dst, src, ty: NirTy::new(expr.ty.clone(), Mode::Native)
                    }),
                    crate::ast::UnaryOp::Not => nir_block.add_instr(Instr::Not { dst, src }),
                    crate::ast::UnaryOp::Await => {
                        nir_block.add_instr(Instr::Move { dst, src });
                    }
                }
                dst
            }
            TypedExprKind::Member { object, field } => {
                let obj_val = self.lower_expr(object, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::FieldGet {
                    dst,
                    obj: obj_val,
                    field: field.clone(),
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Try(inner) => {
                let src = self.lower_expr(inner, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::TryUnwrap {
                    dst,
                    src,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Coalesce { left, right } => {
                let current_block_id = nir_block.id;
                let left_val = self.lower_expr(left, nir_block);
                let right_val = self.lower_expr(right, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;

                let else_block = self.module.new_block_id();
                let merge_block = self.module.new_block_id();

                nir_block.set_terminator(Instr::CondBranch {
                    cond: left_val,
                    then_block: merge_block,
                    else_block: else_block,
                });

                let mut else_nir_block = crate::nir::module::Block::new(else_block);
                else_nir_block.add_instr(Instr::Move { dst, src: right_val });
                else_nir_block.set_terminator(Instr::Branch { target: merge_block });

                let mut merge_nir_block = crate::nir::module::Block::new(merge_block);
                merge_nir_block.add_instr(Instr::Phi {
                    dst,
                    incoming: vec![(left_val, current_block_id), (right_val, else_block)],
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(else_nir_block);
                    nir_func.add_block(merge_nir_block);
                }

                dst
            }
            TypedExprKind::StringInterp(parts) => {
                let mut result_val: Option<ValueId> = None;
                for part in parts {
                    match part {
                        crate::hir::items::TypedInterpPart::Literal(s) => {
                            let str_val = ValueId(self.next_local_index);
                            self.next_local_index += 1;
                            nir_block.add_instr(Instr::Const {
                                dst: str_val,
                                value: ConstValue::String(s.clone()),
                                ty: NirTy::new(Ty::String, Mode::Native),
                            });
                            if let Some(existing) = result_val {
                                let new_result = ValueId(self.next_local_index);
                                self.next_local_index += 1;
                                nir_block.add_instr(Instr::Add {
                                    dst: new_result,
                                    lhs: existing,
                                    rhs: str_val,
                                    ty: NirTy::new(Ty::String, Mode::Native),
                                });
                                result_val = Some(new_result);
                            } else {
                                result_val = Some(str_val);
                            }
                        }
                        crate::hir::items::TypedInterpPart::Expr(e) => {
                            let expr_val = self.lower_expr(e, nir_block);
                            let str_val = ValueId(self.next_local_index);
                            self.next_local_index += 1;
                            nir_block.add_instr(Instr::Const {
                                dst: str_val,
                                value: ConstValue::String("".to_string()),
                                ty: NirTy::new(Ty::String, Mode::Native),
                            });
                            if let Some(existing) = result_val {
                                let new_result = ValueId(self.next_local_index);
                                self.next_local_index += 1;
                                nir_block.add_instr(Instr::Add {
                                    dst: new_result,
                                    lhs: existing,
                                    rhs: str_val,
                                    ty: NirTy::new(Ty::String, Mode::Native),
                                });
                                result_val = Some(new_result);
                            } else {
                                result_val = Some(expr_val);
                            }
                        }
                    }
                }
                result_val.unwrap_or_else(|| {
                    let dst = ValueId(self.next_local_index);
                    self.next_local_index += 1;
                    nir_block.add_instr(Instr::Const {
                        dst,
                        value: ConstValue::String(String::new()),
                        ty: NirTy::new(Ty::String, Mode::Native),
                    });
                    dst
                })
            }
            TypedExprKind::Tuple(items) => {
                let field_vals: Vec<ValueId> = items.iter()
                    .map(|e| self.lower_expr(e, nir_block))
                    .collect();
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::StructNew {
                    dst,
                    fields: field_vals,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::List(items) => {
                let mut field_vals: Vec<ValueId> = Vec::new();
                for item in items {
                    field_vals.push(self.lower_expr(item, nir_block));
                }
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::StructNew {
                    dst,
                    fields: field_vals,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Index { object, index } => {
                let obj_val = self.lower_expr(object, nir_block);
                let idx_val = self.lower_expr(index, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::StructNew {
                    dst,
                    fields: vec![obj_val, idx_val],
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Range { start, end, inclusive } => {
                let start_val = self.lower_expr(start, nir_block);
                let end_val = self.lower_expr(end, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::StructNew {
                    dst,
                    fields: vec![start_val, end_val],
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::IfExpr { condition, then_expr, else_expr } => {
                let cond_val = self.lower_expr(condition, nir_block);
                let then_block = self.module.new_block_id();
                let else_block = self.module.new_block_id();
                let merge_block = self.module.new_block_id();

                nir_block.set_terminator(Instr::CondBranch {
                    cond: cond_val,
                    then_block,
                    else_block,
                });

                let mut then_nir_block = crate::nir::module::Block::new(then_block);
                let then_val = self.lower_expr(then_expr, &mut then_nir_block);
                then_nir_block.set_terminator(Instr::Branch { target: merge_block });

                let mut else_nir_block = crate::nir::module::Block::new(else_block);
                let else_val = if let Some(e) = else_expr {
                    self.lower_expr(e, &mut else_nir_block)
                } else {
                    ValueId(self.next_local_index)
                };
                else_nir_block.set_terminator(Instr::Branch { target: merge_block });

                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                let mut merge_nir_block = crate::nir::module::Block::new(merge_block);
                merge_nir_block.add_instr(Instr::Phi {
                    dst,
                    incoming: vec![(then_val, then_block), (else_val, else_block)],
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(then_nir_block);
                    nir_func.add_block(else_nir_block);
                    nir_func.add_block(merge_nir_block);
                }

                dst
            }
            TypedExprKind::MatchExpr { scrutinee, arms } => {
                let scrut_val = self.lower_expr(scrutinee, nir_block);
                let tag_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::EnumTag { dst: tag_val, src: scrut_val });
                let exit_block = self.module.new_block_id();
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                let mut cases: Vec<(u32, BlockId)> = Vec::new();
                let mut incoming: Vec<(ValueId, BlockId)> = Vec::new();

                for (i, arm) in arms.iter().enumerate() {
                    let body_block = self.module.new_block_id();
                    cases.push((i as u32, body_block));

                    let mut body_nir_block = crate::nir::module::Block::new(body_block);
                    self.lower_stmts(&arm.body, &mut body_nir_block);
                    if !body_nir_block.has_terminator() {
                        let arm_result = if let Some(last) = arm.body.last() {
                            if let crate::hir::items::TypedStmtKind::Expr(e) = &last.kind {
                                self.lower_expr(e, &mut body_nir_block)
                            } else {
                                ValueId(self.next_local_index)
                            }
                        } else {
                            ValueId(self.next_local_index)
                        };
                        incoming.push((arm_result, body_block));
                        body_nir_block.set_terminator(Instr::Branch { target: exit_block });
                    } else {
                        incoming.push((ValueId(0), body_block));
                    }

                    if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                        nir_func.add_block(body_nir_block);
                    }
                }

                nir_block.set_terminator(Instr::Switch {
                    val: tag_val,
                    cases,
                    default: exit_block,
                });

                let mut exit_nir_block = crate::nir::module::Block::new(exit_block);
                exit_nir_block.add_instr(Instr::Phi {
                    dst,
                    incoming,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                exit_nir_block.set_terminator(Instr::Return { val: Some(dst) });

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(exit_nir_block);
                }

                dst
            }
            TypedExprKind::EnumVariant { enum_name, variant } => {
                let tag = self.enum_variants.get(variant).map(|(_, t)| *t).unwrap_or(0);
                let tag_dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst: tag_dst,
                    value: ConstValue::Int(tag as i128),
                    ty: NirTy::new(Ty::Int, Mode::Native),
                });
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::EnumNew {
                    dst,
                    tag: tag_dst,
                    fields: vec![],
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::StructLit { name, fields } => {
                let field_vals: Vec<ValueId> = fields.iter()
                    .map(|(_, e)| self.lower_expr(e, nir_block))
                    .collect();
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::StructNew {
                    dst,
                    fields: field_vals,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
            TypedExprKind::Spread(inner) => {
                self.lower_expr(inner, nir_block)
            }
            TypedExprKind::Closure { params, body } => {
                let captured: Vec<ValueId> = Vec::new();
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                let func_id = self.func_infos.iter()
                    .find(|(n, _, _)| n == &self.current_func.clone().unwrap_or_default())
                    .map(|(_, id, _)| *id)
                    .unwrap_or(FuncId(0));
                nir_block.add_instr(Instr::ClosureNew {
                    dst,
                    func: func_id,
                    captured,
                    ty: NirTy::new(expr.ty.clone(), Mode::Native),
                });
                dst
            }
        }
    }
}

pub fn lower(module: Module) -> NirModule {
    let ctx = LoweringContext::new(module);
    ctx.lower_module()
}

//! NIR lowering — convert HIR to NIR.

use crate::hir::items::{Function, Module, TypedArm, TypedExpr, TypedLit, TypedPattern, TypedStmt};
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
    /// Mode tag applied to every type/signature this lowering produces.
    /// The single-IR invariant (NIR.md §2) is that lowering the same HIR
    /// under either mode yields the same instruction *shape* — mode may
    /// only affect memory-operation selection, never the instruction set.
    mode: Mode,
    // NOTE: no loop-fallthrough cursor. `break`/`continue` have no HIR
    // nodes (typeck drops them), so there is currently no producer that
    // could read one; when HIR gains control-flow exits, the plumbing for
    // targeting loop header/exit blocks goes here.
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
            mode: Mode::Native,
        }
    }

    /// Lower under a different mode tag (used by the single-IR validation
    /// test; production lowering is always native until managed mode lands).
    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// Append a finished block to the function currently being lowered.
    fn add_block_to_current_func(&mut self, block: crate::nir::module::Block) {
        if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
            nir_func.add_block(block);
        }
    }

    /// Redirect emission after a control-flow split inside an expression.
    ///
    /// `current` is the block being built: it already holds the pre-split
    /// instructions plus its terminator (the branch). `next` is the fresh
    /// block where all *later* code must go (merge/continue point, still
    /// unterminated). Swaps their contents so `*current` becomes the next
    /// block (keeping `next`'s id), and appends the pre-split block (which
    /// keeps its original id, so existing branch targets and Phi
    /// `incoming` entries stay valid) to the current function.
    ///
    /// Without this, callers keep appending to the terminated pre-split
    /// block (executing post-branch code *before* the branch) while the
    /// merge block is left unterminated — the VM then either errors with
    /// `NoTerminator` or silently computes wrong values.
    fn split_current_block(
        &mut self,
        current: &mut crate::nir::module::Block,
        mut next: crate::nir::module::Block,
    ) {
        std::mem::swap(current, &mut next);
        self.add_block_to_current_func(next);
    }

    /// Allocate a fresh SSA value id.
    fn fresh_value(&mut self) -> ValueId {
        let id = ValueId(self.next_local_index);
        self.next_local_index += 1;
        id
    }

    pub fn lower_module(mut self) -> NirModule {
        let functions: Vec<Function> = self.hir_functions.drain(..).collect();

        self.func_infos = functions.iter().map(|f| {
            let params: Vec<NirTy> = f.params.iter()
                .map(|p| NirTy::new(p.1.clone(), self.mode))
                .collect();
            let ret_ty = NirTy::new(f.return_ty.clone(), self.mode);
            let sig = FuncSig::new(params, ret_ty, self.mode);
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
            .unwrap_or(FuncId::UNRESOLVED);

        let sig = self.func_infos.iter()
            .find(|(n, _, _)| n == &name)
            .map(|(_, _, s)| s.clone())
            .unwrap_or_else(|| FuncSig::new(vec![], NirTy::new(func.return_ty.clone(), self.mode), self.mode));

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

        let result = self.lower_stmts_with_result(&func.body, &mut block);

        if !block.has_terminator() {
            match result {
                Some(val) => block.set_terminator(Instr::Return { val: Some(val) }),
                None => block.set_terminator(Instr::Return { val: None }),
            }
        }

        if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.id == func_id) {
            nir_func.blocks.insert(0, block);
            // Splits inside the body swap block identities (see
            // `split_current_block`): the temp now at index 0 may hold a
            // merge block while the true entry sits elsewhere. The VM
            // starts at blocks[0], so rotate the true entry to the front.
            // (Ids travel with their content through swaps, so the entry
            // id always addresses the entry code.)
            if let Some(pos) = nir_func.blocks.iter().position(|b| b.id == entry_block) {
                nir_func.blocks.swap(0, pos);
            }
        }

        self.current_func = None;
        self.current_block = None;
    }

    fn lower_stmts(&mut self, stmts: &[TypedStmt], nir_block: &mut crate::nir::module::Block) {
        for stmt in stmts {
            self.lower_stmt(stmt, nir_block);
        }
    }

    fn lower_stmts_with_result(&mut self, stmts: &[TypedStmt], nir_block: &mut crate::nir::module::Block) -> Option<ValueId> {
        // Mirrors the interpreter's eval_body value threading: every
        // statement updates the running value (non-value statements yield
        // Unit; previously the stale prior value leaked through, so a
        // trailing `let` returned whatever the previous expression was).
        // An explicit `return` ends the list: later statements are dead
        // (the interpreter aborts the body there too).
        let mut result = None;
        for stmt in stmts {
            match &stmt.kind {
                crate::hir::items::TypedStmtKind::Expr(e) => {
                    result = Some(self.lower_expr(e, nir_block));
                }
                crate::hir::items::TypedStmtKind::If { condition, then_body, else_body } => {
                    result = self.lower_if_stmt(condition, then_body, else_body, nir_block);
                }
                crate::hir::items::TypedStmtKind::Match { scrutinee, arms } => {
                    let scrut_val = self.lower_expr(scrutinee, nir_block);
                    result = self.lower_match(scrut_val, arms, nir_block);
                }
                crate::hir::items::TypedStmtKind::Return(_) => {
                    self.lower_stmt(stmt, nir_block);
                    result = None;
                    break;
                }
                _ => {
                    self.lower_stmt(stmt, nir_block);
                    result = Some(self.unit_value(nir_block));
                }
            }
        }
        result
    }

    /// A fresh Unit-valued register (statement-value fallback).
    fn unit_value(&mut self, nir_block: &mut crate::nir::module::Block) -> ValueId {
        let dst = self.fresh_value();
        nir_block.add_instr(Instr::Const {
            dst,
            value: ConstValue::Unit,
            ty: NirTy::new(Ty::Unit, self.mode),
        });
        dst
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
                // Statement position discards the value, but the merge swap
                // (control flow) still happens inside.
                let _ = self.lower_if_stmt(condition, then_body, else_body, nir_block);
            }
            TypedStmtKind::While { condition, body } => {
                let header_block = self.module.new_block_id();
                let body_block = self.module.new_block_id();
                let exit_block = self.module.new_block_id();
                let merge_block = self.module.new_block_id();

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

                // The old code left `exit` unterminated (VM NoTerminator on
                // loop exit) and kept emitting post-loop code into the
                // pre-loop block (running it before the loop). Converge via
                // a merge block swapped into `*nir_block` instead.
                let mut exit_nir_block = crate::nir::module::Block::new(exit_block);
                exit_nir_block.set_terminator(Instr::Branch { target: merge_block });

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(header_nir_block);
                    nir_func.add_block(body_nir_block);
                    nir_func.add_block(exit_nir_block);
                }

                let merge = crate::nir::module::Block::new(merge_block);
                self.split_current_block(nir_block, merge);
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
                    ty: NirTy::new(Ty::Bool, self.mode),
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

                // Reachable only via `break` (no HIR node yet — typeck drops
                // break/continue today); still terminated so the block is
                // never a NoTerminator hazard, then converged like While.
                let merge_block = self.module.new_block_id();
                let mut exit_nir_block = crate::nir::module::Block::new(exit_block);
                exit_nir_block.set_terminator(Instr::Branch { target: merge_block });

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(header_nir_block);
                    nir_func.add_block(body_nir_block);
                    nir_func.add_block(exit_nir_block);
                }

                let merge = crate::nir::module::Block::new(merge_block);
                self.split_current_block(nir_block, merge);
            }
            TypedStmtKind::For { binding, iterable, body } => {
                let iter_val = self.lower_expr(iterable, nir_block);

                let len_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::ListLen {
                    dst: len_val,
                    src: iter_val,
                });

                let idx_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst: idx_val,
                    value: ConstValue::Int(0),
                    ty: NirTy::new(Ty::Int, self.mode),
                });
                self.local_values.insert(binding.clone(), idx_val);

                let header_block = self.module.new_block_id();
                let body_block = self.module.new_block_id();
                let cont_block = self.module.new_block_id();
                let exit_block = self.module.new_block_id();
                // Pre-allocate the carried index id: the header Phi needs
                // it before the cont block (which computes it) is built.
                let carried_idx = self.fresh_value();
                let pre_id = nir_block.id;

                nir_block.set_terminator(Instr::Branch { target: header_block });

                // Header threads the loop index through a Phi: initial
                // value on entry from the preheader, incremented value on
                // back-edges. (Previously the header re-read the initial
                // index every iteration — an infinite loop on any
                // non-empty collection, masked until lists actually had
                // length because StructNew used to build Structs.)
                let mut header_nir_block = crate::nir::module::Block::new(header_block);
                let idx_phi = self.fresh_value();
                header_nir_block.add_instr(Instr::Phi {
                    dst: idx_phi,
                    incoming: vec![(idx_val, pre_id), (carried_idx, cont_block)],
                    ty: NirTy::new(Ty::Int, self.mode),
                });
                let cond_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                header_nir_block.add_instr(Instr::ICmp {
                    dst: cond_val,
                    op: CmpOp::Lt,
                    lhs: idx_phi,
                    rhs: len_val,
                });
                header_nir_block.set_terminator(Instr::CondBranch {
                    cond: cond_val,
                    then_block: body_block,
                    else_block: exit_block,
                });

                let mut body_nir_block = crate::nir::module::Block::new(body_block);
                let elem_val = ValueId(self.next_local_index);
                self.next_local_index += 1;
                body_nir_block.add_instr(Instr::ListIndex {
                    dst: elem_val,
                    src: iter_val,
                    index: idx_phi,
                });
                self.local_values.insert(binding.clone(), elem_val);

                let _body_result = self.lower_stmts_with_result(body, &mut body_nir_block);

                if !body_nir_block.has_return_terminator() {
                    body_nir_block.set_terminator(Instr::Branch { target: cont_block });
                }

                let mut cont_nir_block = crate::nir::module::Block::new(cont_block);
                let next_idx = ValueId(self.next_local_index);
                self.next_local_index += 1;
                cont_nir_block.add_instr(Instr::Const {
                    dst: next_idx,
                    value: ConstValue::Int(1),
                    ty: NirTy::new(Ty::Int, self.mode),
                });
                cont_nir_block.add_instr(Instr::Add {
                    dst: carried_idx,
                    lhs: idx_phi,
                    rhs: next_idx,
                    ty: NirTy::new(Ty::Int, self.mode),
                });
                self.local_values.insert(binding.clone(), carried_idx);
                cont_nir_block.set_terminator(Instr::Branch { target: header_block });

                // Loop exit converges to a merge (swapped into current) so
                // post-loop code — including a trailing value the function
                // returns — actually runs. The old `Return None` here
                // discarded loop-carried results (e.g. an accumulator) and
                // skipped everything after the loop.
                let merge_block = self.module.new_block_id();
                let mut exit_nir_block = crate::nir::module::Block::new(exit_block);
                exit_nir_block.set_terminator(Instr::Branch { target: merge_block });

                if let Some(nir_func) = self.module.functions.iter_mut().find(|f| f.name == self.current_func.clone().unwrap_or_default()) {
                    nir_func.add_block(header_nir_block);
                    nir_func.add_block(body_nir_block);
                    nir_func.add_block(cont_nir_block);
                    nir_func.add_block(exit_nir_block);
                }

                let merge = crate::nir::module::Block::new(merge_block);
                self.split_current_block(nir_block, merge);
            }
            TypedStmtKind::Match { scrutinee, arms } => {
                // Statement position discards the value (control flow + swap
                // still happen inside).
                let scrut_val = self.lower_expr(scrutinee, nir_block);
                let _ = self.lower_match(scrut_val, arms, nir_block);
            }
        }
    }

    /// Lower an `if` (condition + arms) with merge convergence, returning
    /// the merged arm value when one flows (tail position). The merge block
    /// is swapped into `*nir_block` so later code emits after the branch.
    /// Arms ending in explicit `return` are excluded from the Phi (control
    /// never reaches the merge from them); a missing `else` yields Unit.
    fn lower_if_stmt(
        &mut self,
        condition: &TypedExpr,
        then_body: &[TypedStmt],
        else_body: &Option<Vec<TypedStmt>>,
        nir_block: &mut crate::nir::module::Block,
    ) -> Option<ValueId> {
        let cond_val = self.lower_expr(condition, nir_block);
        let then_id = self.module.new_block_id();
        let else_id = self.module.new_block_id();
        let merge_id = self.module.new_block_id();

        let mut then_temp = crate::nir::module::Block::new(then_id);
        let then_val = self
            .lower_stmts_with_result(then_body, &mut then_temp)
            .unwrap_or_else(|| self.unit_value(&mut then_temp));
        // An arm ending in explicit `return` never reaches the merge, so it
        // contributes no Phi entry (checked BEFORE appending the Branch).
        let then_returns = then_temp.has_return_terminator();
        if !then_temp.has_terminator() {
            then_temp.set_terminator(Instr::Branch { target: merge_id });
        }
        let then_actual = then_temp.id;
        self.add_block_to_current_func(then_temp);

        let mut else_temp = crate::nir::module::Block::new(else_id);
        let else_val = match else_body {
            Some(stmts) => self
                .lower_stmts_with_result(stmts, &mut else_temp)
                .unwrap_or_else(|| self.unit_value(&mut else_temp)),
            None => self.unit_value(&mut else_temp),
        };
        let else_returns = else_temp.has_return_terminator();
        if !else_temp.has_terminator() {
            else_temp.set_terminator(Instr::Branch { target: merge_id });
        }
        let else_actual = else_temp.id;
        self.add_block_to_current_func(else_temp);

        nir_block.set_terminator(Instr::CondBranch {
            cond: cond_val,
            then_block: then_actual,
            else_block: else_actual,
        });

        let mut incoming = Vec::new();
        if !then_returns {
            incoming.push((then_val, then_actual));
        }
        if !else_returns {
            incoming.push((else_val, else_actual));
        }
        if incoming.is_empty() {
            // No value flows (e.g. both arms return): still swap in an
            // (unterminated, owner-terminated) merge so later code emits
            // after the branch. Dead-but-harmless if never reached.
            let merge = crate::nir::module::Block::new(merge_id);
            self.split_current_block(nir_block, merge);
            None
        } else {
            let dst = self.fresh_value();
            let mut merge = crate::nir::module::Block::new(merge_id);
            merge.add_instr(Instr::Phi {
                dst,
                incoming,
                ty: NirTy::new(Ty::Unknown, self.mode),
            });
            self.split_current_block(nir_block, merge);
            Some(dst)
        }
    }

    /// Lower a `match` (statement or expression form — identical shape) to
    /// a test chain, returning the merged arm value when one flows.
    ///
    /// Shape: the current block branches to the first test; each test
    /// branches to its arm body or the next test; the last test falls
    /// through to a `trap` block (`Unreachable`, mirroring the
    /// interpreter's "no arm matched" panic); taken arms converge on a
    /// merge block (Phi) swapped into `*nir_block`.
    ///
    /// This mirrors `interp::eval_match` exactly: arm order, guard
    /// semantics (bindings visible, `false` tries the next arm), payload
    /// binding, and the quirky-but-specified prelude rules (`Some` needs
    /// exactly one subpattern; `None` ignores subpatterns). The previous
    /// lowering dispatched on *positional arm index* via `Switch`, so any
    /// non-enum scrutinee (ints, Options) always took arm 0, and arm-tail
    /// expressions were evaluated *twice* (once by the body pass, once
    /// for the Phi value).
    fn lower_match(
        &mut self,
        scrut_val: ValueId,
        arms: &[TypedArm],
        nir_block: &mut crate::nir::module::Block,
    ) -> Option<ValueId> {
        if arms.is_empty() {
            nir_block.set_terminator(Instr::Unreachable);
            return None;
        }

        let merge_id = self.module.new_block_id();
        let trap_id = self.module.new_block_id();
        let test_ids: Vec<BlockId> =
            arms.iter().map(|_| self.module.new_block_id()).collect();
        nir_block.set_terminator(Instr::Branch { target: test_ids[0] });

        let mut incoming = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            let next_id =
                if i + 1 < arms.len() { test_ids[i + 1] } else { trap_id };
            let body_id = self.module.new_block_id();

            let mut test = crate::nir::module::Block::new(test_ids[i]);
            self.lower_pattern_test(&arm.pattern, scrut_val, body_id, next_id, &mut test);
            self.add_block_to_current_func(test);

            // Arm body: bindings first (guards see them, as in the
            // interpreter), then an optional guard split, then statements.
            let mut body = crate::nir::module::Block::new(body_id);
            self.bind_pattern(&arm.pattern, scrut_val, &mut body);
            let mut exec = if let Some(guard) = &arm.guard {
                let rest_id = self.module.new_block_id();
                let guard_val = self.lower_expr(guard, &mut body);
                body.set_terminator(Instr::CondBranch {
                    cond: guard_val,
                    then_block: rest_id,
                    else_block: next_id,
                });
                self.add_block_to_current_func(body);
                crate::nir::module::Block::new(rest_id)
            } else {
                body
            };
            let val = self
                .lower_stmts_with_result(&arm.body, &mut exec)
                .unwrap_or_else(|| self.unit_value(&mut exec));
            if !exec.has_terminator() {
                exec.set_terminator(Instr::Branch { target: merge_id });
                incoming.push((val, exec.id));
            }
            self.add_block_to_current_func(exec);
        }

        let mut trap = crate::nir::module::Block::new(trap_id);
        trap.set_terminator(Instr::Unreachable);
        self.add_block_to_current_func(trap);

        if incoming.is_empty() {
            let merge = crate::nir::module::Block::new(merge_id);
            self.split_current_block(nir_block, merge);
            None
        } else {
            let dst = self.fresh_value();
            let mut merge = crate::nir::module::Block::new(merge_id);
            merge.add_instr(Instr::Phi {
                dst,
                incoming,
                ty: NirTy::new(Ty::Unknown, self.mode),
            });
            self.split_current_block(nir_block, merge);
            Some(dst)
        }
    }

    /// Emit one arm's discriminant test into `test`: branch to `arm_id` on
    /// match, `next_id` otherwise. Mirrors `interp::pattern_matches`.
    fn lower_pattern_test(
        &mut self,
        pattern: &TypedPattern,
        scrut_val: ValueId,
        arm_id: BlockId,
        next_id: BlockId,
        test: &mut crate::nir::module::Block,
    ) {
        match pattern {
            // Wildcards and bare identifiers match unconditionally (a lone
            // name is a binding in both the typecker and the interpreter).
            TypedPattern::Wildcard | TypedPattern::Ident(_) => {
                test.set_terminator(Instr::Branch { target: arm_id });
            }
            TypedPattern::Literal(lit) => {
                let expected = self.fresh_value();
                test.add_instr(Instr::Const {
                    dst: expected,
                    value: Self::lit_to_const(lit),
                    ty: NirTy::new(Ty::Unknown, self.mode),
                });
                let t = self.fresh_value();
                test.add_instr(Instr::ICmp {
                    dst: t,
                    op: CmpOp::Eq,
                    lhs: scrut_val,
                    rhs: expected,
                });
                test.set_terminator(Instr::CondBranch {
                    cond: t,
                    then_block: arm_id,
                    else_block: next_id,
                });
            }
            TypedPattern::Variant(name, subs) => {
                match name.as_str() {
                    // Single-payload prelude constructors: truthiness
                    // discriminates Some/Ok from anything else.
                    "Some" | "Ok" if subs.len() == 1 => {
                        test.set_terminator(Instr::CondBranch {
                            cond: scrut_val,
                            then_block: arm_id,
                            else_block: next_id,
                        });
                    }
                    // Empty constructors: inverted truthiness. (`None`
                    // ignores subpatterns entirely, matching the
                    // interpreter.)
                    "None" | "Err" if subs.len() <= 1 => {
                        test.set_terminator(Instr::CondBranch {
                            cond: scrut_val,
                            then_block: next_id,
                            else_block: arm_id,
                        });
                    }
                    _ if self.enum_variants.contains_key(name) => {
                        let tag = self.fresh_value();
                        test.add_instr(Instr::EnumTag { dst: tag, src: scrut_val });
                        let expected = self.fresh_value();
                        let idx = self.enum_variants.get(name).map(|(_, t)| *t).unwrap_or(0);
                        test.add_instr(Instr::Const {
                            dst: expected,
                            value: ConstValue::Int(idx as i128),
                            ty: NirTy::new(Ty::Int, self.mode),
                        });
                        let t = self.fresh_value();
                        test.add_instr(Instr::ICmp {
                            dst: t,
                            op: CmpOp::Eq,
                            lhs: tag,
                            rhs: expected,
                        });
                        test.set_terminator(Instr::CondBranch {
                            cond: t,
                            then_block: arm_id,
                            else_block: next_id,
                        });
                    }
                    // Unknown variant or impossible arity (e.g. bare `Some`
                    // with no subpattern): never matches. Typeck should have
                    // rejected it; fall through rather than mis-firing.
                    _ => {
                        test.set_terminator(Instr::Branch { target: next_id });
                    }
                }
            }
        }
    }

    /// Bind an arm pattern's names for the arm body. Payloads come from
    /// indexed field extraction; a bare identifier aliases the whole
    /// scrutinee value via a Move into a fresh slot (never by aliasing
    /// the scrutinee register itself, so later assignment can't corrupt
    /// it). Mirrors the interpreter's binding table.
    fn bind_pattern(
        &mut self,
        pattern: &TypedPattern,
        scrut_val: ValueId,
        block: &mut crate::nir::module::Block,
    ) {
        match pattern {
            TypedPattern::Ident(name) => {
                let dst = self.fresh_value();
                block.add_instr(Instr::Move { dst, src: scrut_val });
                self.local_values.insert(name.clone(), dst);
            }
            TypedPattern::Wildcard | TypedPattern::Literal(_) => {}
            TypedPattern::Variant(name, subs) => {
                let is_prelude_ctor = matches!(name.as_str(), "Some" | "Ok" | "Err");
                if (is_prelude_ctor && subs.len() == 1) || self.enum_variants.contains_key(name) {
                    for (i, sub) in subs.iter().enumerate() {
                        let payload = self.fresh_value();
                        block.add_instr(Instr::EnumPayload {
                            dst: payload,
                            src: scrut_val,
                            index: i as u32,
                            ty: NirTy::new(Ty::Unknown, self.mode),
                        });
                        self.bind_pattern_leaf(sub, payload, block);
                    }
                }
                // `None`, arity-mismatched and unknown variants bind nothing
                // (their arms are unreachable or payload-less by the same
                // rules the test above enforces).
            }
        }
    }

    /// Bind a single (non-composite-position) subpattern to an extracted
    /// payload value.
    fn bind_pattern_leaf(
        &mut self,
        pattern: &TypedPattern,
        payload: ValueId,
        block: &mut crate::nir::module::Block,
    ) {
        match pattern {
            TypedPattern::Ident(name) => {
                let dst = self.fresh_value();
                block.add_instr(Instr::Move { dst, src: payload });
                self.local_values.insert(name.clone(), dst);
            }
            TypedPattern::Wildcard => {}
            _ => {
                // Literal and nested-variant subpatterns (e.g.
                // `Circle(0)`): valid code with no lowering yet. Fail
                // loudly (compiler ICE with a pointer) rather than
                // silently miscompiling; see NIR.md §6.
                unimplemented!(
                    "non-identifier match subpattern in NIR lowering (see NIR.md §6)"
                );
            }
        }
    }

    fn lit_to_const(lit: &TypedLit) -> ConstValue {
        match lit {
            TypedLit::Int(v) => ConstValue::Int(*v),
            TypedLit::Float(v) => ConstValue::Float(*v),
            TypedLit::Bool(v) => ConstValue::Bool(*v),
            TypedLit::Char(v) => ConstValue::Char(*v),
            TypedLit::String(v) => ConstValue::String(v.clone()),
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
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::FloatLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::Float(*v),
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::BoolLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::Bool(*v),
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::CharLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::Char(*v),
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::StringLit(v) => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst,
                    value: ConstValue::String(v.clone()),
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::None => {
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::OptionNone {
                    dst,
                    ty: NirTy::new(expr.ty.clone(), self.mode),
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
                    ty: NirTy::new(expr.ty.clone(), self.mode),
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
                    ty: NirTy::new(expr.ty.clone(), self.mode),
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
                    ty: NirTy::new(expr.ty.clone(), self.mode),
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

                if let TypedExprKind::Ident(name) = &callee.kind {
                    if name == "print" || name == "println" {
                        let print_val = arg_vals.first().copied().unwrap_or(ValueId(0));
                        nir_block.add_instr(Instr::Print { val: print_val });
                        nir_block.add_instr(Instr::Const {
                            dst,
                            value: ConstValue::Unit,
                            ty: NirTy::new(expr.ty.clone(), self.mode),
                        });
                        return dst;
                    }
                }

                let func_id = if let TypedExprKind::Ident(name) = &callee.kind {
                    self.func_infos.iter()
                        .find(|(n, _, _)| n == name)
                        .map(|(_, id, _)| *id)
                        // Unresolved callee (e.g. a method/member call the
                        // resolver/typeck let through, like `String.starts_with`
                        // on a value type with no lowering support yet) must
                        // NOT silently fall back to FuncId(0) — id 0 is a real,
                        // arbitrary function (whichever the source declares
                        // first), so that fallback used to make the VM quietly
                        // call the wrong function, sometimes recursing into
                        // itself and stack-overflowing instead of reporting a
                        // proper error. FuncId::UNRESOLVED is never assigned by
                        // `new_func_id`, so the VM's `get_function_by_id` lookup
                        // cleanly fails with `VmError::FunctionNotFound`.
                        .unwrap_or(FuncId::UNRESOLVED)
                } else {
                    FuncId::UNRESOLVED
                };
                nir_block.add_instr(Instr::Call {
                    dst,
                    func: func_id,
                    args: arg_vals,
                    ret_ty: NirTy::new(expr.ty.clone(), self.mode),
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
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), self.mode)
                    }),
                    crate::ast::BinOp::Sub => nir_block.add_instr(Instr::Sub {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), self.mode)
                    }),
                    crate::ast::BinOp::Mul => nir_block.add_instr(Instr::Mul {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), self.mode)
                    }),
                    crate::ast::BinOp::Div => nir_block.add_instr(Instr::Div {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), self.mode)
                    }),
                    crate::ast::BinOp::Rem => nir_block.add_instr(Instr::Rem {
                        dst, lhs, rhs, ty: NirTy::new(expr.ty.clone(), self.mode)
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
                        dst, src, ty: NirTy::new(expr.ty.clone(), self.mode)
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
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            // `expr?` — early-return None/Err, otherwise unwrap.
            //
            // Control-flow shape (all blocks terminated; the continue block
            // is swapped into `*nir_block` so later code keeps emitting
            // there — see `split_current_block`):
            //
            //   current:  ...; src = <inner>; cond_branch src -> continue | early
            //   early:    none = option_none | err = result_err(payload(src));
            //             early_return ...
            //   continue: dst = try_unwrap src; ...
            //
            // Branching uses truthiness of the Option/Result value itself
            // (Some/Ok are truthy, None/Err falsy), matching the
            // interpreter. `TryUnwrap` in `continue` is infallible by
            // construction; the VM traps if it ever sees None/Err there.
            TypedExprKind::Try(inner) => {
                let src = self.lower_expr(inner, nir_block);
                let is_result = matches!(inner.ty, Ty::Result(_, _));

                let continue_id = self.module.new_block_id();
                let early_id = self.module.new_block_id();
                nir_block.set_terminator(Instr::CondBranch {
                    cond: src,
                    then_block: continue_id,
                    else_block: early_id,
                });

                let mut early = crate::nir::module::Block::new(early_id);
                if is_result {
                    let payload = self.fresh_value();
                    early.add_instr(Instr::EnumPayload {
                        dst: payload,
                        src,
                        index: 0,
                        ty: NirTy::new(inner.ty.clone(), self.mode),
                    });
                    let err_val = self.fresh_value();
                    early.add_instr(Instr::ResultErr {
                        dst: err_val,
                        val: payload,
                        ty: NirTy::new(inner.ty.clone(), self.mode),
                    });
                    early.set_terminator(Instr::EarlyReturn { val: err_val });
                } else {
                    let none_val = self.fresh_value();
                    early.add_instr(Instr::OptionNone {
                        dst: none_val,
                        ty: NirTy::new(inner.ty.clone(), self.mode),
                    });
                    early.set_terminator(Instr::EarlyReturn { val: none_val });
                }
                self.add_block_to_current_func(early);

                let mut cont = crate::nir::module::Block::new(continue_id);
                let dst = self.fresh_value();
                cont.add_instr(Instr::TryUnwrap {
                    dst,
                    src,
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                self.split_current_block(nir_block, cont);
                dst
            }
            // `l ?? r` — short-circuiting coalesce over Option.
            //
            // Shape: current evaluates `l` only, then branches; `r` is
            // evaluated solely in the else block (matching the interpreter,
            // which never touches `r` when `l` is Some). The merge block is
            // swapped into `*nir_block` so later code keeps emitting there.
            //
            //   current:  ...; left = <l>; cond_branch left -> unwrap | else
            //   unwrap:   u = try_unwrap left; branch merge
            //   else:     rv = <r>; dst = move rv; branch merge
            //   merge:    dst = phi [(u, unwrap), (rv, else)]; ...
            TypedExprKind::Coalesce { left, right } => {
                let left_val = self.lower_expr(left, nir_block);

                let unwrap_id = self.module.new_block_id();
                let else_id = self.module.new_block_id();
                let merge_id = self.module.new_block_id();

                // Lower the else side first: `right` is arbitrary code that
                // may itself split blocks (swapping the temp's identity), so
                // the id used by the branch/Phi below is re-read afterwards.
                // (`unwrap` needs no such treatment: TryUnwrap is atomic.)
                let dst = self.fresh_value();
                let mut else_block = crate::nir::module::Block::new(else_id);
                let right_val = self.lower_expr(right, &mut else_block);
                let else_actual = else_block.id;
                else_block.add_instr(Instr::Move { dst, src: right_val });
                else_block.set_terminator(Instr::Branch { target: merge_id });
                self.add_block_to_current_func(else_block);

                let mut unwrap = crate::nir::module::Block::new(unwrap_id);
                let unwrapped = self.fresh_value();
                unwrap.add_instr(Instr::TryUnwrap {
                    dst: unwrapped,
                    src: left_val,
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                unwrap.set_terminator(Instr::Branch { target: merge_id });
                self.add_block_to_current_func(unwrap);

                nir_block.set_terminator(Instr::CondBranch {
                    cond: left_val,
                    then_block: unwrap_id,
                    else_block: else_actual,
                });

                let mut merge = crate::nir::module::Block::new(merge_id);
                merge.add_instr(Instr::Phi {
                    dst,
                    incoming: vec![(unwrapped, unwrap_id), (right_val, else_actual)],
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                self.split_current_block(nir_block, merge);
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
                                ty: NirTy::new(Ty::String, self.mode),
                            });
                            if let Some(existing) = result_val {
                                let new_result = ValueId(self.next_local_index);
                                self.next_local_index += 1;
                                nir_block.add_instr(Instr::Add {
                                    dst: new_result,
                                    lhs: existing,
                                    rhs: str_val,
                                    ty: NirTy::new(Ty::String, self.mode),
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
                            nir_block.add_instr(Instr::ToString {
                                dst: str_val,
                                src: expr_val,
                            });
                            if let Some(existing) = result_val {
                                let new_result = ValueId(self.next_local_index);
                                self.next_local_index += 1;
                                nir_block.add_instr(Instr::Add {
                                    dst: new_result,
                                    lhs: existing,
                                    rhs: str_val,
                                    ty: NirTy::new(Ty::String, self.mode),
                                });
                                result_val = Some(new_result);
                            } else {
                                result_val = Some(str_val);
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
                        ty: NirTy::new(Ty::String, self.mode),
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
                    field_names: vec![],
                    ty: NirTy::new(expr.ty.clone(), self.mode),
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
                    field_names: vec![],
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::Index { object, index } => {
                let obj_val = self.lower_expr(object, nir_block);
                let idx_val = self.lower_expr(index, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::ListIndex {
                    dst,
                    src: obj_val,
                    index: idx_val,
                });
                dst
            }
            TypedExprKind::Range { start, end, inclusive: _ } => {
                let start_val = self.lower_expr(start, nir_block);
                let end_val = self.lower_expr(end, nir_block);
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::StructNew {
                    dst,
                    fields: vec![start_val, end_val],
                    field_names: vec!["start".to_string(), "end".to_string()],
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            // `if` expression: same convergence discipline as the
            // statement form (merge swapped into `*nir_block`), but the
            // value always flows (expression position guarantees it).
            TypedExprKind::IfExpr { condition, then_expr, else_expr } => {
                let cond_val = self.lower_expr(condition, nir_block);
                let then_block = self.module.new_block_id();
                let else_block = self.module.new_block_id();
                let merge_block = self.module.new_block_id();

                let mut then_nir_block = crate::nir::module::Block::new(then_block);
                let then_val = self.lower_expr(then_expr, &mut then_nir_block);
                // Guarded: a nested empty match can leave Unreachable here;
                // never overwrite an existing terminator.
                if !then_nir_block.has_terminator() {
                    then_nir_block.set_terminator(Instr::Branch { target: merge_block });
                }
                let then_actual = then_nir_block.id;
                self.add_block_to_current_func(then_nir_block);

                let mut else_nir_block = crate::nir::module::Block::new(else_block);
                let else_val = if let Some(e) = else_expr {
                    self.lower_expr(e, &mut else_nir_block)
                } else {
                    // No else: Unit. A *defined* Unit const — the old code
                    // used a bare unallocated id that a later allocation
                    // could silently reuse for another value.
                    let unit = self.fresh_value();
                    else_nir_block.add_instr(Instr::Const {
                        dst: unit,
                        value: ConstValue::Unit,
                        ty: NirTy::new(Ty::Unit, self.mode),
                    });
                    unit
                };
                if !else_nir_block.has_terminator() {
                    else_nir_block.set_terminator(Instr::Branch { target: merge_block });
                }
                let else_actual = else_nir_block.id;
                self.add_block_to_current_func(else_nir_block);

                nir_block.set_terminator(Instr::CondBranch {
                    cond: cond_val,
                    then_block: then_actual,
                    else_block: else_actual,
                });

                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                let mut merge_nir_block = crate::nir::module::Block::new(merge_block);
                merge_nir_block.add_instr(Instr::Phi {
                    dst,
                    incoming: vec![(then_val, then_actual), (else_val, else_actual)],
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                self.split_current_block(nir_block, merge_nir_block);
                dst
            }
            // Match expression: identical to statement match, except the
            // merged value is always needed (fallback Unit when no arm can
            // flow, e.g. empty arm list).
            TypedExprKind::MatchExpr { scrutinee, arms } => {
                let scrut_val = self.lower_expr(scrutinee, nir_block);
                match self.lower_match(scrut_val, arms, nir_block) {
                    Some(v) => v,
                    None => self.unit_value(nir_block),
                }
            }
            TypedExprKind::EnumVariant { enum_name: _, variant } => {
                let tag = self.enum_variants.get(variant).map(|(_, t)| *t).unwrap_or(0);
                let tag_dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::Const {
                    dst: tag_dst,
                    value: ConstValue::Int(tag as i128),
                    ty: NirTy::new(Ty::Int, self.mode),
                });
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::EnumNew {
                    dst,
                    tag: tag_dst,
                    fields: vec![],
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::StructLit { name: _, fields } => {
                let field_vals: Vec<ValueId> = fields.iter()
                    .map(|(_, e)| self.lower_expr(e, nir_block))
                    .collect();
                let field_names: Vec<String> = fields.iter()
                    .map(|(n, _)| n.clone())
                    .collect();
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                nir_block.add_instr(Instr::StructNew {
                    dst,
                    fields: field_vals,
                    field_names,
                    ty: NirTy::new(expr.ty.clone(), self.mode),
                });
                dst
            }
            TypedExprKind::Spread(inner) => {
                self.lower_expr(inner, nir_block)
            }
            TypedExprKind::Closure { params: _, body: _ } => {
                let captured: Vec<ValueId> = Vec::new();
                let dst = ValueId(self.next_local_index);
                self.next_local_index += 1;
                let func_id = self.func_infos.iter()
                    .find(|(n, _, _)| n == &self.current_func.clone().unwrap_or_default())
                    .map(|(_, id, _)| *id)
                    .unwrap_or(FuncId::UNRESOLVED);
                nir_block.add_instr(Instr::ClosureNew {
                    dst,
                    func: func_id,
                    captured,
                    ty: NirTy::new(expr.ty.clone(), self.mode),
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
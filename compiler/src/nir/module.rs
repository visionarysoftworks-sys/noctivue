//! NIR module structure — functions, basic blocks, and module container.

use crate::hir::types::Ty;
use crate::nir::instr::Instr;
use crate::nir::types::{BlockId, FuncId, FuncSig, NirTy, ValueId};
use std::collections::HashMap;

/// A basic block in NIR — sequence of instructions terminated by a terminator.
#[derive(Debug, Clone)]
pub struct Block {
    pub id: BlockId,
    pub params: Vec<(ValueId, NirTy)>, // Block parameters (for phi-free SSA)
    pub instrs: Vec<Instr>,
    pub terminator: Option<Instr>, // Must be a terminator instruction
}

impl Block {
    pub fn new(id: BlockId) -> Self {
        Block { id, params: Vec::new(), instrs: Vec::new(), terminator: None }
    }

    pub fn add_instr(&mut self, instr: Instr) {
        self.instrs.push(instr);
    }

    pub fn set_terminator(&mut self, term: Instr) {
        self.terminator = Some(term);
    }

    pub fn has_terminator(&self) -> bool {
        self.terminator.is_some()
    }
}

/// A NIR function — signature + basic blocks.
#[derive(Debug, Clone)]
pub struct NirFunction {
    pub id: FuncId,
    pub name: String,
    pub sig: FuncSig,
    pub blocks: Vec<Block>,
    /// Mapping from HIR function for debugging
    pub hir_name: String,
}

impl NirFunction {
    pub fn new(id: FuncId, name: String, sig: FuncSig, hir_name: String) -> Self {
        NirFunction { id, name, sig, blocks: Vec::new(), hir_name }
    }

    pub fn add_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    pub fn entry_block(&self) -> Option<&Block> {
        self.blocks.first()
    }

    pub fn entry_block_mut(&mut self) -> Option<&mut Block> {
        self.blocks.first_mut()
    }
}

/// NIR module — container for all functions and type definitions.
#[derive(Debug, Clone)]
pub struct NirModule {
    pub functions: Vec<NirFunction>,
    pub type_defs: HashMap<String, Ty>, // struct/enum definitions
    pub func_by_name: HashMap<String, FuncId>,
    pub next_func_id: u32,
    pub next_block_id: u32,
    pub next_value_id: u32,
}

impl NirModule {
    pub fn new() -> Self {
        NirModule {
            functions: Vec::new(),
            type_defs: HashMap::new(),
            func_by_name: HashMap::new(),
            next_func_id: 0,
            next_block_id: 0,
            next_value_id: 0,
        }
    }

    pub fn add_type_def(&mut self, name: String, ty: Ty) {
        self.type_defs.insert(name, ty);
    }

    pub fn new_func_id(&mut self) -> FuncId {
        let id = FuncId(self.next_func_id);
        self.next_func_id += 1;
        id
    }

    pub fn new_block_id(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }

    pub fn new_value_id(&mut self) -> ValueId {
        let id = ValueId(self.next_value_id);
        self.next_value_id += 1;
        id
    }

    pub fn add_function(&mut self, func: NirFunction) -> FuncId {
        let id = func.id;
        self.func_by_name.insert(func.name.clone(), id);
        self.functions.push(func);
        id
    }

    pub fn get_function(&self, name: &str) -> Option<&NirFunction> {
        self.func_by_name.get(name).and_then(|id| self.functions.iter().find(|f| f.id == *id))
    }

    pub fn get_function_mut(&mut self, name: &str) -> Option<&mut NirFunction> {
        let id = *self.func_by_name.get(name)?;
        self.functions.iter_mut().find(|f| f.id == id)
    }

    /// Get a function by its FuncId
    pub fn get_function_by_id(&self, id: FuncId) -> Option<&NirFunction> {
        self.functions.iter().find(|f| f.id == id)
    }

    /// Get a function by its FuncId (mutable)
    pub fn get_function_by_id_mut(&mut self, id: FuncId) -> Option<&mut NirFunction> {
        self.functions.iter_mut().find(|f| f.id == id)
    }
}

impl Default for NirModule {
    fn default() -> Self {
        Self::new()
    }
}
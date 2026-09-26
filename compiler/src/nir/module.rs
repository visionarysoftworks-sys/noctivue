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

    pub fn has_return_terminator(&self) -> bool {
        matches!(self.terminator, Some(Instr::Return { .. }))
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

// ── Textual NIR — the `.nvir` dump format (NIR.md §7.1) ──────────────────
//
// Text first, deliberately: dumps get read, diffed, and grepped by backend
// and differential work, so the canonical form is the instruction `Display`
// output (the same source of truth as NIR.md §4's inventory) under a small
// module header. A binary encoding is a later, separately-decided format —
// never an accident of this one.
//
// Determinism is part of the contract: type definitions come out sorted by
// name (`type_defs` is a HashMap, so iteration order would otherwise differ
// run to run and every diff would be noise). Functions and blocks keep
// lowering order, which is already deterministic.

/// Header metadata the module itself does not carry.
#[derive(Debug, Clone, Default)]
pub struct NirText {
    /// `(name, version)` from `nestpkg.nvpm`, or `None` for a
    /// manifest-less single-file build (rendered as `-`).
    pub package: Option<(String, String)>,
    /// The joined source set, in order. This is the module's real input
    /// list today; resolved cross-package module paths arrive with the
    /// cross-package import story, not before.
    pub imports: Vec<String>,
}

/// `nvir_version` of the text format. Bump on any incompatible change to
/// the header or body shape.
pub const NVIR_VERSION: u32 = 1;

/// Render `module` as canonical `.nvir` text.
///
/// Fails loudly (never a partial dump) when a block has no terminator:
/// NIR.md §4.1 rule 1 makes an unterminated block a lowering bug, and
/// printing one as if it were fine would hide exactly that.
pub fn to_nir_text(module: &NirModule, meta: &NirText) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(&format!("nvir_version: {NVIR_VERSION}\n"));
    match &meta.package {
        Some((name, version)) => out.push_str(&format!("package: {name} {version}\n")),
        None => out.push_str("package: -\n"),
    }
    if meta.imports.is_empty() {
        out.push_str("imports:\n");
    } else {
        out.push_str("imports:\n");
        for path in &meta.imports {
            out.push_str(&format!("    {path}\n"));
        }
    }

    // Sorted: `type_defs` is a HashMap, and byte-reproducible output is
    // what makes `git diff` useful on a dump.
    let mut types: Vec<(&String, &Ty)> = module.type_defs.iter().collect();
    types.sort_by(|a, b| a.0.cmp(b.0));
    if types.is_empty() {
        out.push_str("types:\n");
    } else {
        out.push_str("types:\n");
        for (name, ty) in types {
            out.push_str(&format!("    {name}: {ty}\n"));
        }
    }

    if module.functions.is_empty() {
        out.push_str("functions:\n");
        return Ok(out);
    }
    out.push_str("functions:\n");
    for func in &module.functions {
        let params: Vec<String> = func.sig.params.iter().map(|p| p.to_string()).collect();
        // No trailing `(mode)`: every `NirTy` already renders its own mode
        // tag (types.rs), so the return type carries the signature's mode.
        out.push_str(&format!(
            "    {} {}({}) -> {}:\n",
            func.id,
            func.name,
            params.join(", "),
            func.sig.ret
        ));
        if func.blocks.is_empty() {
            return Err(format!(
                "function `{}` has no blocks (lowering bug: a lowered function always has an entry block)",
                func.name
            ));
        }
        for block in &func.blocks {
            let params: Vec<String> = block
                .params
                .iter()
                .map(|(v, t)| format!("{v}: {t}"))
                .collect();
            if params.is_empty() {
                out.push_str(&format!("        {}:\n", block.id));
            } else {
                out.push_str(&format!("        {}({}):\n", block.id, params.join(", ")));
            }
            for instr in &block.instrs {
                out.push_str(&format!("            {instr}\n"));
            }
            match &block.terminator {
                Some(term) => out.push_str(&format!("            {term}\n")),
                None => {
                    return Err(format!(
                        "block {} of `{}` has no terminator (NIR.md 4.1 rule 1: every \
                         block must be terminated, dead blocks included)",
                        block.id, func.name
                    ))
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod nir_text_tests {
    use super::*;
    use crate::nir::instr::ConstValue;
    use crate::nir::types::Mode;

    fn int_ty() -> NirTy {
        NirTy::native(Ty::Int)
    }

    fn sample_module() -> NirModule {
        let mut m = NirModule::new();
        // Two types, added out of order on purpose: the dump must sort them.
        m.add_type_def("Zeta".to_string(), Ty::Int);
        m.add_type_def("Alpha".to_string(), Ty::Bool);

        let id = m.new_func_id();
        let mut func = NirFunction::new(
            id,
            "answer".to_string(),
            FuncSig::new(vec![int_ty()], int_ty(), Mode::Native),
            "answer".to_string(),
        );
        let entry = m.new_block_id();
        let dst = m.new_value_id();
        let mut block = Block::new(entry);
        block.add_instr(Instr::Const { dst, value: ConstValue::Int(42), ty: int_ty() });
        block.set_terminator(Instr::Return { val: Some(dst) });
        func.add_block(block);
        m.add_function(func);
        m
    }

    #[test]
    fn dump_is_canonical_and_sorted() {
        let module = sample_module();
        let meta = NirText {
            package: Some(("myapp".to_string(), "0.1.0".to_string())),
            imports: vec!["lib/main.nv".to_string(), "lib/shapes.nv".to_string()],
        };
        let text = to_nir_text(&module, &meta).expect("dump must render");
        let expected = "\
nvir_version: 1
package: myapp 0.1.0
imports:
    lib/main.nv
    lib/shapes.nv
types:
    Alpha: Bool
    Zeta: Int
functions:
    @func0 answer(Int (native)) -> Int (native):
        block0:
            %0 = const 42 : Int (native)
            return %0
";
        assert_eq!(text, expected);
        // Byte-reproducible: same input, same bytes (the sort is what
        // makes this true for the HashMap-backed type table).
        assert_eq!(to_nir_text(&module, &meta).expect("second dump"), text);
    }

    #[test]
    fn manifest_less_build_renders_a_dash_not_a_guess() {
        let text = to_nir_text(&sample_module(), &NirText::default()).expect("dump must render");
        assert!(text.contains("package: -\n"), "got:\n{text}");
        assert!(text.contains("imports:\ntypes:\n"), "empty sections stay present:\n{text}");
    }

    #[test]
    fn unterminated_block_fails_loudly_instead_of_dumping_partial_ir() {
        let mut module = sample_module();
        module.functions[0].blocks[0].terminator = None;
        let err = to_nir_text(&module, &NirText::default())
            .expect_err("an unterminated block must not dump");
        assert!(err.contains("no terminator"), "error should name the cause: {err}");
        assert!(err.contains("answer"), "error should name the function: {err}");
    }
}
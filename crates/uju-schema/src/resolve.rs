use std::collections::HashMap;

use crate::ast;
use crate::diag::Diagnostics;
use crate::ir::TypeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    Type(TypeId),
    Const(u32),
}

#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    symbols: HashMap<String, Symbol>,
    order: Vec<TypeId>,
}

impl SymbolTable {
    pub fn lookup(&self, path: &ast::Path) -> Option<Symbol> {
        todo!()
    }

    pub fn lookup_type(&self, path: &ast::Path) -> Option<TypeId> {
        todo!()
    }

    pub fn item(&self, id: TypeId) -> &ast::Item {
        todo!()
    }

    pub fn topological_order(&self) -> &[TypeId] {
        todo!()
    }
}

pub fn resolve(schema: &ast::Schema, diags: &mut Diagnostics) -> Option<SymbolTable> {
    todo!()
}

fn collect(schema: &ast::Schema, diags: &mut Diagnostics) -> SymbolTable {
    todo!()
}

fn check_references(schema: &ast::Schema, table: &SymbolTable, diags: &mut Diagnostics) {
    todo!()
}

fn check_enums(schema: &ast::Schema, diags: &mut Diagnostics) {
    todo!()
}

fn check_consts(schema: &ast::Schema, table: &SymbolTable, diags: &mut Diagnostics) {
    todo!()
}

fn sort_topologically(
    schema: &ast::Schema,
    table: &mut SymbolTable,
    diags: &mut Diagnostics,
) -> bool {
    todo!()
}

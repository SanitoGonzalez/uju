use crate::ast;
use crate::diag::Diagnostics;
use crate::ir;
use crate::resolve::SymbolTable;

pub fn lower(
    schema: &ast::Schema,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<ir::Schema> {
    todo!()
}

fn lower_namespace(namespace: Option<&ast::Path>) -> Vec<String> {
    todo!()
}

fn lower_struct(
    def: &ast::Struct,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<ir::StructDef> {
    todo!()
}

fn lower_field(
    field: &ast::Field,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<ir::FieldDef> {
    todo!()
}

fn lower_enum(def: &ast::Enum, diags: &mut Diagnostics) -> Option<ir::EnumDef> {
    todo!()
}

fn lower_const(
    def: &ast::Const,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<ir::ConstDef> {
    todo!()
}

fn lower_type(
    ty: &ast::Spanned<ast::TypeRef>,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<ir::Ty> {
    todo!()
}

fn lower_expr(
    expr: &ast::Spanned<ast::Expr>,
    ty: &ir::Ty,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<ir::ConstValue> {
    todo!()
}

fn layout(fields: &[ir::FieldDef], types: &[ir::TypeDef]) -> ir::Size {
    todo!()
}

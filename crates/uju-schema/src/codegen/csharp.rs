use crate::codegen::{Backend, GeneratedFile};
use crate::ir;

pub struct CSharp;

impl Backend for CSharp {
    fn name(&self) -> &str {
        "csharp"
    }

    fn emit(&self, schema: &ir::Schema) -> Vec<GeneratedFile> {
        todo!()
    }
}

fn type_name(schema: &ir::Schema, ty: &ir::Ty) -> String {
    todo!()
}

fn field_name(name: &str) -> String {
    todo!()
}

fn escape_keyword(name: &str) -> String {
    todo!()
}

fn struct_def(schema: &ir::Schema, def: &ir::StructDef) -> String {
    todo!()
}

fn enum_def(def: &ir::EnumDef) -> String {
    todo!()
}

fn const_def(schema: &ir::Schema, def: &ir::ConstDef) -> String {
    todo!()
}

fn encode_method(schema: &ir::Schema, def: &ir::StructDef) -> String {
    todo!()
}

fn decode_method(schema: &ir::Schema, def: &ir::StructDef) -> String {
    todo!()
}

fn encode_expr(schema: &ir::Schema, ty: &ir::Ty, value: &str) -> String {
    todo!()
}

fn decode_expr(schema: &ir::Schema, ty: &ir::Ty) -> String {
    todo!()
}

use std::collections::{HashMap, HashSet};

use crate::ast::{self, Prim, Spanned, TypeRef};
use crate::diag::Diagnostics;
use crate::ir::{RecordKind, TypeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    Type(TypeId),
    Const(u32),
}

#[derive(Debug)]
pub struct SymbolTable<'a> {
    pub types: Vec<TypeEntry<'a>>,
    pub consts: Vec<ConstEntry<'a>>,
    symbols: HashMap<String, Symbol>,
    order: Vec<TypeId>,
    namespace: Option<String>,
}

#[derive(Debug)]
pub struct TypeEntry<'a> {
    pub name: String,
    pub scope: Option<String>,
    pub decl: TypeDecl<'a>,
}

#[derive(Debug)]
pub enum TypeDecl<'a> {
    Enum(&'a ast::Enum),
    Record(RecordDecl<'a>),
}

#[derive(Debug)]
pub struct RecordDecl<'a> {
    pub kind: RecordKind,
    pub ident: &'a ast::Ident,
    pub fields: &'a [ast::Field],
    pub returns: Option<&'a ast::Path>,
}

#[derive(Debug)]
pub struct ConstEntry<'a> {
    pub name: String,
    pub scope: Option<String>,
    pub decl: &'a ast::Const,
}

impl TypeEntry<'_> {
    pub fn field_scope(&self) -> Option<&str> {
        match &self.decl {
            TypeDecl::Record(decl) if decl.kind == RecordKind::Message => Some(&self.name),
            _ => self.scope.as_deref(),
        }
    }

    pub fn span(&self) -> ast::Span {
        match &self.decl {
            TypeDecl::Enum(e) => e.name.span,
            TypeDecl::Record(decl) => decl.ident.span,
        }
    }
}

impl<'a> SymbolTable<'a> {
    pub fn lookup(&self, scope: Option<&str>, path: &ast::Path) -> Option<Symbol> {
        let name = dotted(path);
        if path.0.len() == 1 {
            if let Some(scope) = scope {
                if let Some(symbol) = self.symbols.get(&format!("{scope}.{name}")) {
                    return Some(*symbol);
                }
            }
        }
        if let Some(symbol) = self.symbols.get(&name) {
            return Some(*symbol);
        }
        if let Some(ns) = &self.namespace {
            if let Some(rest) = name.strip_prefix(&format!("{ns}.")) {
                return self.symbols.get(rest).copied();
            }
        }
        None
    }

    pub fn lookup_type(&self, scope: Option<&str>, path: &ast::Path) -> Option<TypeId> {
        match self.lookup(scope, path) {
            Some(Symbol::Type(id)) => Some(id),
            _ => None,
        }
    }

    pub fn entry(&self, id: TypeId) -> &TypeEntry<'a> {
        &self.types[id.0 as usize]
    }

    pub fn topological_order(&self) -> &[TypeId] {
        &self.order
    }
}

pub fn dotted(path: &ast::Path) -> String {
    path.0
        .iter()
        .map(|i| i.node.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub fn strip_optional(ty: &Spanned<TypeRef>) -> (&Spanned<TypeRef>, bool) {
    match &ty.node {
        TypeRef::Optional(inner) => (inner, true),
        _ => (ty, false),
    }
}

pub fn resolve<'a>(schema: &'a ast::Schema, diags: &mut Diagnostics) -> Option<SymbolTable<'a>> {
    let mut table = collect(schema, diags);
    check_references(&table, diags);
    check_enums(&table, diags);
    check_fields(&table, diags);
    sort_topologically(&mut table, diags);
    if diags.has_errors() {
        None
    } else {
        Some(table)
    }
}

fn collect<'a>(schema: &'a ast::Schema, diags: &mut Diagnostics) -> SymbolTable<'a> {
    let mut table = SymbolTable {
        types: Vec::new(),
        consts: Vec::new(),
        symbols: HashMap::new(),
        order: Vec::new(),
        namespace: schema.namespace.as_ref().map(dotted),
    };
    for item in &schema.items {
        collect_item(&mut table, None, item, diags);
    }
    table
}

fn collect_item<'a>(
    table: &mut SymbolTable<'a>,
    scope: Option<&str>,
    item: &'a ast::Item,
    diags: &mut Diagnostics,
) {
    match item {
        ast::Item::Const(c) => {
            if let Some(name) = declare(table, scope, &c.name, false, diags) {
                table.consts.push(ConstEntry {
                    name,
                    scope: scope.map(str::to_string),
                    decl: c,
                });
            }
        }
        ast::Item::Enum(e) => {
            if let Some(name) = declare(table, scope, &e.name, true, diags) {
                table.types.push(TypeEntry {
                    name,
                    scope: scope.map(str::to_string),
                    decl: TypeDecl::Enum(e),
                });
            }
        }
        ast::Item::Struct(s) => {
            if let Some(name) = declare(table, scope, &s.name, true, diags) {
                let kind = match s.kind {
                    ast::StructKind::Struct => RecordKind::Struct,
                    ast::StructKind::Component => RecordKind::Component,
                };
                table.types.push(TypeEntry {
                    name,
                    scope: scope.map(str::to_string),
                    decl: TypeDecl::Record(RecordDecl {
                        kind,
                        ident: &s.name,
                        fields: &s.fields,
                        returns: None,
                    }),
                });
            }
        }
        ast::Item::Message(m) => {
            let Some(name) = declare(table, scope, &m.name, true, diags) else {
                return;
            };
            table.types.push(TypeEntry {
                name: name.clone(),
                scope: scope.map(str::to_string),
                decl: TypeDecl::Record(RecordDecl {
                    kind: RecordKind::Message,
                    ident: &m.name,
                    fields: &m.fields,
                    returns: m.returns.as_ref(),
                }),
            });
            for nested in &m.items {
                collect_item(table, Some(&name), nested, diags);
            }
        }
    }
}

fn declare(
    table: &mut SymbolTable,
    scope: Option<&str>,
    ident: &ast::Ident,
    is_type: bool,
    diags: &mut Diagnostics,
) -> Option<String> {
    let name = match scope {
        Some(scope) => format!("{scope}.{}", ident.node),
        None => ident.node.clone(),
    };
    if table.symbols.contains_key(&name) {
        diags.error(ident.span, format!("duplicate definition `{name}`"));
        return None;
    }
    let symbol = if is_type {
        Symbol::Type(TypeId(table.types.len() as u32))
    } else {
        Symbol::Const(table.consts.len() as u32)
    };
    table.symbols.insert(name.clone(), symbol);
    Some(name)
}

fn check_references(table: &SymbolTable, diags: &mut Diagnostics) {
    for entry in &table.types {
        let TypeDecl::Record(decl) = &entry.decl else {
            continue;
        };
        let scope = entry.field_scope();
        for field in decl.fields {
            check_typeref(&field.ty, scope, table, diags);
        }
        if let Some(returns) = decl.returns {
            match table.lookup(scope, returns) {
                Some(Symbol::Type(id)) => {
                    let is_message = matches!(
                        &table.entry(id).decl,
                        TypeDecl::Record(r) if r.kind == RecordKind::Message
                    );
                    if !is_message {
                        diags.error(
                            returns.span(),
                            format!("`{}` is not a message", dotted(returns)),
                        );
                    }
                }
                _ => diags.error(
                    returns.span(),
                    format!("unknown message `{}`", dotted(returns)),
                ),
            }
        }
    }
    for entry in &table.consts {
        check_typeref(&entry.decl.ty, entry.scope.as_deref(), table, diags);
    }
}

fn check_typeref(
    ty: &Spanned<TypeRef>,
    scope: Option<&str>,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) {
    match &ty.node {
        TypeRef::Prim(_) => {}
        TypeRef::Named(path) => match table.lookup(scope, path) {
            Some(Symbol::Type(_)) => {}
            Some(Symbol::Const(_)) => diags.error(
                path.span(),
                format!("`{}` is a const, not a type", dotted(path)),
            ),
            None => diags.error(path.span(), format!("unknown type `{}`", dotted(path))),
        },
        TypeRef::Vec(t) | TypeRef::Set(t) | TypeRef::Optional(t) => {
            check_typeref(t, scope, table, diags)
        }
        TypeRef::Map(k, v) => {
            check_typeref(k, scope, table, diags);
            check_typeref(v, scope, table, diags);
        }
    }
}

fn check_enums(table: &SymbolTable, diags: &mut Diagnostics) {
    for entry in &table.types {
        let TypeDecl::Enum(e) = &entry.decl else {
            continue;
        };
        let repr = match &e.repr {
            Some(r) => {
                if !r.node.is_unsigned() {
                    diags.error(r.span, "enum repr must be an unsigned integer");
                }
                r.node
            }
            None => Prim::U32,
        };
        let max = match repr {
            Prim::U8 => u8::MAX as i64,
            Prim::U16 => u16::MAX as i64,
            Prim::U32 => u32::MAX as i64,
            _ => i64::MAX,
        };
        let mut next: i64 = 0;
        let mut names = HashSet::new();
        let mut values = HashSet::new();
        for variant in &e.variants {
            if !names.insert(variant.name.node.as_str()) {
                diags.error(
                    variant.name.span,
                    format!("duplicate variant `{}`", variant.name.node),
                );
            }
            let (value, span) = match &variant.value {
                Some(v) => (v.node, v.span),
                None => (next, variant.name.span),
            };
            if value < 0 {
                diags.error(span, "enum value must be non-negative");
            } else if value > max {
                diags.error(span, format!("enum value does not fit in {}", repr.name()));
            } else if !values.insert(value) {
                diags.error(span, format!("duplicate enum value {value}"));
            }
            next = value.saturating_add(1);
        }
    }
}

fn check_fields(table: &SymbolTable, diags: &mut Diagnostics) {
    for entry in &table.types {
        let TypeDecl::Record(decl) = &entry.decl else {
            continue;
        };
        let mut names = HashSet::new();
        for field in decl.fields {
            if !names.insert(field.name.node.as_str()) {
                diags.error(
                    field.name.span,
                    format!("duplicate field `{}`", field.name.node),
                );
            }
        }
    }
}

fn sort_topologically(table: &mut SymbolTable, diags: &mut Diagnostics) {
    let mut state = vec![0u8; table.types.len()];
    let mut order = Vec::with_capacity(table.types.len());
    for idx in 0..table.types.len() {
        visit(idx, table, &mut state, &mut order, diags);
    }
    table.order = order;
}

fn visit(
    idx: usize,
    table: &SymbolTable,
    state: &mut [u8],
    order: &mut Vec<TypeId>,
    diags: &mut Diagnostics,
) {
    match state[idx] {
        2 => return,
        1 => {
            let entry = &table.types[idx];
            diags.error(entry.span(), format!("recursive type `{}`", entry.name));
            return;
        }
        _ => {}
    }
    state[idx] = 1;
    if let TypeDecl::Record(decl) = &table.types[idx].decl {
        let scope = table.types[idx].field_scope();
        for field in decl.fields {
            let (ty, _) = strip_optional(&field.ty);
            if let TypeRef::Named(path) = &ty.node {
                if let Some(dep) = table.lookup_type(scope, path) {
                    visit(dep.0 as usize, table, state, order, diags);
                }
            }
        }
    }
    state[idx] = 2;
    order.push(TypeId(idx as u32));
}

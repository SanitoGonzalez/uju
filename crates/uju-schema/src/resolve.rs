use std::collections::{HashMap, HashSet};

use crate::ast::{self, Prim, Spanned, TypeRef};
use crate::diag::Diagnostics;
use crate::ir::{Name, RecordKind, TypeId};

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
    files: Vec<FileInfo>,
    order: Vec<TypeId>,
}

#[derive(Debug)]
struct FileInfo {
    namespace: Vec<String>,
    uses: Vec<Vec<String>>,
}

#[derive(Debug)]
pub struct TypeEntry<'a> {
    pub name: Name,
    pub file: usize,
    pub scope: Vec<String>,
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
    pub name: Name,
    pub file: usize,
    pub decl: &'a ast::Const,
}

#[derive(Debug, Clone, Copy)]
pub struct Context<'s> {
    pub file: usize,
    pub scope: &'s [String],
}

impl TypeEntry<'_> {
    pub fn context(&self) -> Context<'_> {
        Context {
            file: self.file,
            scope: &self.scope,
        }
    }

    pub fn span(&self) -> ast::Span {
        match &self.decl {
            TypeDecl::Enum(e) => e.name.span,
            TypeDecl::Record(decl) => decl.ident.span,
        }
    }
}

impl ConstEntry<'_> {
    pub fn context(&self) -> Context<'_> {
        Context {
            file: self.file,
            scope: &self.name.scope,
        }
    }
}

impl<'a> SymbolTable<'a> {
    pub fn lookup(&self, cx: Context, path: &ast::Path) -> Option<Symbol> {
        let name = dotted(path);
        let file = &self.files[cx.file];

        for depth in (0..=cx.scope.len()).rev() {
            let mut candidate = file.namespace.clone();
            candidate.extend_from_slice(&cx.scope[..depth]);
            if let Some(symbol) = self.symbols.get(&join(&candidate, &name)) {
                return Some(*symbol);
            }
        }
        if let Some(symbol) = self.symbols.get(&name) {
            return Some(*symbol);
        }
        let mut found = None;
        for used in &file.uses {
            if let Some(symbol) = self.symbols.get(&join(used, &name)) {
                if found.is_some_and(|f| f != *symbol) {
                    return None;
                }
                found = Some(*symbol);
            }
        }
        found
    }

    pub fn lookup_type(&self, cx: Context, path: &ast::Path) -> Option<TypeId> {
        match self.lookup(cx, path) {
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

fn join(prefix: &[String], name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}.{name}", prefix.join("."))
    }
}

pub fn strip_optional(ty: &Spanned<TypeRef>) -> (&Spanned<TypeRef>, bool) {
    match &ty.node {
        TypeRef::Optional(inner) => (inner, true),
        _ => (ty, false),
    }
}

pub fn resolve<'a>(files: &'a [ast::Schema], diags: &mut Diagnostics) -> Option<SymbolTable<'a>> {
    let mut table = collect(files, diags);
    check_uses(files, &table, diags);
    check_references(&table, diags);
    check_enums(&table, diags);
    check_fields(&table, diags);
    sort_topologically(&mut table, diags);
    diags.set_file(0);
    if diags.has_errors() {
        None
    } else {
        Some(table)
    }
}

fn collect<'a>(files: &'a [ast::Schema], diags: &mut Diagnostics) -> SymbolTable<'a> {
    let mut table = SymbolTable {
        types: Vec::new(),
        consts: Vec::new(),
        symbols: HashMap::new(),
        files: Vec::new(),
        order: Vec::new(),
    };
    for (index, file) in files.iter().enumerate() {
        let namespace: Vec<String> = file
            .namespace
            .as_ref()
            .map(|p| p.0.iter().map(|i| i.node.clone()).collect())
            .unwrap_or_default();
        table.files.push(FileInfo {
            namespace: namespace.clone(),
            uses: file
                .uses
                .iter()
                .map(|p| p.0.iter().map(|i| i.node.clone()).collect())
                .collect(),
        });
        diags.set_file(index);
        for item in &file.items {
            collect_item(&mut table, index, &namespace, &[], item, diags);
        }
    }
    table
}

fn collect_item<'a>(
    table: &mut SymbolTable<'a>,
    file: usize,
    namespace: &[String],
    scope: &[String],
    item: &'a ast::Item,
    diags: &mut Diagnostics,
) {
    match item {
        ast::Item::Const(c) => {
            if let Some(name) = declare(table, namespace, scope, &c.name, false, diags) {
                table.consts.push(ConstEntry {
                    name,
                    file,
                    decl: c,
                });
            }
        }
        ast::Item::Enum(e) => {
            if let Some(name) = declare(table, namespace, scope, &e.name, true, diags) {
                table.types.push(TypeEntry {
                    name,
                    file,
                    scope: scope.to_vec(),
                    decl: TypeDecl::Enum(e),
                });
            }
        }
        ast::Item::Struct(s) => {
            if let Some(name) = declare(table, namespace, scope, &s.name, true, diags) {
                let kind = match s.kind {
                    ast::StructKind::Struct => RecordKind::Struct,
                    ast::StructKind::Component => RecordKind::Component,
                };
                table.types.push(TypeEntry {
                    name,
                    file,
                    scope: scope.to_vec(),
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
            let Some(name) = declare(table, namespace, scope, &m.name, true, diags) else {
                return;
            };
            let inner: Vec<String> = scope.iter().cloned().chain([m.name.node.clone()]).collect();
            table.types.push(TypeEntry {
                name,
                file,
                scope: inner.clone(),
                decl: TypeDecl::Record(RecordDecl {
                    kind: RecordKind::Message,
                    ident: &m.name,
                    fields: &m.fields,
                    returns: m.returns.as_ref(),
                }),
            });
            for nested in &m.items {
                collect_item(table, file, namespace, &inner, nested, diags);
            }
        }
    }
}

fn declare(
    table: &mut SymbolTable,
    namespace: &[String],
    scope: &[String],
    ident: &ast::Ident,
    is_type: bool,
    diags: &mut Diagnostics,
) -> Option<Name> {
    let name = Name {
        namespace: namespace.to_vec(),
        scope: scope.to_vec(),
        name: ident.node.clone(),
    };
    let qualified = name.qualified();
    if table.symbols.contains_key(&qualified) {
        diags.error(ident.span, format!("duplicate definition `{qualified}`"));
        return None;
    }
    let symbol = if is_type {
        Symbol::Type(TypeId(table.types.len() as u32))
    } else {
        Symbol::Const(table.consts.len() as u32)
    };
    table.symbols.insert(qualified, symbol);
    Some(name)
}

fn check_uses(files: &[ast::Schema], table: &SymbolTable, diags: &mut Diagnostics) {
    let known: HashSet<&[String]> = table
        .types
        .iter()
        .map(|t| t.name.namespace.as_slice())
        .collect();
    for (index, file) in files.iter().enumerate() {
        diags.set_file(index);
        for used in &file.uses {
            let path: Vec<String> = used.0.iter().map(|i| i.node.clone()).collect();
            if !known.contains(path.as_slice()) {
                diags.error(used.span(), format!("unknown namespace `{}`", dotted(used)));
            }
        }
    }
}

fn check_references(table: &SymbolTable, diags: &mut Diagnostics) {
    for entry in &table.types {
        let TypeDecl::Record(decl) = &entry.decl else {
            continue;
        };
        diags.set_file(entry.file);
        let cx = entry.context();
        for field in decl.fields {
            check_typeref(&field.ty, cx, table, diags);
        }
        if let Some(returns) = decl.returns {
            match table.lookup(cx, returns) {
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
        diags.set_file(entry.file);
        check_typeref(&entry.decl.ty, entry.context(), table, diags);
    }
}

fn check_typeref(ty: &Spanned<TypeRef>, cx: Context, table: &SymbolTable, diags: &mut Diagnostics) {
    match &ty.node {
        TypeRef::Prim(_) => {}
        TypeRef::Named(path) => match table.lookup(cx, path) {
            Some(Symbol::Type(_)) => {}
            Some(Symbol::Const(_)) => diags.error(
                path.span(),
                format!("`{}` is a const, not a type", dotted(path)),
            ),
            None => diags.error(path.span(), format!("unknown type `{}`", dotted(path))),
        },
        TypeRef::Vec(t) | TypeRef::Set(t) | TypeRef::Optional(t) => {
            check_typeref(t, cx, table, diags)
        }
        TypeRef::Map(k, v) => {
            check_typeref(k, cx, table, diags);
            check_typeref(v, cx, table, diags);
        }
    }
}

fn check_enums(table: &SymbolTable, diags: &mut Diagnostics) {
    for entry in &table.types {
        let TypeDecl::Enum(e) = &entry.decl else {
            continue;
        };
        diags.set_file(entry.file);
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
        diags.set_file(entry.file);
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
            diags.set_file(entry.file);
            diags.error(
                entry.span(),
                format!("recursive type `{}`", entry.name.qualified()),
            );
            return;
        }
        _ => {}
    }
    state[idx] = 1;
    if let TypeDecl::Record(decl) = &table.types[idx].decl {
        let cx = table.types[idx].context();
        for field in decl.fields {
            let (ty, _) = strip_optional(&field.ty);
            if let TypeRef::Named(path) = &ty.node {
                if let Some(dep) = table.lookup_type(cx, path) {
                    visit(dep.0 as usize, table, state, order, diags);
                }
            }
        }
    }
    state[idx] = 2;
    order.push(TypeId(idx as u32));
}

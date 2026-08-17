use crate::ast::{self, Expr, Prim, Spanned, TypeRef};
use crate::diag::Diagnostics;
use crate::ir::{self, ConstValue, RecordKind, Size, Ty};
use crate::resolve::{
    ConstEntry, RecordDecl, SymbolTable, TypeDecl, TypeEntry, dotted, strip_optional,
};

pub fn lower(
    schema: &ast::Schema,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<ir::Schema> {
    let mut message_ids = vec![0u32; table.types.len()];
    let mut next_id = 0;
    for (idx, entry) in table.types.iter().enumerate() {
        if let TypeDecl::Record(decl) = &entry.decl {
            if decl.kind == RecordKind::Message {
                message_ids[idx] = next_id;
                next_id += 1;
            }
        }
    }

    let mut types: Vec<Option<ir::TypeDef>> = vec![None; table.types.len()];
    for &id in table.topological_order() {
        let idx = id.0 as usize;
        let entry = &table.types[idx];
        types[idx] = match &entry.decl {
            TypeDecl::Enum(e) => Some(ir::TypeDef::Enum(lower_enum(&entry.name, e))),
            TypeDecl::Record(decl) => {
                lower_record(entry, decl, message_ids[idx], table, &types, diags)
                    .map(ir::TypeDef::Record)
            }
        };
    }

    let mut consts = Vec::new();
    for entry in &table.consts {
        if let Some(def) = lower_const(entry, table, &types, diags) {
            consts.push(def);
        }
    }

    if diags.has_errors() {
        return None;
    }
    Some(ir::Schema {
        namespace: lower_namespace(schema.namespace.as_ref()),
        types: types.into_iter().map(Option::unwrap).collect(),
        consts,
    })
}

fn lower_namespace(namespace: Option<&ast::Path>) -> Vec<String> {
    namespace
        .map(|p| p.0.iter().map(|i| i.node.clone()).collect())
        .unwrap_or_default()
}

fn lower_enum(name: &str, def: &ast::Enum) -> ir::EnumDef {
    let repr = def.repr.as_ref().map(|r| r.node).unwrap_or(Prim::U32);
    let mut variants = Vec::with_capacity(def.variants.len());
    let mut next = 0u64;
    for variant in &def.variants {
        let value = variant
            .value
            .as_ref()
            .map(|v| v.node as u64)
            .unwrap_or(next);
        variants.push(ir::VariantDef {
            name: variant.name.node.clone(),
            value,
        });
        next = value.saturating_add(1);
    }
    ir::EnumDef {
        name: name.to_string(),
        repr,
        variants,
    }
}

fn lower_record(
    entry: &TypeEntry,
    decl: &RecordDecl,
    message_id: u32,
    table: &SymbolTable,
    types: &[Option<ir::TypeDef>],
    diags: &mut Diagnostics,
) -> Option<ir::RecordDef> {
    let scope = entry.field_scope();
    let mut fields = Vec::with_capacity(decl.fields.len());
    for field in decl.fields {
        let (ty_ast, optional) = strip_optional(&field.ty);
        let ty = lower_type(ty_ast, scope, table, diags)?;
        fields.push((field, ty, optional));
    }

    let sizes: Vec<Size> = fields.iter().map(|(_, ty, _)| size_of(ty, types)).collect();
    let optional_fixed = fields
        .iter()
        .zip(&sizes)
        .filter(|((_, _, optional), size)| *optional && matches!(size, Size::Fixed(_)))
        .count() as u32;
    let bitmap_bytes = (optional_fixed + 7) / 8;

    let mut cursor = bitmap_bytes;
    let mut next_bit = 0u32;
    let mut variable = false;
    let mut out = Vec::with_capacity(fields.len());
    for ((field, ty, optional), size) in fields.into_iter().zip(sizes) {
        let offset = cursor;
        let mut bit = None;
        match size {
            Size::Fixed(n) => {
                if optional {
                    bit = Some(next_bit);
                    next_bit += 1;
                }
                cursor += n;
            }
            Size::Variable => {
                variable = true;
                cursor += 2;
            }
        }
        out.push(ir::FieldDef {
            name: field.name.node.clone(),
            ty,
            optional,
            offset,
            bit,
        });
    }

    if decl.kind == RecordKind::Struct && variable {
        diags.error(
            decl.ident.span,
            format!("struct `{}` must be fixed-size", entry.name),
        );
        return None;
    }
    if cursor > u16::MAX as u32 {
        diags.error(
            decl.ident.span,
            format!("fixed part of `{}` exceeds 65535 bytes", entry.name),
        );
        return None;
    }

    let size = if variable {
        Size::Variable
    } else {
        Size::Fixed(cursor)
    };
    let message = (decl.kind == RecordKind::Message).then(|| ir::MessageInfo {
        id: message_id,
        returns: decl.returns.and_then(|path| table.lookup_type(scope, path)),
    });
    Some(ir::RecordDef {
        name: entry.name.clone(),
        kind: decl.kind,
        fields: out,
        layout: ir::Layout {
            bitmap_bytes,
            fixed_size: cursor,
            size,
        },
        message,
    })
}

fn size_of(ty: &Ty, types: &[Option<ir::TypeDef>]) -> Size {
    match ty {
        Ty::Prim(p) => p.size().map(Size::Fixed).unwrap_or(Size::Variable),
        Ty::Ref(id) => types[id.0 as usize]
            .as_ref()
            .map(|t| t.size())
            .unwrap_or(Size::Variable),
        Ty::Vec(_) | Ty::Set(_) | Ty::Map(_, _) => Size::Variable,
    }
}

fn lower_type(
    ty: &Spanned<TypeRef>,
    scope: Option<&str>,
    table: &SymbolTable,
    diags: &mut Diagnostics,
) -> Option<Ty> {
    match &ty.node {
        TypeRef::Prim(p) => Some(Ty::Prim(*p)),
        TypeRef::Named(path) => table.lookup_type(scope, path).map(Ty::Ref),
        TypeRef::Vec(t) => Some(Ty::Vec(Box::new(lower_type(t, scope, table, diags)?))),
        TypeRef::Set(t) => {
            let element = lower_type(t, scope, table, diags)?;
            if element.is_container() {
                diags.error(t.span, "set elements cannot be containers");
                return None;
            }
            Some(Ty::Set(Box::new(element)))
        }
        TypeRef::Map(k, v) => {
            let key = lower_type(k, scope, table, diags)?;
            if key.is_container() {
                diags.error(k.span, "map keys cannot be containers");
                return None;
            }
            let value = lower_type(v, scope, table, diags)?;
            Some(Ty::Map(Box::new(key), Box::new(value)))
        }
        TypeRef::Optional(_) => {
            diags.error(ty.span, "optional is only allowed on fields");
            None
        }
    }
}

fn lower_const(
    entry: &ConstEntry,
    table: &SymbolTable,
    types: &[Option<ir::TypeDef>],
    diags: &mut Diagnostics,
) -> Option<ir::ConstDef> {
    let scope = entry.scope.as_deref();
    let decl = entry.decl;
    let ty = lower_type(&decl.ty, scope, table, diags)?;
    let value = lower_expr(&decl.value, &ty, decl.ty.span, scope, table, types, diags)?;
    Some(ir::ConstDef {
        name: entry.name.clone(),
        ty,
        value,
    })
}

fn lower_expr(
    expr: &Spanned<Expr>,
    ty: &Ty,
    ty_span: ast::Span,
    scope: Option<&str>,
    table: &SymbolTable,
    types: &[Option<ir::TypeDef>],
    diags: &mut Diagnostics,
) -> Option<ConstValue> {
    let mismatch = |diags: &mut Diagnostics| {
        diags.error(expr.span, "const value does not match its type");
        None
    };
    match ty {
        Ty::Prim(p) if p.is_integer() => match &expr.node {
            Expr::Int(x) if int_fits(*x, *p) => Some(ConstValue::Int(*x)),
            Expr::Int(_) => {
                diags.error(expr.span, format!("value out of range for {p}"));
                None
            }
            _ => mismatch(diags),
        },
        Ty::Prim(Prim::F32 | Prim::F64) => match &expr.node {
            Expr::Float(x) => Some(ConstValue::Float(*x)),
            Expr::Int(x) => Some(ConstValue::Float(*x as f64)),
            _ => mismatch(diags),
        },
        Ty::Prim(Prim::Bool) => match &expr.node {
            Expr::Bool(b) => Some(ConstValue::Bool(*b)),
            _ => mismatch(diags),
        },
        Ty::Prim(Prim::String) => match &expr.node {
            Expr::Str(s) => Some(ConstValue::String(s.clone())),
            _ => mismatch(diags),
        },
        Ty::Prim(Prim::Timestamp | Prim::Interval) => match &expr.node {
            Expr::Int(x) => Some(ConstValue::Int(*x)),
            _ => mismatch(diags),
        },
        Ty::Ref(id) => {
            let Some(ir::TypeDef::Enum(def)) = types[id.0 as usize].as_ref() else {
                diags.error(ty_span, "const type must be a scalar, string, or enum");
                return None;
            };
            let Expr::Path(path) = &expr.node else {
                return mismatch(diags);
            };
            let (variant, prefix) = path.0.split_last().unwrap();
            if prefix.is_empty() {
                return mismatch(diags);
            }
            let enum_path = ast::Path(prefix.to_vec());
            if table.lookup_type(scope, &enum_path) != Some(*id) {
                diags.error(
                    expr.span,
                    format!("`{}` is not a variant of `{}`", dotted(path), def.name),
                );
                return None;
            }
            match def.variants.iter().position(|v| v.name == variant.node) {
                Some(index) => Some(ConstValue::Variant {
                    ty: *id,
                    index: index as u32,
                }),
                None => {
                    diags.error(
                        variant.span,
                        format!("no variant `{}` in `{}`", variant.node, def.name),
                    );
                    None
                }
            }
        }
        _ => {
            diags.error(ty_span, "const type must be a scalar, string, or enum");
            None
        }
    }
}

fn int_fits(value: i64, prim: Prim) -> bool {
    match prim {
        Prim::I8 => i8::try_from(value).is_ok(),
        Prim::I16 => i16::try_from(value).is_ok(),
        Prim::I32 => i32::try_from(value).is_ok(),
        Prim::I64 => true,
        Prim::U8 => u8::try_from(value).is_ok(),
        Prim::U16 => u16::try_from(value).is_ok(),
        Prim::U32 => u32::try_from(value).is_ok(),
        Prim::U64 => value >= 0,
        _ => false,
    }
}

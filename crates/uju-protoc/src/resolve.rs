//! Name collection and resolution over parsed schemas.
//!
//! A [`Resolver`] walks every schema once, assigning a [`TypeId`] to each
//! type declaration and a [`ConstId`] to each const, and can then resolve
//! paths from any scope. Types and consts share their scopes: one name
//! means one thing. A name is looked up first through the lexical scopes
//! (enclosing declarations, then the source's namespace), then as a path
//! qualified by a visible namespace's full name, and finally as an
//! unqualified name from a `use`d namespace.

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, SourceId};
use crate::ir::{ConstId, NamespaceId, TypeId};
use crate::parser::ast;

/// What a name in a scope refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    Type(TypeId),
    Const(ConstId),
}

type Scope<'src> = HashMap<&'src str, Symbol>;

/// A type declaration, with everything needed to lower it.
pub struct TypeSite<'a, 'src> {
    pub source: SourceId,
    pub namespace: NamespaceId,
    pub parent: Option<TypeId>,
    /// Never an [`ast::Item::Const`]; those are [`ConstSite`]s.
    pub item: &'a ast::Item<'src>,
    scope: Scope<'src>,
}

/// A const declaration, with everything needed to lower it.
pub struct ConstSite<'a, 'src> {
    pub source: SourceId,
    pub namespace: NamespaceId,
    pub parent: Option<TypeId>,
    pub item: &'a ast::Const<'src>,
}

struct NamespaceSite<'src> {
    name: Vec<&'src str>,
    scope: Scope<'src>,
}

struct SourceSite {
    namespace: NamespaceId,
    uses: Vec<NamespaceId>,
}

pub struct Resolver<'a, 'src> {
    namespaces: Vec<NamespaceSite<'src>>,
    sources: Vec<SourceSite>,
    pub types: Vec<TypeSite<'a, 'src>>,
    pub consts: Vec<ConstSite<'a, 'src>>,
}

impl<'a, 'src> Resolver<'a, 'src> {
    /// Collect every declaration of `schemas`, which are indexed by
    /// [`SourceId`], reporting duplicate names and unknown `use`s.
    pub fn new(schemas: &'a [ast::Schema<'src>]) -> (Self, Vec<Diagnostic>) {
        let mut resolver = Resolver {
            namespaces: Vec::new(),
            sources: Vec::new(),
            types: Vec::new(),
            consts: Vec::new(),
        };
        let mut diagnostics = Vec::new();

        for (index, schema) in schemas.iter().enumerate() {
            let source = SourceId(index);
            let namespace = resolver.intern_namespace(&schema.namespace);
            resolver.sources.push(SourceSite {
                namespace,
                uses: Vec::new(),
            });
            for item in &schema.items {
                resolver.collect(source, namespace, None, item, &mut diagnostics);
            }
        }

        // Resolved once every schema has registered its namespace, so that
        // `use` is insensitive to the order sources are passed in.
        for (index, schema) in schemas.iter().enumerate() {
            for use_ in &schema.uses {
                match resolver.find_namespace(use_) {
                    Some(id) => {
                        let uses = &mut resolver.sources[index].uses;
                        if !uses.contains(&id) {
                            uses.push(id);
                        }
                    }
                    None => diagnostics.push(Diagnostic::new(
                        SourceId(index),
                        use_.span.clone(),
                        format!("unknown namespace `{use_}`"),
                    )),
                }
            }
        }

        (resolver, diagnostics)
    }

    pub fn namespace_len(&self) -> usize {
        self.namespaces.len()
    }

    pub fn namespace_name(&self, id: NamespaceId) -> &[&'src str] {
        &self.namespaces[id.0].name
    }

    /// Resolve `path` to a type, as seen from `scope` (the innermost
    /// enclosing type declaration, if any) within `source`.
    pub fn resolve(
        &self,
        source: SourceId,
        scope: Option<TypeId>,
        path: &ast::Path<'src>,
    ) -> Result<TypeId, Diagnostic> {
        match self.resolve_symbol(source, scope, path)? {
            Symbol::Type(id) => Ok(id),
            Symbol::Const(_) => Err(Diagnostic::new(
                source,
                path.span.clone(),
                format!("`{path}` is a const, not a type"),
            )),
        }
    }

    /// Resolve `path` to whatever it names, as in [`Resolver::resolve`].
    pub fn resolve_symbol(
        &self,
        source: SourceId,
        scope: Option<TypeId>,
        path: &ast::Path<'src>,
    ) -> Result<Symbol, Diagnostic> {
        let (first, first_span) = &path.segments[0];

        // Lexical scopes: enclosing declarations innermost-first, then the
        // source's own namespace. The innermost match wins, even if walking
        // the rest of the path out of it fails.
        let mut cursor = scope;
        loop {
            let entries = match cursor {
                Some(id) => &self.types[id.0].scope,
                None => &self.namespaces[self.sources[source.0].namespace.0].scope,
            };
            if let Some(&symbol) = entries.get(first) {
                return self.walk(source, path, symbol, 1);
            }
            match cursor {
                Some(id) => cursor = self.types[id.0].parent,
                None => break,
            }
        }

        let site = &self.sources[source.0];

        // A path qualified by a visible namespace's full name; the longest
        // prefix wins when namespace names nest (`foo` and `foo.bar`).
        let mut qualified: Option<NamespaceId> = None;
        for &id in [site.namespace].iter().chain(&site.uses) {
            let name = &self.namespaces[id.0].name;
            if path.segments.len() > name.len()
                && name.iter().zip(&path.segments).all(|(a, (b, _))| a == b)
                && qualified.is_none_or(|q| self.namespaces[q.0].name.len() < name.len())
            {
                qualified = Some(id);
            }
        }
        if let Some(id) = qualified {
            let namespace = &self.namespaces[id.0];
            let (name, span) = &path.segments[namespace.name.len()];
            return match namespace.scope.get(name) {
                Some(&symbol) => self.walk(source, path, symbol, namespace.name.len() + 1),
                None => Err(Diagnostic::new(
                    source,
                    span.clone(),
                    format!("no `{name}` in namespace `{}`", namespace.name.join(".")),
                )),
            };
        }

        // An unqualified name from a `use`d namespace.
        let mut found = Vec::new();
        for &id in &site.uses {
            if let Some(&target) = self.namespaces[id.0].scope.get(first) {
                found.push((id, target));
            }
        }
        match found[..] {
            [(_, target)] => self.walk(source, path, target, 1),
            [] => Err(Diagnostic::new(
                source,
                first_span.clone(),
                format!("cannot find `{first}` in this scope"),
            )),
            _ => Err(Diagnostic::new(
                source,
                first_span.clone(),
                format!(
                    "`{first}` is ambiguous; it is declared in namespaces {}",
                    found
                        .iter()
                        .map(|(id, _)| format!("`{}`", self.namespaces[id.0].name.join(".")))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
            )),
        }
    }

    /// Follow the segments of `path` from `from` onwards, starting at
    /// `symbol`, which the segment before `from` names.
    fn walk(
        &self,
        source: SourceId,
        path: &ast::Path<'src>,
        mut symbol: Symbol,
        from: usize,
    ) -> Result<Symbol, Diagnostic> {
        for (index, (name, span)) in path.segments.iter().enumerate().skip(from) {
            let id = match symbol {
                Symbol::Type(id) => id,
                Symbol::Const(_) => {
                    let (outer, outer_span) = &path.segments[index - 1];
                    return Err(Diagnostic::new(
                        source,
                        outer_span.clone(),
                        format!("`{outer}` is a const and has no nested declarations"),
                    ));
                }
            };
            let site = &self.types[id.0];
            symbol = *site.scope.get(name).ok_or_else(|| {
                Diagnostic::new(
                    source,
                    span.clone(),
                    format!(
                        "`{}` has no nested declaration `{name}`",
                        site.item.name().0
                    ),
                )
            })?;
        }
        Ok(symbol)
    }

    /// Register `item`, and what is nested in it, and declare its name in
    /// the scope of `parent`, or of `namespace` at the top level.
    fn collect(
        &mut self,
        source: SourceId,
        namespace: NamespaceId,
        parent: Option<TypeId>,
        item: &'a ast::Item<'src>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let symbol = match item {
            ast::Item::Const(item) => {
                let id = ConstId(self.consts.len());
                self.consts.push(ConstSite {
                    source,
                    namespace,
                    parent,
                    item,
                });
                Symbol::Const(id)
            }
            _ => {
                let id = TypeId(self.types.len());
                self.types.push(TypeSite {
                    source,
                    namespace,
                    parent,
                    item,
                    scope: Scope::new(),
                });
                for nested in item.body().map_or(&[][..], |body| &body.items) {
                    self.collect(source, namespace, Some(id), nested, diagnostics);
                }
                Symbol::Type(id)
            }
        };
        self.declare(source, namespace, parent, item.name(), symbol, diagnostics);
    }

    fn declare(
        &mut self,
        source: SourceId,
        namespace: NamespaceId,
        owner: Option<TypeId>,
        name: &ast::Ident<'src>,
        symbol: Symbol,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let scope = match owner {
            Some(id) => &mut self.types[id.0].scope,
            None => &mut self.namespaces[namespace.0].scope,
        };
        if scope.insert(name.0, symbol).is_some() {
            diagnostics.push(Diagnostic::new(
                source,
                name.1.clone(),
                format!(
                    "the name `{}` is declared more than once in this scope",
                    name.0
                ),
            ));
        }
    }

    fn intern_namespace(&mut self, path: &ast::Path<'src>) -> NamespaceId {
        let name: Vec<&'src str> = path.segments.iter().map(|(segment, _)| *segment).collect();
        if let Some(index) = self.namespaces.iter().position(|site| site.name == name) {
            return NamespaceId(index);
        }
        self.namespaces.push(NamespaceSite {
            name,
            scope: Scope::new(),
        });
        NamespaceId(self.namespaces.len() - 1)
    }

    fn find_namespace(&self, path: &ast::Path<'_>) -> Option<NamespaceId> {
        self.namespaces
            .iter()
            .position(|site| {
                site.name.len() == path.segments.len()
                    && site
                        .name
                        .iter()
                        .zip(&path.segments)
                        .all(|(a, (b, _))| a == b)
            })
            .map(NamespaceId)
    }
}

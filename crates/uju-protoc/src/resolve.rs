//! Name collection and resolution over parsed schemas.
//!
//! A [`Resolver`] walks every schema once, assigning a [`TypeId`] to each
//! declaration, and can then resolve paths from any scope. A name is looked up first through the lexical scopes
//! (enclosing declarations, then the source's namespace), then as a path
//! qualified by a visible namespace's full name, and finally as an
//! unqualified name from a `use`d namespace.

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, SourceId};
use crate::ir::{NamespaceId, TypeId};
use crate::parser::ast;

type Scope<'src> = HashMap<&'src str, TypeId>;

/// A type declaration, with everything needed to lower it.
pub struct TypeSite<'a, 'src> {
    pub source: SourceId,
    pub namespace: NamespaceId,
    pub parent: Option<TypeId>,
    pub item: &'a ast::Item<'src>,
    scope: Scope<'src>,
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
}

impl<'a, 'src> Resolver<'a, 'src> {
    /// Collect every declaration of `schemas`, which are indexed by
    /// [`SourceId`], reporting duplicate names and unknown `use`s.
    pub fn new(schemas: &'a [ast::Schema<'src>]) -> (Self, Vec<Diagnostic>) {
        let mut resolver = Resolver {
            namespaces: Vec::new(),
            sources: Vec::new(),
            types: Vec::new(),
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
                let id = resolver.collect(source, namespace, None, item, &mut diagnostics);
                resolver.declare(source, namespace, None, item.name(), id, &mut diagnostics);
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

    /// Resolve `path` as seen from `scope` (the innermost enclosing type
    /// declaration, if any) within `source`.
    pub fn resolve(
        &self,
        source: SourceId,
        scope: Option<TypeId>,
        path: &ast::Path<'src>,
    ) -> Result<TypeId, Diagnostic> {
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
            if let Some(&id) = entries.get(first) {
                return self.walk(source, path, id, 1);
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
                Some(&id) => self.walk(source, path, id, namespace.name.len() + 1),
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

    /// Follow the segments of `path` from `from` onwards, starting at `id`.
    fn walk(
        &self,
        source: SourceId,
        path: &ast::Path<'src>,
        mut id: TypeId,
        from: usize,
    ) -> Result<TypeId, Diagnostic> {
        for (name, span) in &path.segments[from..] {
            let site = &self.types[id.0];
            id = *site.scope.get(name).ok_or_else(|| {
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
        Ok(id)
    }

    fn collect(
        &mut self,
        source: SourceId,
        namespace: NamespaceId,
        parent: Option<TypeId>,
        item: &'a ast::Item<'src>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> TypeId {
        let id = TypeId(self.types.len());
        self.types.push(TypeSite {
            source,
            namespace,
            parent,
            item,
            scope: Scope::new(),
        });
        if let Some(body) = item.body() {
            for nested in &body.items {
                let nested_id = self.collect(source, namespace, Some(id), nested, diagnostics);
                self.declare(
                    source,
                    namespace,
                    Some(id),
                    nested.name(),
                    nested_id,
                    diagnostics,
                );
            }
        }
        id
    }

    fn declare(
        &mut self,
        source: SourceId,
        namespace: NamespaceId,
        owner: Option<TypeId>,
        name: &ast::Ident<'src>,
        id: TypeId,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let scope = match owner {
            Some(id) => &mut self.types[id.0].scope,
            None => &mut self.namespaces[namespace.0].scope,
        };
        if scope.insert(name.0, id).is_some() {
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

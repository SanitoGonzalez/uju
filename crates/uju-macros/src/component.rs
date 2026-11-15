use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, Result, Type};

pub fn expand(ast: DeriveInput) -> Result<TokenStream> {
    if !ast.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &ast.generics,
            "generic types cannot derive Component",
        ));
    }

    let ident = &ast.ident;

    // `#[component(storage = Tracked<Self>)]` picks the table
    let mut storage: Option<Type> = None;
    for attr in &ast.attrs {
        if !attr.path().is_ident("component") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("storage") {
                storage = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error("expected `storage = Type`"))
            }
        })?;
    }
    let storage = match storage {
        Some(storage) => quote!(#storage),
        None => quote!(::uju::ecs::storage::sparse_set::SparseSet<#ident>),
    };

    Ok(quote! {
        const _: () = {
            #[::uju::linkme::distributed_slice(::uju::ecs::component::COMPONENTS)]
            #[linkme(crate = ::uju::linkme)]
            static REGISTRATION: ::uju::ecs::component::Registration =
                ::uju::ecs::component::Registration {
                    name: ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident)),
                    id: ::core::cell::UnsafeCell::new(u16::MAX),
                    new_table: ::uju::ecs::component::new_table::<#ident>,
                };

            impl ::uju::ecs::component::Component for #ident {
                type Storage = #storage;

                #[inline(always)]
                fn id() -> ::uju::ecs::component::Id {
                    unsafe { *REGISTRATION.id.get() }
                }
            }
        };
    })
}

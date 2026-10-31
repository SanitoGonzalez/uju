use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, Result};

pub fn expand(ast: DeriveInput) -> Result<TokenStream> {
    if !ast.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &ast.generics,
            "generic types cannot derive Component",
        ));
    }

    let ident = &ast.ident;

    Ok(quote! {
        const _: () = {
            #[::uju::linkme::distributed_slice(::uju::ecs::component::COMPONENTS)]
            #[linkme(crate = ::uju::linkme)]
            static REGISTRATION: ::uju::ecs::component::Registration =
                ::uju::ecs::component::Registration {
                    name: ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident)),
                    id: ::core::cell::UnsafeCell::new(u16::MAX),
                    new_table: || ::std::boxed::Box::new(
                        ::uju::ecs::storage::sparse_set::SparseSet::<#ident>::new(),
                    ),
                };

            impl ::uju::ecs::component::Component for #ident {
                #[inline(always)]
                fn id() -> ::uju::ecs::component::Id {
                    unsafe { *REGISTRATION.id.get() }
                }
            }
        };
    })
}

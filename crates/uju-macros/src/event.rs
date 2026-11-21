use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, Result};

pub fn expand(ast: DeriveInput) -> Result<TokenStream> {
    if !ast.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &ast.generics,
            "generic types cannot derive Event",
        ));
    }

    let ident = &ast.ident;

    Ok(quote! {
        const _: () = {
            #[::uju::linkme::distributed_slice(::uju::ecs::event::EVENTS)]
            #[linkme(crate = ::uju::linkme)]
            static REGISTRATION: ::uju::ecs::event::Registration =
                ::uju::ecs::event::Registration {
                    name: ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident)),
                    id: ::core::cell::UnsafeCell::new(u16::MAX),
                    new_queue: ::uju::ecs::event::new_queue::<#ident>,
                };

            impl ::uju::ecs::event::Event for #ident {
                #[inline(always)]
                fn id() -> ::uju::ecs::event::Id {
                    unsafe { *REGISTRATION.id.get() }
                }
            }
        };
    })
}

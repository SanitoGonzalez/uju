use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, Result};

pub fn expand(ast: DeriveInput) -> Result<TokenStream> {
    if !ast.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &ast.generics,
            "generic types cannot derive Unique",
        ));
    }

    let ident = &ast.ident;

    Ok(quote! {
        const _: () = {
            #[::uju::linkme::distributed_slice(::uju::ecs::unique::UNIQUES)]
            #[linkme(crate = ::uju::linkme)]
            static REGISTRATION: ::uju::ecs::unique::Registration =
                ::uju::ecs::unique::Registration {
                    name: ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident)),
                    id: ::core::cell::UnsafeCell::new(u16::MAX),
                };

            impl ::uju::ecs::unique::Unique for #ident {
                #[inline(always)]
                fn id() -> ::uju::ecs::unique::Id {
                    unsafe { *REGISTRATION.id.get() }
                }
            }
        };
    })
}

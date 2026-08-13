use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Attribute, DeriveInput, Fields, GenericParam, Result, TypeParam, parse_macro_input, parse_quote,
};

pub fn expand(mut ast: DeriveInput) -> Result<TokenStream> {
    ast.generics
        .make_where_clause()
        .predicates
        .push(parse_quote!(Self: 'static));

    let ident = &ast.ident;
    let (impl_generics, type_generics, where_clause) = ast.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics uju_ecs::component::Component for #ident #type_generics #where_clause {}
    })
}

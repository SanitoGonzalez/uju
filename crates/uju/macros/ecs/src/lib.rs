mod component;
mod unique;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    component::expand(ast).unwrap_or_else(syn::Error::into_compile_error).into()
}

#[proc_macro_derive(Unique, attributes(unique))]
pub fn derive_unique(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    unique::expand(ast).unwrap_or_else(syn::Error::into_compile_error).into()
}

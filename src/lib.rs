#![doc = include_str!("../README.md")]
#![feature(proc_macro_span)]

mod error;
mod exports;
mod files;
mod imports;
mod module;
mod result;
mod source;

use std::{fs::File, io::Read, path::PathBuf};

use files::AbsoluteRustFilePathBuf;
use proc_macro::Span;
use quote::ToTokens;
use source::Sourcecode;
use syn::{
    parse::{Parse, ParseStream},
    token::Brace,
    Token,
};

struct MacroInput {
    wgsl_path: String,
    requested_invocation: Option<String>,
}

impl Parse for MacroInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let a = input.parse::<syn::LitStr>()?;
        let b = if input.is_empty() {
            None
        } else {
            input.parse::<Token![,]>()?;
            Some(input.parse::<syn::LitStr>()?.value())
        };

        Ok(Self {
            wgsl_path: a.value(),
            requested_invocation: b,
        })
    }
}

#[proc_macro_attribute]
pub fn include_wgsl_oil(
    path: proc_macro::TokenStream,
    module: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // Parse module definitions and error if it contains anything
    let mut module = syn::parse_macro_input!(module as syn::ItemMod);
    if let Some(content) = &mut module.content {
        if !content.1.is_empty() {
            let item = syn::parse_quote_spanned! {content.0.span=>
                compile_error!(
                    "`include_wgsl_oil` expects an empty module into which to inject the shader objects, \
                    but found a module body - try removing everything within the curly braces `{ ... }`.");
            };

            module.content = Some((Brace::default(), vec![item]));
        }
    } else {
        module.content = Some((Brace::default(), vec![]));
    }
    module.semi = None;

    let input = syn::parse_macro_input!(path as MacroInput);
    let mut requested_path = input.wgsl_path;

    let root = std::env::var("CARGO_MANIFEST_DIR").expect("proc macros should be run using cargo");

    if !requested_path.starts_with('/') {
        requested_path = format!("{root}/{}", requested_path);
    }

    let invocation_path = match input.requested_invocation {
        Some(requested_invocation) => {
            let invocation_path = if requested_invocation.starts_with('/') {
                requested_invocation
            } else {
                format!("{root}/{}", requested_invocation)
            };

            AbsoluteRustFilePathBuf::new(PathBuf::from(invocation_path))
        }
        None => AbsoluteRustFilePathBuf::new(Span::call_site().source_file().path()),
    };

    let sourcecode = Sourcecode::new(invocation_path, requested_path);
    let mut result = sourcecode.complete();

    result.validate();

    // Inject items
    module
        .content
        .as_mut()
        .expect("set to some at start")
        .1
        .append(&mut result.items());

    module.to_token_stream().into()
}

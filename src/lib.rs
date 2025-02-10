#![doc = include_str!("../README.md")]
#![feature(proc_macro_span, if_let_guard, let_chains)]

mod error;
mod exports;
mod files;
mod imports;
mod module;
mod result;
mod source;

use std::{collections::HashMap, env, fs, path::PathBuf};

use files::AbsoluteRustFilePathBuf;
use naga_oil::compose::ShaderDefValue;
use proc_macro::Span;
use quote::ToTokens;
use source::Sourcecode;
use syn::{
    bracketed, parenthesized,
    parse::{Parse, ParseStream},
    spanned::Spanned,
    token::Brace,
    Ident, Token,
};

struct Kv<T, K> {
    key: T,
    value: K,
}

impl<T: Parse, K: Parse> Parse for Kv<T, K> {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key = input.parse::<T>()?;
        input.parse::<Token![=]>()?;
        let value = input.parse::<K>()?;

        Ok(Self { key, value })
    }
}

#[derive(Clone)]
struct TypedValue {
    ty: syn::Ident,
    value: syn::Lit,
}

impl Parse for TypedValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty = input.parse::<syn::Ident>()?;

        if ty != "Bool" && ty != "Int" && ty != "UInt" {
            return Err(syn::Error::new(
                ty.span(),
                "expected one of `Bool`, `Int`, `UInt`",
            ));
        }

        let v;
        parenthesized!(v in input);

        Ok(Self {
            ty,
            value: v.parse()?,
        })
    }
}

impl From<TypedValue> for ShaderDefValue {
    fn from(value: TypedValue) -> Self {
        match value.ty.to_string().as_str() {
            "Bool" if let syn::Lit::Bool(b) = value.value => ShaderDefValue::Bool(b.value),
            "Bool" => panic!("Expected a boolean literal for Bool() constant"),
            "Int"
                if let syn::Lit::Int(ref i) = value.value
                    && let Ok(num) = i.base10_parse::<i32>() =>
            {
                ShaderDefValue::Int(num)
            }
            "Int" => panic!("Expected i32 literal for Int() constant"),
            "UInt"
                if let syn::Lit::Int(ref i) = value.value
                    && let Ok(num) = i.base10_parse::<u32>() =>
            {
                ShaderDefValue::UInt(num)
            }
            "UInt" => panic!("Expected u32 literal for UInt() constant"),
            _ => panic!(),
        }
    }
}

#[derive(Default)]
struct Constants {
    inner: Vec<(String, TypedValue)>,
}

impl Parse for Constants {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let p = input.parse_terminated(Kv::<syn::Ident, TypedValue>::parse, Token![,])?;

        Ok(Self {
            inner: p
                .into_iter()
                .map(|kv| (kv.key.to_string(), kv.value))
                .collect(),
        })
    }
}

struct MacroInput {
    wgsl_path: String,
    includes: HashMap<String, (Vec<String>, PathBuf, String)>,
    constants: Constants,
}

impl Parse for MacroInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut wgsl_path = String::new();
        let mut includes = HashMap::new();
        let mut constants = Constants::default();

        while !input.is_empty() {
            let ident = input.parse::<Ident>()?;
            match ident.to_string().as_str() {
                "path" => {
                    input.parse::<Token![=]>()?;
                    wgsl_path = input.parse::<syn::LitStr>()?.value();
                }
                "includes" => {
                    input.parse::<Token![=]>()?;
                    let inner;
                    bracketed!(inner in input);
                    let p = inner.parse_terminated(<syn::LitStr as Parse>::parse, Token![,])?;
                    let mut include_paths: Vec<_> = p
                        .iter()
                        .map(|p| {
                            let path = p.value();
                            if path.starts_with("/") {
                                PathBuf::from(path)
                            } else {
                                PathBuf::from(format!(
                                    "{}/{}",
                                    env::var("CARGO_MANIFEST_DIR").unwrap(),
                                    path
                                ))
                            }
                        })
                        .collect();

                    let mut new_includes = HashMap::new();

                    while let Some(buf) = include_paths.pop() {
                        if buf.is_dir() {
                            let Ok(entries) = fs::read_dir(&buf) else {
                                return Err(syn::Error::new(
                                    p.span(),
                                    format!("Failed to read directory {buf:?}"),
                                ));
                            };

                            include_paths.extend(entries.map(|m| m.unwrap().path()));
                        } else {
                            match fs::read_to_string(&buf) {
                                Err(e) => {
                                    return Err(syn::Error::new(
                                        p.span(),
                                        format!("Failed to read file {buf:?} to string:{e:?}"),
                                    ))
                                }
                                Ok(source) => {
                                    let (name, reqs, _) =
                                        naga_oil::compose::get_preprocessor_data(&source);

                                    let name = name.unwrap_or(format!(
                                        r#""{}""#,
                                        buf.to_string_lossy().replace("\\", "/")
                                    ));

                                    let name = name
                                        .strip_prefix(r#""./"#)
                                        .map(|name| format!(r#""{name}"#))
                                        .unwrap_or(name);

                                    let reqs =
                                        reqs.into_iter().map(|req| req.import).collect::<Vec<_>>();

                                    if new_includes.contains_key(&name)
                                        || includes.contains_key(&name)
                                    {
                                        eprintln!("warning: duplicate definition for `{name}`");
                                    }

                                    eprintln!("Including {name} from {buf:?}");

                                    new_includes
                                        .insert(name, (reqs, buf, source.replace("@export", "")));
                                }
                            }
                        }
                    }

                    includes.extend(new_includes);
                }
                "constants" => {
                    input.parse::<Token![=]>()?;
                    constants = input.parse::<Constants>()?;
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "expected one of `path`, `includes`, `constants`",
                    ));
                }
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            wgsl_path,
            includes,
            constants,
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

    let mut input = syn::parse_macro_input!(path as MacroInput);

    let root = std::env::var("CARGO_MANIFEST_DIR").expect("proc macros should be run using cargo");

    if !input.wgsl_path.starts_with('/') {
        input.wgsl_path = format!("{root}/{}", input.wgsl_path);
    }

    let path = Span::call_site().source_file().path();
    let rel = path.to_str().unwrap();
    let abs = PathBuf::from(format!("{root}/{rel}"));

    let sourcecode = Sourcecode::new(AbsoluteRustFilePathBuf::new(abs), input);
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

//! Procedural macro implementation for `#[tool_router]` (see `lib.rs`).
//!
//! When `server_handler` is set, we emit a second `impl ServerHandler` item decorated with
//! `#[::rmcp::tool_handler]` so `tool_handler` expands in a later proc-macro pass—keeping all
//! tool dispatch and `get_info` logic in `tool_handler.rs` without duplicating it here.

use std::collections::HashMap;

use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{spanned::Spanned, Attribute, Ident, ImplItem, ImplItemFn, ItemImpl, Visibility};

use crate::tool::ToolAttribute;

#[derive(FromMeta)]
#[darling(default)]
pub struct ToolRouterAttribute {
    pub router: Ident,
    pub vis: Option<Visibility>,
    /// When set, also emit `#[::rmcp::tool_handler]` on `impl ServerHandler for Self` so callers
    /// can skip a separate `#[tool_handler]` block (expanded in a later macro pass).
    pub server_handler: bool,
}

impl Default for ToolRouterAttribute {
    fn default() -> Self {
        Self {
            router: format_ident!("tool_router"),
            vis: None,
            server_handler: false,
        }
    }
}

fn tool_attribute(fn_item: &ImplItemFn) -> Option<&Attribute> {
    fn_item.attrs.iter().find(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "tool")
    })
}

fn effective_tool_name(tool_attr: &Attribute, handler: &Ident) -> syn::Result<String> {
    let attribute = match &tool_attr.meta {
        syn::Meta::Path(_) => ToolAttribute::default(),
        syn::Meta::List(list) => {
            let attr_args = NestedMeta::parse_meta_list(list.tokens.clone())?;
            ToolAttribute::from_list(&attr_args)?
        }
        syn::Meta::NameValue(_) => {
            return Err(syn::Error::new_spanned(
                tool_attr,
                "tool attributes must use `#[tool]` or `#[tool(...)]` syntax",
            ));
        }
    };

    Ok(attribute.name.unwrap_or_else(|| handler.to_string()))
}

fn reject_duplicate_effective_tool_names(
    tool_attr_fns: &[(&Ident, &Attribute)],
) -> syn::Result<()> {
    let mut declared_names = HashMap::with_capacity(tool_attr_fns.len());

    for (handler, tool_attr) in tool_attr_fns {
        let effective_name = effective_tool_name(tool_attr, handler)?;
        if let Some((first_handler, first_span)) = declared_names.get(&effective_name) {
            let mut error = syn::Error::new(
                tool_attr.span(),
                format!(
                    "duplicate effective tool name `{effective_name}`; first declared by `{first_handler}`"
                ),
            );
            error.combine(syn::Error::new(
                *first_span,
                format!("first declaration of `{effective_name}` is here"),
            ));
            return Err(error);
        }
        declared_names.insert(effective_name, (handler.to_string(), handler.span()));
    }

    Ok(())
}

pub fn tool_router(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attr_args = NestedMeta::parse_meta_list(attr)?;
    let ToolRouterAttribute {
        router,
        vis,
        server_handler,
    } = ToolRouterAttribute::from_list(&attr_args)?;
    let mut item_impl = syn::parse2::<ItemImpl>(input)?;
    // find all function marked with `#[rmcp::tool]`
    let tool_attr_fns: Vec<_> = item_impl
        .items
        .iter()
        .filter_map(|item| {
            if let syn::ImplItem::Fn(fn_item) = item {
                tool_attribute(fn_item).map(|attr| (&fn_item.sig.ident, attr))
            } else {
                None
            }
        })
        .collect();
    reject_duplicate_effective_tool_names(&tool_attr_fns)?;
    let mut routers = Vec::with_capacity(tool_attr_fns.len());
    for (handler, _) in tool_attr_fns {
        let tool_attr_fn_ident = format_ident!("{handler}_tool_attr");
        routers.push(quote! {
            .with_route((Self::#tool_attr_fn_ident(), Self::#handler))
        })
    }
    let router_fn = syn::parse2::<ImplItem>(quote! {
        #vis fn #router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
            rmcp::handler::server::router::tool::ToolRouter::<Self>::new()
                #(#routers)*
        }
    })?;
    item_impl.items.push(router_fn);

    if !server_handler {
        return Ok(item_impl.into_token_stream());
    }

    if item_impl.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            item_impl,
            "`server_handler` is only supported on inherent impl blocks (e.g. `impl MyType { ... }`)",
        ));
    }

    let self_ty = &item_impl.self_ty;
    let (impl_generics, ty_generics, where_clause) = item_impl.generics.split_for_impl();

    Ok(quote! {
        #item_impl

        #[::rmcp::tool_handler(router = Self::#router())]
        impl #impl_generics ::rmcp::ServerHandler for #self_ty #ty_generics #where_clause {}
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn tool_router_attribute_parses_router_visibility_and_defaults_server_handler_off()
    -> syn::Result<()> {
        let attr = quote! {
            router = test_router,
            vis = "pub(crate)"
        };
        let attr_args = NestedMeta::parse_meta_list(attr)?;
        let ToolRouterAttribute {
            router,
            vis,
            server_handler,
        } = ToolRouterAttribute::from_list(&attr_args)?;
        assert_eq!(router.to_string(), "test_router");
        assert!(vis.is_some(), "vis = \"pub(crate)\" should parse");
        assert!(
            !server_handler,
            "server_handler should default to false when omitted"
        );
        Ok(())
    }

    #[test]
    fn tool_router_attribute_parses_server_handler_flag() -> syn::Result<()> {
        let attr = quote! {
            router = custom_router,
            server_handler
        };
        let attr_args = NestedMeta::parse_meta_list(attr)?;
        let ToolRouterAttribute {
            router,
            server_handler,
            ..
        } = ToolRouterAttribute::from_list(&attr_args)?;
        assert_eq!(router.to_string(), "custom_router");
        assert!(server_handler);
        Ok(())
    }

    #[test]
    fn tool_router_rejects_duplicate_explicit_effective_names() {
        let error = tool_router(
            quote! {},
            quote! {
                impl Handler {
                    #[tool(name = "shared")]
                    fn first(&self) {}

                    #[tool(name = "shared")]
                    fn second(&self) {}
                }
            },
        )
        .expect_err("duplicate explicit names must be rejected during macro expansion");

        assert_eq!(
            error.to_string(),
            "duplicate effective tool name `shared`; first declared by `first`"
        );
    }

    #[test]
    fn tool_router_rejects_explicit_name_that_collides_with_implicit_name() {
        let error = tool_router(
            quote! {},
            quote! {
                impl Handler {
                    #[tool(name = "shared")]
                    fn explicit_name(&self) {}

                    #[tool]
                    fn shared(&self) {}
                }
            },
        )
        .expect_err("explicit and implicit duplicate names must be rejected during macro expansion");

        assert_eq!(
            error.to_string(),
            "duplicate effective tool name `shared`; first declared by `explicit_name`"
        );
    }

    #[test]
    fn tool_router_accepts_unique_explicit_and_implicit_names() {
        tool_router(
            quote! {},
            quote! {
                impl Handler {
                    #[tool(name = "renamed")]
                    fn explicit_name(&self) {}

                    #[tool]
                    fn implicit_name(&self) {}
                }
            },
        )
        .expect("unique effective names must remain valid");
    }
}

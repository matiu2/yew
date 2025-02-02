use super::HtmlProp;
use super::HtmlPropSuffix;
use crate::Peek;
use boolinator::Boolinator;
use proc_macro2::Span;
use quote::{quote, quote_spanned, ToTokens};
use syn::buffer::Cursor;
use syn::parse;
use syn::parse::{Parse, ParseStream, Result as ParseResult};
use syn::spanned::Spanned;
use syn::{Ident, Token, Type};

pub struct HtmlComponent(HtmlComponentInner);

impl Peek<()> for HtmlComponent {
    fn peek(cursor: Cursor) -> Option<()> {
        let (punct, cursor) = cursor.punct()?;
        (punct.as_char() == '<').as_option()?;

        HtmlComponent::peek_type(cursor)
    }
}

impl Parse for HtmlComponent {
    fn parse(input: ParseStream) -> ParseResult<Self> {
        let lt = input.parse::<Token![<]>()?;
        let HtmlPropSuffix { stream, div, gt } = input.parse()?;
        if div.is_none() {
            return Err(syn::Error::new_spanned(
                HtmlComponentTag { lt, gt },
                "expected component tag be of form `< .. />`",
            ));
        }

        match parse(stream) {
            Ok(comp) => Ok(HtmlComponent(comp)),
            Err(err) => {
                if err.to_string().starts_with("unexpected end of input") {
                    Err(syn::Error::new_spanned(div, err.to_string()))
                } else {
                    Err(err)
                }
            }
        }
    }
}

impl ToTokens for HtmlComponent {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let HtmlComponentInner { ty, props } = &self.0;
        let vcomp_scope = Ident::new("__yew_vcomp_scope", Span::call_site());

        let validate_props = if let Some(Props::List(ListProps(vec_props))) = props {
            let prop_ref = Ident::new("__yew_prop_ref", Span::call_site());
            let check_props = vec_props.iter().map(|HtmlProp { label, .. }| {
                quote! { #prop_ref.#label; }
            });

            // This is a hack to avoid allocating memory but still have a reference to a props
            // struct so that attributes can be checked against it

            #[cfg(has_maybe_uninit)]
            let unallocated_prop_ref = quote! {
                let #prop_ref: <#ty as ::yew::html::Component>::Properties = unsafe { ::std::mem::MaybeUninit::uninit().assume_init() };
            };

            #[cfg(not(has_maybe_uninit))]
            let unallocated_prop_ref = quote! {
                let #prop_ref: <#ty as ::yew::html::Component>::Properties = unsafe { ::std::mem::uninitialized() };
            };

            quote! {
                #unallocated_prop_ref
                #(#check_props)*
            }
        } else {
            quote! {}
        };

        let init_props = if let Some(props) = props {
            match props {
                Props::List(ListProps(vec_props)) => {
                    let set_props = vec_props.iter().map(|HtmlProp { label, value }| {
                        quote_spanned! { value.span()=>
                            .#label(<::yew::virtual_dom::vcomp::VComp<_> as ::yew::virtual_dom::vcomp::Transformer<_, _, _>>::transform(#vcomp_scope.clone(), #value))
                        }
                    });

                    quote! {
                        <<#ty as ::yew::html::Component>::Properties as ::yew::html::Properties>::builder()
                            #(#set_props)*
                            .build()
                    }
                }
                Props::With(WithProps(props)) => quote! { #props },
            }
        } else {
            quote! {
                <<#ty as ::yew::html::Component>::Properties as ::yew::html::Properties>::builder().build()
            }
        };

        let validate_comp = quote_spanned! { ty.span()=>
            trait __yew_validate_comp {
                type C: ::yew::html::Component;
            }
            impl __yew_validate_comp for () {
                type C = #ty;
            }
        };

        tokens.extend(quote! {{
            // Validation nevers executes at runtime
            if false {
                #validate_comp
                #validate_props
            }

            let #vcomp_scope: ::yew::virtual_dom::vcomp::ScopeHolder<_> = ::std::default::Default::default();
            ::yew::virtual_dom::VNode::VComp(
                ::yew::virtual_dom::VComp::new::<#ty>(#init_props, #vcomp_scope)
            )
        }});
    }
}

impl HtmlComponent {
    fn double_colon(mut cursor: Cursor) -> Option<Cursor> {
        for _ in 0..2 {
            let (punct, c) = cursor.punct()?;
            (punct.as_char() == ':').as_option()?;
            cursor = c;
        }

        Some(cursor)
    }

    fn peek_type(mut cursor: Cursor) -> Option<()> {
        let mut type_str: String = "".to_owned();
        let mut colons_optional = true;

        loop {
            let mut found_colons = false;
            let mut post_colons_cursor = cursor;
            if let Some(c) = Self::double_colon(post_colons_cursor) {
                found_colons = true;
                post_colons_cursor = c;
            } else if !colons_optional {
                break;
            }

            if let Some((ident, c)) = post_colons_cursor.ident() {
                cursor = c;
                if found_colons {
                    type_str += "::";
                }
                type_str += &ident.to_string();
            } else {
                break;
            }

            // only first `::` is optional
            colons_optional = false;
        }

        (!type_str.is_empty()).as_option()?;
        (type_str.to_lowercase() != type_str).as_option()
    }
}

pub struct HtmlComponentInner {
    ty: Type,
    props: Option<Props>,
}

impl Parse for HtmlComponentInner {
    fn parse(input: ParseStream) -> ParseResult<Self> {
        let ty = input.parse()?;
        // backwards compat
        let _ = input.parse::<Token![:]>();

        let props = if let Some(prop_type) = Props::peek(input.cursor()) {
            match prop_type {
                PropType::List => input.parse().map(Props::List).map(Some)?,
                PropType::With => input.parse().map(Props::With).map(Some)?,
            }
        } else {
            None
        };

        Ok(HtmlComponentInner { ty, props })
    }
}

struct HtmlComponentTag {
    lt: Token![<],
    gt: Token![>],
}

impl ToTokens for HtmlComponentTag {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let HtmlComponentTag { lt, gt } = self;
        tokens.extend(quote! {#lt#gt});
    }
}

enum PropType {
    List,
    With,
}

enum Props {
    List(ListProps),
    With(WithProps),
}

impl Peek<PropType> for Props {
    fn peek(cursor: Cursor) -> Option<PropType> {
        let (ident, _) = cursor.ident()?;
        let prop_type = if ident.to_string() == "with" {
            PropType::With
        } else {
            PropType::List
        };

        Some(prop_type)
    }
}

struct ListProps(Vec<HtmlProp>);
impl Parse for ListProps {
    fn parse(input: ParseStream) -> ParseResult<Self> {
        let mut props: Vec<HtmlProp> = Vec::new();
        while HtmlProp::peek(input.cursor()).is_some() {
            props.push(input.parse::<HtmlProp>()?);
        }

        for prop in &props {
            if prop.label.to_string() == "type" {
                return Err(syn::Error::new_spanned(&prop.label, "expected identifier"));
            }
            if !prop.label.extended.is_empty() {
                return Err(syn::Error::new_spanned(&prop.label, "expected identifier"));
            }
        }

        // alphabetize
        props.sort_by(|a, b| {
            a.label
                .to_string()
                .partial_cmp(&b.label.to_string())
                .unwrap()
        });

        Ok(ListProps(props))
    }
}

struct WithProps(Ident);
impl Parse for WithProps {
    fn parse(input: ParseStream) -> ParseResult<Self> {
        let with = input.parse::<Ident>()?;
        if with.to_string() != "with" {
            return Err(input.error("expected to find `with` token"));
        }
        let props = input.parse::<Ident>()?;
        let _ = input.parse::<Token![,]>();
        Ok(WithProps(props))
    }
}

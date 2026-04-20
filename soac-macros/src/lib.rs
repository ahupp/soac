use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    braced, parse_macro_input, parse_quote, Attribute, Data, DeriveInput, Expr, Fields, ItemEnum,
    Pat, Path, Token, Type,
};

fn enum_variants(input: &DeriveInput) -> syn::Result<Vec<&syn::Variant>> {
    let Data::Enum(data_enum) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "DelegateMatchDefault can only be derived for enums",
        ));
    };

    data_enum
        .variants
        .iter()
        .map(|variant| match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => Ok(variant),
            _ => Err(syn::Error::new_spanned(
                variant,
                "DelegateMatchDefault requires one-field tuple variants",
            )),
        })
        .collect()
}

fn item_enum_variants(input: &ItemEnum) -> syn::Result<Vec<&syn::Variant>> {
    input
        .variants
        .iter()
        .map(|variant| match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => Ok(variant),
            _ => Err(syn::Error::new_spanned(
                variant,
                "enum_broadcast requires one-field tuple variants",
            )),
        })
        .collect()
}

enum EnumBroadcastTarget {
    HasMeta,
    WithMeta,
    ChildVisitable,
    Mappable,
    PrettyPrint,
    Debug,
}

impl EnumBroadcastTarget {
    fn parse(path: Path) -> syn::Result<Self> {
        let Some(segment) = path.segments.last() else {
            return Err(syn::Error::new_spanned(path, "expected trait name"));
        };

        match segment.ident.to_string().as_str() {
            "HasMeta" => Ok(Self::HasMeta),
            "WithMeta" => Ok(Self::WithMeta),
            "ChildVisitable" => Ok(Self::ChildVisitable),
            "Mappable" => Ok(Self::Mappable),
            "PrettyPrint" => Ok(Self::PrettyPrint),
            "Debug" => Ok(Self::Debug),
            _ => Err(syn::Error::new_spanned(
                segment,
                "unsupported enum_broadcast target; supported targets are HasMeta, WithMeta, ChildVisitable, Mappable, PrettyPrint, and Debug",
            )),
        }
    }

    fn impl_tokens(
        &self,
        enum_name: &syn::Ident,
        generics: &syn::Generics,
        variants: &[&syn::Variant],
    ) -> proc_macro2::TokenStream {
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
        let meta_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => node.meta(),
            }
        });
        let with_meta_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => node.with_meta(meta.clone()).into(),
            }
        });
        let map_children_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => map.map_instr(Self::#variant_name(node)),
            }
        });
        let try_map_children_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => map.try_map_instr(Self::#variant_name(node)),
            }
        });
        let map_same_children_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => node.map_same_children(map).into(),
            }
        });
        let try_map_same_children_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => Ok(node.try_map_same_children(map)?.into()),
            }
        });
        let walk_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => node.visit_children(&mut *visitor),
            }
        });
        let walk_mut_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => node.visit_children_mut(&mut *visitor),
            }
        });
        let debug_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            match variant_name.to_string().as_str() {
                "Literal" => quote! {
                    Self::#variant_name(node) => node.literal.fmt(f),
                },
                "Await" => quote! {
                    Self::#variant_name(node) => write!(f, "await {:?}", node.value),
                },
                "Yield" => quote! {
                    Self::#variant_name(node) => write!(f, "yield {:?}", node.value),
                },
                "YieldFrom" => quote! {
                    Self::#variant_name(node) => write!(f, "yield from {:?}", node.value),
                },
                _ => quote! {
                    Self::#variant_name(node) => node.fmt(f),
                },
            }
        });
        let pretty_print_arms = variants.iter().map(|variant| {
            let variant_name = &variant.ident;
            quote! {
                Self::#variant_name(node) => crate::block_py::PrettyPrint::fmt_pretty(node, printer),
            }
        });

        match self {
            Self::HasMeta => quote! {
                impl #impl_generics HasMeta for #enum_name #ty_generics #where_clause {
                    fn meta(&self) -> Meta {
                        match self {
                            #( #meta_arms )*
                        }
                    }
                }
            },
            Self::WithMeta => quote! {
                impl #impl_generics WithMeta for #enum_name #ty_generics #where_clause {
                    fn with_meta(self, meta: Meta) -> Self {
                        match self {
                            #( #with_meta_arms )*
                        }
                    }
                }
            },
            Self::ChildVisitable => quote! {
                impl #impl_generics ChildVisitable<Self> for #enum_name #ty_generics #where_clause {
                    fn visit_children<V>(&self, visitor: &mut V)
                    where
                        V: crate::block_py::Visit<Self> + ?Sized,
                    {
                        match self {
                            #( #walk_arms )*
                        }
                    }

                    fn visit_children_mut<V>(&mut self, visitor: &mut V)
                    where
                        V: crate::block_py::VisitMut<Self> + ?Sized,
                    {
                        match self {
                            #( #walk_mut_arms )*
                        }
                    }
                }
            },
            Self::Mappable => quote! {
                impl #impl_generics Mappable<Self> for #enum_name #ty_generics #where_clause {
                    type Mapped<T: Instr> = T;

                    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
                    where
                        T: Instr,
                        M: MapInstr<Self, T>,
                    {
                        match self {
                            #( #map_children_arms )*
                        }
                    }

                    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
                    where
                        T: Instr,
                        M: TryMapInstr<Self, T, Error>,
                    {
                        match self {
                            #( #try_map_children_arms )*
                        }
                    }

                    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<Self>
                    where
                        M: MapInstr<Self, Self>,
                    {
                        match self {
                            #( #map_same_children_arms )*
                        }
                    }

                    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<Self>, Error>
                    where
                        M: TryMapInstr<Self, Self, Error>,
                    {
                        match self {
                            #( #try_map_same_children_arms )*
                        }
                    }
                }
            },
            Self::PrettyPrint => quote! {
                impl #impl_generics crate::block_py::PrettyPrint for #enum_name #ty_generics #where_clause {
                    fn fmt_pretty(
                        &self,
                        printer: &mut crate::block_py::PrettyPrinter<'_>,
                    ) -> std::fmt::Result {
                        match self {
                            #( #pretty_print_arms )*
                        }
                    }
                }
            },
            Self::Debug => quote! {
                impl #impl_generics std::fmt::Debug for #enum_name #ty_generics #where_clause {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        match self {
                            #( #debug_arms )*
                        }
                    }
                }
            },
        }
    }
}

#[proc_macro_attribute]
pub fn enum_broadcast(attr: TokenStream, item: TokenStream) -> TokenStream {
    let targets = parse_macro_input!(attr with Punctuated::<Path, Token![,]>::parse_terminated);
    let item = parse_macro_input!(item as ItemEnum);
    let enum_name = &item.ident;
    let generics = &item.generics;
    let variants = match item_enum_variants(&item) {
        Ok(variants) => variants,
        Err(error) => return error.into_compile_error().into(),
    };

    let targets = match targets
        .into_iter()
        .map(EnumBroadcastTarget::parse)
        .collect::<syn::Result<Vec<_>>>()
    {
        Ok(targets) => targets,
        Err(error) => return error.into_compile_error().into(),
    };

    let impls = targets
        .iter()
        .map(|target| target.impl_tokens(enum_name, generics, &variants));

    quote! {
        #item
        #( #impls )*
    }
    .into()
}

#[proc_macro_derive(DelegateMatchDefault)]
pub fn derive_delegate_match_default(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let enum_name = &input.ident;
    let macro_name = format_ident!("__soac_match_default_{}", enum_name);
    let variants_macro_name = format_ident!("__soac_enum_variants_{}", enum_name);
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let variants = match enum_variants(&input) {
        Ok(variants) => variants,
        Err(error) => return error.into_compile_error().into(),
    };

    let variant_names = variants
        .iter()
        .map(|variant| variant.ident.clone())
        .collect::<Vec<_>>();
    let maybe_emit_default_arms = variants.iter().map(|variant| {
        let variant_name = &variant.ident;
        quote! {
            (@maybe_push_default
                [$value:expr]
                [$($arms:tt)*]
                [$rest:ident]
                [$default_expr:expr]
                [$($excluded:ident),*]
                [#variant_name]
                [#variant_name $(, $scan_tail:ident)*]
                [$($tail:ident),*]
            ) => {
                #macro_name!(
                    @emit_match
                    [$value]
                    [$($arms)*]
                    [$rest]
                    [$default_expr]
                    [$($excluded),*]
                    [$($tail),*]
                )
            };
            (@maybe_push_default
                [$value:expr]
                [$($arms:tt)*]
                [$rest:ident]
                [$default_expr:expr]
                [$($excluded:ident),*]
                [#variant_name]
                [$scan_head:ident $(, $scan_tail:ident)*]
                [$($tail:ident),*]
            ) => {
                #macro_name!(
                    @maybe_push_default
                    [$value]
                    [$($arms)*]
                    [$rest]
                    [$default_expr]
                    [$($excluded),*]
                    [#variant_name]
                    [$($scan_tail),*]
                    [$($tail),*]
                )
            };
            (@maybe_push_default
                [$value:expr]
                [$($arms:tt)*]
                [$rest:ident]
                [$default_expr:expr]
                [$($excluded:ident),*]
                [#variant_name]
                []
                [$($tail:ident),*]
            ) => {
                #macro_name!(
                    @emit_match
                    [$value]
                    [$($arms)* #enum_name::#variant_name($rest) => $default_expr,]
                    [$rest]
                    [$default_expr]
                    [$($excluded),*]
                    [$($tail),*]
                )
            };
        }
    });

    quote! {
        impl #impl_generics #enum_name #ty_generics #where_clause {
            #[doc(hidden)]
            pub const __SOAC_DERIVED_DELEGATE_MATCH_DEFAULT: () = ();
        }

        #[doc(hidden)]
        #[allow(unused_macros)]
        macro_rules! #variants_macro_name {
            ($callback:ident $(, $args:tt)*) => {
                $callback!($($args)* [ #( #variant_names ),* ])
            };
        }

        #[doc(hidden)]
        #[allow(unused_imports)]
        pub(crate) use #variants_macro_name;

        #[doc(hidden)]
        #[allow(unused_macros)]
        macro_rules! #macro_name {
            ($value:expr, [$($excluded:ident),*], { $($body:tt)* }) => {
                #macro_name!(@collect [$value] [$($excluded),*] [] $($body)*)
            };
            (@collect [$value:expr] [$($excluded:ident),*] [$($special_arms:tt)*] match_rest($rest:ident) => $default_expr:expr $(,)?) => {{
                #macro_name!(
                    @emit_match
                    [$value]
                    [$($special_arms)*]
                    [$rest]
                    [$default_expr]
                    [$($excluded),*]
                    [ #( #variant_names ),* ]
                )
            }};
            (@collect [$value:expr] [$($excluded:ident),*] [$($special_arms:tt)*] $special_pattern:pat $(if $guard:expr)? => $special_expr:expr, $($rest_body:tt)*) => {
                #macro_name!(
                    @collect
                    [$value]
                    [$($excluded),*]
                    [$($special_arms)* $special_pattern $(if $guard)? => $special_expr,]
                    $($rest_body)*
                )
            };
            (@emit_match [$value:expr] [$($arms:tt)*] [$rest:ident] [$default_expr:expr] [$($excluded:ident),*] []) => {{
                #[allow(unreachable_patterns)]
                match $value {
                    $($arms)*
                }
            }};
            (@emit_match [$value:expr] [$($arms:tt)*] [$rest:ident] [$default_expr:expr] [$($excluded:ident),*] [$head:ident $(, $tail:ident)*]) => {
                #macro_name!(
                    @maybe_push_default
                    [$value]
                    [$($arms)*]
                    [$rest]
                    [$default_expr]
                    [$($excluded),*]
                    [$head]
                    [$($excluded),*]
                    [$($tail),*]
                )
            };
            #( #maybe_emit_default_arms )*
        }

        #[doc(hidden)]
        #[allow(unused_imports)]
        pub(crate) use #macro_name;
    }
    .into()
}

fn variant_ident_from_pat(pat: &Pat) -> Option<syn::Ident> {
    let Pat::TupleStruct(tuple_struct) = pat else {
        return None;
    };

    tuple_struct
        .path
        .segments
        .last()
        .map(|segment| segment.ident.clone())
}

fn enum_ident_from_type(self_ty: &Type) -> syn::Result<syn::Ident> {
    let Type::Path(type_path) = self_ty else {
        return Err(syn::Error::new_spanned(
            self_ty,
            "match_default! requires a path-qualified enum type",
        ));
    };

    if type_path.path.segments.len() < 2 {
        return Err(syn::Error::new_spanned(
            self_ty,
            "match_default! requires a path-qualified enum type like crate::module::Enum",
        ));
    }

    let Some(segment) = type_path.path.segments.last() else {
        return Err(syn::Error::new_spanned(
            self_ty,
            "match_default! could not resolve the enum type name",
        ));
    };

    Ok(segment.ident.clone())
}

enum MatchDefaultArm {
    Special {
        attrs: Vec<Attribute>,
        pat: Pat,
        guard: Option<Expr>,
        body: Expr,
    },
    Rest {
        ident: syn::Ident,
        body: Expr,
    },
}

impl Parse for MatchDefaultArm {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let pat: Pat = input.call(Pat::parse_multi_with_leading_vert)?;
        let guard = if input.peek(Token![if]) {
            input.parse::<Token![if]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        input.parse::<Token![=>]>()?;
        let body: Expr = input.parse()?;

        if let Pat::Ident(pat_ident) = &pat {
            if pat_ident.by_ref.is_none()
                && pat_ident.mutability.is_none()
                && pat_ident.subpat.is_none()
            {
                if !attrs.is_empty() {
                    return Err(syn::Error::new_spanned(
                        &attrs[0],
                        "rest => ... does not support attributes",
                    ));
                }
                if guard.is_some() {
                    return Err(syn::Error::new_spanned(
                        &pat,
                        "rest => ... does not support a guard",
                    ));
                }
                return Ok(Self::Rest {
                    ident: pat_ident.ident.clone(),
                    body,
                });
            }
        }

        Ok(Self::Special {
            attrs,
            pat,
            guard,
            body,
        })
    }
}

struct MatchDefaultInput {
    scrutinee: Expr,
    enum_ty: Type,
    arms: Punctuated<MatchDefaultArm, Token![,]>,
}

impl Parse for MatchDefaultInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let scrutinee: Expr = input.parse()?;
        input.parse::<Token![:]>()?;
        let enum_ty: Type = input.parse()?;
        let content;
        braced!(content in input);
        let arms = content.parse_terminated(MatchDefaultArm::parse, Token![,])?;
        Ok(Self {
            scrutinee,
            enum_ty,
            arms,
        })
    }
}

fn expand_match_default(input: MatchDefaultInput) -> syn::Result<Expr> {
    let enum_name = enum_ident_from_type(&input.enum_ty)?;
    let mut special_arms = Vec::new();
    let mut excluded_variants = Vec::new();
    let mut rest_arm = None;

    for (index, arm) in input.arms.iter().enumerate() {
        match arm {
            MatchDefaultArm::Rest { ident, body } => {
                if index + 1 != input.arms.len() {
                    return Err(syn::Error::new_spanned(
                        ident,
                        "rest => ... must be the final match arm",
                    ));
                }
                if rest_arm.is_some() {
                    return Err(syn::Error::new_spanned(
                        ident,
                        "rest => ... may only appear once",
                    ));
                }
                rest_arm = Some((ident.clone(), body.clone()));
            }
            MatchDefaultArm::Special {
                attrs,
                pat,
                guard,
                body,
            } => {
                special_arms.push((attrs.clone(), pat.clone(), guard.clone(), body.clone()));
                if let Some(variant_ident) = variant_ident_from_pat(pat) {
                    excluded_variants.push(variant_ident);
                }
            }
        }
    }

    let Some((rest_ident, default_expr)) = rest_arm else {
        return Err(syn::Error::new_spanned(
            &input.enum_ty,
            "match_default! requires a final rest => ... arm",
        ));
    };

    let macro_name = format_ident!("__soac_match_default_{}", enum_name);
    let scrutinee = &input.scrutinee;
    let enum_ty = &input.enum_ty;
    let Type::Path(enum_ty_path) = enum_ty else {
        return Err(syn::Error::new_spanned(
            enum_ty,
            "match_default! requires a path-qualified enum type",
        ));
    };
    let mut macro_path = enum_ty_path.path.clone();
    let Some(last_segment) = macro_path.segments.last_mut() else {
        return Err(syn::Error::new_spanned(
            enum_ty,
            "match_default! could not resolve the enum helper path",
        ));
    };
    last_segment.ident = macro_name.clone();
    last_segment.arguments = syn::PathArguments::None;
    let special_arms = special_arms.iter().map(|arm| {
        let attrs = &arm.0;
        let pat = &arm.1;
        let body = &arm.3;
        let guard = arm.2.as_ref().map(|guard| quote!(if #guard));
        quote! {
            #(#attrs)*
            #pat #guard => #body,
        }
    });

    Ok(parse_quote!({
        let _ = <#enum_ty>::__SOAC_DERIVED_DELEGATE_MATCH_DEFAULT;
        #[allow(unused_imports)]
        use #macro_path;
        #macro_name!(#scrutinee, [ #( #excluded_variants ),* ], {
            #( #special_arms )*
            match_rest(#rest_ident) => #default_expr,
        })
    }))
}

#[proc_macro]
pub fn match_default(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as MatchDefaultInput);
    match expand_match_default(input) {
        Ok(expr) => quote!(#expr).into(),
        Err(error) => error.into_compile_error().into(),
    }
}

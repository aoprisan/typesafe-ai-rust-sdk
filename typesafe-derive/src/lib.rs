//! `#[derive(Rubric)]` and `#[derive(RubricChoice)]` for
//! [`typesafe-ai-sdk`](https://docs.rs/typesafe-ai-sdk).
//!
//! Use them through the library's `derive` feature, which re-exports both as `typesafe::Rubric`
//! and `typesafe::RubricChoice` next to the traits they implement; the traits, the attributes and
//! examples are documented there. The generated code names the library as `::typesafe`.

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::quote;
use syn::ext::IdentExt;
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Data, DeriveInput, Error, Expr, Fields, Ident, Lit, LitStr, Meta, Result, Token,
    bracketed, parse_macro_input,
};

/// Implement `typesafe::rubric::Rubric` for a struct whose fields are the questions.
#[proc_macro_derive(Rubric, attributes(noul, choice, score, rubric))]
pub fn derive_rubric(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    rubric(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Implement `typesafe::rubric::RubricChoice` (and `FromStr`) for an enum whose variants are the
/// options of a choice.
#[proc_macro_derive(RubricChoice, attributes(option, rubric))]
pub fn derive_rubric_choice(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    rubric_choice(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Noul,
    Choice,
    Score,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Noul => "noul",
            Kind::Choice => "choice",
            Kind::Score => "score",
        }
    }

    /// The keys each attribute takes after its instructions.
    fn keys(self) -> &'static str {
        match self {
            Kind::Noul => "`yes` or `no`",
            Kind::Choice => "`labels`",
            Kind::Score => "`levels`",
        }
    }
}

/// One `#[noul(...)]`, `#[choice(...)]` or `#[score(...)]`, parsed.
struct Question {
    kind: Kind,
    instructions: Option<LitStr>,
    yes: Option<LitStr>,
    no: Option<LitStr>,
    labels: Vec<LitStr>,
    levels: Vec<LitStr>,
}

fn rubric(input: &DeriveInput) -> Result<Tokens> {
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "#[derive(Rubric)] needs a struct with named fields, one per question",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(Error::new_spanned(
            &input.ident,
            "#[derive(Rubric)] needs named fields: the field name is the question name",
        ));
    };

    let mut seen: HashSet<String> = HashSet::new();
    let mut questions = Vec::new();
    let mut decoders = Vec::new();
    for field in &fields.named {
        let ident = field.ident.as_ref().expect("named field");
        let ty = &field.ty;
        let mut found = Vec::new();
        for attr in &field.attrs {
            for kind in [Kind::Noul, Kind::Choice, Kind::Score] {
                if attr.path().is_ident(kind.name()) {
                    found.push((attr, kind));
                }
            }
        }
        let (attr, kind) = match found.as_slice() {
            [one] => *one,
            [] => {
                return Err(Error::new_spanned(
                    ident,
                    "every field of a Rubric is a question: add #[noul(\"...\")], #[choice(\"...\")] or #[score(\"...\", levels = [...])]",
                ));
            }
            [_, (second, _), ..] => {
                return Err(Error::new_spanned(
                    second,
                    "a field is one question: keep one of #[noul], #[choice] and #[score]",
                ));
            }
        };
        let q = parse_question(attr, kind)?;

        let name = match rename(&field.attrs)? {
            Some(lit) => lit.value(),
            None => ident.unraw().to_string(),
        };
        if !seen.insert(name.clone()) {
            return Err(Error::new_spanned(
                ident,
                format!(
                    "two fields ask the question {name:?}; rename one with #[rubric(rename = \"...\")]"
                ),
            ));
        }

        let instructions = match (&q.instructions, doc(&field.attrs)) {
            (Some(lit), _) => lit.clone(),
            (None, Some(text)) => LitStr::new(&text, ident.span()),
            (None, None) => {
                return Err(Error::new_spanned(
                    attr,
                    format!(
                        "the question needs instructions: #[{}(\"...\")] or a doc comment on the field",
                        kind.name()
                    ),
                ));
            }
        };

        let question = match kind {
            Kind::Noul => {
                let yes = q.yes.iter();
                let no = q.no.iter();
                quote! {
                    ::typesafe::Noul::new(#instructions)
                        #(.when_true(#yes))*
                        #(.when_false(#no))*
                }
            }
            Kind::Choice => {
                let labels = &q.labels;
                quote! {
                    <#ty as ::typesafe::rubric::ChoiceField>::question(#instructions.into())
                        #(.label(#labels))*
                }
            }
            Kind::Score => {
                if q.levels.is_empty() {
                    return Err(Error::new_spanned(
                        attr,
                        "a score needs its levels, lowest first: #[score(\"...\", levels = [\"Low\", \"High\"])]",
                    ));
                }
                let levels = &q.levels;
                quote!(::typesafe::Score::new(#instructions, [#(#levels),*]))
            }
        };
        questions.push(quote!(.with(#name, #question)));

        decoders.push(match kind {
            Kind::Noul => quote! {
                #ident: <#ty as ::typesafe::rubric::NoulField>::from_noul(
                    ::typesafe::rubric::__private::noul(response, #name)?,
                )
            },
            Kind::Choice => quote! {
                #ident: ::typesafe::rubric::__private::choice::<#ty>(response, #name)?
            },
            Kind::Score => quote! {
                #ident: <#ty as ::typesafe::rubric::ScoreField>::from_score(
                    ::typesafe::rubric::__private::score(response, #name)?,
                )
            },
        });
    }
    if questions.is_empty() {
        return Err(Error::new_spanned(
            &input.ident,
            "a Rubric needs at least one question",
        ));
    }

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::typesafe::rubric::Rubric for #name #ty_generics #where_clause {
            fn questions() -> ::typesafe::Questions {
                ::typesafe::Questions::new() #(#questions)*
            }

            fn from_response(
                response: &::typesafe::SystemOneResponse,
            ) -> ::typesafe::Result<Self> {
                ::core::result::Result::Ok(Self { #(#decoders,)* })
            }
        }
    })
}

/// `("instructions", key = value, ...)`, where the instructions are optional; a bare
/// `#[noul]` has neither.
fn parse_question(attr: &Attribute, kind: Kind) -> Result<Question> {
    let mut q = Question {
        kind,
        instructions: None,
        yes: None,
        no: None,
        labels: Vec::new(),
        levels: Vec::new(),
    };
    if let Meta::Path(_) = attr.meta {
        return Ok(q);
    }
    attr.parse_args_with(|input: ParseStream| {
        if input.peek(LitStr) {
            q.instructions = Some(input.parse()?);
            if input.is_empty() {
                return Ok(());
            }
            input.parse::<Token![,]>()?;
        }
        while !input.is_empty() {
            let key: Ident = input.call(Ident::parse_any)?;
            input.parse::<Token![=]>()?;
            match (q.kind, key.to_string().as_str()) {
                (Kind::Noul, "yes") => q.yes = Some(input.parse()?),
                (Kind::Noul, "no") => q.no = Some(input.parse()?),
                (Kind::Choice, "labels") => q.labels = string_list(input)?,
                (Kind::Score, "levels") => q.levels = string_list(input)?,
                _ => {
                    return Err(Error::new(
                        key.span(),
                        format!(
                            "unknown key `{key}` in #[{}]; it takes {}",
                            kind.name(),
                            kind.keys()
                        ),
                    ));
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(())
    })?;
    Ok(q)
}

/// `["a", "b", ...]`
fn string_list(input: ParseStream) -> Result<Vec<LitStr>> {
    let content;
    bracketed!(content in input);
    Ok(Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?
        .into_iter()
        .collect())
}

/// `#[rubric(rename = "...")]`, the only thing `#[rubric]` holds.
fn rename(attrs: &[Attribute]) -> Result<Option<LitStr>> {
    let mut renamed = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("rubric")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                renamed = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error("#[rubric] takes `rename = \"...\"`"))
            }
        })?;
    }
    Ok(renamed)
}

/// The doc comment, its lines joined by spaces, or `None` when there is none.
fn doc(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(e) => match &e.lit {
                    Lit::Str(s) => Some(s.value().trim().to_owned()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .filter(|line| !line.is_empty())
        .collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

fn rubric_choice(input: &DeriveInput) -> Result<Tokens> {
    let Data::Enum(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "#[derive(RubricChoice)] needs an enum: each variant is one option",
        ));
    };
    if data.variants.is_empty() {
        return Err(Error::new_spanned(
            &input.ident,
            "a choice needs at least one option",
        ));
    }
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "#[derive(RubricChoice)] does not take generics",
        ));
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut options = Vec::new();
    let mut from_label = Vec::new();
    let mut to_label = Vec::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(Error::new_spanned(
                variant,
                "an option is a bare variant; it cannot carry fields",
            ));
        }
        let ident = &variant.ident;
        let label = match rename(&variant.attrs)? {
            Some(lit) => lit.value(),
            None => snake_case(&ident.unraw().to_string()),
        };
        if !seen.insert(label.clone()) {
            return Err(Error::new_spanned(
                ident,
                format!(
                    "two variants have the label {label:?}; rename one with #[rubric(rename = \"...\")]"
                ),
            ));
        }
        let description = match option_description(&variant.attrs)? {
            Some(lit) => Some(lit.value()),
            None => doc(&variant.attrs),
        };
        let description = match description {
            Some(d) => quote!(::core::option::Option::Some(#d)),
            None => quote!(::core::option::Option::None),
        };
        options.push(quote!((#label, #description)));
        from_label.push(quote!(#label => ::core::option::Option::Some(Self::#ident)));
        to_label.push(quote!(Self::#ident => #label));
    }

    let name = &input.ident;
    Ok(quote! {
        #[automatically_derived]
        impl ::typesafe::rubric::RubricChoice for #name {
            const OPTIONS: &'static [(&'static str, ::core::option::Option<&'static str>)] =
                &[#(#options),*];

            fn from_label(label: &str) -> ::core::option::Option<Self> {
                match label {
                    #(#from_label,)*
                    _ => ::core::option::Option::None,
                }
            }

            fn label(&self) -> &'static str {
                match self {
                    #(#to_label,)*
                }
            }
        }

        #[automatically_derived]
        impl ::typesafe::rubric::ChoiceField for #name {
            fn question(instructions: ::typesafe::serde_json::Value) -> ::typesafe::Choice {
                <Self as ::typesafe::rubric::RubricChoice>::choice(instructions)
            }

            fn from_choice(
                answer: &::typesafe::ChoiceAnswer,
            ) -> ::core::result::Result<Self, ::typesafe::rubric::UnknownLabel> {
                <Self as ::typesafe::rubric::RubricChoice>::parse_label(&answer.choice)
            }
        }

        #[automatically_derived]
        impl ::core::str::FromStr for #name {
            type Err = ::typesafe::rubric::UnknownLabel;

            fn from_str(s: &str) -> ::core::result::Result<Self, Self::Err> {
                <Self as ::typesafe::rubric::RubricChoice>::parse_label(s)
            }
        }
    })
}

/// `#[option("description")]`
fn option_description(attrs: &[Attribute]) -> Result<Option<LitStr>> {
    let mut description = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("option")) {
        description = Some(attr.parse_args::<LitStr>()?);
    }
    Ok(description)
}

/// `Billing` → `billing`, `NeedsHuman` → `needs_human`, `HTTPError` → `http_error`.
fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push('_');
            }
        }
        out.extend(c.to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::snake_case;

    #[test]
    fn labels_are_snake_case() {
        assert_eq!(snake_case("Billing"), "billing");
        assert_eq!(snake_case("NeedsHuman"), "needs_human");
        assert_eq!(snake_case("HTTPError"), "http_error");
        assert_eq!(snake_case("Tier2Support"), "tier2_support");
        assert_eq!(snake_case("already_snake"), "already_snake");
    }
}

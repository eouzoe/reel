//! Procedural attribute macros for the sealed `ClassA` / `ClassB` / `ClassC`
//! effect-class traits in the `reel-effects` crate.
//!
//! Three macros are exported:
//!
//! - [`class_a`] — declares the annotated item as a Class A effect
//!   (pure local, reversible).
//! - [`class_b`] — declares the annotated item as a Class B effect
//!   (idempotent remote).
//! - [`class_c`] — declares the annotated item as a Class C effect
//!   (irreversible remote).
//!
//! Each macro:
//!
//! 1. Re-emits the annotated item unchanged.
//! 2. Generates `impl ::reel_effects::__private::Sealed for $Type {}` so
//!    that the user may write the matching `impl ::reel_effects::ClassA` /
//!    `ClassB` / `ClassC` for the same type — the seal is the only
//!    documented external route into the three sealed traits.
//! 3. Emits a `const _: () = { … }` marker named
//!    `__REEL_EFFECT_CLASS_<X>` whose presence is the L3 anchor for
//!    cross-checking against `[package.metadata.reel] effect-class = "X"`
//!    in the crate manifest.  Applying two different effect-class macros
//!    to the same type produces both a duplicate `Sealed` impl error and a
//!    duplicate-const error — drift is therefore impossible without two
//!    independent compile failures.
//!
//! # Sealing trade-off (read before changing)
//!
//! Macros expand at the **call site** — the generated `impl Sealed` lives
//! in the user's crate.  To make the macros' output compile, `reel-effects`
//! re-exports `Sealed` via the `#[doc(hidden)] pub mod __private` module.
//! This is the established Rust pattern for derive-style macros over sealed
//! traits (cf. `serde` / `pin-project` / `clap`).  The seal is no longer
//! purely mechanical (it relies on the `__private` re-export being treated
//! as off-API by humans) but the macros remain the documented path: any
//! impl that bypasses them must spell out `::reel_effects::__private::Sealed`,
//! which `#[doc(hidden)]` keeps out of the published API surface and which
//! `#[deny(rustdoc::private_intra_doc_links)]` would flag in downstream docs.
//!
//! # References
//!
//! - ADR-0003 (effect-class taxonomy)
//! - ADR-0006 (effect-isolation layers)
//! - ADR-0025 (canonical 6+3+3 ontology)
//! - the design notes § 5 (L1 sealed traits)
//! - the design notes § 5 (macro shape)

#![deny(rustdoc::broken_intra_doc_links)]
#![warn(missing_docs)]
#![allow(
    clippy::multiple_crate_versions,
    clippy::lint_groups_priority,
    reason = "pre-existing workspace lint configuration; matched by reel-effects"
)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Item, parse2};

/// Effect class identifiers used by the three attribute macros.
#[derive(Clone, Copy)]
enum Class {
    A,
    B,
    C,
}

impl Class {
    const fn letter(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        }
    }

    const fn const_prefix(self) -> &'static str {
        match self {
            Self::A => "__REEL_EFFECT_CLASS_A",
            Self::B => "__REEL_EFFECT_CLASS_B",
            Self::C => "__REEL_EFFECT_CLASS_C",
        }
    }

    fn marker_ident_for(self, ty: &syn::Ident) -> syn::Ident {
        syn::Ident::new(&format!("{}__{}", self.const_prefix(), ty), proc_macro2::Span::call_site())
    }
}

/// Mark a `struct` or `enum` as a Class A (pure local, reversible) effect
/// type, generating the sealed-trait token impl needed to write
/// `impl ::reel_effects::ClassA for Self`.
///
/// See the [crate-level documentation](crate) for the expansion shape and
/// the sealing trade-off.
///
/// # Errors
///
/// Compile errors are emitted (via [`syn::Error::to_compile_error`]) if:
///
/// - The macro is applied to anything other than a `struct` or `enum`.
/// - The annotated item carries generic type parameters (current limitation;
///   the sealed impl shape cannot be inferred without an explicit `where`
///   clause supplied by the caller).
/// - The macro receives any attribute arguments (`#[reel::class_a(...)]` is
///   reserved for future expansion).
#[proc_macro_attribute]
pub fn class_a(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(Class::A, args.into(), input.into()).into()
}

/// Mark a `struct` or `enum` as a Class B (idempotent remote) effect type,
/// generating the sealed-trait token impl needed to write
/// `impl ::reel_effects::ClassB for Self`.
///
/// See the [crate-level documentation](crate) for the expansion shape.
///
/// # Errors
///
/// Same conditions as [`class_a`].
#[proc_macro_attribute]
pub fn class_b(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(Class::B, args.into(), input.into()).into()
}

/// Mark a `struct` or `enum` as a Class C (irreversible remote) effect type,
/// generating the sealed-trait token impl needed to write
/// `impl ::reel_effects::ClassC for Self`.
///
/// See the [crate-level documentation](crate) for the expansion shape and
/// the commit-UI obligations that Class C imposes on adapters.
///
/// # Errors
///
/// Same conditions as [`class_a`].
#[proc_macro_attribute]
pub fn class_c(args: TokenStream, input: TokenStream) -> TokenStream {
    expand(Class::C, args.into(), input.into()).into()
}

fn expand(class: Class, args: TokenStream2, input: TokenStream2) -> TokenStream2 {
    if !args.is_empty() {
        return syn::Error::new_spanned(
            args,
            format!(
                "#[reel::class_{}] does not accept attribute arguments \
                 (got: see invocation site)",
                class.letter().to_ascii_lowercase()
            ),
        )
        .to_compile_error();
    }

    let item: Item = match parse2(input) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };

    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "syn::Item is #[non_exhaustive]; we reject anything that isn't a struct or enum"
    )]
    let (ident, generics_span) = match &item {
        Item::Struct(s) => (s.ident.clone(), &s.generics),
        Item::Enum(e) => (e.ident.clone(), &e.generics),
        other => {
            return syn::Error::new_spanned(
                other,
                format!(
                    "#[reel::class_{}] may only be applied to a struct or enum",
                    class.letter().to_ascii_lowercase()
                ),
            )
            .to_compile_error();
        }
    };

    if !generics_span.params.is_empty() {
        return syn::Error::new_spanned(
            generics_span,
            format!(
                "#[reel::class_{}] does not currently support generic effect \
                 types (open question: ADR follow-up).  Hand-write the \
                 `impl ::reel_effects::__private::Sealed` + `impl ClassA/B/C` \
                 pair for now.",
                class.letter().to_ascii_lowercase()
            ),
        )
        .to_compile_error();
    }

    // Per-type drift marker.  Two effect-class macros applied to the same
    // type would each try to emit a const named
    // `__REEL_EFFECT_CLASS_<X>__<Type>`, but the duplicate `impl Sealed`
    // expansion already fails the compile.  The per-type suffix avoids
    // collisions between unrelated `#[class_a]` annotated types living in
    // the same module.
    let marker_ident = class.marker_ident_for(&ident);
    let letter = class.letter();

    quote! {
        #item

        #[doc(hidden)]
        impl ::reel_effects::__private::Sealed for #ident {}

        #[doc(hidden)]
        #[allow(non_upper_case_globals, reason = "compiler-generated drift marker")]
        const #marker_ident: &str = #letter;
    }
}

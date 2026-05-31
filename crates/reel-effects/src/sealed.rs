//! Private sealed-trait mechanism.
//!
//! The canonical Rust sealed-trait pattern: a `pub` trait inside a
//! `pub(crate)` module.  The module is unreachable from outside the crate,
//! so the `pub` visibility of `Sealed` is meaningless to external users.
//! Any attempt by an external crate to write `impl Sealed for Foo` fails at
//! visibility resolution.
//!
//! See
//! effect-class-rules § 5
//! and
//! 03-effect-isolation § 2.

/// Private supertrait that seals the three effect-class traits.
///
/// The module `sealed` is `pub(crate)`, so this trait is only nameable
/// within `reel-effects`.  External `impl`s of `ClassA`, `ClassB`, and
/// `ClassC` are therefore impossible.
pub trait Sealed {}

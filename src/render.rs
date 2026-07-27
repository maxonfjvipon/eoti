//! The pretty-printer.
//!
//! Types in named-binder form: every variable named once, the single-use ones
//! inlined, the rest bound in a trailing `where` clause, and whatever stays open
//! quantified up front.

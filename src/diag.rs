//! Diagnostics.
//!
//! The machine-readable verdict a compiler gates on: a document shaped like an
//! LSP diagnostic list, extended with the `@loc` of each offending object, so an
//! editor's language server can pass it straight through.

//! Structural type inference and pre-run type checker for EO, over XMIR.
//!
//! The checker consumes only XMIR — EO's φ-calculus XML intermediate
//! representation — so it stays decoupled from any particular EO compiler or
//! runtime. It infers a type for every object and reports the mistakes that
//! would otherwise surface only at run time, such as dispatching `.plus` on a
//! `string`.
//!
//! The type system is structural subtyping in the MLsub tradition: a type is a
//! shape rather than a name, `bytes` is the one base type, and `@` models width
//! subtyping. The design lives in `docs/eo-type-inference.tex` and its behavior
//! is frozen as a contract in `conformance/`.

pub mod diag;
pub mod infer;
pub mod render;
pub mod resolve;
pub mod solver;
pub mod types;
pub mod xmir;

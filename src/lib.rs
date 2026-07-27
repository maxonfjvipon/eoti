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

/// How much stack the checker is given to recurse in.
const ROOM: usize = 256 * 1024 * 1024;

/// Run something on a thread with room to recurse.
///
/// Constraining, instantiation and rendering all walk the type graph by
/// following it, and a deep graph wants more stack than a thread is handed by
/// default — two megabytes for a test thread, eight for a main one, either of
/// which a large enough program will exhaust. Asking for the room up front turns
/// a crash on somebody's big codebase into a limit nobody reaches.
///
/// # Panics
///
/// If the thread cannot be started, or the work panics.
pub fn deeply<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(ROOM)
            .spawn_scoped(scope, work)
            .expect("cannot start a thread with room to recurse")
            .join()
            .expect("the checker gave up")
    })
}

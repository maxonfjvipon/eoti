//! Forma resolution through `@loc`.
//!
//! A return type such as `Φ.posix.return` is also a locator into the object
//! graph, so a non-primitive forma can be resolved to the shape of the object it
//! names instead of staying opaque. Resolution is memoized, cycle-guarded and
//! best-effort: what cannot be typed in isolation falls back to opaque rather
//! than failing.

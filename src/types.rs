//! The type vocabulary.
//!
//! A type is a shape, not a name. An unknown carries an MLsub level and the
//! bounds that flow through it, `bytes` is the single base type, a record holds
//! its fields and its ordered voids, and an option is a value that may be `⊥`.

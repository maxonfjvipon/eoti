//! The constraint solver.
//!
//! Constraining says one type must be usable as another, accumulating lower and
//! upper bounds on every unknown it passes through. Levels keep a formation's
//! parameters fresh per use while its recursion and context stay shared, and
//! extrusion lowers a deep unknown when it meets a shallower one.

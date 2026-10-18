//! What the `wordle-opt` solver and its checker share: words and their
//! responses ([`core`]), word lists ([`dict`]), the table of every guess's
//! response to every candidate ([`matrix`]) and the policy tree format with
//! its validation ([`tree`]), and the optimality-certificate file format
//! ([`certfile`]), plus a cheap bound on how many responses a guess can give
//! on a set ([`profile`]).

pub mod certfile;
pub mod core;
pub mod dict;
pub mod matrix;
pub mod profile;
pub mod tree;

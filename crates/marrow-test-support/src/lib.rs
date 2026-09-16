//! The image-construction fixtures more than one crate's tests build over.
//!
//! Each one is a contract, not a convenience. The plan is the census a draft is admitted
//! under; the site seam is the only producer path to an operation site; the forger names
//! the container's digest slot by offset, which is the container format itself. A copy
//! per crate is a copy free to drift, and drift here does not fail loudly: it yields
//! fixtures that stop at an earlier gate, and a test asserting a rejection still passes,
//! at the wrong phase, for the wrong reason.
//!
//! Every consumer names this crate as a `[dev-dependencies]` path edge, so nothing here
//! reaches a production build and no production visibility widens to serve it. A fixture
//! that needs a private constructor does not belong here; it belongs beside the tests of
//! the crate that owns the constructor.

mod draft;
mod forgery;

pub use draft::{admitted, admitted_plan, site};
pub use forgery::{forge, rehash};

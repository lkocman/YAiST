//! The handbook documents every module and action as the code defines them:
//! each module has its chapter, included in the book, and the generated
//! blocks match the actions. Fix with `yaist docs`.

use std::path::Path;

#[test]
fn handbook_is_up_to_date() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/handbook");
    let stale = yaist_modules::handbook::update(&yaist_modules::registry(), &dir, false).unwrap_or_else(|e| panic!("{e}"));
    assert!(stale.is_empty(), "the handbook is out of date, run `yaist docs` (or `cargo run -p yaist --bin yaist -- docs`): {stale:?}");
}

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

/// Every screenshot a chapter shows is in docs/screenshots.
#[test]
fn handbook_images_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    for entry in std::fs::read_dir(root.join("handbook/modules")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("image::") {
                let name = rest.split('[').next().unwrap();
                assert!(root.join("screenshots").join(name).exists(), "{}: no docs/screenshots/{name}", path.display());
            }
        }
    }
}

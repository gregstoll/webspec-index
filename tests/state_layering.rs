use std::fs;
use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out)
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path)
        }
    }
}

#[test]
fn state_layer_never_imports_effects() {
    let mut files = Vec::new();
    walk(Path::new("src/state"), &mut files);
    assert!(!files.is_empty());
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains("crate::effects"),
            "{} imports crate::effects",
            file.display()
        );
        assert!(
            !text.contains("webspec_index::effects"),
            "{} imports effects",
            file.display()
        );
    }
}

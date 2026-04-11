use super::{fixture_root, parse_snapshot_fixture};

#[test]
fn default_fixture_root_is_snapshot_dir() {
    let root = fixture_root().expect("fixture root");
    assert!(root.ends_with("snapshot"), "{root:?}");
}

#[test]
fn parses_rendered_snapshot_fixture_blocks() {
    let contents = r#"# sample case

x = 1

# ==

# module_init: _dp_module_init
#
# function _dp_module_init() [kind=function, bind=_dp_module_init, target=local, qualname=_dp_module_init]
#     __dp_store_global(globals(), "x", 1)

# another case

if flag:
    y = 2

# ==

# module_init: _dp_module_init
# function _dp_module_init() [kind=function, bind=_dp_module_init, target=local, qualname=_dp_module_init]
#     if_term flag:
"#;
    let blocks = parse_snapshot_fixture(contents).expect("parse snapshot fixture");
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].name, "sample case");
    assert_eq!(blocks[0].input, "\nx = 1\n\n");
    assert_eq!(blocks[1].name, "another case");
    assert_eq!(blocks[1].input, "\nif flag:\n    y = 2\n\n");
}

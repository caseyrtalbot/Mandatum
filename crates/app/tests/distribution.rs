use std::ffi::OsStr;
use std::path::Path;

#[test]
fn public_executable_is_named_mandatum() {
    let binary = Path::new(env!("CARGO_BIN_EXE_mandatum"));
    assert_eq!(binary.file_name(), Some(OsStr::new("mandatum")));
}

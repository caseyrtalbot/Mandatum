fn main() {
    match mandatum_native_lab::visual_diff::run_cli(std::env::args().skip(1)) {
        Ok(true) => {}
        Ok(false) => std::process::exit(2),
        Err(error) => {
            eprintln!("visual-diff: {error}");
            std::process::exit(1);
        }
    }
}

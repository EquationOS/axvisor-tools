use std::env;
use std::path::PathBuf;

fn main() {
    let eqmanager_header = "../eqdriver/includes/eqmanager.h";

    // Tell cargo to rerun if header changes
    println!("cargo:rerun-if-changed={}", eqmanager_header);

    let bindings = bindgen::Builder::default()
        .header(eqmanager_header)
        .allowlist_type("eq_.*")
        .allowlist_function(".*")
        .allowlist_var("EQ_.*")
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("eqioctl.rs"))
        .expect("Couldn't write bindings!");
}

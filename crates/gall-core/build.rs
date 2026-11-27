fn main() {
    println!("cargo:rerun-if-env-changed=GALL_PACKAGE_MANAGED");
}

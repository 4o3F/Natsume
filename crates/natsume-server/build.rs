fn main() {
    println!("cargo:rerun-if-changed=diesel/business/migrations");
    println!("cargo:rerun-if-changed=diesel/journal/migrations");
}

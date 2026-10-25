//! Compile the checked-in SQL grammar for both repository and source packages.
fn main() {
    let source = std::path::Path::new("grammars/sql/src");
    println!("cargo:rerun-if-changed=grammars/sql/src");
    let mut build = cc::Build::new();
    build.std("c11").include(source);
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flag("-utf-8");
    }
    build
        .file(source.join("parser.c"))
        .file(source.join("scanner.c"))
        .compile("oxide-sql-grammar");
}

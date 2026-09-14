#[cfg(feature = "gen")]
fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let proto_dir = format!("{manifest}/../../build/language/spec");
    let out_dir = format!("{manifest}/../rust-sass-embedded-pb/src");
    prost_build::Config::new()
        .out_dir(&out_dir)
        .compile_protos(&[format!("{proto_dir}/embedded_sass.proto")], &[proto_dir])
        .unwrap();
    // prost-build names the output after the proto package
    // (sass.embedded_protocol.rs); normalize it to embedded_sass.rs.
    std::fs::rename(
        format!("{out_dir}/sass.embedded_protocol.rs"),
        format!("{out_dir}/embedded_sass.rs"),
    )
    .unwrap();
    eprintln!("wrote rust-sass-embedded-pb/src/embedded_sass.rs");
}

#[cfg(not(feature = "gen"))]
fn main() {
    eprintln!(
        "rust-sass-embedded-pb-gen: regenerate via `cargo run -p rust-sass-embedded-pb-gen --features gen`"
    );
    std::process::exit(1);
}

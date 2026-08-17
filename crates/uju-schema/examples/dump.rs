use uju_schema::codegen::Backend;
fn main() {
    let s1 = std::fs::read_to_string("crates/uju-schema/tests/sample1.uju").unwrap();
    let s2 = std::fs::read_to_string("crates/uju-schema/tests/sample2.uju").unwrap();
    let sources = vec![
        uju_schema::Source::new("sample1.uju", s1),
        uju_schema::Source::new("sample2.uju", s2),
    ];
    match uju_schema::compile(&sources) {
        Ok(schema) => {
            for f in uju_schema::codegen::rust::Rust.emit(&schema) {
                println!("{}", f.contents);
            }
        }
        Err(d) => println!("{}", uju_schema::render(&sources, &d)),
    }
}

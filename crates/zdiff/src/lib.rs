use std::path::Path;

pub mod cached_file;
pub mod diff_builder;
pub mod diff_ir;
pub mod lexer;
pub mod myers;

pub fn read_file_contents<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    use std::fs::File;
    use std::io::Read;

    let mut file = File::open(&path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

pub mod lexer;

//// TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{Lexer, RawToken, TokenKind};

    // use std::fs::{self, File};
    // use std::path::Path;
    // use tempfile::{TempDir, tempdir};

    // fn create_file(path: &Path) {
    //     File::create(path).expect("failed to create file");
    // }

    #[test]
    fn test_function_syntax() {
        let cases = [
            // Declarations
            ("C Decl", r#"void main(int argc, char *argv[])"#),
            ("Rust Decl", r#"fn main()"#),
            ("Python Decl", r#"def main(argc, argv):"#),
            // Definitions
            ("C Def", "void main(int argc, char *argv[])\n{\n    std::out << \"Hello, world!\" << std::endl;\n}"),
            ("Rust Def", "fn main()\n{\n    println!(\"Hello, world!\");\n}\n"),
            ("Python Def", "def main(argc, argv):\n    print(\"Hello, world!\")"),
        ];

        for (name, src) in cases {
            let mut lexer = Lexer::new(src);
            let tokens = lexer.parse();

            assert!(
                !tokens.iter().any(|t| matches!(t.kind, TokenKind::Unknown)),
                "[{}] Lexer found unknown tokens: {:?}", 
                name, 
                tokens.iter()
                    .filter(|t| matches!(t.kind, TokenKind::Unknown))
                    .map(|t| lexer.token_value(t))
                    .collect::<Vec<_>>()
            );

            let reconstructed = lexer.reconstruct_source(&tokens);
            assert_eq!(
                reconstructed, 
                src, 
                "[{}] Reconstruction mismatch!\nExpected: {:?}\nGot:      {:?}", 
                name, 
                src, 
                reconstructed
            );
        }
    }

    #[test]
    fn test_greedy_operators_exhaustive() {
        let input = "x / / y // comment\nx /* block */ y";
        let mut lexer = Lexer::new(input);
        let tokens = lexer.parse();

        // Verification Table:
        // Index | Token Kind | Value
        // -------------------------
        // 0     | Identifier | "x"
        // 1     | Whitespace | " "
        // 2     | Symbol     | "/"
        // 3     | Whitespace | " "
        // 4     | Symbol     | "/"
        // 5     | Whitespace | " "
        // 6     | Identifier | "y"
        // 7     | Whitespace | " "
        // 8     | Comment    | "// comment"
        // 9     | Whitespace | "\n"
        // 10    | Identifier | "x"
        // 11    | Whitespace | " "
        // 12    | Comment    | "/* block */"
        // 13    | Whitespace | " "
        // 14    | Identifier | "y"

        let expected = vec![
            (TokenKind::Identifier, "x"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Symbol, "/"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Symbol, "/"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Identifier, "y"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Comment, "// comment"),
            (TokenKind::Whitespace, "\n"),
            (TokenKind::Identifier, "x"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Comment, "/* block */"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Identifier, "y"),
        ];

        assert_eq!(tokens.len(), expected.len(), "Token count mismatch.");

        for (i, (kind, value)) in expected.into_iter().enumerate() {
            assert_eq!(
                tokens[i].kind, kind,
                "Token[{}] kind mismatch. Expected {:?}, got {:?}", i, kind, tokens[i].kind
            );
            assert_eq!(
                lexer.token_value(&tokens[i]), value,
                "Token[{}] value mismatch. Expected {:?}, got {:?}", i, value, lexer.token_value(&tokens[i])
            );
        }
    }

    #[test]
    fn test_complex_strings() {
        let input = r#"" " "" "with symbols !@#" "unclosed"#;
        let mut lexer = Lexer::new(input);
        let tokens = lexer.parse();

        assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::String)), 
            "Lexer failed to identify any String tokens in input: {}", input);
        
        let reconstructed = lexer.reconstruct_source(&tokens);
        assert_eq!(reconstructed, input, "String reconstruction failed. Original: {}, Got: {}", input, reconstructed);
    }

    #[test]
    fn test_numeric_boundaries() {
        let input = "123.456 789";
        let mut lexer = Lexer::new(input);
        let tokens = lexer.parse();

        assert_eq!(tokens[0].kind, TokenKind::Number, "Expected '123' to be Number, got {:?}", tokens[0].kind);
        assert_eq!(tokens[1].kind, TokenKind::Symbol, "Expected '.' to be Symbol, got {:?}", tokens[1].kind);
        assert_eq!(tokens[2].kind, TokenKind::Number, "Expected '456' to be Number, got {:?}", tokens[2].kind);
        assert_eq!(tokens[3].kind, TokenKind::Whitespace, "Expected ' ' to be Whitespace, got {:?}", tokens[3].kind);
        assert_eq!(tokens[4].kind, TokenKind::Number, "Expected '789' to be Number, got {:?}", tokens[4].kind);
    }

    #[test]
    fn test_unicode_and_whitespace() {
        let input = "let 🦀 = \"value\";\t\n ";
        let mut lexer = Lexer::new(input);
        let tokens = lexer.parse();

        for token in &tokens {
            assert!(!matches!(token.kind, TokenKind::Unknown), 
                "Unknown token found: {:?} ('{}')", 
                token, &input[token.span.clone()]);
        }
        
        assert_eq!(lexer.reconstruct_source(&tokens), input, "Unicode/Whitespace reconstruction failed.");
    }

    #[test]
    fn test_lex_idempotency() {
        let input = "fn main() { let x = 5; } // check";
        let mut lexer1 = Lexer::new(input);
        let tokens1 = lexer1.parse();
        
        let reconstructed = lexer1.reconstruct_source(&tokens1);
        let mut lexer2 = Lexer::new(&reconstructed);
        let tokens2 = lexer2.parse();

        assert_eq!(tokens1.len(), tokens2.len(), 
            "Idempotency failed: different token counts. Original: {}, New: {}", tokens1.len(), tokens2.len());
            
        for (i, (t1, t2)) in tokens1.iter().zip(tokens2.iter()).enumerate() {
            assert_eq!(t1.kind, t2.kind, 
                "Token kind mismatch at index {}.\nToken 1: {:?}\nToken 2: {:?}", i, t1, t2);
            assert_eq!(t1.span, t2.span, 
                "Token spawn mismatch at index {}.\nToken 1: {:?}\nToken 2: {:?}", i, t1, t2);
        }
    }

    #[test]
    fn test_files_simple() {
        let cpp_file = r#"
    #include <iostream>

    // Main entry point
    int main() {
        std::cout << "Hello from C++" << std::endl;
        return 0;
    }
    "#;

        let rust_file = r#"
    fn main() {
        /* Macros use the ! symbol */
        println!("Hello from Rust");
    }
    "#;

        let python_file = r#"
    #!/usr/bin/env python3
    import os

    def main():
        print("Hello from Python")

    if __name__ == "__main__":
        main()
    "#;

        let files = [
            ("main.cpp", cpp_file),
            ("main.rs", rust_file),
            ("main.py", python_file),
        ];

        for (filename, content) in files {
            let mut lexer = Lexer::new(content);
            let tokens = lexer.parse();

            assert!(
                !tokens.iter().any(|t| matches!(t.kind, TokenKind::Unknown)),
                "[{}] Lexer failed to categorize some characters: {:?}", 
                filename,
                tokens.iter()
                    .filter(|t| matches!(t.kind, TokenKind::Unknown))
                    .map(|t| lexer.token_value(t))
                    .collect::<Vec<_>>()
            );

            let reconstructed = lexer.reconstruct_source(&tokens);
            assert_eq!(
                reconstructed, 
                content, 
                "[{}] Reconstruction failed. Content was modified during lexing.", 
                filename
            );

            match filename {
                "main.cpp" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "#"));
                },
                "main.rs" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "!"));
                },
                "main.py" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "__name__"));
                },
                _ => unreachable!(),
            }
        }
    }

    #[test]
fn test_files_advanced() {
    let advanced_cpp = r#"
    #include <vector>
    #include <memory>
    #include <algorithm>

    /* * Advanced Template Meta-programming 
    * and modern C++ features.
    */
    namespace engine {
        template <typename T>
        class ResourceManager {
        private:
            std::vector<std::shared_ptr<T>> resources;
            size_t total_allocated = 0;

        public:
            ResourceManager() = default;

            auto add(T&& item) -> void {
                resources.push_back(std::make_shared<T>(std::move(item)));
                total_allocated += sizeof(T);
            }

            template <typename F>
            void for_each(F func) {
                std::for_each(resources.begin(), resources.end(), [&](auto& res) {
                    if (res != nullptr) {
                        func(*res);
                    }
                });
            }

            auto size() const { return resources.size(); }
        };
    }

    int main(int argc, char** argv) {
        engine::ResourceManager<int> manager;
        for(int i = 0; i < 100; ++i) {
            manager.add(i * 2);
        }
        // Check bits: 0xFF & 0b1010
        int bit_check = 0xAF >> 2;
        return bit_check > 0 ? 0 : 1;
    }
    "#;

        let advanced_rust = r#"
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// A complex trait for asynchronous processing
    pub trait Processor {
        type Output;
        fn process(&self, data: &str) -> Self::Output;
    }

    #[derive(Debug, Clone)]
    struct Node<T> where T: Processor {
        id: u64,
        inner: Arc<Mutex<T>>,
        metadata: HashMap<String, String>,
    }

    impl<T> Node<T> where T: Processor {
        pub fn new(id: u64, p: T) -> Self {
            Self {
                id,
                inner: Arc::new(Mutex::new(p)),
                metadata: HashMap::new(),
            }
        }

        pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
            let lock = self.inner.lock().unwrap();
            let _ = lock.process("input_data");
            println!("Node {} finished processing.", self.id);
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn test_node() {
            let input = "x += 5; // increment";
            assert!(input.contains("+="));
        }
    }
    "#;

    let advanced_python = r#"
    import numpy as np
    import pandas as pd
    from datetime import datetime

    class DataPipeline:
        """
        Handles heavy data transformations
        """
        def __init__(self, name: str):
            self.name = name
            self.start_time = datetime.now()
            self._cache = {}

        @property
        def status(self) -> str:
            return f"Pipeline {self.name} started at {self.start_time}"

        def process_frame(self, df: pd.DataFrame) -> pd.DataFrame:
            # Complex filtering
            mask = (df['val'] > 0.5) & (df['category'] != 'ignore')
            df['transformed'] = df['val'].apply(lambda x: x ** 2 if x > 0 else -1)
            
            # Multiline string check
            query = """
            SELECT * FROM results
            WHERE score > 90
            AND status = 'PASS'
            """
            return df[mask]

    def run_simulation():
        data = np.random.rand(1000, 3)
        cols = ['a', 'b', 'c']
        df = pd.DataFrame(data, columns=cols)
        pipe = DataPipeline("Sim_01")
        print(pipe.status)
        return pipe.process_frame(df)

    if __name__ == "__main__":
        results = run_simulation()
        print(f"Processed {len(results)} rows.")
    "#;

        let workload = [
            ("Advanced C++", advanced_cpp),
            ("Advanced Rust", advanced_rust),
            ("Advanced Python", advanced_python),
        ];

        for (name, source) in workload {
            let mut lexer = Lexer::new(source);
            let tokens = lexer.parse();

            let unknown_tokens: Vec<_> = tokens.iter()
                .filter(|t| t.kind == TokenKind::Unknown)
                .map(|t| lexer.token_value(t))
                .collect();

            assert!(
                unknown_tokens.is_empty(),
                "[{}] Lexer failed on these characters: {:?}", 
                name, unknown_tokens
            );

            let reconstructed = lexer.reconstruct_source(&tokens);
            assert_eq!(
                reconstructed, source,
                "[{}] RECONSTRUCTION FAILURE. Loss of data detected.", name
            );

            match name {
                "Advanced C++" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "ResourceManager"));
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == ">>")); // Shift or nested template
                },
                "Advanced Rust" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "Processor"));
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "Box"));
                },
                "Advanced Python" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "lambda"));
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "status"));
                },
                _ => {}
            }
        }
    }
}
#[cfg(test)]
mod compile_pipeline_tests {
    use crate::compile::pipeline::run_pipeline;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempFile {
        path: PathBuf,
    }

    impl TempFile {
        fn new(source: &str) -> Self {
            let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "olive_pipeline_{}__{}.liv",
                std::process::id(),
                id
            ));
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(source.as_bytes()).unwrap();
            TempFile { path }
        }

        fn path(&self) -> &str {
            self.path.to_str().unwrap()
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "olive_pipeline_dir_{}__{}",
                std::process::id(),
                id
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }

        fn join(&self, name: impl AsRef<std::path::Path>) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn pipeline_empty_source() {
        let f = TempFile::new("");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "empty source should parse successfully");
    }

    #[test]
    fn pipeline_single_expression() {
        let f = TempFile::new("42\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "single expression should compile");
    }

    #[test]
    fn pipeline_simple_let() {
        let f = TempFile::new("let x = 42\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "let binding should compile");
    }

    #[test]
    fn pipeline_function_def() {
        let f = TempFile::new("fn add(a: i64, b: i64) -> i64:\n    return a + b\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "function def should compile");
    }

    #[test]
    fn pipeline_multiple_functions() {
        let f = TempFile::new(
            "fn inc(x: i64) -> i64:\n    return x + 1\nfn dec(x: i64) -> i64:\n    return x - 1\n",
        );
        let result = run_pipeline(f.path());
        assert!(result.is_ok());
    }

    #[test]
    fn pipeline_struct_def() {
        let f = TempFile::new("struct Point:\n    x: i64\n    y: i64\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok());
    }

    #[test]
    fn pipeline_with_import_self() {
        let dir = TempDir::new();
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);

        let mod_name = format!("test_helper_{id}");
        let mod_path = dir.join(format!("{mod_name}.liv"));
        std::fs::write(&mod_path, "fn helper() -> i64:\n    return 99\n").unwrap();

        let main_path = dir.join(format!("main_{id}.liv"));
        let main_src = format!("from {mod_name} import helper\nlet result = helper()\n");
        std::fs::write(&main_path, &main_src).unwrap();

        let result = run_pipeline(main_path.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn pipeline_syntax_error_reported() {
        let f = TempFile::new("let x =\n");
        let result = run_pipeline(f.path());
        assert!(result.is_err(), "incomplete let should fail");
    }

    #[test]
    fn pipeline_type_error_reported() {
        let f = TempFile::new("let x: i64 = \"hello\"\n");
        let result = run_pipeline(f.path());
        assert!(result.is_err(), "type mismatch should fail");
    }

    #[test]
    fn pipeline_undefined_var_reported() {
        let f = TempFile::new("let y = undefined_var\n");
        let result = run_pipeline(f.path());
        assert!(result.is_err(), "undefined var should fail");
    }

    #[test]
    fn pipeline_generic_function() {
        let f = TempFile::new("fn identity[T](x: T) -> T:\n    return x\nlet y = identity(42)\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "generic function should compile");
    }

    #[test]
    fn pipeline_mut_binding() {
        let f = TempFile::new("let mut x = 0\nx = x + 1\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "mutable binding should compile");
    }

    #[test]
    fn pipeline_nonexistent_file() {
        let result = run_pipeline("/nonexistent/file.liv");
        assert!(result.is_err(), "nonexistent file should fail");
    }

    #[test]
    fn pipeline_recursive_function() {
        let f = TempFile::new(
            "fn factorial(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * factorial(n - 1)\n",
        );
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "recursive function should compile");
    }

    #[test]
    fn pipeline_empty_function() {
        let f = TempFile::new("fn noop():\n    pass\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "empty function with pass should compile");
    }

    #[test]
    fn pipeline_if_expression() {
        let f =
            TempFile::new("fn f(x: i64) -> i64:\n    if x > 0:\n        return x\n    return 0\n");
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "if expression should compile");
    }

    #[test]
    fn pipeline_dict_get_default() {
        let f = TempFile::new(
            "fn main():\n    let d: {str: int} = {\"a\": 1}\n    print(d.get(\"a\", 0))\n",
        );
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "dict.get with default should compile");
    }

    #[test]
    fn pipeline_dict_get_too_many_args() {
        let f = TempFile::new(
            "fn main():\n    let d: {str: int} = {\"a\": 1}\n    print(d.get(\"a\", 0, 9))\n",
        );
        let result = run_pipeline(f.path());
        assert!(result.is_err(), "dict.get with three args should fail");
    }

    #[test]
    fn pipeline_match_catch_all_narrows_to_success() {
        // A catch-all binding after the error arm narrows to the success member,
        // so a `str` method resolves on it instead of staying `str | E`.
        let f = TempFile::new(
            "enum E:\n    A\nfn f(x: int) -> str | E:\n    if x == 0:\n        return A\n    return \"hi\"\nfn g(x: int) -> str:\n    match f(x):\n        A:\n            return \"none\"\n        v:\n            return v.upper()\n",
        );
        let result = run_pipeline(f.path());
        assert!(
            result.is_ok(),
            "catch-all binding should narrow to the success type"
        );
    }

    #[test]
    fn pipeline_import_meta_compiles() {
        let f = TempFile::new(
            "import meta\nfn main():\n    print(meta.VERSION)\n    print(meta.NAME)\n    print(meta.AUTHOR)\n    print(meta.PIT_VERSION)\n",
        );
        let result = run_pipeline(f.path());
        assert!(result.is_ok(), "import meta should compile without a pod");
    }

    #[test]
    fn pipeline_from_meta_import_compiles() {
        let f = TempFile::new(
            "from meta import VERSION, NAME, AUTHOR, PIT_VERSION\nfn main():\n    print(VERSION)\n    print(NAME)\n    print(AUTHOR)\n    print(PIT_VERSION)\n",
        );
        let result = run_pipeline(f.path());
        assert!(
            result.is_ok(),
            "from meta import should compile without a pod"
        );
    }

    #[test]
    fn pipeline_import_meta_with_pod_meta_set() {
        crate::compile::loader::set_pod_meta(crate::compile::loader::PodMeta {
            name: "testpod".to_string(),
            version: "1.2.3".to_string(),
            author: "testauthor".to_string(),
        });
        let f = TempFile::new(
            "import meta\nfn main():\n    print(meta.VERSION)\n    print(meta.PIT_VERSION)\n",
        );
        let result = run_pipeline(f.path());
        assert!(
            result.is_ok(),
            "import meta should compile with pod meta set"
        );
    }
}

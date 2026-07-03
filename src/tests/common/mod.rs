use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A RAII-based test environment helper to manage temporary file operations
/// without polluting the host workspace or relying on external dependencies.
pub struct TestEnv {
    pub sandbox_dir: PathBuf,
}

impl TestEnv {
    pub fn new(test_name: &str) -> Self {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("fitseal_test_{}_{}", test_name, nanos));
        fs::create_dir_all(&path).expect("Failed to create temporary test directory");
        Self { sandbox_dir: path }
    }

    pub fn write_file(&self, relative_path: &str, content: &str) -> PathBuf {
        let full_path = self.sandbox_dir.join(relative_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent directories");
        }
        fs::write(&full_path, content).expect("Failed to write test file");
        full_path
    }

    pub fn read_file(&self, relative_path: &str) -> String {
        let full_path = self.sandbox_dir.join(relative_path);
        fs::read_to_string(full_path).expect("Failed to read test file")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.sandbox_dir);
    }
}

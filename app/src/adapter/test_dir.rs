use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TestDir {
    root: PathBuf,
}

impl TestDir {
    pub fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("wdc-test-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn file(&self, relative: &str, bytes: usize) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; bytes]).unwrap();
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

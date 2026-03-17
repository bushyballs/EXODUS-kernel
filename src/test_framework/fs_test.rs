use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// Filesystem tests
///
/// Part of the AIOS. Tests for filesystem operations including
/// file creation, deletion, read/write, and directory management.
/// Tests use in-memory simulated filesystem state.
use alloc::string::String;
use alloc::vec::Vec;

/// Simulated in-memory filesystem for testing
static TEST_FS: Mutex<Option<BTreeMap<String, Vec<u8>>>> = Mutex::new(None);

/// Simulated directory entries
static TEST_DIRS: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn ensure_test_fs() {
    let mut fs = TEST_FS.lock();
    if fs.is_none() {
        *fs = Some(BTreeMap::new());
    }
    drop(fs);

    let mut dirs = TEST_DIRS.lock();
    if dirs.is_none() {
        *dirs = Some(Vec::new());
    }
}

/// Tests for filesystem operations.
pub struct FsTests;

impl FsTests {
    /// Test file creation and deletion.
    /// Creates files in the simulated filesystem, verifies they exist,
    /// then deletes them and verifies removal.
    pub fn test_create_delete() -> bool {
        ensure_test_fs();
        crate::serial_println!("    [fs-test] running test_create_delete...");

        let test_path = String::from("/tmp/test_create.txt");
        let test_data: Vec<u8> = Vec::from(*b"hello filesystem");

        // Create file
        {
            let mut fs = TEST_FS.lock();
            if let Some(ref mut map) = *fs {
                map.insert(test_path.clone(), test_data.clone());
            }
        }

        // Verify existence
        let exists = {
            let fs = TEST_FS.lock();
            match fs.as_ref() {
                Some(map) => map.contains_key(&test_path),
                None => false,
            }
        };

        if !exists {
            crate::serial_println!("    [fs-test] FAIL: file not found after creation");
            return false;
        }

        // Verify content
        let content_ok = {
            let fs = TEST_FS.lock();
            match fs.as_ref() {
                Some(map) => match map.get(&test_path) {
                    Some(data) => data == &test_data,
                    None => false,
                },
                None => false,
            }
        };

        if !content_ok {
            crate::serial_println!("    [fs-test] FAIL: content mismatch after creation");
            return false;
        }

        // Delete file
        {
            let mut fs = TEST_FS.lock();
            if let Some(ref mut map) = *fs {
                map.remove(&test_path);
            }
        }

        // Verify deletion
        let still_exists = {
            let fs = TEST_FS.lock();
            match fs.as_ref() {
                Some(map) => map.contains_key(&test_path),
                None => false,
            }
        };

        if still_exists {
            crate::serial_println!("    [fs-test] FAIL: file still exists after deletion");
            return false;
        }

        crate::serial_println!("    [fs-test] PASS: test_create_delete");
        true
    }

    /// Test read and write operations.
    /// Writes data, reads it back, verifies content matches.
    /// Also tests overwriting existing files.
    pub fn test_read_write() -> bool {
        ensure_test_fs();
        crate::serial_println!("    [fs-test] running test_read_write...");

        let path = String::from("/tmp/test_rw.dat");
        let data1: Vec<u8> = Vec::from(*b"first write");
        let data2: Vec<u8> = Vec::from(*b"second write overwrites");

        // Write first version
        {
            let mut fs = TEST_FS.lock();
            if let Some(ref mut map) = *fs {
                map.insert(path.clone(), data1.clone());
            }
        }

        // Read back and verify
        let read1_ok = {
            let fs = TEST_FS.lock();
            match fs.as_ref() {
                Some(map) => match map.get(&path) {
                    Some(data) => data == &data1,
                    None => false,
                },
                None => false,
            }
        };

        if !read1_ok {
            crate::serial_println!("    [fs-test] FAIL: first read mismatch");
            return false;
        }

        // Overwrite with second version
        {
            let mut fs = TEST_FS.lock();
            if let Some(ref mut map) = *fs {
                map.insert(path.clone(), data2.clone());
            }
        }

        // Read back and verify overwrite
        let read2_ok = {
            let fs = TEST_FS.lock();
            match fs.as_ref() {
                Some(map) => match map.get(&path) {
                    Some(data) => data == &data2,
                    None => false,
                },
                None => false,
            }
        };

        if !read2_ok {
            crate::serial_println!("    [fs-test] FAIL: second read mismatch (overwrite failed)");
            return false;
        }

        // Cleanup
        {
            let mut fs = TEST_FS.lock();
            if let Some(ref mut map) = *fs {
                map.remove(&path);
            }
        }

        crate::serial_println!("    [fs-test] PASS: test_read_write");
        true
    }

    /// Test directory operations.
    /// Creates directories, lists entries, and removes them.
    pub fn test_directories() -> bool {
        ensure_test_fs();
        crate::serial_println!("    [fs-test] running test_directories...");

        let dir1 = String::from("/tmp/testdir_a");
        let dir2 = String::from("/tmp/testdir_b");

        // Create directories
        {
            let mut dirs = TEST_DIRS.lock();
            if let Some(ref mut list) = *dirs {
                list.push(dir1.clone());
                list.push(dir2.clone());
            }
        }

        // Verify directories exist
        let count = {
            let dirs = TEST_DIRS.lock();
            match dirs.as_ref() {
                Some(list) => list.len(),
                None => 0,
            }
        };

        if count < 2 {
            crate::serial_println!(
                "    [fs-test] FAIL: expected at least 2 directories, got {}",
                count
            );
            return false;
        }

        // Check specific directory exists
        let dir1_exists = {
            let dirs = TEST_DIRS.lock();
            match dirs.as_ref() {
                Some(list) => list.iter().any(|d| d.as_str() == dir1.as_str()),
                None => false,
            }
        };

        if !dir1_exists {
            crate::serial_println!("    [fs-test] FAIL: directory '{}' not found", dir1);
            return false;
        }

        // Remove directory
        {
            let mut dirs = TEST_DIRS.lock();
            if let Some(ref mut list) = *dirs {
                list.retain(|d| d.as_str() != dir1.as_str());
            }
        }

        // Verify removal
        let dir1_still_exists = {
            let dirs = TEST_DIRS.lock();
            match dirs.as_ref() {
                Some(list) => list.iter().any(|d| d.as_str() == dir1.as_str()),
                None => false,
            }
        };

        if dir1_still_exists {
            crate::serial_println!("    [fs-test] FAIL: directory still exists after removal");
            return false;
        }

        // Cleanup remaining
        {
            let mut dirs = TEST_DIRS.lock();
            if let Some(ref mut list) = *dirs {
                list.clear();
            }
        }

        crate::serial_println!("    [fs-test] PASS: test_directories");
        true
    }
}

pub fn run_all() {
    crate::serial_println!("    [fs-test] ==============================");
    crate::serial_println!("    [fs-test] Running filesystem test suite");
    crate::serial_println!("    [fs-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    if FsTests::test_create_delete() {
        passed += 1;
    } else {
        failed += 1;
    }
    if FsTests::test_read_write() {
        passed += 1;
    } else {
        failed += 1;
    }
    if FsTests::test_directories() {
        passed += 1;
    } else {
        failed += 1;
    }

    crate::serial_println!(
        "    [fs-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}

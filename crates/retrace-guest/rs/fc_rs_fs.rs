// M53 t1 (spec §3b): `std::fs`, end to end, in a temporary directory named from the pid. Prints each
// result (never an inode, a time or the directory's name) and ends with `removed=yes`.
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};

fn main() {
    let root = std::env::temp_dir().join(format!("fc_rs_fs.{}", std::process::id()));
    let deep = root.join("a/b/c");
    println!("create_dir_all={:?}", fs::create_dir_all(&deep).is_ok());
    let f1 = deep.join("f1");
    println!("write={:?}", fs::write(&f1, b"0123456789").is_ok());
    {
        let f = File::options().write(true).open(&f1).unwrap();
        println!("set_len={:?}", f.set_len(6).is_ok());
        println!("sync_all={:?}", f.sync_all().is_ok());
    }
    println!("len={}", fs::metadata(&f1).unwrap().len());
    let f2 = deep.join("f2");
    println!("copy={:?}", fs::copy(&f1, &f2).ok());
    let f3 = deep.join("f3");
    println!("rename={:?}", fs::rename(&f2, &f3).is_ok());
    let hl = deep.join("hl");
    println!("hard_link={:?}", fs::hard_link(&f1, &hl).is_ok());
    let sl = deep.join("sl");
    println!("symlink={:?}", symlink("f1", &sl).is_ok());
    println!("read_link={:?}", fs::read_link(&sl).ok());
    println!("set_permissions={:?}", fs::set_permissions(&f1, fs::Permissions::from_mode(0o600)).is_ok());
    println!("mode={:o}", fs::metadata(&f1).unwrap().permissions().mode() & 0o7777);
    println!("is_symlink={}", fs::symlink_metadata(&sl).unwrap().file_type().is_symlink());
    let mut names: Vec<String> =
        fs::read_dir(&deep).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    println!("read_dir={names:?}");
    println!("remove_file={:?}", fs::remove_file(&f3).is_ok());
    println!("remove_dir_all={:?}", fs::remove_dir_all(&root).is_ok());
    println!("removed={}", if root.exists() { "no" } else { "yes" });
    std::io::stdout().flush().unwrap();
}

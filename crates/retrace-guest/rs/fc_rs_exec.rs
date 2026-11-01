// M54 t5: Rust's `Command::exec` through `PATH` misses. The program sets the misses up itself: a directory
// holding a NON-executable file named `exec_target_dyn` (EACCES from the kernel), a nonexistent directory
// (ENOENT), then the real target's directory, where the last attempt succeeds. std searches PATH itself when
// the command sets its own, so each miss is a real `execve` the recorder answers. Nothing prints a path that
// differs between runs: the directory is fixed-named and reused. `exec` returns only on failure.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let target = std::env::args().nth(1).expect("usage: fc_rs_exec <path of exec_target_dyn>");
    let tdir = std::path::Path::new(&target).parent().expect("the target has a directory").to_owned();
    let noexec = std::env::temp_dir().join("fc_rs_exec.noexec");
    let _ = fs::remove_dir_all(&noexec);
    fs::create_dir_all(&noexec).unwrap();
    let f = noexec.join("exec_target_dyn");
    fs::write(&f, b"not executable\n").unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
    let path = format!("/nonexistent:{}:{}", noexec.display(), tdir.display());
    println!("exec through PATH misses");
    let err = Command::new("exec_target_dyn").arg("rs").env("PATH", path).exec();
    println!("exec returned: {err}");
    std::process::exit(1);
}

//! Optional off-site backup upload via an operator-configured command.
//!
//! Rather than baking an object-storage SDK into the engine (heavy, and it locks
//! you to one provider), Nucleus shells out to a command you configure with
//! `NUCLEUS_BACKUP_UPLOAD_CMD` after each backup file is written. That command is
//! whatever already moves files to your store — `aws s3 cp`, `rclone copy`,
//! `mc cp`, `gsutil cp`, `scp`, … The placeholder `{}` is replaced by the backup
//! file's absolute path; if absent, the path is appended as the final argument.
//!
//! Examples:
//! - `aws s3 cp {} s3://my-bucket/nucleus/`
//! - `rclone copy {} remote:nucleus`

use std::path::Path;
use std::process::Command;

/// Split a command template into `(program, args)`, substituting `{}` with the
/// backup file path (or appending it if no placeholder is present). Whitespace-
/// separated; quoted arguments with embedded spaces are not supported (point the
/// command at a wrapper script if you need them).
fn build_command(template: &str, file: &Path) -> Option<(String, Vec<String>)> {
    let path = file.to_string_lossy().into_owned();
    let mut tokens = template.split_whitespace().map(str::to_string);
    let program = tokens.next()?;
    let mut args: Vec<String> = tokens.collect();
    let mut substituted = false;
    for a in args.iter_mut() {
        if a == "{}" {
            *a = path.clone();
            substituted = true;
        }
    }
    if !substituted {
        args.push(path);
    }
    Some((program, args))
}

/// Run the configured upload command for `file`. Returns an error string on a
/// non-zero exit or spawn failure; the caller logs it (a failed upload must not
/// break the backup itself).
pub fn upload(template: &str, file: &Path) -> Result<(), String> {
    let (program, args) =
        build_command(template, file).ok_or_else(|| "empty upload command".to_string())?;
    let status = Command::new(&program)
        .args(&args)
        .status()
        .map_err(|e| format!("spawn `{program}` failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("upload command exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn substitutes_placeholder() {
        let (prog, args) =
            build_command("aws s3 cp {} s3://b/n/", &PathBuf::from("/data/x.redb")).unwrap();
        assert_eq!(prog, "aws");
        assert_eq!(args, vec!["s3", "cp", "/data/x.redb", "s3://b/n/"]);
    }

    #[test]
    fn appends_path_when_no_placeholder() {
        let (prog, args) =
            build_command("rclone copy remote:nucleus", &PathBuf::from("/d/y.patch")).unwrap();
        assert_eq!(prog, "rclone");
        assert_eq!(args, vec!["copy", "remote:nucleus", "/d/y.patch"]);
    }

    #[test]
    fn empty_template_is_none() {
        assert!(build_command("   ", &PathBuf::from("/d/z")).is_none());
    }
}

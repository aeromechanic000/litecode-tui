pub mod executor;
pub mod landlock;
pub mod seatbelt;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
const BLOCKED_COMMANDS: &[&str] = &[
    "sudo",
    "su",
    "mkfs",
    "dd",
    "format",
    "chmod",
    "chown",
    "systemctl",
    "service",
    "launchctl",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
    "rm",
    "rmdir",
    "del",
    "curl|bash",
    "wget|bash",
    "tee",
    "crontab",
];

#[allow(dead_code)]
const ALLOWED_COMMANDS: &[&str] = &[
    "cargo", "rustc", "rustup", "python", "python3", "uv", "pip", "pip3", "node", "npm", "npx",
    "bun", "deno", "go", "gcc", "g++", "make", "cmake", "git", "curl", "wget", "ls", "cat", "head",
    "tail", "find", "grep", "rg", "fd", "echo", "pwd", "which", "env", "dotnet", "java", "javac",
    "bash", "sh", "zsh", "docker", "podman", "pytest", "jest", "vitest", "mkdir", "touch", "cp",
    "mv", "test", // read-only system inspection (macOS and/or Linux); mutating tools with
    // destructive subcommands (diskutil, hdiutil) are deliberately excluded
    "df", "du", "stat", "file", "wc", "uname", "hostname", "id", "whoami", "date", "uptime", "ps",
    "sysctl", "free", "lsblk", "sw_vers", "vm_stat", "system_profiler",
];

#[allow(dead_code)]
pub struct Sandbox {
    workspace: PathBuf,
    allowed: HashSet<String>,
    blocked: HashSet<String>,
}

#[allow(dead_code)]
impl Sandbox {
    pub fn new(workspace: PathBuf) -> Self {
        let allowed: HashSet<String> = ALLOWED_COMMANDS.iter().map(|s| s.to_string()).collect();
        let blocked: HashSet<String> = BLOCKED_COMMANDS.iter().map(|s| s.to_string()).collect();
        Self {
            workspace,
            allowed,
            blocked,
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn validate_path(&self, path: &Path) -> Result<PathBuf, SandboxError> {
        let canonical_target = if path.exists() {
            path.canonicalize()
                .map_err(|e| SandboxError::PathResolution(e.to_string()))?
        } else {
            // For new files, canonicalize parent and append filename
            let parent = path.parent().unwrap_or(Path::new("."));
            let canonical_parent = parent
                .canonicalize()
                .map_err(|e| SandboxError::PathResolution(e.to_string()))?;
            canonical_parent.join(
                path.file_name()
                    .ok_or_else(|| SandboxError::PathResolution("No filename".into()))?,
            )
        };

        let canonical_workspace = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.clone());

        if !canonical_target.starts_with(&canonical_workspace) {
            return Err(SandboxError::PathOutsideWorkspace {
                path: path.to_path_buf(),
                workspace: canonical_workspace,
            });
        }

        Ok(canonical_target)
    }

    pub fn validate_command(&self, cmd: &str, args: &[String]) -> Result<(), SandboxError> {
        let base = Path::new(cmd)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(cmd)
            .to_string();

        // Check explicit blocks
        if self.blocked.contains(&base) {
            return Err(SandboxError::CommandBlocked(base));
        }

        // rm is only allowed within workspace context (we check paths separately)
        // but -rf / or similar patterns are always blocked
        let args_str = args.join(" ");
        if args_str.contains("-rf /")
            || args_str.contains("-r /")
            || args_str.contains("/ --recursive")
        {
            return Err(SandboxError::DangerousArguments(args_str));
        }

        // Check if command is in allowlist (skip check if it's a path-like)
        if !cmd.contains('/') && !self.allowed.contains(&base) {
            return Err(SandboxError::CommandNotAllowed(base));
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum SandboxError {
    #[error("Path outside workspace: {} (workspace: {})", .path.display(), .workspace.display())]
    PathOutsideWorkspace { path: PathBuf, workspace: PathBuf },
    #[error("Cannot resolve path: {0}")]
    PathResolution(String),
    #[error("Command blocked: {0}")]
    CommandBlocked(String),
    #[error("Command not allowed: {0}")]
    CommandNotAllowed(String),
    #[error("Dangerous arguments: {0}")]
    DangerousArguments(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Sandbox) {
        let dir = TempDir::new().unwrap();
        let sandbox = Sandbox::new(dir.path().to_path_buf());
        (dir, sandbox)
    }

    #[test]
    fn path_inside_workspace_allowed() {
        let (dir, sandbox) = setup();
        let file = dir.path().join("src/main.rs");
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(&file, "fn main(){}").unwrap();
        assert!(sandbox.validate_path(&file).is_ok());
    }

    #[test]
    fn path_outside_workspace_rejected() {
        let (_dir, sandbox) = setup();
        let result = sandbox.validate_path(Path::new("/etc/passwd"));
        assert!(matches!(
            result,
            Err(SandboxError::PathOutsideWorkspace { .. })
        ));
    }

    #[test]
    fn dotdot_traversal_rejected() {
        let (dir, sandbox) = setup();
        let escape = dir.path().join("../../../etc/passwd");
        let result = sandbox.validate_path(&escape);
        assert!(result.is_err());
    }

    #[test]
    fn allowed_commands_pass() {
        let (_dir, sandbox) = setup();
        for cmd in &["cargo", "python", "node", "uv", "git", "npm", "go", "gcc"] {
            assert!(
                sandbox.validate_command(cmd, &[]).is_ok(),
                "Command {} should be allowed",
                cmd
            );
        }
    }

    #[test]
    fn read_only_inspection_commands_pass() {
        let (_dir, sandbox) = setup();
        for cmd in &["df", "du", "stat", "sysctl", "sw_vers", "system_profiler"] {
            assert!(
                sandbox.validate_command(cmd, &[]).is_ok(),
                "Command {} should be allowed",
                cmd
            );
        }
    }

    #[test]
    fn destructive_inspection_tools_still_rejected() {
        // diskutil/hdiutil have destructive subcommands (eraseDisk, attach -nomount)
        // and are deliberately kept off the allowlist
        let (_dir, sandbox) = setup();
        for cmd in &["diskutil", "hdiutil"] {
            assert!(
                sandbox.validate_command(cmd, &[]).is_err(),
                "Command {} should be rejected",
                cmd
            );
        }
    }

    #[test]
    fn blocked_commands_rejected() {
        let (_dir, sandbox) = setup();
        for cmd in &["sudo", "mkfs", "chmod", "chown", "shutdown"] {
            assert!(
                sandbox.validate_command(cmd, &[]).is_err(),
                "Command {} should be blocked",
                cmd
            );
        }
    }

    #[test]
    fn dangerous_rm_args_rejected() {
        let (_dir, sandbox) = setup();
        assert!(sandbox
            .validate_command("rm", &["-rf".into(), "/".into()])
            .is_err());
    }

    #[test]
    fn unknown_command_without_path_rejected() {
        let (_dir, sandbox) = setup();
        assert!(sandbox.validate_command("malware_tool", &[]).is_err());
    }

    #[test]
    fn path_command_bypasses_allowlist() {
        let (_dir, sandbox) = setup();
        // Commands with a path separator (e.g. ./run.sh) bypass allowlist
        assert!(sandbox.validate_command("./run.sh", &[]).is_ok());
    }
}

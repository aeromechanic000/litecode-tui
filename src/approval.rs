use std::collections::HashSet;

/// Risk level for a pending action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Read-only operations: file read, directory listing. Never need approval.
    Safe,
    /// Standard writes: create/modify files, run allowed commands.
    Write,
    /// Destructive operations: delete files, run dangerous commands. Requires YY.
    Destructive,
}

/// Classify a file action by risk level.
#[allow(dead_code)]
pub fn classify_file_action(action: &str) -> RiskLevel {
    match action {
        "delete" | "remove" => RiskLevel::Destructive,
        "create" | "modify" | "write" | "edit" => RiskLevel::Write,
        _ => RiskLevel::Write,
    }
}

/// Classify a shell command by risk level.
pub fn classify_command(cmd: &str, args: &[&str]) -> RiskLevel {
    let base = std::path::Path::new(cmd)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(cmd);

    // Destructive command patterns
    let destructive = ["rm", "rmdir", "del"];
    if destructive.contains(&base) {
        return RiskLevel::Destructive;
    }

    // Destructive argument patterns
    let args_str = args.join(" ");
    if args_str.contains("-rf /") || args_str.contains("-r /") || args_str.contains("/ --recursive")
    {
        return RiskLevel::Destructive;
    }

    // Read-only commands
    let safe = [
        "ls", "cat", "head", "tail", "find", "grep", "rg", "fd", "echo", "pwd", "which", "env",
        "git", "status", "diff", "log", "show", "branch", // read-only system inspection
        "df", "du", "stat", "file", "wc", "uname", "hostname", "id", "whoami", "date", "uptime",
        "ps", "sysctl", "free", "lsblk", "sw_vers", "vm_stat", "system_profiler",
    ];
    if safe.contains(&base) {
        // git subcommands: only read-only ones are safe
        if base == "git" {
            let safe_git = [
                "status", "log", "diff", "show", "branch", "tag", "remote", "stash", "list",
            ];
            if let Some(sub) = args.first() {
                return if safe_git.contains(sub) {
                    RiskLevel::Safe
                } else {
                    RiskLevel::Write
                };
            }
        }
        return RiskLevel::Safe;
    }

    RiskLevel::Write
}

/// Session-scoped approval cache. Once the user approves a specific
/// tool signature (e.g. "write:/src/main.rs"), it stays approved.
pub struct ApprovalCache {
    approved: HashSet<String>,
}

impl ApprovalCache {
    pub fn new() -> Self {
        Self {
            approved: HashSet::new(),
        }
    }

    /// Build a signature for a file action: "write:path" or "delete:path".
    #[allow(dead_code)]
    pub fn file_signature(action: &str, path: &str) -> String {
        format!("{}:{}", action, path)
    }

    /// Build a signature for a command: "exec:cmd arg1 arg2".
    #[allow(dead_code)]
    pub fn command_signature(cmd: &str, args: &[&str]) -> String {
        if args.is_empty() {
            format!("exec:{}", cmd)
        } else {
            format!("exec:{} {}", cmd, args.join(" "))
        }
    }

    /// Check if a signature has been approved this session.
    pub fn is_approved(&self, signature: &str) -> bool {
        self.approved.contains(signature)
    }

    /// Record an approval for the rest of the session.
    #[allow(dead_code)]
    pub fn approve(&mut self, signature: &str) {
        self.approved.insert(signature.to_string());
    }

    /// Record an approval for all files with the same action type.
    /// e.g. approve all "write" actions, or all "delete" actions.
    #[allow(dead_code)]
    pub fn approve_action_type(&mut self, action: &str) {
        // Wildcard: "write:*" or "delete:*"
        self.approved.insert(format!("{}:*", action));
    }

    /// Check if an action type has wildcard approval.
    #[allow(dead_code)]
    pub fn is_action_approved(&self, action: &str) -> bool {
        self.approved.contains(&format!("{}:*", action))
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.approved.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.approved.is_empty()
    }
}

impl Default for ApprovalCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Decision from the approval system for a given action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ApprovalDecision {
    /// Auto-approved (safe operation or cached approval).
    Approved,
    /// Needs standard y/n confirmation.
    NeedsConfirmation,
    /// Needs double-key (YY) confirmation for destructive ops.
    NeedsDestructiveConfirmation,
}

/// Evaluate whether an action needs user confirmation.
#[allow(dead_code)]
pub fn evaluate_approval(
    risk: RiskLevel,
    mode: crate::app::AppMode,
    cache: &ApprovalCache,
    signature: &str,
) -> ApprovalDecision {
    // Safe operations never need approval
    if risk == RiskLevel::Safe {
        return ApprovalDecision::Approved;
    }

    // Plan mode: no writes allowed at all
    if mode == crate::app::AppMode::Plan {
        return ApprovalDecision::NeedsConfirmation;
    }

    // Auto mode: auto-approve everything (sandbox still enforces)
    if mode == crate::app::AppMode::Auto {
        return ApprovalDecision::Approved;
    }

    // Edit mode: check cache first
    if cache.is_approved(signature) {
        return ApprovalDecision::Approved;
    }

    // Edit mode: risk-based confirmation
    match risk {
        RiskLevel::Destructive => ApprovalDecision::NeedsDestructiveConfirmation,
        RiskLevel::Write => ApprovalDecision::NeedsConfirmation,
        RiskLevel::Safe => ApprovalDecision::Approved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_file_actions() {
        assert_eq!(classify_file_action("create"), RiskLevel::Write);
        assert_eq!(classify_file_action("modify"), RiskLevel::Write);
        assert_eq!(classify_file_action("delete"), RiskLevel::Destructive);
        assert_eq!(classify_file_action("remove"), RiskLevel::Destructive);
    }

    #[test]
    fn classify_safe_commands() {
        assert_eq!(classify_command("ls", &[]), RiskLevel::Safe);
        assert_eq!(classify_command("cat", &["file.txt"]), RiskLevel::Safe);
        assert_eq!(
            classify_command("grep", &["pattern", "file"]),
            RiskLevel::Safe
        );
        assert_eq!(classify_command("git", &["status"]), RiskLevel::Safe);
        assert_eq!(classify_command("git", &["log"]), RiskLevel::Safe);
        assert_eq!(classify_command("df", &["-h"]), RiskLevel::Safe);
        assert_eq!(classify_command("stat", &["file.txt"]), RiskLevel::Safe);
        assert_eq!(classify_command("sw_vers", &[]), RiskLevel::Safe);
    }

    #[test]
    fn classify_destructive_commands() {
        assert_eq!(classify_command("rm", &["file"]), RiskLevel::Destructive);
        assert_eq!(
            classify_command("rm", &["-rf", "/"]),
            RiskLevel::Destructive
        );
    }

    #[test]
    fn classify_write_commands() {
        assert_eq!(classify_command("cargo", &["build"]), RiskLevel::Write);
        assert_eq!(classify_command("python", &["script.py"]), RiskLevel::Write);
        assert_eq!(classify_command("git", &["commit"]), RiskLevel::Write);
        assert_eq!(classify_command("git", &["push"]), RiskLevel::Write);
    }

    #[test]
    fn approval_cache_basic() {
        let mut cache = ApprovalCache::new();
        let sig = ApprovalCache::file_signature("write", "src/main.rs");
        assert!(!cache.is_approved(&sig));
        cache.approve(&sig);
        assert!(cache.is_approved(&sig));
    }

    #[test]
    fn approval_cache_wildcard() {
        let mut cache = ApprovalCache::new();
        assert!(!cache.is_action_approved("write"));
        cache.approve_action_type("write");
        assert!(cache.is_action_approved("write"));
        assert!(!cache.is_action_approved("delete"));
    }

    #[test]
    fn approval_cache_command() {
        let mut cache = ApprovalCache::new();
        let sig = ApprovalCache::command_signature("cargo", &["test"]);
        cache.approve(&sig);
        assert!(cache.is_approved(&sig));
        // Different args = different signature
        let sig2 = ApprovalCache::command_signature("cargo", &["build"]);
        assert!(!cache.is_approved(&sig2));
    }

    #[test]
    fn evaluate_safe_auto_approved() {
        let cache = ApprovalCache::new();
        let decision = evaluate_approval(RiskLevel::Safe, crate::app::AppMode::Edit, &cache, "any");
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[test]
    fn evaluate_edit_mode_write() {
        let cache = ApprovalCache::new();
        let decision = evaluate_approval(
            RiskLevel::Write,
            crate::app::AppMode::Edit,
            &cache,
            "write:src/main.rs",
        );
        assert_eq!(decision, ApprovalDecision::NeedsConfirmation);
    }

    #[test]
    fn evaluate_edit_mode_destructive() {
        let cache = ApprovalCache::new();
        let decision = evaluate_approval(
            RiskLevel::Destructive,
            crate::app::AppMode::Edit,
            &cache,
            "delete:src/main.rs",
        );
        assert_eq!(decision, ApprovalDecision::NeedsDestructiveConfirmation);
    }

    #[test]
    fn evaluate_auto_mode_approves_all() {
        let cache = ApprovalCache::new();
        let decision = evaluate_approval(
            RiskLevel::Destructive,
            crate::app::AppMode::Auto,
            &cache,
            "delete:src/main.rs",
        );
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[test]
    fn evaluate_cached_approval() {
        let mut cache = ApprovalCache::new();
        let sig = "write:src/main.rs";
        cache.approve(sig);
        let decision = evaluate_approval(RiskLevel::Write, crate::app::AppMode::Edit, &cache, sig);
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[test]
    fn file_signature_format() {
        assert_eq!(
            ApprovalCache::file_signature("write", "src/main.rs"),
            "write:src/main.rs"
        );
        assert_eq!(
            ApprovalCache::file_signature("delete", "temp.log"),
            "delete:temp.log"
        );
    }

    #[test]
    fn command_signature_format() {
        assert_eq!(
            ApprovalCache::command_signature("cargo", &["test"]),
            "exec:cargo test"
        );
        assert_eq!(ApprovalCache::command_signature("ls", &[]), "exec:ls");
    }
}

use anyhow::Result;
use std::path::Path;

const SEARCH_SKILL: &str = include_str!("../skills_builtin/search.md");
const REVIEW_SKILL: &str = include_str!("../skills_builtin/review.md");
const EXPLAIN_SKILL: &str = include_str!("../skills_builtin/explain.md");
const SIMPLIFY_SKILL: &str = include_str!("../skills_builtin/simplify.md");
const TEST_SKILL: &str = include_str!("../skills_builtin/test.md");
const TRANSLATE_SKILL: &str = include_str!("../skills_builtin/translate.md");
const COUNT_FILES_SKILL: &str = include_str!("../skills_builtin/count-files.md");

const BUILTIN_SKILLS: &[&str] = &[
    SEARCH_SKILL,
    REVIEW_SKILL,
    EXPLAIN_SKILL,
    SIMPLIFY_SKILL,
    TEST_SKILL,
    TRANSLATE_SKILL,
    COUNT_FILES_SKILL,
];

/// Seed built-in skills into `dir`, writing only missing files (never overwrites
/// user edits). Returns the names of the skills it restored, so callers can
/// report them at startup.
pub fn populate_skills(dir: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;

    let mut restored = Vec::new();
    for skill_content in BUILTIN_SKILLS {
        if let Some(skill) = super::parser::parse_skill(skill_content) {
            let path = dir.join(format!("{}.md", skill.name));
            if !path.exists() {
                std::fs::write(&path, skill_content)?;
                restored.push(skill.name.clone());
            }
        }
    }

    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn populate_creates_skill_files() {
        let dir = TempDir::new().unwrap();
        let skills_dir = dir.path().join("skills");
        let restored = populate_skills(&skills_dir).unwrap();

        assert!(skills_dir.join("search.md").exists());
        assert!(skills_dir.join("review.md").exists());
        assert!(skills_dir.join("explain.md").exists());
        assert!(skills_dir.join("simplify.md").exists());
        assert!(skills_dir.join("test.md").exists());
        assert!(skills_dir.join("translate.md").exists());
        assert!(skills_dir.join("count-files.md").exists());
        // First run into an empty dir restores every built-in.
        assert_eq!(restored.len(), BUILTIN_SKILLS.len());
    }

    #[test]
    fn populate_does_not_overwrite() {
        let dir = TempDir::new().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let custom_content = "---\nname: review\n---\nMy custom review prompt";
        std::fs::write(skills_dir.join("review.md"), custom_content).unwrap();

        let restored = populate_skills(&skills_dir).unwrap();

        let content = std::fs::read_to_string(skills_dir.join("review.md")).unwrap();
        assert!(content.contains("My custom review prompt"));
        // A pre-existing skill is neither rewritten nor reported as restored.
        assert!(!restored.contains(&"review".to_string()));
    }

    #[test]
    fn populate_reports_only_missing() {
        let dir = TempDir::new().unwrap();
        let skills_dir = dir.path().join("skills");
        populate_skills(&skills_dir).unwrap();

        // Delete one skill; the next run restores exactly that one.
        std::fs::remove_file(skills_dir.join("translate.md")).unwrap();
        let restored = populate_skills(&skills_dir).unwrap();

        assert_eq!(restored, vec!["translate".to_string()]);
        assert!(skills_dir.join("translate.md").exists());
    }
}

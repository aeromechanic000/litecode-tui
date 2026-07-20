pub mod builtin;
pub mod parser;

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub trigger: String,
    pub content: String,
}

pub struct SkillRegistry {
    skills: Vec<Skill>,
}

impl SkillRegistry {
    pub fn load_from_dir(dir: &Path) -> Self {
        let mut skills = Vec::new();

        if !dir.exists() {
            return Self { skills };
        }

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Some(skill) = parser::parse_skill(&content) {
                            skills.push(skill);
                        }
                    }
                }
            }
        }

        Self { skills }
    }

    pub fn empty() -> Self {
        Self { skills: Vec::new() }
    }

    /// Construct a registry from an owned list of skills. Useful for tests and
    /// for assembling a registry in-memory.
    #[allow(dead_code)] // currently exercised only by tests
    pub fn from_skills(skills: Vec<Skill>) -> Self {
        Self { skills }
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    pub fn list(&self) -> &[Skill] {
        &self.skills
    }

    #[allow(dead_code)]
    pub fn match_trigger(&self, input: &str) -> Option<&Skill> {
        let input_lower = input.to_lowercase();
        self.skills.iter().find(|s| {
            s.trigger
                .split(',')
                .any(|t| input_lower.contains(&t.trim().to_lowercase()))
        })
    }

    /// Return all skills whose trigger keywords appear (case-insensitive
    /// substring) in `input`, preserving registry order. Skills with no trigger
    /// are skipped — an empty keyword would match every input via the
    /// `str::contains("")` rule, so they would be auto-injected on every turn.
    /// Used by the plan→execute path to surface relevant domain rules from
    /// `~/.litepilot/skills` without the user invoking `/skill_name`.
    pub fn match_triggers(&self, input: &str) -> Vec<&Skill> {
        let input_lower = input.to_lowercase();
        self.skills
            .iter()
            .filter(|s| {
                s.trigger
                    .split(',')
                    .map(|t| t.trim().to_lowercase())
                    .filter(|t| !t.is_empty())
                    .any(|t| input_lower.contains(&t))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(name: &str, desc: &str, trigger: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.to_string(),
            trigger: trigger.to_string(),
            content: "skill body".to_string(),
        }
    }

    #[test]
    fn get_skill_by_name() {
        let registry = SkillRegistry {
            skills: vec![
                make_skill("review", "Review code", "review"),
                make_skill("search", "Search files", "search, find"),
            ],
        };
        assert!(registry.get("review").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn match_trigger_keyword() {
        let registry = SkillRegistry {
            skills: vec![
                make_skill("review", "Review code", "code review, review"),
                make_skill("search", "Search files", "search, find knowledge"),
            ],
        };
        assert!(registry.match_trigger("please code review this").is_some());
        assert!(registry.match_trigger("find knowledge about X").is_some());
        assert!(registry.match_trigger("random unrelated text").is_none());
    }

    #[test]
    fn match_triggers_returns_all_matches() {
        let registry = SkillRegistry {
            skills: vec![
                make_skill("count-files", "Count files", "count, how many"),
                make_skill("review", "Review code", "review"),
                make_skill("search", "Search files", "search, find knowledge"),
            ],
        };
        // Input matches two skills.
        let matched = registry.match_triggers("count how many files");
        let names: Vec<&str> = matched.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["count-files"]);

        // Two-trigger input hits both.
        let matched = registry.match_triggers("count then review");
        let names: Vec<&str> = matched.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["count-files", "review"]);
    }

    #[test]
    fn match_triggers_skips_empty_trigger() {
        // A skill with an empty trigger must never match (would match everything).
        let mut no_trigger = make_skill("anything", "No trigger", "");
        no_trigger.trigger = String::new();
        let registry = SkillRegistry {
            skills: vec![no_trigger],
        };
        assert!(registry.match_triggers("any input at all").is_empty());
    }

    #[test]
    fn match_triggers_case_insensitive() {
        let registry = SkillRegistry {
            skills: vec![make_skill("count-files", "Count files", "count files")],
        };
        assert_eq!(
            registry
                .match_triggers("COUNT FILES please")
                .first()
                .unwrap()
                .name,
            "count-files"
        );
    }

    #[test]
    fn empty_registry() {
        let registry = SkillRegistry::empty();
        assert!(registry.list().is_empty());
        assert!(registry.get("anything").is_none());
    }
}

// Heuristic request router: selects model tier based on request content.
// Routes: code & Q&A -> exec_model, reviews & analysis -> eval_model.
//
// Currently unwired: the two-tier pipeline routes by model size, not request
// keywords. Kept (with its tests) for a future caller.
#![allow(dead_code)]

/// Which model tier to route a request to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Exec,
    Eval,
}

/// Classify a user request into a model tier based on keyword heuristics.
pub fn classify_request(input: &str) -> ModelTier {
    let lower = input.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    // Eval patterns — review, audit, check, verify, test
    let eval_patterns = [
        "review",
        "audit",
        "check",
        "verify",
        "test",
        "inspect",
        "analyze",
        "security",
        "vulnerability",
    ];
    if contains_any(&words, &eval_patterns) {
        return ModelTier::Eval;
    }

    // Default: all other requests (code, questions, explanations) use Exec
    ModelTier::Exec
}

/// Check if any pattern appears as a whole word or as a substring in any word.
fn contains_any(words: &[&str], patterns: &[&str]) -> bool {
    let flat = words.join(" ");
    patterns.iter().any(|p| flat.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_question_routes_exec() {
        assert_eq!(classify_request("What is Rust?"), ModelTier::Exec);
        assert_eq!(
            classify_request("How does borrowing work?"),
            ModelTier::Exec
        );
        assert_eq!(classify_request("Explain this function"), ModelTier::Exec);
    }

    #[test]
    fn code_creation_routes_exec() {
        assert_eq!(
            classify_request("Create a REST API with authentication"),
            ModelTier::Exec
        );
        assert_eq!(
            classify_request("Fix the bug in login flow"),
            ModelTier::Exec
        );
        assert_eq!(
            classify_request("Add error handling to the parser"),
            ModelTier::Exec
        );
    }

    #[test]
    fn review_routes_eval() {
        assert_eq!(
            classify_request("Review this code for security issues"),
            ModelTier::Eval
        );
        assert_eq!(
            classify_request("Check if there are any bugs"),
            ModelTier::Eval
        );
        assert_eq!(
            classify_request("Analyze the performance of this module"),
            ModelTier::Eval
        );
    }

    #[test]
    fn short_input_routes_exec() {
        assert_eq!(classify_request("hello"), ModelTier::Exec);
        assert_eq!(classify_request("list files"), ModelTier::Exec);
    }

    #[test]
    fn long_input_routes_exec() {
        let long = "I have a complex system that needs to handle concurrent requests with proper error handling and retry logic across multiple services";
        assert_eq!(classify_request(long), ModelTier::Exec);
    }

    #[test]
    fn eval_beats_exec_patterns() {
        // "test" is an eval pattern, should win over potential exec patterns
        assert_eq!(
            classify_request("Write test cases for the parser"),
            ModelTier::Eval
        );
    }
}

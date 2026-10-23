//! Authorization policy shared by authentication and configuration projection.

pub(super) fn permits(
    revoked: bool,
    route: &str,
    requested_route: &str,
    models: &[String],
    requested_model: Option<&str>,
) -> bool {
    !revoked
        && route == requested_route
        && (models.is_empty()
            || requested_model.is_some_and(|model| models.iter().any(|allowed| allowed == model)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_granted_route_and_model_are_permitted() {
        let models = vec!["model-a".into()];
        assert!(permits(false, "codex", "codex", &models, Some("model-a")));
        assert!(!permits(true, "codex", "codex", &models, Some("model-a")));
        assert!(!permits(false, "codex", "claude", &models, Some("model-a")));
        assert!(!permits(false, "codex", "codex", &models, Some("model-b")));
        assert!(!permits(false, "codex", "codex", &models, None));
        assert!(permits(false, "codex", "codex", &[], None));
    }
}

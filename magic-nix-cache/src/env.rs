use std::fmt::{self, Display};

#[derive(Clone, Copy)]
pub enum Environment {
    GitHubActions,
    GitLabCI,
    Other,
}

impl Environment {
    pub fn determine() -> Self {
        if env_var_is_true("GITHUB_ACTIONS") {
            return Environment::GitHubActions;
        }

        if env_var_is_true("GITLAB_CI") {
            return Environment::GitLabCI;
        }

        Environment::Other
    }

    pub fn is_github_actions(&self) -> bool {
        matches!(self, Self::GitHubActions)
    }

    pub fn is_gitlab_ci(&self) -> bool {
        matches!(self, Self::GitLabCI)
    }
}

impl Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use Environment::*;

        write!(
            f,
            "{}",
            match self {
                GitHubActions => "GitHub Actions",
                GitLabCI => "GitLab CI",
                Other => "an unspecified environment",
            }
        )
    }
}

fn env_var_is_true(e: &str) -> bool {
    std::env::var(e).is_ok_and(|v| v == "true")
}

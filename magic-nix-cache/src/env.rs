use crate::error::Error;
use std::env;

pub enum Environment {
    GitHubActions,
    GitLabCI,
    Other,
}

impl Environment {
    pub fn is_github_actions(&self) -> bool {
        matches!(self, Self::GitHubActions)
    }

    pub fn is_gitlab_ci(&self) -> bool {
        matches!(self, Self::GitLabCI)
    }
}

impl ToString for Environment {
    fn to_string(&self) -> String {
        use Environment::*;

        String::from(match self {
            GitHubActions => "GitHub Actions",
            GitLabCI => "GitLab CI",
            _ => "an unspecified environment",
        })
    }
}

pub fn determine_environment() -> Environment {
    if env_var_is_true("GITHUB_ACTIONS") {
        Environment::GitHubActions
    }

    if env_var_is_true("CI") && env_var_is_true("GITLAB_CI") {
        Environment::GitLabCI
    }

    Environment::Other
}

fn env_var_is_true(e: &str) -> bool {
    env::var(e).unwrap_or(String::from("")) == String::from("true")
}

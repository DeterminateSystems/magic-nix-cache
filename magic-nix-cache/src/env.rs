use std::{
    env,
    fmt::{self, Display},
};

#[derive(Clone)]
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

pub fn determine_environment() -> Environment {
    if env_var_is_true("GITHUB_ACTIONS") {
        return Environment::GitHubActions;
    }

    if env_var_is_true("CI") && env_var_is_true("GITLAB_CI") {
        return Environment::GitLabCI;
    }

    Environment::Other
}

fn env_var_is_true(e: &str) -> bool {
    &env::var(e).unwrap_or(String::from("")) == "true"
}

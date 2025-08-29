use serde::{Deserialize, Serialize};

const GITHUB_ACTOR_TYPE_USER: &str = "User";
const GITHUB_ACTOR_TYPE_ORGANIZATION: &str = "Organization";

#[derive(Serialize, Deserialize)]
pub struct WorkflowData {
    event: WorkflowDataEvent,
}

#[derive(Serialize, Deserialize)]
pub struct WorkflowDataEvent {
    repository: WorkflowDataEventRepo,
}

#[derive(Serialize, Deserialize)]
pub struct WorkflowDataEventRepo {
    owner: WorkflowDataEventRepoOwner,
}

#[derive(Serialize, Deserialize)]
pub struct WorkflowDataEventRepoOwner {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

pub(crate) fn get_actions_event_data() -> color_eyre::Result<WorkflowData> {
    let github_context = std::env::var("GITHUB_CONTEXT")?;
    let workflow_data: WorkflowData = serde_json::from_str::<WorkflowData>(&github_context)?;

    Ok(workflow_data)
}

pub(crate) fn print_unauthenticated_error() {
    let mut msg = "::error title=FlakeHub registration required.::Unable to authenticate to FlakeHub. Individuals must register at FlakeHub.com; Organizations must create an organization at FlakeHub.com.".to_string();
    if let Ok(workflow_data) = get_actions_event_data() {
        let owner = workflow_data.event.repository.owner;
        if owner.kind == GITHUB_ACTOR_TYPE_USER {
            msg = format!(
                "::error title=FlakeHub registration required.::Please create an account for {} on FlakeHub.com to publish flakes.",
                &owner.login
            );
        } else if owner.kind == GITHUB_ACTOR_TYPE_ORGANIZATION {
            msg = format!(
                "::error title=FlakeHub registration required.::Please create an organization for {} on FlakeHub.com to publish flakes.",
                &owner.login
            );
        }
    };
    println!("{msg}");
}

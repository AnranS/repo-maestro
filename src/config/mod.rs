pub mod agent_profile;
pub mod agent_resolver;
pub mod analyze;
pub mod channels;
pub mod deliberate;
pub mod discovery;
pub mod plan;
pub mod projects;
pub mod settings;

pub use agent_profile::{AgentProfile, ProfileOutput, ProfileTrigger, TriggerStage};
pub use agent_resolver::{
    resolve_reviewer, resolve_writer, trigger_matches, MatchFacts, ProfileSource, ResolvedReviewer,
    ResolvedWriter, ReviewerSource, WriterInputs,
};
pub use analyze::{
    analyze, architecture_brief, detect_contract_drift, plan_impact, wire_contract_dependencies,
    AnalyzeReport, ContractDrift, Finding, TaskImpact, WiredEdge,
};
pub use deliberate::{
    deliberate, position_message, render_transcript, Conflict, DeliberationReport, Position,
};
pub use discovery::{
    discover, merge_project_from_discovery, project_from_discovery, promote_contracts,
    promote_contracts_with_siblings, render_plan as render_discovery_plan, resolve_sibling_roots,
    DiscoverOptions, DiscoveredEdge, DiscoveredProject, DiscoveryReport,
};
pub use plan::{
    parse_output_ref, Acceptance, Goal, Plan, PlanTask, TaskInput, TaskKind, TaskOutput,
};
pub use projects::{Contracts, Defaults, Project, ProjectsConfig};
pub use settings::{ChatSettings, Settings};

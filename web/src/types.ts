export type TaskStatus =
  | "pending"
  | "running"
  | "awaiting_approval"
  | "done"
  | "failed"
  | "skipped"
  | "cancelled"

export interface Artifacts {
  pr_url?: string
  branch?: string
  files_changed?: string[]
}

export interface Usage {
  input_tokens: number
  output_tokens: number
  cost_usd?: number | null
  model?: string | null
}

export interface TaskState {
  id: string
  project: string
  agent: string
  status: TaskStatus
  started_at?: string
  ended_at?: string
  chat_id?: string
  error?: string
  attempts?: number
  /** Change-risk classified from this task's diff once it completed. */
  risk_level?: string | null
  artifacts?: Artifacts
  log_path: string
  depends_on: string[]
  parallel_group?: string | null
  requires_approval_after: boolean
  kind: "agent" | "verify" | string
  memory_used?: string[]
  skills_triggered?: string[]
  usage?: Usage | null
  steps?: number | null
  role?: string | null
  resolved_agent_profile?: string | null
  resolved_review_profile?: string | null
  workspace_path?: string | null
  worktree_path?: string | null
}

export interface Acceptance {
  describe: string
  check: string
}

export interface Goal {
  description?: string
  acceptance?: Acceptance[]
}

export interface AcceptanceResult {
  describe: string
  check: string
  passed: boolean
  exit_code?: number | null
  output?: string
  started_at: string
  ended_at: string
}

/** A pending task's change + risk verdict, for the approval review card. */
export interface TaskDiff {
  diff: string
  files: { status: string; path: string }[]
  risk: { level: "high" | "low" | string; reasons: string[] }
}

export interface TaskTrajectory {
  task: string
  total_steps: number
  buckets: Record<string, number>
  tokens?: { input_tokens: number; output_tokens: number } | null
  steps: { seq: number; ts: string; command: string; bucket: string; status?: string | null }[]
  truncated: boolean
}

export interface RunOutcomeTask {
  task: string
  project: string
  files: { status: string; path: string }[]
  diff: string
  risk: { level: "high" | "low" | string; reasons: string[] }
}

export interface ContractDrift {
  contract: string
  producer: string
  consumer: string
  reason: string
}

export interface RunOutcome {
  goal?: Goal | null
  acceptance_results?: AcceptanceResult[]
  verified: boolean
  status: string
  total_files: number
  risk: { level: "high" | "low" | string; reasons: string[] }
  tasks: RunOutcomeTask[]
  drift?: ContractDrift[]
}

export interface RunState {
  run_id: string
  spec: string
  started_at: string
  ended_at?: string | null
  status: "running" | "done" | "failed" | "cancelled" | string
  max_parallel: number
  tasks: Record<string, TaskState>
  approvals_pending: string[]
  task_order: string[]
  session_id?: string | null
  usage?: Usage | null
  budget_tokens?: number | null
  pending_gate?: "plan" | "outcome" | string | null
  goal?: Goal | null
  acceptance_results?: AcceptanceResult[]
  verified?: boolean
  auto_actions?: AutoAction[]
}

/** A decision maestro made on the user's behalf during a run. */
export interface AutoAction {
  kind: "contract_wired" | "retry" | "circuit_break" | "integration_conflict" | string
  task?: string | null
  detail: string
}

export interface RunSummary {
  run_id: string
  spec: string
  status: string
  started_at: string
  total_tokens?: number
  cost_usd?: number | null
  budget_tokens?: number | null
}

/** F-112: read-only run monitor projection. */
export interface RunProgress {
  total: number
  done: number
  failed: number
  running: number
  pending: number
  awaiting_approval: number
  skipped: number
  cancelled: number
  settled: number
}

export interface MonitorTaskRef {
  task_id: string
  project: string
  status: TaskStatus | string
  kind: string
  title?: string | null
  resolved_agent_profile?: string | null
  resolved_review_profile?: string | null
  risk_level?: string | null
}

export interface FindingSummary {
  kind: string
  severity: string
  count: number
}

/** F-123: read-only review-gate projection. v1 only ever has status "pending". */
export type GateScope = "run" | "task"
export type GateKind = "plan" | "outcome" | "task_approval"
export type GateStatus = "pending" | "approved" | "rejected" | "request_changes"

export interface GateEvidence {
  // plan
  project_count?: number | null
  task_count?: number | null
  dependency_count?: number | null
  // outcome
  verified?: boolean | null
  acceptance_passed?: number | null
  acceptance_total?: number | null
  // task approval
  risk_level?: string | null
  findings_count?: number | null
}

export interface ReviewGate {
  gate_id: string
  scope: GateScope
  kind: GateKind
  status: GateStatus
  task_id?: string | null
  summary: string
  evidence: GateEvidence
}

export interface RunMonitor {
  schema_version: "maestro.run_monitor.v1" | string
  run_id: string
  status: RunState["status"]
  spec: string
  started_at: string
  ended_at?: string | null
  progress: RunProgress
  active_tasks: MonitorTaskRef[]
  blocked_tasks: MonitorTaskRef[]
  approvals_pending: MonitorTaskRef[]
  findings_summary: FindingSummary[]
  /** F-123: currently-pending review gates (always present, possibly empty). */
  gates: ReviewGate[]
  usage: Usage
  budget_tokens?: number | null
  updated_at?: string | null
}

export interface TaskEvidence {
  id: string
  project: string
  agent: string
  kind: string
  status: string
  started_at?: string | null
  ended_at?: string | null
  duration_ms?: number | null
  workspace_path?: string | null
  worktree_path?: string | null
  log_path: string
}

export interface ParallelWindow {
  started_at: string
  ended_at: string
  concurrency: number
  task_ids: string[]
  projects: string[]
}

export interface AcceptanceEvidence {
  describe: string
  check: string
  passed: boolean
  exit_code?: number | null
  output_excerpt?: string
  started_at: string
  ended_at: string
}

export interface BrowserCheckEvidence {
  describe: string
  check: string
  passed: boolean
  browser_related: boolean
  evidence_path?: string | null
}

export interface BrowserEvidenceSummary {
  present: boolean
  summary_path?: string | null
  artifact_count: number
  checks?: BrowserCheckEvidence[]
  screenshots?: string[]
  traces?: string[]
  dom_snapshots?: string[]
  videos?: string[]
  network_failures?: string[]
  console_errors?: string[]
}

export interface RunEvidence {
  run_id: string
  spec: string
  status: string
  verified: boolean
  max_parallel: number
  max_observed_parallelism: number
  task_count: number
  tasks: TaskEvidence[]
  parallel_windows: ParallelWindow[]
  acceptance: AcceptanceEvidence[]
  browser: BrowserEvidenceSummary
}

/** One record in a run's finding ledger (F-110). */
export interface Finding {
  schema_version: string
  finding_id: string
  run_id: string
  seq: number
  task_id?: string
  kind: "risk" | "refute" | "approval" | "learn" | "doctor" | "channel"
  severity: "info" | "low" | "medium" | "high" | "critical"
  confidence?: number
  summary: string
  evidence_refs: string[]
  source: string
  status: "open"
  created_at: string
  provenance?: {
    producer: string
    producer_version?: string
    inputs_digest?: string
  }
}

export type TaskApprovalState = "pending" | "approved" | "rejected"

export interface TaskArtifactSummary {
  kind: string
  path?: string | null
  available: boolean
}

/** F-112: read-only task-detail projection. */
/** F-125: read-only per-node tool-policy projection. */
export type ArtifactSource = "agent_task" | "hook" | "verification" | "external" | "user"

export interface ArtifactRef {
  kind: string
  source: ArtifactSource
  task_id?: string | null
  path?: string | null
  uri?: string | null
  name?: string | null
  bytes?: number | null
}

export type Enforcement = "hard" | "soft" | "unsupported" | "not_applicable"
export type ToolPolicyStatus = "present" | "absent"

export interface CapabilityPolicy {
  name: string
  requested: boolean
  enforcement: Enforcement
  allowed_commands?: string[]
}

export interface RetryPolicy {
  attempts: number
  max_retries?: number | null
  idempotency: string
}

export interface TaskToolPolicy {
  status: ToolPolicyStatus
  capabilities: CapabilityPolicy[]
  declared_effects: string[]
  observed_effects: string[]
  retry: RetryPolicy
  required_evidence: ArtifactRef[]
  audit_gaps: string[]
  pending_gate_id?: string | null
}

// F-136a1: read-only RuntimeProfile safety label. `unknown` / `requires_review` are
// explicit NON-safe states — never render them as "allow".
export type RuntimeProfile =
  | "review_only"
  | "write_local"
  | "network_allowlisted"
  | "tool_limited"
  | "high_risk_vm"
  | "requires_review"
  | "unknown"

export interface RuntimeProfileView {
  profile: RuntimeProfile
  reasons: string[]
  /** REQUESTED capabilities only `soft`-enforced — advisory, NOT hard-blocked. */
  advisory: string[]
  /** REQUESTED capabilities with `unsupported` enforcement (unknown provider). */
  unsupported: string[]
}

export interface RuntimeProfileCount {
  profile: RuntimeProfile
  count: number
}

export interface RuntimeProfileSummary {
  worst: RuntimeProfile
  counts: RuntimeProfileCount[]
  task_total: number
}

export interface TaskDetail {
  schema_version: "maestro.task_detail.v1" | string
  run_id: string
  task_id: string
  project: string
  status: TaskStatus | string
  kind: string
  agent: string
  role?: string | null
  resolved_agent_profile?: string | null
  resolved_review_profile?: string | null
  depends_on: string[]
  downstream: string[]
  risk_level?: string | null
  attempts: number
  started_at?: string | null
  ended_at?: string | null
  approval?: TaskApprovalState | null
  artifacts: TaskArtifactSummary[]
  findings: Finding[]
  last_error?: string | null
  /** F-125: always present; `status: "absent"` means no permission evidence. */
  tool_policy: TaskToolPolicy
  /** F-136a1: always present; absent permission projects as `unknown`/`requires_review`. */
  runtime_profile: RuntimeProfileView
}

// F-116: read-only prompt context-layer manifest (provenance + size only).
export interface ContextLayerRef {
  kind: string
  ref: string
  count?: number
}

export interface ContextLayer {
  order: number
  id: string
  kind: string
  label: string
  source: string
  item_count: number
  content_bytes: number
  estimated_tokens: number
  truncated?: boolean
  omitted?: boolean
  omitted_reason?: string
  refs: ContextLayerRef[]
}

export interface TaskContextManifest {
  schema_version: string
  run_id: string
  task_id: string
  project: string
  kind: string
  agent: string
  model?: string
  role?: string
  resolved_agent_profile?: string
  resolved_review_profile?: string
  total_context_bytes: number
  estimated_input_tokens: number
  layers: ContextLayer[]
  created_at: string
}

export interface ReplayEvent {
  seq: number
  timestamp: string
  kind: string
  task_id?: string | null
  message?: string | null
}

export interface ReplayTask {
  id: string
  project: string
  status: string
  depends_on?: string[]
  started_at?: string | null
  ended_at?: string | null
  duration_ms?: number | null
}

export interface RunReplay {
  run_id: string
  status: string
  event_count: number
  max_observed_parallelism: number
  events: ReplayEvent[]
  tasks: ReplayTask[]
}

// F-120: live typed run-event stream frames. The SSE stream emits `run_event`
// (one RunEvent) and, in balanced mode, `run_event_gap` (a shed activity span).
// Only short, validated fields are surfaced in the UI — never raw payload/refs.
export interface RunEventWire {
  schema_version: string
  event_id: string
  run_id: string
  seq: number
  timestamp: string
  kind: string
  task_id?: string | null
  status?: string | null
  severity?: string | null
  message?: string | null
  display?: { label: string; tone?: string | null; icon?: string | null } | null
}

export interface RunEventGapWire {
  schema_version: string
  run_id: string
  from_seq: number
  to_seq: number
  count: number
  reason: string
  delivery: string
  classes: string[]
}

export interface ActionArgs {
  [k: string]: string
}

export type ActionVerb =
  | "run"
  | "approve"
  | "status"
  | "rerun"
  | "plan_validate"
  | "work"

export type ActionStatus =
  | "pending"
  | "approved"
  | "rejected"
  | "running"
  | "done"
  | "failed"

export interface Action {
  id: string
  verb: ActionVerb
  args: ActionArgs
  status?: ActionStatus | null
  label: string
  output?: string | null
}

export interface Message {
  id: string
  role: "user" | "assistant" | "system"
  content: string
  timestamp: string
  actions?: Action[]
  /** Reasoning/thinking trace from models that emit one (Claude
   *  `-thinking-*`, cursor reasoning). Rendered collapsibly under the answer. */
  thinking?: string
}

export interface Session {
  id: string
  title: string
  created_at: string
  updated_at: string
  cursor_chat_id?: string | null
  messages: Message[]
  tags?: string[]
  cursor_model?: string | null
  chat_provider?: string | null
}

export interface ModelInfo {
  id: string
  label?: string | null
  aliases?: string[]
  provider?: string | null
}

export interface DefaultsConfig {
  agent: string
  branch_prefix: string
  max_parallel: number
  agent_model?: string | null
  cursor_model?: string | null
  tagger_model?: string | null
  // F-136a2: GET returns these; they are READ-ONLY in the web (not in PUT). The
  // Settings page DISPLAYS them — making them editable is a later cut.
  max_total_tasks?: number
  gate_on_high_risk?: boolean
  gate_on_policy_violation?: boolean
  refute_on_high_risk?: boolean
  auto_pr?: boolean
}

// F-136a2: read-only per-provider enforcement matrix (GET /api/providers/profiles),
// projected from Rust provider_permission_profile. `soft` is advisory (not
// hard-blocked); `unsupported` is an unknown provider (fail-closed).
export interface ProviderEnforcementProfile {
  provider_id: string
  shell: Enforcement
  git_write: Enforcement
  network: Enforcement
  fs_write: Enforcement
  external_dir: Enforcement
  mcp: Enforcement
}

export interface ArchModuleView {
  name: string
  type?: string | null
  stack: string[]
  path: string
  provides?: string | null
  consumes?: string | null
  memory_scope: string[]
}

export interface ArchEdgeView {
  from: string
  to: string
  file: string
  kind?: "contract" | "dependency" | "dependency+contract" | "import" | "import+contract" | string
  confidence?: number | null
  /** True when discovery inferred the edge from source imports (not declared). */
  inferred?: boolean
  /** Cross-module import count behind a code-graph-rolled edge (thicker = stronger). */
  weight?: number
  evidence?: string[]
}

export interface ArchitectureView {
  modules: ArchModuleView[]
  edges: ArchEdgeView[]
}

export interface ProjectMemoryView {
  name: string
  type?: string | null
  stack: string[]
  role?: string | null
  memory_scope: string[]
  path: string
  contracts: { provides?: string | null; consumes?: string | null }
  l1_facts: { topic: string; file: string; preview: string }[]
  l2_decisions: { file: string; bytes: number; preview: string }[]
  recent_runs: {
    run_id: string
    spec: string
    status: string
    started_at: string
    verified: boolean
    task_ids_in_project: string[]
  }[]
}

export interface AddProjectBody {
  name: string
  path: string
  type?: string
  stack?: string[]
  agent?: string
  memory_scope?: string[]
  provides?: string
  consumes?: string
  dependencies?: string[]
  agent_model?: string
  cursor_model?: string
}

export interface SessionMeta {
  id: string
  title: string
  created_at: string
  updated_at: string
  message_count: number
  is_current: boolean
  tags: string[]
}

export interface ExternalSession {
  source: string
  workspace_id: string
  id: string
  path: string
  modified_at?: string | null
  bytes: number
}

export interface ExternalSummary {
  source: string
  session_count: number
  workspace_count: number
}

export interface ExternalListing {
  summaries: ExternalSummary[]
  sessions: ExternalSession[]
}

export type SkillScope =
  | { kind: "global" }
  | { kind: "project"; value: string }

export interface Skill {
  name: string
  scope: SkillScope
  description?: string | null
  trigger?: string | null
  content: string
  path: string
}

export type SkillsByScope = Record<string, Omit<Skill, "content">[]>

// ─── F-121 skill inventory (maestro.skill_inventory.v1) — metadata only ───

export type IssueSeverity = "info" | "warning" | "error"
export type RefResolution = "resolved" | "missing" | "invalid" | "deferred"

export interface InventoryIssue {
  code: string
  severity: IssueSeverity
  scope?: string | null
  skill?: string | null
  message: string
  suggestions?: string[]
}

export interface SkillInventorySummary {
  scope_count: number
  skill_count: number
  visible_count: number
  profile_ref_count: number
  missing_ref_count: number
  skipped_count: number
  truncated_count: number
}

export interface SkillDescriptor {
  scope: string
  name: string
  declared_name?: string | null
  description?: string | null
  trigger?: string | null
  source_ref: string
  file_bytes: number
  frontmatter_bytes_read: number
  truncated: boolean
  issues?: InventoryIssue[]
}

export interface ProfileSkillRef {
  ref: string
  resolution: RefResolution
  resolved_scope?: string | null
  resolved_name?: string | null
  reason?: string | null
}

export interface ProfileSkillResolution {
  profile: string
  enabled: boolean
  role?: string | null
  model_profile?: string | null
  declared_skills: ProfileSkillRef[]
}

export interface SkillInventory {
  schema_version: string
  project?: string | null
  profile?: string | null
  summary: SkillInventorySummary
  visible_skills: SkillDescriptor[]
  profile_resolution?: ProfileSkillResolution | null
  budget: Record<string, number>
  issues?: InventoryIssue[]
}

export type MemoryIndex = Record<string, string[]>

export interface DocsPage {
  id: string
  title: string
  file: string
}

export interface DocsGroup {
  id: string
  title: string
  pages: DocsPage[]
}

export interface DocsIndex {
  version: number
  lang: string
  languages: string[]
  title: string
  description?: string
  groups: DocsGroup[]
}

export type FsEntryKind = "dir" | "file" | "symlink" | "other"

export interface FsEntry {
  name: string
  kind: FsEntryKind
  hidden: boolean
}

export interface FsListing {
  path: string
  parent: string | null
  home: string | null
  workspace_root: string | null
  entries: FsEntry[]
  truncated: boolean
}

/** File-level code knowledge graph (from codegraph). */
export interface CodeGraph {
  nodes: CodeFileNode[]
  edges: CodeFileEdge[]
  root?: string | null
  available: boolean
  /** "native" | "codegraph" | "understand-anything" */
  source?: string
  layers?: CodeLayer[]
  tour?: CodeTourStep[]
}
export interface CodeGraphEngine {
  engine: string
  label: string
  installed: boolean
  built: boolean
  active: boolean
  cost: string
  hint: string
}
export interface CodeGraphBuildResult {
  ok: boolean
  engine?: string
  manual?: boolean
  command?: string
  note?: string
}
export interface CodeFileNode {
  path: string
  language?: string | null
  symbols: number
  /** Understand-Anything enrichment */
  summary?: string | null
  tags?: string[]
  complexity?: string | null
  layer?: string | null
}
export interface CodeLayer {
  id: string
  name: string
  description?: string | null
  files: string[]
}
export interface CodeTourStep {
  order: number
  title: string
  description: string
  files?: string[]
  language_lesson?: string | null
}
export interface CodeFileEdge {
  source: string
  target: string
  kind: string
}
export interface CodeSymbol {
  name: string
  kind: string
  line?: number | null
  signature?: string | null
}

/** Full content of one memory item (white-box view). */
export interface MemoryItemDetail {
  id: string
  kind: string
  title: string
  content: string
  path?: string | null
  editable: boolean
}

/** Knowledge graph over L2 decisions: project + run nodes. */
export interface MemoryGraph {
  nodes: MemGraphNode[]
  edges: MemGraphEdge[]
}
export interface MemGraphNode {
  id: string
  kind: "project" | "run" | string
  label: string
  status?: string | null
  run_id?: string | null
  /** project: decision count; run: projects touched. */
  weight: number
}
export interface MemGraphEdge {
  source: string
  target: string
  kind: "produced" | "consumes" | string
}

/** Decision-level star map: stars clustered into project systems, stitched by
 * run constellations, with contract gravity. */
export interface StarMap {
  stars: Star[]
  projects: StarProject[]
  runs: Constellation[]
  contracts: ContractLink[]
}
export interface Star {
  id: string
  project: string
  run_id?: string | null
  title: string
  status?: string | null
  ts?: string | null
  /** 0..1, 1 = newest. Drives brightness. */
  recency: number
  /** projects the producing run touched. Drives size. */
  blast: number
}
export interface StarProject {
  name: string
  stars: number
}
export interface Constellation {
  run_id: string
  spec: string
  status?: string | null
  star_ids: string[]
}
export interface ContractLink {
  from: string
  to: string
}

/** A recalled memory item (L1 fact / L2 decision / replan), ranked by TF-IDF. */
export interface MemoryHit {
  id: string
  kind: "fact" | "decision" | "replan" | string
  project?: string | null
  title: string
  excerpt: string
  score: number
  updated_ms?: number | null
}

/** One agent-to-agent coordination message in the local mailbox. */
export interface MailMessage {
  id: string
  created_at: string
  updated_at: string
  from: string
  to: string
  project?: string | null
  task?: string | null
  subject: string
  body: string
  blocking: boolean
  status: "open" | "resolved"
  resolution?: string | null
  /** When present, a structured question to answer (rendered as buttons). */
  ask?: Ask | null
  answer?: string[] | null
}

/** A structured human-decision prompt (mirrors src/ask). */
export interface Ask {
  prompt: string
  options: AskOption[]
  multi_select?: boolean
  default: string[]
}
export interface AskOption {
  key: string
  label: string
}

// ─── F-118 runtime health (maestro.runtime_health.v1) ───────────────────
export type HealthStatus = "pass" | "warn" | "fail" | "skip"
export type HealthSeverity = "info" | "low" | "medium" | "high"

export interface HealthRef {
  kind: string
  ref: string
}
export interface RuntimeHealthCheck {
  id: string
  label: string
  status: HealthStatus
  severity: HealthSeverity
  message: string
  fix?: string
  duration_ms?: number
  refs: HealthRef[]
}
export interface ProbeResult {
  status: HealthStatus
  duration_ms?: number
  timed_out: boolean
  message: string
}
export interface ProviderCapabilitySummary {
  model_override: boolean
  non_interactive: boolean
  streaming: boolean
  resume: boolean
  worktree_isolation: boolean
  tool_trace: string
}
export interface ProviderHealth {
  id: string
  display: string
  kind: string
  adapter_available: boolean
  installed: boolean
  probe: ProbeResult
  capabilities: ProviderCapabilitySummary
  message: string
}
export interface RuntimeHealthSummary {
  total: number
  passed: number
  warnings: number
  failed: number
  skipped: number
  overall: HealthStatus
  top_issue?: string
}
export interface RuntimeHealthReport {
  schema_version: string
  generated_at: string
  summary: RuntimeHealthSummary
  checks: RuntimeHealthCheck[]
  providers: ProviderHealth[]
}

// ─── F-128 Delivery Web UI (read-only projection; mirrors src/schema/delivery.rs) ───

export type DeliveryStage =
  | "intake"
  | "clarify"
  | "spec"
  | "plan"
  | "execute"
  | "accept"
  | "closeout"
  | "rejected"
  | "parked"
  | "duplicate"
  | "changes_requested"
  | "cancelled"

export type AcceptVerdict = "pending" | "accepted" | "changes_requested" | "partial" | "rejected"

export type WritebackStatus = "skipped" | "intent_emitted" | "posted" | "failed"

// F-134: the external Feishu/doc post receipt — refs only (never the doc body).
export interface WritebackReceipt {
  message_ref?: string | null
  doc_revision?: string | null
  error?: string | null
  at: string
}

// `blocked_on` is a serde externally-tagged enum: unit variants serialize to a
// string, the struct variant to an object. Match that shape exactly.
export type BlockedOn =
  | "spec_confirm"
  | "pm_accept"
  | "nothing"
  | { open_clarify_questions: { count: number } }

export interface DeliveryRef {
  kind: string
  path?: string | null
  uri?: string | null
  name?: string | null
}

export interface DeliveryView {
  schema_version: string
  delivery_id: string
  stage: DeliveryStage
  blocked_on: BlockedOn
  source_refs: DeliveryRef[]
  run_id?: string | null
  plan_path?: string | null
  accept_verdict?: AcceptVerdict | null
  pm_accepted_by?: string | null
  closeout_summary?: string | null
  writeback_status?: WritebackStatus | null
  /** F-134: the external post receipt (message/doc ref or error) — refs only. */
  writeback_receipt?: WritebackReceipt | null
  /** F-136a1: worst-case RuntimeProfile + per-profile breakdown across the linked
   *  run's tasks. `null` when no run is linked yet (never fabricated). */
  runtime_profile_summary?: RuntimeProfileSummary | null
  /** F-133: current rework round (1-based) + how many prior rounds were superseded. */
  round: number
  superseded_count: number
}

// One row of `GET /api/deliveries` — an envelope: the projected view, or a
// visible corrupt stub (never silently dropped).
export type DeliveryListEntry =
  | { status: "ok"; delivery_id: string; view: DeliveryView }
  | { status: "corrupt"; delivery_id: string; error: string }

// F-131: slim live run status for the Delivery detail (GET /api/deliveries/:id/run-status).
// `linked` = resolved from execute.run_id (true) vs the in-flight RunState.delivery_id
// back-ref (false — a detached run not yet linked back). `run: null` = genuinely no run.
export interface RunStatusProgress {
  total: number
  done: number
  failed: number
  running: number
  pending: number
}
export interface RunStatusLite {
  run_id: string
  status: "running" | "done" | "failed" | "cancelled" | string
  linked: boolean
  progress: RunStatusProgress
  started_at: string
  ended_at?: string | null
}

// F-132: Delivery audit timeline (GET /api/deliveries/:id/timeline). A refs-first
// projection of DeliverySpec.audit — ids/refs/uris/summaries only, no copied bodies.
export interface TimelineRef {
  kind: string
  value?: string | null
  uri?: string | null
  summary?: string | null
}
export interface TimelineEvent {
  at: string
  stage: DeliveryStage
  by?: string | null
  reason?: string | null
  refs?: TimelineRef[]
}

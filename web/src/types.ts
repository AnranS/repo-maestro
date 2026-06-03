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

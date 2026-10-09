// The focus outlines that read the accent directly, each with the ground it is
// drawn on. A ring on a surface that stays dark in both themes must read
// `--ring` instead (ringConsumers.walk.test.ts); the ones listed here sit on
// grounds that follow the theme, and focusRingGrounds.walk.test.ts holds each
// to 3:1 against that ground in both themes. `match` is the rule's own
// selector, or its first selector when the rule groups several.

export interface DirectRingGround {
  file: string;
  match: string;
  ground: string;
  /** The opaque surface a translucent `ground` is drawn on: the walk
   * composites the two before reading the ratio. Omit when the ground is
   * opaque. */
  over?: string;
}

const card = "--panel-card";
const centre = "--ground-center";
const side = "--panel-side";
const composer = "--panel-composer";
const menu = "--panel-menu";

export const DIRECT_RING_GROUNDS: readonly DirectRingGround[] = [
  {
    file: "src/components/PermissionCard.css",
    match: ".permission-card-verbose-trigger:focus-visible",
    ground: card,
  },
  {
    file: "src/components/PermissionCard.css",
    match: ".permission-card-primary-action:focus-visible",
    ground: card,
  },
  {
    file: "src/components/PermissionCard.css",
    match: ".permission-card-question-option:has(:focus-visible)",
    ground: card,
  },
  {
    file: "src/components/PickerChip.css",
    match: ".workspace-mode-chip-trigger:focus-visible",
    ground: composer,
  },
  { file: "src/features/design/design.css", match: ".design-history-popover:focus", ground: menu },
  { file: "src/features/design/design.css", match: ".design-history-open:hover", ground: menu },
  { file: "src/features/design/design.css", match: ".design-canvas-node:hover", ground: centre },
  {
    file: "src/features/design/design.css",
    match: ".design-canvas-artifact-selected",
    ground: centre,
  },
  {
    file: "src/features/design/design.css",
    match: ".design-skill-mode-control:focus-visible",
    ground: card,
  },
  {
    file: "src/features/design/design.css",
    match: ".design-craft-title-row input:focus-visible",
    ground: card,
  },
  {
    file: "src/features/design/design.css",
    match: ".design-canvas-section-overlay:focus-visible",
    ground: centre,
  },
  { file: "src/features/oracle/oracle.css", match: ".oracle-search:focus-within", ground: side },
  { file: "src/features/oracle/oracle.css", match: ".oracle-result:focus-visible", ground: card },
  { file: "src/features/settings/devices.css", match: ".dev-kebab:focus-visible", ground: card },
  {
    file: "src/features/settings/settingsSwitch.css",
    match: ".settings-switch:focus-visible",
    ground: card,
  },
  {
    file: "src/features/settings/general.css",
    match: ".machine-segment-option:focus-within",
    ground: card,
  },
  { file: "src/features/settings/providers.css", match: ".prov-chev:focus-visible", ground: card },
  {
    file: "src/features/settings/settings.css",
    match: ".settings-back-row:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/AgentTaskPill.css",
    match: ".agent-task-pill-head:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/BackgroundTasksPill.css",
    match: ".background-tasks-pill:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/TasksPanel.css",
    match: ".tasks-panel-open:focus-visible",
    ground: side,
  },
  {
    file: "src/features/workspace/QueueTrack.css",
    match: ".workspace-queue-row:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/QueueTrack.css",
    match: ".workspace-queue-edit:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/Workspace.css",
    match: ".workspace-chat-thought-trigger:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/Workspace.css",
    match: ".workspace-command-option:focus-visible",
    ground: menu,
  },
  {
    file: "src/features/workspace/Workspace.css",
    match: ".workspace-composer .workspace-send-action:focus-visible",
    ground: "--composer-fill",
    over: centre,
  },
  {
    file: "src/features/workspace/Workspace.css",
    match: ".workspace-composer-preview-remove:focus-visible",
    ground: "--composer-fill",
    over: centre,
  },
  {
    file: "src/features/workspace/Workspace.css",
    match: ".workspace-composer-file-remove:focus-visible",
    ground: "--composer-fill",
    over: centre,
  },
  {
    file: "src/features/workspace/Workspace.css",
    match: ".workspace-composer.is-drop-target",
    ground: centre,
  },
  {
    file: "src/features/workspace/paneHeader/GoalLine.css",
    match: ".goal-line-chevron:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/panel/panel.css",
    match: ".workspace-panel-kebab-active",
    ground: side,
  },
  {
    file: "src/features/workspace/sidebar/sidebar.css",
    match: ".sidebar-foot:focus-visible",
    ground: side,
  },
  {
    file: "src/features/workspace/sidebar/sidebar.css",
    match: ".workspace-row-selected",
    ground: "--fill-selected-soft",
  },
  {
    file: "src/features/workspace/strip/strip.css",
    match: ".workspace-session-tab-selected",
    ground: "--selection",
  },
  {
    file: "src/features/workspace/strip/strip.css",
    match: ".workspace-session-tab-multiselected",
    ground: "--fill-selected-soft",
  },
  {
    file: "src/features/workspace/strip/strip.css",
    match: "button.workspace-rate:focus-visible",
    ground: "--ground-app",
  },
  {
    file: "src/features/workspace/strip/strip.css",
    match: ".workspace-overview-option:focus-visible",
    ground: menu,
  },
  {
    file: "src/features/workspace/timeline/TurnFooter.css",
    match: ".turn-footer-detail-trigger:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/timeline/TurnRail.css",
    match: ".turn-rail-dot:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/timeline/timeline.css",
    match: ".timeline-copy-chip:focus-visible",
    ground: centre,
  },
  {
    file: "src/features/workspace/timeline/timeline.css",
    match: ".chat-image-lightbox-close:focus-visible",
    ground: card,
  },
];

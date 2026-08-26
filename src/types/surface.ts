export type SurfaceKey =
  | 'workspace'
  | 'polis'
  | 'pubvia'
  | 'design'
  | 'settings';

export type SurfaceTone = 'terracotta' | 'purple' | 'green' | 'ochre';

export interface SurfaceDefinition {
  key: SurfaceKey;
  label: string;
  eyebrow: string;
  description: string;
  tone: SurfaceTone;
}

export const SURFACES = [
  {
    key: 'workspace',
    label: 'Workspace',
    eyebrow: 'sessions · projects · worktrees',
    description: 'The workspace surface will host projects, sessions, terminals, and permissions.',
    tone: 'terracotta',
  },
  {
    key: 'polis',
    label: 'Polis',
    eyebrow: 'the codebase as a city',
    description: 'The Polis surface will mount the isometric codebase view from the v1 port.',
    tone: 'purple',
  },
  {
    key: 'pubvia',
    label: 'Pubvia',
    eyebrow: 'research writing',
    description: 'Pubvia is reserved for a later out-of-process plugin milestone.',
    tone: 'ochre',
  },
  {
    key: 'design',
    label: 'Design',
    eyebrow: 'minimal visual workspace',
    description: 'The Design surface will be rebuilt as a small, focused module.',
    tone: 'terracotta',
  },
  {
    key: 'settings',
    label: 'Settings',
    eyebrow: 'general · providers · devices',
    description: 'Settings will contain the daemon, provider, Oracle, and device controls.',
    tone: 'purple',
  },
] as const satisfies readonly SurfaceDefinition[];

// Kookaburra demo state — realistic workspace/tile content.
// Terminal cells are represented as arrays of lines; each line is an array of
// {t, c} spans where c is an ANSI color key from THEME.ansi.

const THEME = {
  bg: 'oklch(0.185 0.012 60)',
  bgDeep: 'oklch(0.145 0.010 60)',
  bgDim: 'oklch(0.215 0.012 60)',
  fg: 'oklch(0.94 0.008 85)',
  fgDim: 'oklch(0.68 0.012 85)',
  fgFaint: 'oklch(0.48 0.012 85)',
  accent: 'oklch(0.80 0.17 68)',      // kookaburra amber
  accentDeep: 'oklch(0.58 0.16 50)',  // darker beak
  teal: 'oklch(0.72 0.10 200)',       // worktree / cool
  green: 'oklch(0.78 0.16 145)',
  red: 'oklch(0.70 0.18 25)',
  magenta: 'oklch(0.72 0.16 330)',
  blue: 'oklch(0.72 0.14 245)',
  yellow: 'oklch(0.86 0.14 90)',
  gridLine: 'oklch(0.28 0.012 60)',
};

// ANSI palette keys → THEME entry
const ANSI = {
  d: THEME.fg,
  m: THEME.fgDim,
  f: THEME.fgFaint,
  a: THEME.accent,
  A: THEME.accentDeep,
  t: THEME.teal,
  g: THEME.green,
  r: THEME.red,
  y: THEME.yellow,
  b: THEME.blue,
  p: THEME.magenta,
};

// shorthand: L(...spans)  where each span is [text, colorKey]
const L = (...spans) => spans.map(([t, c]) => ({ t, c: c || 'd' }));
const plain = (s) => [{ t: s, c: 'd' }];
const dim = (s) => [{ t: s, c: 'm' }];

// ─── Tile payloads ──────────────────────────────────────────────

const claudeCodingTile = {
  title: 'claude · implementing auth middleware',
  cwd: '~/code/api',
  branch: 'claude/auth-mw-a7f',
  worktree: true,
  primary: true,
  running: 'claude',
  lines: [
    L(['● ', 'a'], ['Reading ', 'm'], ['src/middleware/auth.rs', 'b']),
    L(['  ', 'd'], ['└─ 412 lines, identified 3 hook points', 'f']),
    L(['', 'd']),
    L(['● ', 'a'], ['Writing ', 'm'], ['src/middleware/auth.rs', 'b']),
    L(['  ', 'd'], ['● ', 'g'], ['added ', 'm'], ['verify_jwt()', 'y'], [' at L34', 'f']),
    L(['  ', 'd'], ['● ', 'g'], ['added ', 'm'], ['extract_claims()', 'y'], [' at L58', 'f']),
    L(['  ', 'd'], ['○ ', 'y'], ['refactoring ', 'm'], ['middleware_chain()', 'y']),
    L(['', 'd']),
    L(['● ', 'a'], ['Running ', 'm'], ['cargo test --lib auth', 'b']),
    L(['   Compiling ', 'f'], ['api v0.3.2 ', 'm'], ['(', 'f'], ['~/code/api', 'b'], [')', 'f']),
    L(['    Finished ', 'g'], ['`test` profile in 3.84s', 'f']),
    L(['     Running ', 'f'], ['unittests src/lib.rs', 'm']),
    L(['', 'd']),
    L(['running 7 tests', 'm']),
    L(['test auth::tests::rejects_expired_token    ... ', 'm'], ['ok', 'g']),
    L(['test auth::tests::rejects_missing_header   ... ', 'm'], ['ok', 'g']),
    L(['test auth::tests::accepts_valid_hs256      ... ', 'm'], ['ok', 'g']),
    L(['test auth::tests::accepts_valid_rs256      ... ', 'm'], ['ok', 'g']),
    L(['test auth::tests::claims_round_trip        ... ', 'm'], ['ok', 'g']),
    L(['test auth::tests::refresh_token_rotation   ... ', 'm'], ['ok', 'g']),
    L(['test auth::tests::blocks_none_algorithm    ... ', 'm'], ['ok', 'g']),
    L(['', 'd']),
    L(['test result: ', 'm'], ['ok', 'g'], ['. 7 passed; 0 failed; 0 ignored', 'm']),
    L(['', 'd']),
    L(['● ', 'a'], ['All tests green. Writing the handoff note...', 'd']),
    L(['▊', 'a']),
  ],
  cursor: { row: 25, col: 1, blink: true },
};

const claudeThinkingTile = {
  title: 'claude · exploring zod vs valibot',
  cwd: '~/code/api',
  branch: 'claude/auth-mw-b22',
  worktree: true,
  running: 'claude',
  generating: true,
  lines: [
    L(['● ', 'a'], ['Thinking', 'm'], ['…', 'f']),
    L([' ', 'd']),
    L(['  The user wants to compare runtime validation libraries', 'f']),
    L(['  for the request body. Three candidates:', 'f']),
    L([' ', 'd']),
    L(['  1. ', 'a'], ['zod', 'y'], [' — mature, large bundle, great DX', 'f']),
    L(['  2. ', 'a'], ['valibot', 'y'], [' — tree-shakeable, smaller, newer', 'f']),
    L(['  3. ', 'a'], ['arktype', 'y'], [' — fastest but unfamiliar syntax', 'f']),
    L([' ', 'd']),
    L(['  Given the handoff constraint about bundle size,', 'f']),
    L(['  valibot looks strongest. Let me check their pars', 'f'], ['▊', 'a']),
  ],
  cursor: { row: 11, col: 50, blink: false },
};

const logsTile = {
  title: 'pnpm dev',
  cwd: '~/code/api',
  branch: null,
  worktree: false,
  running: 'pnpm',
  follow: true,
  lines: [
    L(['[', 'f'], ['10:42:18', 'm'], ['] ', 'f'], ['GET  /health', 'g'], ['           200  ', 'm'], ['2ms', 'f']),
    L(['[', 'f'], ['10:42:18', 'm'], ['] ', 'f'], ['GET  /api/session', 'g'], ['      200  ', 'm'], ['18ms', 'f']),
    L(['[', 'f'], ['10:42:19', 'm'], ['] ', 'f'], ['POST /api/auth/login', 'g'], ['   200  ', 'm'], ['124ms', 'f']),
    L(['[', 'f'], ['10:42:19', 'm'], ['] ', 'f'], ['GET  /api/me', 'g'], ['           200  ', 'm'], ['6ms', 'f']),
    L(['[', 'f'], ['10:42:20', 'm'], ['] ', 'f'], ['POST /api/items', 'y'], ['        422  ', 'm'], ['3ms', 'f']),
    L(['  ', 'd'], ['→ body missing "title"', 'f']),
    L(['[', 'f'], ['10:42:21', 'm'], ['] ', 'f'], ['POST /api/items', 'g'], ['        201  ', 'm'], ['42ms', 'f']),
    L(['[', 'f'], ['10:42:21', 'm'], ['] ', 'f'], ['GET  /api/items', 'g'], ['        200  ', 'm'], ['8ms', 'f']),
    L(['[', 'f'], ['10:42:22', 'm'], ['] ', 'f'], ['DELETE /api/items/42', 'g'], ['   204  ', 'm'], ['14ms', 'f']),
    L(['[', 'f'], ['10:42:23', 'm'], ['] ', 'f'], ['GET  /api/items', 'g'], ['        200  ', 'm'], ['7ms', 'f']),
    L(['[', 'f'], ['10:42:23', 'm'], ['] ', 'f'], ['POST /api/auth/logout', 'g'], ['  200  ', 'm'], ['11ms', 'f']),
    L(['[', 'f'], ['10:42:24', 'm'], ['] ', 'f'], ['GET  /health', 'g'], ['           200  ', 'm'], ['1ms', 'f']),
    L(['[', 'f'], ['10:42:24', 'm'], ['] ', 'f'], ['GET  /api/session', 'g'], ['      401  ', 'r'], ['3ms', 'f']),
    L(['[', 'f'], ['10:42:25', 'm'], ['] ', 'f'], ['POST /api/auth/refresh', 'g'], [' 200  ', 'm'], ['22ms', 'f']),
    L(['[', 'f'], ['10:42:26', 'm'], ['] ', 'f'], ['GET  /api/me', 'g'], ['           200  ', 'm'], ['5ms', 'f']),
    L(['[', 'f'], ['10:42:27', 'm'], ['] ', 'f'], ['GET  /api/items', 'g'], ['        200  ', 'm'], ['9ms', 'f']),
    L([' ', 'd']),
    L(['~ ', 'm'], ['watching', 'b'], [' for changes ', 'f'], ['▊', 'a']),
  ],
  cursor: { row: 17, col: 27, blink: true },
};

const gitTile = {
  title: 'git',
  cwd: '~/code/api',
  branch: 'main',
  worktree: false,
  running: 'zsh',
  lines: [
    L(['~/code/api', 'b'], [' ', 'd'], ['(main)', 'g'], [' $ ', 'a'], ['git worktree list', 'd']),
    L(['/Users/j/code/api                  a7f3c09 [main]', 'm']),
    L(['/Users/j/.kookaburra/wt/api-a7f    b2e4111 [claude/auth-mw-a7f]', 'm']),
    L(['/Users/j/.kookaburra/wt/api-b22    71cd890 [claude/auth-mw-b22]', 'm']),
    L(['/Users/j/.kookaburra/wt/api-c19    3fa2187 [claude/auth-mw-c19]', 'm']),
    L([' ', 'd']),
    L(['~/code/api', 'b'], [' ', 'd'], ['(main)', 'g'], [' $ ', 'a'], ['git log --oneline -10', 'd']),
    L(['a7f3c09', 'y'], [' (', 'm'], ['HEAD -> main', 'g'], [', ', 'm'], ['origin/main', 'r'], [') handoff: strip UX final', 'd']),
    L(['8fc2d40', 'y'], [' rename: mod.rs → lib.rs', 'd']),
    L(['1b90ae1', 'y'], [' feat: pty resize propagation', 'd']),
    L(['9fe7c22', 'y'], [' fix: glyphon atlas race on resize', 'd']),
    L(['d340a88', 'y'], [' chore: bump alacritty_terminal to 0.24', 'd']),
    L(['2cd1f89', 'y'], [' feat: tokyo night theme', 'd']),
    L(['55aa02e', 'y'], [' feat: 3x2 grid default layout', 'd']),
    L(['3ca9e02', 'y'], [' feat: egui strip integration', 'd']),
    L(['0d887f1', 'y'], [' chore: cargo workspace scaffold', 'd']),
    L(['ff01241', 'y'], [' init', 'd']),
    L([' ', 'd']),
    L(['~/code/api', 'b'], [' ', 'd'], ['(main)', 'g'], [' $ ', 'a'], ['▊', 'a']),
  ],
  cursor: { row: 20, col: 20, blink: true },
};

const htopTile = {
  title: 'htop',
  cwd: '~',
  branch: null,
  worktree: false,
  running: 'htop',
  lines: [
    L(['  1  ', 'm'], ['[', 'f'], ['|||||||||', 'g'], ['         34.2%] ', 'f'], ['  Tasks: ', 'm'], ['248', 'y'], [', 987 thr; ', 'f'], ['5', 'g'], [' running', 'f']),
    L(['  2  ', 'm'], ['[', 'f'], ['||||||||||||||| ', 'g'], ['  51.8%] ', 'f'], ['  Load: ', 'm'], ['1.42 1.18 1.02', 'y']),
    L(['  3  ', 'm'], ['[', 'f'], ['|||||           ', 'g'], ['  18.4%] ', 'f'], ['  Uptime: ', 'm'], ['3 days, 14:22', 'y']),
    L(['  4  ', 'm'], ['[', 'f'], ['||              ', 'g'], ['   6.1%] ', 'f'], [' ', 'd']),
    L(['  Mem', 'm'], ['[', 'f'], ['||||||||||||', 'a'], ['    ', 'f'], ['20.4G', 'y'], ['/', 'f'], ['32G', 'y'], ['] ', 'f']),
    L(['  Swp', 'm'], ['[', 'f'], ['                ', 'f'], ['0/0', 'm'], ['] ', 'f']),
    L([' ', 'd']),
    L([' PID   USER       CPU%  MEM%  TIME+    COMMAND', 'f']),
    L([' 47283 j           18.4  2.1   0:42.18  ', 'f'], ['claude', 'y'], [' code --resume', 'f']),
    L([' 47201 j           12.7  1.8   0:28.33  ', 'f'], ['node', 'g'], [' pnpm dev', 'f']),
    L([' 47192 j            8.2  0.9   0:11.04  ', 'f'], ['kookaburra', 'a']),
    L([' 46001 j            4.1  0.4   0:08.90  ', 'f'], ['cargo', 'y'], [' build', 'f']),
    L([' 17003 root         2.0  0.1   4:11.08  ', 'f'], ['WindowServer', 'm']),
    L([' 12004 j            1.2  3.2   2:22.44  ', 'f'], ['Slack', 'm']),
    L([' 11000 j            0.8  1.1   0:42.02  ', 'f'], ['zsh', 'm']),
    L([' 11001 j            0.4  0.2   0:01.18  ', 'f'], ['zsh', 'm']),
    L([' 11002 j            0.3  0.2   0:00.48  ', 'f'], ['zsh', 'm']),
    L([' ', 'd']),
    L(['F1', 'a'], ['Help ', 'f'], ['F2', 'a'], ['Setup ', 'f'], ['F3', 'a'], ['Search ', 'f'], ['F4', 'a'], ['Filter ', 'f'], ['F10', 'a'], ['Quit', 'f']),
  ],
  cursor: null,
};

const notesTile = {
  title: 'nvim · NOTES.md',
  cwd: '~/code/api',
  branch: null,
  worktree: false,
  running: 'nvim',
  lines: [
    L(['  1 ', 'f'], ['# Auth middleware — comparison', 'a']),
    L(['  2 ', 'f']),
    L(['  3 ', 'f'], ['Three branches, three approaches:', 'd']),
    L(['  4 ', 'f']),
    L(['  5 ', 'f'], ['- ', 'm'], ['**a7f**', 'y'], [': jsonwebtoken crate, sync', 'd']),
    L(['  6 ', 'f'], ['- ', 'm'], ['**b22**', 'y'], [': biscuit-auth, async-first', 'd']),
    L(['  7 ', 'f'], ['- ', 'm'], ['**c19**', 'y'], [': hand-rolled, zero deps', 'd']),
    L(['  8 ', 'f']),
    L(['  9 ', 'f'], ['## Scoring', 'a']),
    L([' 10 ', 'f']),
    L([' 11 ', 'f'], ['|           | perf | deps | dx  |', 'd']),
    L([' 12 ', 'f'], ['|-----------|------|------|-----|', 'f']),
    L([' 13 ', 'f'], ['| a7f       | B+   | C    | A   |', 'd']),
    L([' 14 ', 'f'], ['| b22       | A    | B    | B+  |', 'd']),
    L([' 15 ', 'f'], ['| c19       | A+   | A+   | C-  |', 'd']),
    L([' 16 ', 'f']),
    L([' 17 ', 'f'], ['Leaning b22. Will wait for all three benchmarks.', 'd']),
    L([' 18 ', 'f']),
    L(['~', 'f']),
    L(['~', 'f']),
    L(['-- NORMAL --', 'g'], ['              ', 'd'], ['17,52', 'f'], ['         ', 'd'], ['All', 'f']),
  ],
  cursor: { row: 17, col: 48, blink: false },
};

// ─── Workspaces ─────────────────────────────────────────────────

const WORKSPACES = [
  {
    id: 'ws-auth',
    label: 'auth refactor',
    repo: 'api',
    layout: '3x2',
    tiles: [claudeCodingTile, claudeThinkingTile, {
      ...claudeCodingTile,
      title: 'claude · zero-dep approach',
      branch: 'claude/auth-mw-c19',
      primary: false,
      generating: true,
      lines: [
        L(['● ', 'a'], ['Writing ', 'm'], ['src/middleware/auth.rs', 'b']),
        L(['  ', 'd'], ['● ', 'g'], ['implemented ', 'm'], ['base64_url_decode', 'y']),
        L(['  ', 'd'], ['● ', 'g'], ['implemented ', 'm'], ['hmac_sha256', 'y']),
        L(['  ', 'd'], ['○ ', 'y'], ['implementing ', 'm'], ['verify_signature', 'y']),
        L([' ', 'd']),
        L(['  writing constant-time comparison', 'f'], ['▊', 'a']),
      ],
      cursor: { row: 6, col: 38, blink: false },
    }, logsTile, gitTile, notesTile],
  },
  {
    id: 'ws-bench',
    label: 'benchmarks',
    repo: 'api',
    layout: '2x2',
    tiles: [htopTile, logsTile, gitTile, notesTile],
  },
  {
    id: 'ws-docs',
    label: 'docs · playground',
    repo: 'docs',
    layout: '2x1',
    tiles: [notesTile, gitTile],
  },
  {
    id: 'ws-shell',
    label: 'shell',
    repo: null,
    layout: '1x1',
    tiles: [gitTile],
  },
];

Object.assign(window, { THEME, ANSI, WORKSPACES, L });

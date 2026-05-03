# Vik UI Design Direction

This document is the source of truth for later Vik UI implementation work. It
defines the product stance, visual system, page map, and interaction rules for a
future operator interface. This round does not implement UI code.

## Product Stance

Vik is an operator console for a local coding-agent orchestrator. The interface
should feel like infrastructure software: dense, legible, calm, and built for
repeated monitoring/debugging work. Avoid marketing composition, oversized hero
sections, decorative gradients, and illustration-heavy pages.

The primary user is an engineer running or supervising Vik. Optimize for:

- Quickly spotting stalled, retrying, blocked, or completed work.
- Moving from a fleet-level signal to a single issue/session with one click.
- Understanding what Vik decided, what Codex did, where files/logs live, and
  what action is safe next.
- Copying exact commands, paths, JSON snippets, and issue/session identifiers.

## Visual System

### Color

Use a neutral interface with one clear brand/action color and separate semantic
status colors.

- Main color: `#0891B2` cyan. Use for primary actions, active nav, selected
  tabs, focus rings, and small trend accents.
- Main color hover: `#0E7490`.
- Main color subtle background: `#ECFEFF`.
- Light background: `#F8FAFC`.
- Light surface: `#FFFFFF`.
- Light raised surface: `#F1F5F9`.
- Light border: `#D8E0EA`.
- Light text: `#0F172A`.
- Muted light text: `#64748B`.
- Dark background: `#0B0F14`.
- Dark surface: `#111820`.
- Dark raised surface: `#18212B`.
- Dark border: `#2B3642`.
- Dark text: `#E5EDF5`.
- Muted dark text: `#91A0AF`.
- Success: `#16A34A`.
- Warning: `#D97706`.
- Error: `#DC2626`.
- Info: `#2563EB`.
- Paused/canceled: `#7C3AED`.

Do not let the app read as all-cyan. Most surface area should be neutral; color
should identify state, selection, and action priority.

### Typography

Use system-native sans fonts unless a product shell later adds a font package:

```css
font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont,
  "Segoe UI", sans-serif;
```

Use monospace for commands, JSON, IDs, paths, timestamps, and log streams:

```css
font-family: "JetBrains Mono", "SFMono-Regular", Consolas, "Liberation Mono",
  monospace;
```

Type scale:

- Page title: 24px, 650 weight, 32px line height.
- Section title: 18px, 600 weight, 26px line height.
- Panel title: 14px, 600 weight, 20px line height.
- Body/table text: 13px, 400 or 500 weight, 20px line height.
- Dense metadata: 12px, 500 weight, 16px line height.
- Code/log text: 12px, 400 weight, 18px line height.

Use `font-weight: 500` for most UI labels. Reserve `600` or `650` for headings,
selected rows, and critical values. Letter spacing stays `0`.

### Shape, Radix, And Shadows

Use Radix primitives for accessible dialogs, menus, tooltips, tabs, switches,
checkboxes, popovers, scroll areas, and toasts.

- Default radius: 6px.
- Repeated item/card radius: 8px maximum.
- Input/button height: 32px for dense toolbars, 36px for forms.
- Icon button: 32px square.
- Border-first depth: prefer 1px borders over shadows for static panels.
- Shadow style:
  - Static panels: no shadow or `0 1px 2px rgba(15, 23, 42, 0.06)`.
  - Popovers/dialogs: `0 12px 32px rgba(15, 23, 42, 0.18)`.
  - Dark popovers: `0 12px 32px rgba(0, 0, 0, 0.42)`.

Cards are for repeated rows, modals, and framed tools only. Do not nest cards
inside cards. Page sections should be full-width layout regions with constrained
content.

### Icons And Controls

Use lucide icons where available. Preferred pairings:

- Refresh: `RefreshCw`.
- Start/retry: `Play` or `RotateCcw`.
- Stop/cancel: `Square`.
- Logs: `ScrollText`.
- Issue detail: `CircleDot`.
- Settings/config: `SlidersHorizontal`.
- Service: `Server`.
- Copy: `Copy`.
- External link: `ExternalLink`.
- Error: `CircleAlert`.
- Success: `CircleCheck`.

Use tooltips for icon-only controls. Prefer segmented controls for view modes,
tabs for page sections, switches for binary settings, and menus for secondary
commands.

### Light And Dark Modes

Implement light mode first and keep dark mode first-class. The UI must not
invert status meaning between modes. Dark mode should reduce glow and preserve
text contrast rather than lean on saturated neon colors.

Required theme behavior:

- Respect system preference by default.
- Keep a manual `light | dark | system` mode switch in settings.
- Persist user preference locally.
- Ensure log/code blocks use slightly raised surfaces in both modes.

### Layout Density

Use an app shell, not a landing page.

- Left nav: 240px desktop, collapses to icon rail at tablet width.
- Top bar: 48px, with workspace/project selector, refresh state, and theme menu.
- Main content max width: none for tables; 1200px for forms and docs-like pages.
- Default page padding: 24px desktop, 16px tablet, 12px mobile.
- Grid gap: 16px desktop, 12px mobile.
- Table row height: 40px standard, 32px compact mode.

Every page should support keyboard scanning: stable table columns, visible focus
rings, sticky headers for long tables, and no layout shift when data updates.

## Page Map

The current codebase supports a daemon/CLI, Linear tracker reads, per-issue
workspaces, workflow config parsing/reload, Codex app-server sessions, retry and
reconciliation state, optional HTTP observability, JSON logs, and local service
commands. The UI should expose these as five primary pages.

### 1. Operations Overview

Route: `/`

Purpose: show the current health of Vik at a glance and route the operator to
the issue or system area that needs attention.

Primary content:

- Summary strip: running count, retrying count, max concurrency, poll interval,
  input tokens, output tokens, total tokens, and active rate-limit signal.
- Running issues table: issue key, Linear state, run state, turns, last event,
  last message, session id, workspace path, started at, last event time, token
  use.
- Retry queue table: issue key, attempt, due time, error summary.
- Recent events feed: compact chronological stream across issues.
- Manual refresh action that maps to `POST /api/v1/refresh`.

Layout:

- Top summary strip uses small metric cells, not oversized stat cards.
- Running table occupies the main column.
- Retry queue and recent events sit in a right rail on wide desktop and stack
  below on mobile.
- Empty state should say no active runs and still show poll interval,
  configured tracker, workspace root, and service status.

Interactions:

- Click issue key or row to open Issue Run Detail.
- Filter by state: All, Running, Retrying, Stalled, Failed, Completed.
- Compact/detailed table density segmented control.
- Refresh button shows queued/coalesced outcome.

### 2. Issue Run Detail

Route: `/issues/:identifier`

Purpose: explain exactly what happened or is happening for one Linear issue.

Primary content:

- Header: issue key, title when available, Linear state, Vik run status,
  attempt count, current retry attempt, workspace path, session id.
- Run timeline: preparing workspace, hooks, prompt render, agent process,
  session initialization, turns, completion, failure, cancellation, or retry.
- Session panel: thread id, turn id, app-server pid, turn count, last Codex
  event, token totals, last message.
- Error panel: latest error, retry due time, suggested operator action when the
  state is blocked by configuration or credentials.
- Artifacts panel: workspace path, log file path, session JSONL path when known,
  Linear issue link, PR link when known.
- Raw debug drawer: formatted JSON from `/api/v1/:issue_identifier`.

Layout:

- Two-column desktop layout: timeline as the main column, sticky run summary in
  the right column.
- Mobile layout stacks summary first, then timeline, then raw data.
- Timeline items should have stable heights and status icons so updates do not
  jump the page.

Interactions:

- Copy controls for paths, ids, and JSON.
- Tabs for Timeline, Session, Logs, Raw.
- Inline external links open Linear/GitHub in a new tab.
- Retry/cancel controls may be shown disabled until the backend supports them.

### 3. Workflow Configuration

Route: `/workflow`

Purpose: make the active `WORKFLOW.md` understandable without replacing the
file as the source of truth.

Primary content:

- Effective config summary grouped by tracker, polling, workspace, hooks, agent,
  Codex, logging, and HTTP server.
- Validation state for dispatch readiness: tracker kind, API key presence,
  project slug, positive polling interval, Codex command, model config, hook
  timeout.
- Front matter viewer with syntax highlighting and copy action.
- Prompt template preview area showing rendered static sections and issue
  variable placeholders.
- Reload state: last valid config, current invalid config warning, reload time.

Layout:

- Left column: section navigation for config groups.
- Main column: selected group details and validation.
- Right rail: source file path, last loaded time, check command, and reload
  outcome.

Interactions:

- Tabs: Effective, Source, Prompt Preview, Validation.
- Do not provide inline editing until backend write support exists.
- Show secrets as presence/absence only; never render secret values.

### 4. Service And Environment

Route: `/service`

Purpose: help the operator understand how Vik is running locally and how to
start, stop, inspect, or containerize it.

Primary content:

- Service status: running/stopped/stale, pid, command, workflow path, cwd, port,
  state file, log file.
- CLI command reference for daemon, check, service install/status/logs/restart,
  and Docker run.
- Environment readiness: Linear key present, GitHub token present, OpenAI key
  present, Codex command present, `gh` availability, workspace root writable.
- Filesystem safety panel: sanitized workspace names, workspace-root boundary,
  cleanup behavior for terminal issues.

Layout:

- Status and readiness panels across the top.
- Command reference below, grouped by local service and Docker.
- Safety policy panel remains visible on the right on desktop.

Interactions:

- Copy command buttons.
- Logs button deep-links to Logs And Sessions with the service log selected.
- Start/stop/restart controls may be disabled until backend support exists.

### 5. Logs And Sessions

Route: `/logs`

Purpose: give engineers a fast way to inspect structured daemon logs and raw
Codex session JSONL without tailing files manually.

Primary content:

- Log source selector: daemon stdout, daily daemon file, service file, session
  JSONL.
- Filters: issue key, session id, event type, severity, text search, time range.
- Virtualized log table: timestamp, level, issue, session, event, message,
  structured fields.
- Detail drawer: full JSON event with copy action.
- Session transcript view: app-server lifecycle, tool calls, turn counts,
  token/rate-limit usage.

Layout:

- Filter toolbar is sticky and dense.
- Log rows use monospace only for payload fields, not every label.
- Detail drawer opens from the right and is resizable.

Interactions:

- Pause/resume live tail.
- Copy selected row JSON.
- Open issue detail from any row with an issue key.
- Preserve filters in the URL.

## Cross-Page States

Every page needs these states:

- Loading: skeleton rows with stable dimensions.
- Empty: specific text naming the missing data source.
- Error: concise message, exact failing endpoint or file path, retry action.
- Stale data: visible timestamp and muted warning.
- Unauthorized/missing secret: presence-only explanation and command/env hint,
  never the secret value.
- Narrow viewport: controls collapse into menus before text overlaps.

## Data Display Rules

- Always show Linear issue identifiers as the primary object label.
- Keep raw IDs visible but secondary; truncate middle with copy control.
- Timestamps use absolute date/time with a relative helper only when space
  allows.
- Token counts use compact formatting in summaries and exact values in details.
- Paths are monospace, middle-truncated, copyable, and horizontally scrollable
  in raw panels.
- Error messages preserve exact text in detail views but summarize in tables.

## Accessibility

- Minimum body contrast: WCAG AA.
- Focus ring: 2px `#0891B2` with 2px offset.
- Table rows must be reachable by keyboard and expose row actions through a
  menu button.
- Icon-only actions require accessible labels and tooltips.
- Status must never be communicated by color alone; include icon and text.
- Toasts must not hide permanent errors. Durable problems belong in page panels.

## Mockup Asset Briefs

Generated image-2 mockups should be treated as visual references only. The
written rules above are canonical when implementation details conflict.

Required generated assets:

1. Operations Overview desktop mockup, light mode.
2. Issue Run Detail desktop mockup, dark mode.
3. Workflow Configuration desktop mockup, light mode.
4. Service And Environment plus Logs And Sessions combined operator mockup,
   dark mode.

Mockups should show dense operational UI, realistic tables and panels, subtle
Radix-style controls, 6px to 8px radii, neutral surfaces, cyan primary actions,
and semantic status accents. They should not show marketing hero content,
decorative blobs, nested cards, large rounded pills, or one-color gradient
themes.

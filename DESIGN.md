# Vik UI Design Direction

This document is the source of truth for later Vik UI implementation work. It
defines the product stance, visual system, and shared interaction rules for a
future operator interface. Page-specific briefs live on the follow-up Linear UI
phase issues. This round does not implement UI code.

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

## Page Brief Ownership

Keep this document focused on global design direction. Use the follow-up Linear
issues for page and workflow implementation briefs:

- VIK-27: operator shell and navigation.
- VIK-28: operations overview.
- VIK-29: issue run detail.
- VIK-30: workflow configuration view.
- VIK-31: service and logs view.
- VIK-32: cross-surface readiness pass.

When implementing a page, combine this global design direction with that page's
Linear issue description. If the generated mockups and written rules conflict,
the written rules are canonical.

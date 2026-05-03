# Design System: Vik Operator Console

This document is the source of truth for later Vik UI implementation work. It
defines global product stance, visual tokens, component behavior, layout rules,
and interaction standards for a future operator interface. This round does not
implement UI code.

## 1. Visual Theme and Atmosphere

Vik should feel like infrastructure software: dense, legible, calm, and built
for repeated monitoring and debugging work. The interface is an operator
console, not a marketing site. It should prioritize scan speed, exact state,
and fast movement from a fleet-level signal to one issue or session.

The visual tone is neutral and technical. Most surface area uses quiet slate
and white values in light mode, or near-black and blue-gray values in dark
mode. Cyan is the single brand/action accent and should appear as selection,
focus, primary actions, and small trend details. Status colors stay semantic
and distinct from the brand accent.

The primary user is an engineer running or supervising Vik. Optimize for:

- Spotting stalled, retrying, blocked, running, or completed work quickly.
- Reading dense tables without visual noise.
- Copying exact commands, paths, JSON snippets, issue IDs, and session IDs.
- Understanding what Vik decided, what Codex did, where files and logs live,
  and what action is safe next.

Avoid oversized hero areas, decorative gradients, illustration-heavy layouts,
floating orb effects, nested cards, and broad marketing composition.

## 2. Color Palette and Roles

### Light Theme Tokens

- **Background** (`#F8FAFC`): page canvas and app shell background.
- **Surface** (`#FFFFFF`): primary panels, tables, forms, and dialogs.
- **Raised Surface** (`#F1F5F9`): code blocks, log panes, selected rows, and
  subtle grouped areas.
- **Border** (`#D8E0EA`): default dividers, panel outlines, table separators,
  and input borders.
- **Strong Border** (`#B8C4D2`): active row edge, selected nav edge, and dense
  split panes.
- **Text** (`#0F172A`): headings, strong labels, and primary values.
- **Muted Text** (`#64748B`): secondary labels, timestamps, helper text, and
  lower-priority metadata.
- **Disabled Text** (`#94A3B8`): disabled controls and unavailable values.

### Dark Theme Tokens

- **Background** (`#0B0F14`): page canvas and app shell background.
- **Surface** (`#111820`): primary panels, tables, forms, and dialogs.
- **Raised Surface** (`#18212B`): code blocks, log panes, selected rows, and
  subtle grouped areas.
- **Border** (`#2B3642`): default dividers, panel outlines, table separators,
  and input borders.
- **Strong Border** (`#3A4756`): active row edge, selected nav edge, and dense
  split panes.
- **Text** (`#E5EDF5`): headings, strong labels, and primary values.
- **Muted Text** (`#91A0AF`): secondary labels, timestamps, helper text, and
  lower-priority metadata.
- **Disabled Text** (`#667586`): disabled controls and unavailable values.

### Accent and Feedback Tokens

- **Vik Cyan** (`#0891B2`): primary actions, active nav, selected tabs, focus
  rings, links, and small trend accents.
- **Vik Cyan Hover** (`#0E7490`): hover and pressed state for cyan controls.
- **Vik Cyan Subtle** (`#ECFEFF`): light selected row background and subtle
  active states.
- **Dark Cyan Subtle** (`#103842`): dark selected row background and subtle
  active states.
- **Success** (`#16A34A`): completed, healthy, connected, merged, and passing.
- **Warning** (`#D97706`): retrying, delayed, waiting, rate limited, and stale.
- **Error** (`#DC2626`): failed, blocked, missing auth, invalid config, and
  terminal error.
- **Info** (`#2563EB`): informational states and external references.
- **Paused** (`#7C3AED`): canceled, paused, duplicate, or manually held states.

Do not let the app read as all-cyan. Neutral surfaces should dominate. Use
color to identify state, selection, action priority, and risk.

## 3. Typography Rules

### Font Family

Use system-native sans fonts unless a future product shell adds a font package:

```css
font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont,
  "Segoe UI", sans-serif;
```

Use monospace for commands, JSON, IDs, paths, timestamps, counters, logs, and
fixed-width table values:

```css
font-family: "JetBrains Mono", "SFMono-Regular", Consolas, "Liberation Mono",
  monospace;
```

### Hierarchy

| Role | Size | Weight | Line Height | Letter Spacing | Use |
| --- | --- | --- | --- | --- | --- |
| Page Title | 24px | 650 | 32px | 0 | Main screen title |
| Section Title | 18px | 600 | 26px | 0 | Major page section |
| Panel Title | 14px | 600 | 20px | 0 | Table, drawer, and panel heading |
| Body | 13px | 400 | 20px | 0 | Standard table and form text |
| Body Emphasis | 13px | 500 | 20px | 0 | Primary row values and labels |
| Dense Metadata | 12px | 500 | 16px | 0 | Timestamps, IDs, badges, small labels |
| Code and Logs | 12px | 400 | 18px | 0 | Commands, JSON, paths, and log streams |

Use `font-weight: 500` for most UI labels. Reserve `600` or `650` for headings,
selected rows, active nav, and critical values. Keep letter spacing at `0`.
Use tabular numbers where counts, timestamps, durations, or token totals must
align.

## 4. Component Styling

Use Radix primitives for accessible dialogs, menus, tooltips, tabs, switches,
checkboxes, popovers, scroll areas, and toasts. Use lucide icons where
available. Tooltips are required for icon-only controls.

### Buttons

**Primary**

- Background: `#0891B2`.
- Hover background: `#0E7490`.
- Text: `#FFFFFF`.
- Height: 32px in dense toolbars, 36px in forms and dialogs.
- Radius: 6px.
- Padding: 0 12px for text buttons, 0 10px for icon and text buttons.
- Font: 13px, weight 500.
- Use: start, retry, save, apply, refresh, and other primary actions.

**Secondary**

- Background: transparent or theme surface.
- Text: theme text.
- Border: 1px solid theme border.
- Hover: use raised surface, not a saturated fill.
- Radius: 6px.
- Use: cancel, reset, copy, open external link, and lower-priority actions.

**Icon Button**

- Size: 32px square.
- Radius: 6px.
- Icon: 16px or 18px.
- Hover: raised surface.
- Focus: 2px cyan outline with 2px offset.

Preferred lucide pairings:

- Refresh: `RefreshCw`.
- Start or retry: `Play` or `RotateCcw`.
- Stop or cancel: `Square`.
- Logs: `ScrollText`.
- Issue detail: `CircleDot`.
- Settings and config: `SlidersHorizontal`.
- Service: `Server`.
- Copy: `Copy`.
- External link: `ExternalLink`.
- Error: `CircleAlert`.
- Success: `CircleCheck`.

### Cards and Panels

- Background: theme surface.
- Border: 1px solid theme border.
- Radius: 8px maximum.
- Shadow: none for static panels, or `0 1px 2px rgba(15, 23, 42, 0.06)` in
  light mode only when separation needs help.
- Use cards only for repeated items, modals, and genuinely framed tools.
- Do not nest cards inside cards.
- Page sections should be full-width layout regions with constrained content.

### Inputs and Forms

- Height: 32px for dense controls, 36px for standard forms.
- Border: 1px solid theme border.
- Radius: 6px.
- Background: theme surface.
- Placeholder: muted text.
- Focus: 2px cyan outline with 2px offset, or 1px cyan border plus outline when
  the control is inside a dense toolbar.
- Validation: show concise inline text and semantic color. Do not rely on color
  alone.

### Navigation

- Desktop left nav width: 240px.
- Tablet nav: collapse to icon rail.
- Top bar height: 48px.
- Active nav: cyan accent, raised surface, or 3px leading edge.
- Nav labels: 13px, weight 500.
- Icons: 18px.
- Keep workspace or project selector, refresh state, and theme menu in the top
  bar.

### Badges, Tables, and Empty States

- Badges use 12px text, weight 500, 4px radius, and semantic subtle
  backgrounds.
- Tables use 40px standard rows and 32px compact rows.
- Table headers are sticky for long tables and use 12px weight 600 text.
- Selected rows use subtle cyan background plus strong border or left edge.
- Empty states stay operational: show configured tracker, workspace root,
  service state, and the next available command instead of marketing copy.

## 5. Layout Principles

### Spacing System

- Base unit: 4px.
- Common scale: 4px, 8px, 12px, 16px, 24px, 32px, 48px.
- Default page padding: 24px desktop, 16px tablet, 12px mobile.
- Grid gap: 16px desktop, 12px mobile.
- Toolbar gap: 8px.
- Dense inline metadata gap: 6px to 8px.

### App Shell

Use an app shell as the first screen. Do not create a landing page for Vik
operator UI work.

- Left nav: 240px desktop, icon rail at tablet width.
- Top bar: 48px.
- Main content max width: none for tables and monitoring views.
- Main content max width: 1200px for forms, configuration, and docs-like views.
- Prefer split panes and right rails for secondary operational context on wide
  screens.

### Density and Rhythm

The UI should feel dense but not cramped. Repeated operational screens should
favor tables, split panes, compact metric strips, and sticky headers over large
cards. Keep content aligned to stable columns so live updates do not shift the
layout.

## 6. Depth and Elevation

| Level | Treatment | Use |
| --- | --- | --- |
| Flat | No shadow | Page background and table bodies |
| Panel | 1px border, no shadow | Static panels, cards, tables, forms |
| Subtle Raised | `0 1px 2px rgba(15, 23, 42, 0.06)` | Light mode panels needing separation |
| Popover | `0 12px 32px rgba(15, 23, 42, 0.18)` | Menus, popovers, dropdowns in light mode |
| Dark Popover | `0 12px 32px rgba(0, 0, 0, 0.42)` | Menus, popovers, dropdowns in dark mode |
| Focus | `2px solid #0891B2` outline | Keyboard focus on interactive elements |

Depth should be border-first. Use shadows for transient layers, not for every
panel. Avoid glass effects, heavy blur, glow, and decorative z-axis treatments.
Log panes and code blocks should feel slightly inset or raised through surface
color and border, not heavy shadow.

## 7. Motion and Interaction

- Keep transitions short: 120ms to 180ms for hover, focus, tab, and menu
  changes.
- Use `ease-out` for entering layers and `ease-in` for dismissing layers.
- Respect `prefers-reduced-motion`; disable non-essential animation when set.
- Hover states should change background, border, or text color. Avoid scale
  effects on dense controls.
- Loading states should preserve layout dimensions. Use skeleton rows or muted
  placeholders where data tables would otherwise jump.
- Selection states must be visible through both color and structural treatment,
  such as a left edge, border, check mark, or active icon.
- Error and retry states need explicit text with the semantic color; never rely
  on icon or color alone.
- Copy actions should show a short toast or inline confirmation.

## 8. Responsive Behavior

| Name | Width | Key Changes |
| --- | --- | --- |
| Mobile | <640px | Single column, collapsed nav, stacked panels, 12px page padding |
| Tablet | 640px-1024px | Icon rail nav, 16px page padding, secondary rails stack below |
| Desktop | >1024px | Full app shell, 24px page padding, split panes and right rails |
| Large Desktop | >1280px | Wider tables, persistent side context, no artificial max width for ops views |

Touch targets should be at least 36px high for dense controls and 44px high
where touch interaction is likely. Tables may become card-like row summaries on
small screens, but keep key metadata visible: issue ID, status, last event,
last update, and primary action.

Avoid hiding critical operational state behind hover-only affordances. On
mobile, expose row actions through menus or expandable detail regions. Keep
code, JSON, and log blocks horizontally scrollable instead of wrapping paths or
structured payloads into unreadable text.

## 9. Agent Usage Guide

### Prefer

- Build app screens, not landing pages.
- Use neutral surfaces with cyan reserved for primary action, focus, and active
  state.
- Use semantic status colors consistently across light and dark themes.
- Use Radix primitives for accessible overlays, tabs, menus, switches, and
  form controls.
- Use lucide icons in icon buttons and add tooltips for icon-only actions.
- Use dense tables, split panes, compact metric strips, sticky headers, and
  stable column widths for monitoring workflows.
- Keep copy short, factual, and operational.

### Avoid

- Do not add page maps, page-specific briefs, or workflow implementation scope
  to this file.
- Do not use decorative gradients, orbs, bokeh, marketing hero sections, or
  illustration-first layouts.
- Do not create nested cards or floating card sections.
- Do not make the interface read as all-cyan.
- Do not use viewport-scaled font sizes or negative letter spacing.
- Do not hide critical states behind color alone.
- Do not commit generated image assets for this design round.

### Quick Token Reference

- Primary action: `#0891B2`.
- Primary action hover: `#0E7490`.
- Light background: `#F8FAFC`.
- Light surface: `#FFFFFF`.
- Light border: `#D8E0EA`.
- Light text: `#0F172A`.
- Dark background: `#0B0F14`.
- Dark surface: `#111820`.
- Dark border: `#2B3642`.
- Dark text: `#E5EDF5`.
- Radius: 6px default, 8px maximum for repeated cards.
- Sans font: Inter, ui-sans-serif, system-ui, -apple-system,
  BlinkMacSystemFont, "Segoe UI", sans-serif.
- Mono font: "JetBrains Mono", "SFMono-Regular", Consolas, "Liberation Mono",
  monospace.

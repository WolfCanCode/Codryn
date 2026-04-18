# Graph Page Enhancement Design

**Date:** 2026-04-18
**Status:** Approved

## Problem

The current `GraphPage.tsx` has three distinct issues:

1. **Scrolling** — `min-h-screen` on the root div plus `py-6 px-6` padding on `<main>` pushes the layout taller than the viewport. The browser can scroll the page, which is wrong for a full-screen canvas tool.
2. **Minimap covered** — The minimap canvas sits at `z-10`; the node detail popover is `z-30`. When a selected node is near the bottom-right corner the popover overlaps the minimap. Additionally the minimap redraws inside `onRenderFramePost` on every animation frame, which is expensive.
3. **Performance** — `force-graph` uses a Canvas 2D renderer. For graphs above ~500 nodes the frame rate degrades. Text labels, edge curvature, and per-frame minimap redraws compound the cost.

## Solution Overview

Migrate the graph renderer from `force-graph` (Canvas 2D) to **sigma.js v3 + graphology** (WebGL), fix the layout to eliminate scroll, make the minimap interactive, and keep the node detail panel tracking its node as the view pans/zooms.

---

## 1. Layout fix (App.tsx + index.css)

**Goal:** graph page fills exactly the viewport — no scroll, no padding waste.

- Change root `<div>` in `App.tsx` from `min-h-screen` → `h-screen overflow-hidden`.
- Remove `py-6` from `<main>` when on a graph route (keep `px-6 py-6` for all other routes). The graph route already sets `overflow-hidden`; also remove the horizontal padding (`px-6`) so the graph is fully flush.
- In `index.css` add `html, body { height: 100%; overflow: hidden; }` scoped to not break other pages (or rely on the `h-screen overflow-hidden` chain being unbroken).

**Result:** header (56 px) + top-bar (36 px) + graph panel = exactly `100vh`. No scroll possible.

---

## 2. Renderer migration: sigma.js v3 + graphology

**Packages to add:**
```
sigma
graphology
graphology-types
graphology-layout-forceatlas2
```

**Packages to remove:**
```
force-graph
```

### Graph data model

Use `graphology.MultiDirectedGraph`. Build it from the same `GraphData` type returned by `fetchLayout`. Nodes carry `label`, `color` (from `NODE_COLORS`), `size` (from `NODE_SIZES`), `x`, `y` (random initial positions). Edges carry `type`, `color` (from `EDGE_COLORS`), `size`.

### Rendering

- Instantiate `new Sigma(graph, containerElement, settings)`.
- Use sigma's built-in `NodeCircleProgram` for nodes and `EdgeArrowProgram` for directed edges.
- Node labels: sigma's built-in label renderer. Show labels only for the top 15 nodes by edge degree when zoom < 2; show all labels when zoom ≥ 2 (mirrors current `LABEL_ZOOM_SHOW_ALL` logic). Configure via sigma's `labelRenderedSizeThreshold` and a custom `labelSelector` callback.
- Node/edge highlight on selection: update the `highlighted` attribute on nodes/edges in the graphology instance; sigma re-renders automatically. Dimmed nodes get color `#d4d4d8`.

### Force layout

Run `graphology-layout-forceatlas2` in a **web worker** (the library ships a `FA2Layout` worker class). This keeps the UI thread free during simulation. Show the "simulating…" badge while the worker is running. Stop after `5000` iterations or when the energy delta falls below `0.001`.

### Filtering (search, label, edge type)

Apply filters by rebuilding the graphology graph from `rawData` (same logic as current `buildGraph`). Call `sigma.refresh()` after graph mutation.

### ResizeObserver

Call `sigma.resize()` inside the existing `ResizeObserver` callback.

### Cleanup

Call `sigma.kill()` on unmount (replaces `graphRef.current._destructor()`).

---

## 3. Node detail panel

**Behavior:**
- Appears next to the clicked node.
- Tracks the node's screen position on every sigma camera update (`sigma.on('afterRender', ...)` or `sigma.getCamera().on('updated', ...)`).
- Clamped so it never enters the minimap zone: avoid the bottom-right `172 × 150 px` rect. If the projected position would land there, shift the panel up or left.
- Never goes off-screen (existing `Math.max`/`Math.min` clamping retained).
- `z-index: 30` (same as now — the minimap will be `z-40` so it always wins).
- Dismissed with the ✕ button or by clicking the background.

**Content unchanged:** node name, label badge with `NODE_COLORS` color, `file_path`, qualified id, Callers list, Callees list (each clickable to focus that node).

---

## 4. Interactive minimap

**Implementation:** a separate `<canvas ref={minimapCanvasRef}>` HTML element (not drawn inside sigma's WebGL canvas). Updated via a `sigma.getCamera().on('updated', drawMinimap)` listener and also after layout ticks complete.

**Position:** `absolute bottom-3 right-3`, `z-index: 40` — always above the detail panel and all other overlays.

**Appearance:** unchanged from current (zinc-200 background, colored dots per `NODE_COLORS`, dashed viewport rectangle).

**Interaction:**
- **Click:** convert minimap canvas coordinates → graph coordinates → call `sigma.getCamera().animate({ x, y }, { duration: 300 })`.
- **Drag the viewport rect:** `mousedown` on canvas, track `mousemove`, update camera on each move event, `mouseup` to release. Use `requestAnimationFrame` throttling during drag.
- Cursor: `crosshair` normally, `grab`/`grabbing` when hovering/dragging the viewport rect.

**Header label:** `OVERVIEW · drag to navigate` (9 px, zinc-400).

---

## 5. Files changed

| File | Change |
|---|---|
| `ui/package.json` | Add `sigma`, `graphology`, `graphology-layout-forceatlas2`; remove `force-graph` |
| `ui/src/App.tsx` | `h-screen overflow-hidden` on root; remove graph-route padding |
| `ui/src/index.css` | `html, body { height: 100%; overflow: hidden }` |
| `ui/src/pages/GraphPage.tsx` | Full renderer rewrite using sigma.js |

No new files. No other pages touched.

---

## 6. What is NOT changing

- All `NODE_COLORS`, `EDGE_COLORS`, `NODE_SIZES` constants — identical values.
- Sidebar content: search input, label filter, edge filter, node legend, edge legend.
- Flow tab: routes/components lists, backend flow, frontend flow — all unchanged.
- Top bar: project badge, node/edge counts, synthetic-edges badge, simulating badge.
- Zoom controls: `+`, `−`, fit buttons.
- RPC layer (`fetchLayout`, `fetchRoutes`, `fetchBackendFlow`, `fetchFrontendFlow`).

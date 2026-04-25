import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Crosshair, Minus, Plus, Unlink } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

import { ProjectAvatar } from './ProjectAvatar'
import { getTopLanguages, type ProjectEntry, type ProjectLink } from './shared'

type Side = 'top' | 'right' | 'bottom' | 'left'

interface Point {
  x: number
  y: number
}

interface CanvasNode {
  entry: ProjectEntry
  x: number
  y: number
  width: number
  height: number
}

interface CanvasEdge {
  link: ProjectLink
  start: Point
  end: Point
  mid: Point
  d: string
}

interface RelationshipLayout {
  nodes: CanvasNode[]
  bounds: {
    minX: number
    minY: number
    maxX: number
    maxY: number
    width: number
    height: number
  }
}

interface ProjectRelationshipCanvasProps {
  entries: ProjectEntry[]
  links: ProjectLink[]
  onLinkProjects: (source: string, target: string) => Promise<void>
  onRequestUnlink: (source: string, target: string) => void
  onOpenProject: (projectName: string) => void
}

const NODE_WIDTH = 268
const NODE_HEIGHT = 120
const LEVEL_GAP_X = 156
const NODE_GAP_Y = 44
const COMPONENT_GAP_X = 112
const COMPONENT_GAP_Y = 88
const VIEW_PADDING = 72
const MIN_SCALE = 0.45
const MAX_SCALE = 1.6
const EDGE_CLEARANCE = 18
const EDGE_LANE_GAP = 20

function clampScale(scale: number) {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale))
}

function nodeHandle(node: CanvasNode, side: Side): Point {
  if (side === 'top') return { x: node.x + node.width / 2, y: node.y }
  if (side === 'right') return { x: node.x + node.width, y: node.y + node.height / 2 }
  if (side === 'bottom') return { x: node.x + node.width / 2, y: node.y + node.height }
  return { x: node.x, y: node.y + node.height / 2 }
}

interface NodeObstacle {
  name: string
  left: number
  right: number
  top: number
  bottom: number
}

function segmentHitsObstacle(a: Point, b: Point, obstacles: NodeObstacle[], excluded: Set<string>) {
  if (Math.abs(a.x - b.x) < 0.5) {
    const x = a.x
    const top = Math.min(a.y, b.y)
    const bottom = Math.max(a.y, b.y)
    return obstacles.some(obstacle => {
      if (excluded.has(obstacle.name)) return false
      if (x < obstacle.left || x > obstacle.right) return false
      return !(bottom < obstacle.top || top > obstacle.bottom)
    })
  }
  if (Math.abs(a.y - b.y) < 0.5) {
    const y = a.y
    const left = Math.min(a.x, b.x)
    const right = Math.max(a.x, b.x)
    return obstacles.some(obstacle => {
      if (excluded.has(obstacle.name)) return false
      if (y < obstacle.top || y > obstacle.bottom) return false
      return !(right < obstacle.left || left > obstacle.right)
    })
  }
  return true
}

function formatLanguageSummary(entry: ProjectEntry) {
  const labels = getTopLanguages(entry.languages, 3).map(badge =>
    badge.label.replace(/\s*\(\d[\d,]*\)$/, ''),
  )
  if (labels.length > 0) return labels.join(' · ')
  return 'No language data'
}

function polylinePath(points: Point[]) {
  return points.map((point, index) => `${index === 0 ? 'M' : 'L'} ${point.x} ${point.y}`).join(' ')
}

function buildOrthogonalPath(start: Point, end: Point, laneX: number) {
  const points = [start, { x: laneX, y: start.y }, { x: laneX, y: end.y }, end]
  return {
    d: polylinePath(points),
    mid: { x: laneX, y: start.y + (end.y - start.y) / 2 },
  }
}

function buildHorizontalBypassPath(start: Point, end: Point, laneY: number, direction: number) {
  const stub = 26 * direction
  const points = [
    start,
    { x: start.x + stub, y: start.y },
    { x: start.x + stub, y: laneY },
    { x: end.x - stub, y: laneY },
    { x: end.x - stub, y: end.y },
    end,
  ]
  return {
    d: polylinePath(points),
    mid: { x: (start.x + end.x) / 2, y: laneY },
    points,
  }
}

function buildSmartEdge(
  sourceNode: CanvasNode,
  targetNode: CanvasNode,
  obstacles: NodeObstacle[],
  bounds: { minX: number; maxX: number; minY: number; maxY: number },
) {
  const sourceCenterX = sourceNode.x + sourceNode.width / 2
  const targetCenterX = targetNode.x + targetNode.width / 2
  const startSide: Side = sourceCenterX <= targetCenterX ? 'right' : 'left'
  const endSide: Side = sourceCenterX <= targetCenterX ? 'left' : 'right'
  const start = nodeHandle(sourceNode, startSide)
  const end = nodeHandle(targetNode, endSide)
  const excluded = new Set([sourceNode.entry.project.name, targetNode.entry.project.name])
  const direction = start.x <= end.x ? 1 : -1

  const preferredLane = direction > 0
    ? Math.max(start.x, end.x) - Math.abs(end.x - start.x) / 2
    : Math.min(start.x, end.x) + Math.abs(end.x - start.x) / 2

  const candidateLanes: number[] = [preferredLane]
  for (let step = 1; step <= 12; step++) {
    candidateLanes.push(preferredLane + step * EDGE_LANE_GAP, preferredLane - step * EDGE_LANE_GAP)
  }
  candidateLanes.push(bounds.maxX + EDGE_CLEARANCE * 2, bounds.minX - EDGE_CLEARANCE * 2)

  for (const laneX of candidateLanes) {
    const points = [start, { x: laneX, y: start.y }, { x: laneX, y: end.y }, end]
    let blocked = false
    for (let index = 0; index < points.length - 1; index++) {
      if (segmentHitsObstacle(points[index], points[index + 1], obstacles, excluded)) {
        blocked = true
        break
      }
    }
    if (!blocked) {
      return { start, end, ...buildOrthogonalPath(start, end, laneX) }
    }
  }

  const preferredLaneY = start.y + (end.y - start.y) / 2
  const candidateLaneYs: number[] = [preferredLaneY]
  for (let step = 1; step <= 12; step++) {
    candidateLaneYs.push(preferredLaneY + step * EDGE_LANE_GAP, preferredLaneY - step * EDGE_LANE_GAP)
  }
  candidateLaneYs.push(bounds.maxY + EDGE_CLEARANCE * 2, bounds.minY - EDGE_CLEARANCE * 2)

  for (const laneY of candidateLaneYs) {
    const candidate = buildHorizontalBypassPath(start, end, laneY, direction)
    let blocked = false
    for (let index = 0; index < candidate.points.length - 1; index++) {
      if (segmentHitsObstacle(candidate.points[index], candidate.points[index + 1], obstacles, excluded)) {
        blocked = true
        break
      }
    }
    if (!blocked) {
      return { start, end, d: candidate.d, mid: candidate.mid }
    }
  }

  const fallbackLane = direction > 0 ? bounds.maxX + EDGE_CLEARANCE * 2 : bounds.minX - EDGE_CLEARANCE * 2
  return { start, end, ...buildOrthogonalPath(start, end, fallbackLane) }
}

function buildEdges(nodes: CanvasNode[], links: ProjectLink[]): CanvasEdge[] {
  const nodeByName = new Map(nodes.map(node => [node.entry.project.name, node]))
  const obstacles: NodeObstacle[] = nodes.map(node => ({
    name: node.entry.project.name,
    left: node.x - EDGE_CLEARANCE,
    right: node.x + node.width + EDGE_CLEARANCE,
    top: node.y - EDGE_CLEARANCE,
    bottom: node.y + node.height + EDGE_CLEARANCE,
  }))

  const minX = Math.min(...nodes.map(node => node.x), 0)
  const maxX = Math.max(...nodes.map(node => node.x + node.width), NODE_WIDTH)
  const minY = Math.min(...nodes.map(node => node.y), 0)
  const maxY = Math.max(...nodes.map(node => node.y + node.height), NODE_HEIGHT)

  return links
    .map(link => {
      const sourceNode = nodeByName.get(link.a)
      const targetNode = nodeByName.get(link.b)
      if (!sourceNode || !targetNode) return null
      const { start, end, mid, d } = buildSmartEdge(sourceNode, targetNode, obstacles, { minX, maxX, minY, maxY })
      return { link, start, end, mid, d }
    })
    .filter((edge): edge is CanvasEdge => Boolean(edge))
}

function computeBounds(nodes: CanvasNode[]) {
  const minX = Math.min(...nodes.map(node => node.x), 0)
  const minY = Math.min(...nodes.map(node => node.y), 0)
  const maxX = Math.max(...nodes.map(node => node.x + node.width), NODE_WIDTH)
  const maxY = Math.max(...nodes.map(node => node.y + node.height), NODE_HEIGHT)
  return {
    minX,
    minY,
    maxX,
    maxY,
    width: maxX - minX,
    height: maxY - minY,
  }
}

function buildLayout(entries: ProjectEntry[], links: ProjectLink[]): RelationshipLayout {
  const adjacency = new Map<string, Set<string>>()
  const byName = new Map(entries.map(entry => [entry.project.name, entry]))

  for (const entry of entries) adjacency.set(entry.project.name, new Set())
  for (const link of links) {
    adjacency.get(link.a)?.add(link.b)
    adjacency.get(link.b)?.add(link.a)
  }

  const visited = new Set<string>()
  const names = entries.map(entry => entry.project.name).sort((a, b) => a.localeCompare(b))
  const components: string[][] = []
  for (const name of names) {
    if (visited.has(name)) continue
    const stack = [name]
    const component: string[] = []
    visited.add(name)
    while (stack.length > 0) {
      const current = stack.pop()
      if (!current) continue
      component.push(current)
      const neighbors = [...(adjacency.get(current) ?? [])].sort((a, b) => a.localeCompare(b))
      for (const neighbor of neighbors) {
        if (visited.has(neighbor)) continue
        visited.add(neighbor)
        stack.push(neighbor)
      }
    }
    components.push(component)
  }

  const sortedComponents = components.sort((a, b) => {
    const aLinked = a.some(name => (adjacency.get(name)?.size ?? 0) > 0) ? 1 : 0
    const bLinked = b.some(name => (adjacency.get(name)?.size ?? 0) > 0) ? 1 : 0
    if (aLinked !== bLinked) return bLinked - aLinked
    if (a.length !== b.length) return b.length - a.length
    return a[0].localeCompare(b[0])
  })

  const nodes: CanvasNode[] = []
  const maxColumnHeight = 560
  let cursorX = 0
  let cursorY = 0
  let columnWidth = 0

  for (const component of sortedComponents) {
    const componentEntries = component
      .map(name => byName.get(name))
      .filter((entry): entry is ProjectEntry => Boolean(entry))
    const degree = (name: string) => adjacency.get(name)?.size ?? 0

    const levelMap = new Map<string, number>()
    if (componentEntries.length === 1) {
      levelMap.set(componentEntries[0].project.name, 0)
    } else {
      const root = [...componentEntries]
        .sort((a, b) => degree(b.project.name) - degree(a.project.name) || a.project.name.localeCompare(b.project.name))[0]
      const queue = [root.project.name]
      levelMap.set(root.project.name, 0)
      while (queue.length > 0) {
        const current = queue.shift()
        if (!current) continue
        const currentLevel = levelMap.get(current) ?? 0
        const neighbors = [...(adjacency.get(current) ?? [])]
          .filter(neighbor => component.includes(neighbor))
          .sort((a, b) => degree(b) - degree(a) || a.localeCompare(b))
        for (const neighbor of neighbors) {
          if (levelMap.has(neighbor)) continue
          levelMap.set(neighbor, currentLevel + 1)
          queue.push(neighbor)
        }
      }

      for (const entry of componentEntries) {
        if (!levelMap.has(entry.project.name)) {
          levelMap.set(entry.project.name, 0)
        }
      }
    }

    const levels = new Map<number, ProjectEntry[]>()
    for (const entry of componentEntries) {
      const level = levelMap.get(entry.project.name) ?? 0
      const bucket = levels.get(level) ?? []
      bucket.push(entry)
      levels.set(level, bucket)
    }

    const orderedLevels = [...levels.entries()]
      .sort((a, b) => a[0] - b[0])
      .map(([level, bucket]) => [
        level,
        [...bucket].sort(
          (a, b) => degree(b.project.name) - degree(a.project.name) || a.project.name.localeCompare(b.project.name),
        ),
      ] as const)

    const componentWidth = orderedLevels.length * NODE_WIDTH + Math.max(0, orderedLevels.length - 1) * LEVEL_GAP_X
    const componentHeight = Math.max(
      ...orderedLevels.map(([, bucket]) => bucket.length * NODE_HEIGHT + Math.max(0, bucket.length - 1) * NODE_GAP_Y),
    )

    if (cursorY > 0 && cursorY + componentHeight > maxColumnHeight) {
      cursorY = 0
      cursorX += columnWidth + COMPONENT_GAP_X
      columnWidth = 0
    }

    for (const [level, bucket] of orderedLevels) {
      const totalHeight = bucket.length * NODE_HEIGHT + Math.max(0, bucket.length - 1) * NODE_GAP_Y
      const startY = cursorY + (componentHeight - totalHeight) / 2
      for (const [index, entry] of bucket.entries()) {
        const node: CanvasNode = {
          entry,
          x: cursorX + level * (NODE_WIDTH + LEVEL_GAP_X),
          y: startY + index * (NODE_HEIGHT + NODE_GAP_Y),
          width: NODE_WIDTH,
          height: NODE_HEIGHT,
        }
        nodes.push(node)
      }
    }

    cursorY += componentHeight + COMPONENT_GAP_Y
    columnWidth = Math.max(columnWidth, componentWidth)
  }

  return {
    nodes,
    bounds: computeBounds(nodes),
  }
}

export function ProjectRelationshipCanvas({
  entries,
  links,
  onLinkProjects,
  onRequestUnlink,
  onOpenProject,
}: ProjectRelationshipCanvasProps) {
  const viewportRef = useRef<HTMLDivElement>(null)
  const [viewportSize, setViewportSize] = useState({ width: 0, height: 0 })
  const [transform, setTransform] = useState({ x: 0, y: 0, scale: 1 })
  const [hoveredEdgeKey, setHoveredEdgeKey] = useState<string | null>(null)
  const [hoveredNode, setHoveredNode] = useState<string | null>(null)
  const [pendingLink, setPendingLink] = useState<{ source: string; side: Side; pointerWorld: Point } | null>(null)
  const [linkBusy, setLinkBusy] = useState(false)
  const [nodePositions, setNodePositions] = useState<Record<string, Point>>({})
  const panStateRef = useRef<{ pointerId: number; originX: number; originY: number; startX: number; startY: number } | null>(
    null,
  )
  const nodeDragRef = useRef<{
    pointerId: number
    projectName: string
    startPointer: Point
    startNode: Point
    moved: boolean
  } | null>(null)
  const didAutoFitRef = useRef(false)

  const baseLayout = useMemo(() => buildLayout(entries, links), [entries, links])
  const layout = useMemo(() => {
    const nodes = baseLayout.nodes.map(node => {
      const override = nodePositions[node.entry.project.name]
      return override ? { ...node, x: override.x, y: override.y } : node
    })
    return {
      nodes,
      edges: buildEdges(nodes, links),
      bounds: computeBounds(nodes),
    }
  }, [baseLayout.nodes, links, nodePositions])
  const layoutKey = useMemo(
    () => `${entries.map(entry => entry.project.name).join('|')}::${links.map(link => link.key).join('|')}`,
    [entries, links],
  )

  const fitToViewport = useCallback(() => {
    const viewport = viewportRef.current
    if (!viewport) return
    const width = viewport.clientWidth
    const height = viewport.clientHeight
    if (width <= 0 || height <= 0) return

    const scale = clampScale(
      Math.min(
        (width - VIEW_PADDING * 2) / Math.max(layout.bounds.width, 1),
        (height - VIEW_PADDING * 2) / Math.max(layout.bounds.height, 1),
      ),
    )
    const x = (width - layout.bounds.width * scale) / 2 - layout.bounds.minX * scale
    const y = (height - layout.bounds.height * scale) / 2 - layout.bounds.minY * scale
    setTransform({ x, y, scale })
  }, [layout.bounds.height, layout.bounds.minX, layout.bounds.minY, layout.bounds.width])

  useLayoutEffect(() => {
    const viewport = viewportRef.current
    if (!viewport) return
    const updateSize = () => setViewportSize({ width: viewport.clientWidth, height: viewport.clientHeight })
    updateSize()
    const observer = new ResizeObserver(updateSize)
    observer.observe(viewport)
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    didAutoFitRef.current = false
    setNodePositions({})
  }, [layoutKey])

  useEffect(() => {
    if (didAutoFitRef.current) return
    if (viewportSize.width <= 0 || viewportSize.height <= 0) return
    fitToViewport()
    didAutoFitRef.current = true
  }, [fitToViewport, viewportSize.height, viewportSize.width])

  useEffect(() => {
    const handlePointerUp = () => {
      panStateRef.current = null
      setPendingLink(null)
      nodeDragRef.current = null
    }
    window.addEventListener('pointerup', handlePointerUp)
    return () => window.removeEventListener('pointerup', handlePointerUp)
  }, [])

  const toWorld = useCallback(
    (clientX: number, clientY: number) => {
      const viewport = viewportRef.current
      if (!viewport) return { x: 0, y: 0 }
      const rect = viewport.getBoundingClientRect()
      return {
        x: (clientX - rect.left - transform.x) / transform.scale,
        y: (clientY - rect.top - transform.y) / transform.scale,
      }
    },
    [transform.scale, transform.x, transform.y],
  )

  const toScreen = useCallback(
    (point: Point) => ({
      x: point.x * transform.scale + transform.x,
      y: point.y * transform.scale + transform.y,
    }),
    [transform.scale, transform.x, transform.y],
  )

  const updateZoom = useCallback(
    (delta: number) => {
      const viewport = viewportRef.current
      if (!viewport) return
      const nextScale = clampScale(transform.scale + delta)
      if (nextScale === transform.scale) return
      const center = { x: viewport.clientWidth / 2, y: viewport.clientHeight / 2 }
      const worldCenter = {
        x: (center.x - transform.x) / transform.scale,
        y: (center.y - transform.y) / transform.scale,
      }
      setTransform({
        scale: nextScale,
        x: center.x - worldCenter.x * nextScale,
        y: center.y - worldCenter.y * nextScale,
      })
    },
    [transform.scale, transform.x, transform.y],
  )

  const handleViewportPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    if (event.target instanceof Element && event.target.closest('[data-pan-stop="true"]')) return
    event.currentTarget.setPointerCapture(event.pointerId)
    panStateRef.current = {
      pointerId: event.pointerId,
      originX: transform.x,
      originY: transform.y,
      startX: event.clientX,
      startY: event.clientY,
    }
  }

  const handleViewportPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (pendingLink) {
      setPendingLink(current =>
        current
          ? {
              ...current,
              pointerWorld: toWorld(event.clientX, event.clientY),
            }
          : current,
      )
      return
    }
    const nodeDrag = nodeDragRef.current
    if (nodeDrag && nodeDrag.pointerId === event.pointerId) {
      const pointerWorld = toWorld(event.clientX, event.clientY)
      const dx = pointerWorld.x - nodeDrag.startPointer.x
      const dy = pointerWorld.y - nodeDrag.startPointer.y
      if (!nodeDrag.moved && (Math.abs(dx) > 3 || Math.abs(dy) > 3)) {
        nodeDrag.moved = true
      }
      setNodePositions(current => ({
        ...current,
        [nodeDrag.projectName]: {
          x: nodeDrag.startNode.x + dx,
          y: nodeDrag.startNode.y + dy,
        },
      }))
      return
    }
    const panState = panStateRef.current
    if (!panState || panState.pointerId !== event.pointerId) return
    setTransform(current => ({
      ...current,
      x: panState.originX + (event.clientX - panState.startX),
      y: panState.originY + (event.clientY - panState.startY),
    }))
  }

  const handleViewportPointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    const nodeDrag = nodeDragRef.current
    if (nodeDrag?.pointerId === event.pointerId) {
      if (!nodeDrag.moved) {
        onOpenProject(nodeDrag.projectName)
      }
      nodeDragRef.current = null
    }
    if (panStateRef.current?.pointerId === event.pointerId) {
      panStateRef.current = null
    }
    if (pendingLink) {
      setPendingLink(null)
    }
  }

  const beginLink = (event: React.PointerEvent<HTMLButtonElement>, source: string, side: Side) => {
    event.stopPropagation()
    const pointerWorld = toWorld(event.clientX, event.clientY)
    setPendingLink({ source, side, pointerWorld })
  }

  const completeLink = async (event: React.PointerEvent<HTMLButtonElement>, target: string) => {
    event.stopPropagation()
    if (!pendingLink || pendingLink.source === target || linkBusy) {
      setPendingLink(null)
      return
    }
    setLinkBusy(true)
    try {
      await onLinkProjects(pendingLink.source, target)
    } finally {
      setPendingLink(null)
      setLinkBusy(false)
    }
  }

  const pendingLinkPath = useMemo(() => {
    if (!pendingLink) return null
    const sourceNode = layout.nodes.find(node => node.entry.project.name === pendingLink.source)
    if (!sourceNode) return null
    const start = nodeHandle(sourceNode, pendingLink.side)
    const end = pendingLink.pointerWorld
    const laneX = end.x >= start.x ? Math.max(start.x, end.x) + EDGE_LANE_GAP : Math.min(start.x, end.x) - EDGE_LANE_GAP
    return buildOrthogonalPath(start, end, laneX).d
  }, [layout.nodes, pendingLink])

  const beginNodeDrag = (event: React.PointerEvent<HTMLDivElement>, node: CanvasNode) => {
    if (event.button !== 0) return
    if (event.target instanceof Element && event.target.closest('button')) return
    event.stopPropagation()
    event.currentTarget.setPointerCapture(event.pointerId)
    const pointerWorld = toWorld(event.clientX, event.clientY)
    nodeDragRef.current = {
      pointerId: event.pointerId,
      projectName: node.entry.project.name,
      startPointer: pointerWorld,
      startNode: { x: node.x, y: node.y },
      moved: false,
    }
  }

  return (
    <div className="relative overflow-hidden rounded-[28px] border border-border/60 bg-slate-100/90 shadow-[0_24px_80px_-48px_rgba(15,23,42,0.55)]">
      <style>{`
        @keyframes relationship-dash {
          from { stroke-dashoffset: 0; }
          to { stroke-dashoffset: -20; }
        }
      `}</style>

      <div className="flex items-center justify-between border-b border-slate-200/80 bg-white px-5 py-3">
        <div>
          <h3 className="text-sm font-semibold text-slate-900">Relationship</h3>
          <p className="text-xs text-slate-500">Drag handles to link projects and hover a line to unlink.</p>
        </div>
        <Badge variant="outline" className="bg-slate-50 text-[11px] text-slate-600">
          {links.length.toLocaleString()} links
        </Badge>
      </div>

      <div
        ref={viewportRef}
        className={cn(
          'relative h-[720px] overflow-hidden',
          'bg-[radial-gradient(circle_at_center,rgba(148,163,184,0.22)_1px,transparent_1.5px)] bg-[length:18px_18px]',
          'bg-slate-50',
        )}
        onPointerDown={handleViewportPointerDown}
        onPointerMove={handleViewportPointerMove}
        onPointerUp={handleViewportPointerUp}
      >
        <div className="pointer-events-none absolute inset-0 bg-[linear-gradient(180deg,rgba(255,255,255,0.52),rgba(248,250,252,0.18))]" />

        <div
          className="absolute left-0 top-0 origin-top-left"
          style={{
            width: layout.bounds.maxX + VIEW_PADDING,
            height: layout.bounds.maxY + VIEW_PADDING,
            transform: `translate(${transform.x}px, ${transform.y}px) scale(${transform.scale})`,
          }}
        >
          <svg
            className="absolute left-0 top-0 overflow-visible"
            width={layout.bounds.maxX + VIEW_PADDING}
            height={layout.bounds.maxY + VIEW_PADDING}
            viewBox={`0 0 ${layout.bounds.maxX + VIEW_PADDING} ${layout.bounds.maxY + VIEW_PADDING}`}
            fill="none"
          >
            {layout.edges.map(edge => {
              const active = hoveredEdgeKey === edge.link.key
              return (
                <g key={edge.link.key}>
                  <path
                    d={edge.d}
                    stroke={active ? '#60a5fa' : '#93c5fd'}
                    strokeWidth={active ? 3 : 2.5}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    fill="none"
                  />
                  <path
                    d={edge.d}
                    stroke={active ? '#3b82f6' : '#60a5fa'}
                    strokeWidth={active ? 2.1 : 1.45}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeDasharray="6 8"
                    style={{ animation: 'relationship-dash 1.2s linear infinite' }}
                    fill="none"
                  />
                  <path
                    d={edge.d}
                    stroke="rgba(15,23,42,0.001)"
                    strokeWidth={24}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    pointerEvents="stroke"
                    data-pan-stop="true"
                    onPointerEnter={() => setHoveredEdgeKey(edge.link.key)}
                    onPointerLeave={() => setHoveredEdgeKey(current => (current === edge.link.key ? null : current))}
                    onClick={(event) => {
                      event.stopPropagation()
                      onRequestUnlink(edge.link.a, edge.link.b)
                    }}
                  />
                </g>
              )
            })}

            {pendingLinkPath ? (
              <path
                d={pendingLinkPath}
                stroke="#60a5fa"
                strokeWidth={2}
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeDasharray="6 8"
                fill="none"
              />
            ) : null}
          </svg>

          {layout.nodes.map(node => {
            const languageSummary = formatLanguageSummary(node.entry)
            const isHovered = hoveredNode === node.entry.project.name
            const handles: Side[] = ['top', 'right', 'bottom', 'left']
            return (
              <div
                key={node.entry.project.name}
                className="group absolute"
                style={{ left: node.x, top: node.y, width: node.width, height: node.height }}
                onPointerEnter={() => setHoveredNode(node.entry.project.name)}
                onPointerLeave={() => setHoveredNode(current => (current === node.entry.project.name ? null : current))}
                data-pan-stop="true"
                onPointerDown={(event) => beginNodeDrag(event, node)}
              >
                  <div
                    className={cn(
                    'relative h-full cursor-grab select-none rounded-[18px] border border-slate-200 bg-white px-4 py-3 shadow-sm',
                    'transition-[transform,box-shadow,border-color] duration-200',
                    'group-hover:-translate-y-0.5 group-hover:border-sky-200 group-hover:shadow-md',
                    nodeDragRef.current?.projectName === node.entry.project.name ? 'cursor-grabbing' : '',
                  )}
                  data-pan-stop="true"
                >
                  <div className="flex select-none items-center gap-3">
                    <ProjectAvatar projectName={node.entry.project.name} className="h-10 w-10 rounded-xl" />
                    <div className="min-w-0 flex-1 select-none">
                      <h4 className="truncate text-[15px] font-semibold text-slate-900">{node.entry.project.name}</h4>
                      <p className="mt-0.5 truncate text-[12px] font-medium text-slate-500">
                        {node.entry.schema.total_nodes.toLocaleString()} nodes · {node.entry.schema.total_edges.toLocaleString()} edges
                      </p>
                      <p className="mt-1 truncate text-[12px] text-slate-400">{languageSummary}</p>
                    </div>
                  </div>

                  <div className="mt-3 flex items-center gap-2">
                    <Badge
                      variant="outline"
                      className="h-5 rounded-full border-sky-100 bg-sky-50 px-2 text-[10px] font-semibold text-sky-700"
                    >
                      {node.entry.links.length} {node.entry.links.length === 1 ? 'link' : 'links'}
                    </Badge>
                  </div>

                  {handles.map(side => {
                    const point = nodeHandle(node, side)
                    const hidden = !isHovered && pendingLink?.source !== node.entry.project.name
                    return (
                      <button
                        key={side}
                        type="button"
                        className={cn(
                          'absolute z-10 h-4 w-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-sky-500 bg-white transition-all',
                          hidden ? 'pointer-events-none opacity-0' : 'pointer-events-auto opacity-100',
                          'hover:scale-110 hover:bg-sky-50 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/70',
                        )}
                        style={{ left: point.x - node.x, top: point.y - node.y }}
                        data-pan-stop="true"
                        onPointerDown={(event) => beginLink(event, node.entry.project.name, side)}
                        onPointerUp={(event) => void completeLink(event, node.entry.project.name)}
                        aria-label={`Link from ${node.entry.project.name}`}
                      />
                    )
                  })}
                </div>
              </div>
            )
          })}
        </div>

        {layout.edges.map(edge => {
          const isActive = hoveredEdgeKey === edge.link.key
          if (!isActive) return null
          const position = toScreen(edge.mid)
          return (
            <button
              key={edge.link.key}
              type="button"
              className="absolute z-20 inline-flex -translate-x-1/2 -translate-y-1/2 items-center gap-1 rounded-full border border-slate-200 bg-white px-2.5 py-1 text-[11px] font-medium text-slate-700 shadow-md"
              style={{ left: position.x, top: position.y }}
              data-pan-stop="true"
              onClick={(event) => {
                event.stopPropagation()
                onRequestUnlink(edge.link.a, edge.link.b)
              }}
            >
              <Unlink className="h-3.5 w-3.5" />
              Unlink
            </button>
          )
        })}

        <div className="absolute bottom-4 right-4 flex items-center gap-2 rounded-full border border-slate-200 bg-white p-1.5 shadow-md">
          <Button
            variant="ghost"
            size="icon"
            className="h-9 w-9 rounded-full"
            data-pan-stop="true"
            onClick={() => updateZoom(-0.12)}
          >
            <Minus className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-9 w-9 rounded-full"
            data-pan-stop="true"
            onClick={() => updateZoom(0.12)}
          >
            <Plus className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-9 w-9 rounded-full"
            data-pan-stop="true"
            onClick={fitToViewport}
          >
            <Crosshair className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </div>
  )
}

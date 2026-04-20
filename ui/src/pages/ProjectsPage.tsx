import { useState, useEffect, useMemo, useCallback, useRef, useLayoutEffect, useId, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useNavigate } from 'react-router-dom'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { callTool } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Project, SchemaInfo } from '@/lib/types'
import {
  Loader2, RefreshCw, RotateCcw, Trash2, Link2, Unlink, Sparkles,
  FolderOpen, ArrowDown, AlertTriangle, ArrowRight,
} from 'lucide-react'

const LABEL_COLORS: Record<string, string> = {
  Project: '#c62828', Folder: '#37474f', File: '#546e7a', Module: '#7b1fa2',
  Class: '#e64a19', Function: '#1976d2', Method: '#388e3c', Interface: '#f9a825',
}

const GRAPH_TYPE_PREVIEW = 4

/** Config / markup-only languages — hide from stack badges to reduce noise */
const SKIP_LANGS = new Set([
  'JSON', 'YAML', 'Markdown', 'TOML', 'XML', 'INI', 'Makefile', 'CSS', 'SCSS', 'HTML',
  'GraphQL', 'SQL', 'CMake', 'Meson', 'Kustomize', 'VimScript', 'Unknown',
])

/** [devicon slug, variant] — icons from https://devicon.dev/ (CDN mirror: devicons/devicon) */
const LANG_MAP: Record<string, [string, string]> = {
  Java: ['java', 'original'], Kotlin: ['kotlin', 'original'], TypeScript: ['typescript', 'original'],
  TSX: ['react', 'original'], JavaScript: ['javascript', 'original'], Python: ['python', 'original'],
  Rust: ['rust', 'original'], Go: ['go', 'original-wordmark'], 'C#': ['csharp', 'original'],
  'C++': ['cplusplus', 'original'], C: ['c', 'original'], Ruby: ['ruby', 'original'], PHP: ['php', 'original'],
  Swift: ['swift', 'original'], Dart: ['dart', 'original'], Scala: ['scala', 'original'],
  Elixir: ['elixir', 'original'], Haskell: ['haskell', 'original'], Lua: ['lua', 'original'],
  Perl: ['perl', 'original'], R: ['r', 'original'], Julia: ['julia', 'original'], Vue: ['vuejs', 'original'],
  Svelte: ['svelte', 'original'], Bash: ['bash', 'original'], Zig: ['zig', 'original'],
  Elm: ['elm', 'original'], Clojure: ['clojure', 'original'], Erlang: ['erlang', 'original'],
  Groovy: ['gradle', 'original'], Dockerfile: ['docker', 'original'], Nix: ['nixos', 'original'],
  OCaml: ['ocaml', 'original'], 'F#': ['fsharp', 'original'], MATLAB: ['matlab', 'original'],
  HCL: ['terraform', 'original'], Fortran: ['fortran', 'original'], COBOL: ['cobol', 'original'],
  Verilog: ['verilog', 'original'], Protobuf: ['protobuf', 'original'], CUDA: ['cuda', 'original'],
  GLSL: ['opengl', 'original'],
}

const FW_MAP: Record<string, [string, string]> = {
  'Spring Boot': ['spring', 'original'],
  Angular: ['angular', 'original'],
  'Next.js': ['nextjs', 'plain'],
  'AWS Lambda': ['amazonwebservices', 'plain-wordmark'],
  Serverless: ['amazonwebservices', 'plain-wordmark'],
  Express: ['express', 'original'],
}

const LIB_MAP: Record<string, [string, string]> = {
  React: ['react', 'original'],
  'Solid.js': ['solidjs', 'plain'],
  Vue: ['vuejs', 'original'],
  'Node.js': ['nodejs', 'plain'],
}

function deviconUrl(slug: string, variant: string) {
  return `https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/${slug}/${slug}-${variant}.svg`
}

interface LangRow { language: string; count: number }
interface LangInfo {
  languages?: LangRow[]
  frameworks?: string[]
  libraries?: string[]
}

interface ProjectEntry { project: Project; schema: SchemaInfo; links: string[]; languages: LangInfo }

function formatDate(iso: string) {
  return new Date(iso).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

/** Normalize `/api/languages` JSON (objects with language+count, not string[]). */
function normalizeLangInfo(raw: unknown): LangInfo {
  if (!raw || typeof raw !== 'object') return { languages: [], frameworks: [], libraries: [] }
  const o = raw as Record<string, unknown>
  const frameworks = Array.isArray(o.frameworks)
    ? o.frameworks.filter((x): x is string => typeof x === 'string')
    : []
  const libraries = Array.isArray(o.libraries)
    ? o.libraries.filter((x): x is string => typeof x === 'string')
    : []
  const languages: LangRow[] = []
  if (Array.isArray(o.languages)) {
    for (const item of o.languages) {
      if (typeof item === 'string') languages.push({ language: item, count: 0 })
      else if (item && typeof item === 'object' && 'language' in item) {
        const row = item as { language?: unknown; count?: unknown }
        if (typeof row.language === 'string')
          languages.push({ language: row.language, count: Number(row.count) || 0 })
      }
    }
  }
  return { languages, frameworks, libraries }
}

interface TechBadge { key: string; label: string; icon?: string; title: string }

function getTechStackBadges(info: LangInfo): TechBadge[] {
  const out: TechBadge[] = []
  const seen = new Set<string>()
  const add = (b: TechBadge) => {
    if (seen.has(b.key)) return
    seen.add(b.key)
    out.push(b)
  }

  for (const fw of info.frameworks ?? []) {
    const m = FW_MAP[fw]
    add({
      key: `fw:${fw}`,
      label: fw,
      icon: m ? deviconUrl(m[0], m[1]) : undefined,
      title: m ? `${fw} (framework)` : `${fw} (framework, no Devicon mapping)`,
    })
  }

  const libSet = new Set(info.libraries ?? [])
  for (const lib of info.libraries ?? []) {
    const m = LIB_MAP[lib]
    add({
      key: `lib:${lib}`,
      label: lib,
      icon: m ? deviconUrl(m[0], m[1]) : undefined,
      title: m ? `${lib} (library)` : `${lib} (library, no Devicon mapping)`,
    })
  }

  const langRows = [...(info.languages ?? [])].sort((a, b) => b.count - a.count)
  for (const { language: lang, count } of langRows) {
    if (SKIP_LANGS.has(lang)) continue
    if (libSet.has('React') && lang === 'TSX') continue
    const m = LANG_MAP[lang]
    add({
      key: `lang:${lang}`,
      label: count > 0 ? `${lang} (${count.toLocaleString()})` : lang,
      icon: m ? deviconUrl(m[0], m[1]) : undefined,
      title: m
        ? `${lang}: ${count.toLocaleString()} indexed files`
        : `${lang}: ${count.toLocaleString()} indexed files (no Devicon slug; see devicon.dev)`,
    })
  }
  return out
}

const LINK_LINE = '#1976d2'
const LINK_LINE_HOVER = '#0d47a1'

/** Default trunk depth: just below the lower of the two linked cards (tight U, few corners). */
const LINK_COMPACT_PAD = 14
/** Minimum vertical gap between different links’ horizontal trunks. */
const LINK_LANE_SPACING = 18
/** Stub before horizontal jog on the elaborate fallback path only. */
const LINK_STUB_DOWN = 10

interface CardObstacle {
  name: string
  left: number
  top: number
  right: number
  bottom: number
}

function verticalBandHitsObstacle(
  x: number,
  yLo: number,
  yHi: number,
  obstacles: CardObstacle[],
  exclude: Set<string>,
): boolean {
  const lo = Math.min(yLo, yHi)
  const hi = Math.max(yLo, yHi)
  for (const o of obstacles) {
    if (exclude.has(o.name)) continue
    if (x < o.left || x > o.right) continue
    if (hi < o.top || lo > o.bottom) continue
    return true
  }
  return false
}

function horizontalSegmentHitsObstacle(
  y: number,
  x1: number,
  x2: number,
  obstacles: CardObstacle[],
  exclude: Set<string>,
): boolean {
  const xl = Math.min(x1, x2)
  const xh = Math.max(x1, x2)
  for (const o of obstacles) {
    if (exclude.has(o.name)) continue
    if (y < o.top || y > o.bottom) continue
    if (xh < o.left || xl > o.right) continue
    return true
  }
  return false
}

function axisSegmentHitsObstacle(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  obstacles: CardObstacle[],
  exclude: Set<string>,
): boolean {
  if (Math.abs(x1 - x2) < 0.5) {
    return verticalBandHitsObstacle(x1, y1, y2, obstacles, exclude)
  }
  if (Math.abs(y1 - y2) < 0.5) {
    return horizontalSegmentHitsObstacle(y1, x1, x2, obstacles, exclude)
  }
  return false
}

function orthogonalPolylineHitsObstacles(
  points: [number, number][],
  obstacles: CardObstacle[],
  exclude: Set<string>,
): boolean {
  for (let i = 0; i < points.length - 1; i++) {
    const [ax, ay] = points[i]
    const [bx, by] = points[i + 1]
    if (axisSegmentHitsObstacle(ax, ay, bx, by, obstacles, exclude)) return true
  }
  return false
}

function pickClearX(
  xPref: number,
  yLo: number,
  yHi: number,
  obstacles: CardObstacle[],
  exclude: Set<string>,
  widenLevel = 0,
): number {
  const base = [0, 56, -56, 112, -112, 168, -168, 220, -220, 280, -280, 360, -360]
  const extra: number[] = []
  for (let w = 1; w <= widenLevel; w++) {
    extra.push(w * 44, -w * 44)
  }
  for (const dx of [...base, ...extra]) {
    const x = xPref + dx
    if (!verticalBandHitsObstacle(x, yLo, yHi, obstacles, exclude)) return x
  }
  return xPref
}

/** Tight “U”: bottom → down → across → up (2 corners). */
function buildSimpleU(
  p: { xa: number; ya: number; xb: number; yb: number },
  yMid: number,
): [number, number][] {
  return [
    [p.xa, p.ya],
    [p.xa, yMid],
    [p.xb, yMid],
    [p.xb, p.yb],
  ]
}

function buildLinkPoints(
  p: { xa: number; ya: number; xb: number; yb: number },
  xA: number,
  xB: number,
  yLandA: number,
  yLandB: number,
  yMid: number,
): [number, number][] {
  return [
    [p.xa, p.ya],
    [p.xa, yLandA],
    [xA, yLandA],
    [xA, yMid],
    [xB, yMid],
    [xB, yLandB],
    [p.xb, yLandB],
    [p.xb, p.yb],
  ]
}

function pathDFromPoints(pts: [number, number][]): string {
  return pts.map(([x, y], i) => `${i === 0 ? 'M' : 'L'} ${x} ${y}`).join(' ')
}

interface LinkPathSeg {
  key: string
  a: string
  b: string
  d: string
  labelX: number
  labelY: number
}

function ProjectLinkOverlay({
  wrapRef,
  gridRef,
  cardRefs,
  links,
  hoverKey,
  setHoverKey,
  onLineClick,
  children,
}: {
  wrapRef: React.RefObject<HTMLDivElement | null>
  gridRef: React.RefObject<HTMLDivElement | null>
  cardRefs: React.MutableRefObject<Record<string, HTMLDivElement | null>>
  links: { key: string; a: string; b: string }[]
  hoverKey: string | null
  setHoverKey: (k: string | null) => void
  onLineClick: (a: string, b: string) => void
  children: ReactNode
}) {
  const uid = useId().replace(/:/g, '')
  const arrowId = `proj-link-arr-${uid}`
  const arrowHoverId = `proj-link-arr-h-${uid}`

  const [geom, setGeom] = useState<{
    w: number
    h: number
    segments: LinkPathSeg[]
  }>({ w: 0, h: 0, segments: [] })
  const [hoverClient, setHoverClient] = useState<{ x: number; y: number } | null>(null)
  const hoverClearTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const linkSig = useMemo(() => [...links].sort((a, b) => a.key.localeCompare(b.key)).map(l => l.key).join('|'), [links])

  const measure = useCallback(() => {
    const wrap = wrapRef.current
    if (!wrap) return
    const cr = wrap.getBoundingClientRect()
    const sl = wrap.scrollLeft
    const st = wrap.scrollTop
    let bw = Math.max(cr.width, 1)
    let bh = Math.max(cr.height, 1)

    const obstacles: CardObstacle[] = Object.entries(cardRefs.current)
      .filter((e): e is [string, HTMLDivElement] => e[1] != null)
      .map(([name, el]) => {
        const r = el.getBoundingClientRect()
        return {
          name,
          left: r.left - cr.left + sl,
          top: r.top - cr.top + st,
          right: r.right - cr.left + sl,
          bottom: r.bottom - cr.top + st,
        }
      })

    const sorted = [...links].sort((a, b) => a.key.localeCompare(b.key))
    const pairGeoms: { L: (typeof links)[0]; xa: number; ya: number; xb: number; yb: number }[] = []
    for (const L of sorted) {
      const elA = cardRefs.current[L.a]
      const elB = cardRefs.current[L.b]
      if (!elA || !elB) continue
      const ra = elA.getBoundingClientRect()
      const rb = elB.getBoundingClientRect()
      const xa = ra.left + ra.width / 2 - cr.left + sl
      const ya = ra.bottom - cr.top + st
      const xb = rb.left + rb.width / 2 - cr.left + sl
      const yb = rb.bottom - cr.top + st
      pairGeoms.push({ L, xa, ya, xb, yb })
      bw = Math.max(bw, xa + 8, xb + 8)
    }

    const segs: LinkPathSeg[] = []
    if (pairGeoms.length > 0) {
      const pairs = [...pairGeoms].sort((a, b) => {
        const da = Math.max(a.ya, a.yb)
        const db = Math.max(b.ya, b.yb)
        return da - db || a.L.key.localeCompare(b.L.key)
      })
      let prevYMid = -Infinity
      for (const p of pairs) {
        const exclude = new Set([p.L.a, p.L.b])
        let yMid = Math.max(Math.max(p.ya, p.yb) + LINK_COMPACT_PAD, prevYMid + LINK_LANE_SPACING)
        let g = 0
        while (g < 48 && horizontalSegmentHitsObstacle(yMid, p.xa, p.xb, obstacles, exclude)) {
          g++
          yMid += 8
        }
        let raised = 0
        while (
          raised < 40
          && orthogonalPolylineHitsObstacles(buildSimpleU(p, yMid), obstacles, exclude)
        ) {
          raised++
          yMid += 8
        }

        let pts: [number, number][] = buildSimpleU(p, yMid)
        let labelX = (p.xa + p.xb) / 2
        if (!orthogonalPolylineHitsObstacles(pts, obstacles, exclude)) {
          prevYMid = yMid
          segs.push({
            key: p.L.key,
            a: p.L.a,
            b: p.L.b,
            d: pathDFromPoints(pts),
            labelX,
            labelY: yMid,
          })
          bh = Math.max(bh, yMid + 28)
          continue
        }

        let attempt = 0
        let yLandA = p.ya + LINK_STUB_DOWN
        let yLandB = p.yb + LINK_STUB_DOWN
        let widen = 0
        let xA = pickClearX(p.xa, Math.min(yLandA, yMid), Math.max(yLandA, yMid), obstacles, exclude, widen)
        let xB = pickClearX(p.xb, Math.min(yLandB, yMid), Math.max(yLandB, yMid), obstacles, exclude, widen)
        pts = buildLinkPoints(p, xA, xB, yLandA, yLandB, yMid)
        const yLandCeil = yMid - 10
        while (orthogonalPolylineHitsObstacles(pts, obstacles, exclude) && attempt < 56) {
          attempt++
          yLandA = Math.min(p.ya + LINK_STUB_DOWN + attempt * 10, yLandCeil)
          yLandB = Math.min(p.yb + LINK_STUB_DOWN + attempt * 10, yLandCeil)
          widen = Math.min(12, Math.floor(attempt / 3))
          xA = pickClearX(p.xa, Math.min(yLandA, yMid), Math.max(yLandA, yMid), obstacles, exclude, widen)
          xB = pickClearX(p.xb, Math.min(yLandB, yMid), Math.max(yLandB, yMid), obstacles, exclude, widen)
          pts = buildLinkPoints(p, xA, xB, yLandA, yLandB, yMid)
        }
        labelX = (xA + xB) / 2
        prevYMid = yMid
        segs.push({
          key: p.L.key,
          a: p.L.a,
          b: p.L.b,
          d: pathDFromPoints(pts),
          labelX,
          labelY: yMid,
        })
        bw = Math.max(bw, xA + 16, xB + 16, p.xa + 8, p.xb + 8)
        bh = Math.max(bh, yMid + 32, yLandA + 8, yLandB + 8)
      }
    }

    setGeom({ w: bw, h: bh, segments: segs })
  }, [links, wrapRef, cardRefs])

  useLayoutEffect(() => {
    measure()
    const wrap = wrapRef.current
    const grid = gridRef.current
    if (!wrap) return
    const ro = new ResizeObserver(measure)
    ro.observe(wrap)
    if (grid) ro.observe(grid)
    const t = window.setTimeout(measure, 50)
    const t2 = window.setTimeout(measure, 200)
    const t3 = window.setTimeout(measure, 500)
    let raf2 = 0
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(measure)
    })
    window.addEventListener('resize', measure)
    return () => {
      ro.disconnect()
      clearTimeout(t)
      clearTimeout(t2)
      clearTimeout(t3)
      cancelAnimationFrame(raf1)
      cancelAnimationFrame(raf2)
      window.removeEventListener('resize', measure)
    }
  }, [measure, wrapRef, gridRef])

  /** First paint after data + cards mount often needs extra passes (images, fonts, route stay). */
  useEffect(() => {
    const ts = [0, 80, 250, 500, 1200].map(ms => window.setTimeout(measure, ms))
    return () => ts.forEach(clearTimeout)
  }, [linkSig, measure])

  useEffect(() => {
    return () => {
      if (hoverClearTimer.current) clearTimeout(hoverClearTimer.current)
    }
  }, [])

  const hoverSeg = geom.segments.find(s => s.key === hoverKey)

  const vb = geom.w > 0 && geom.h > 0 ? `0 0 ${geom.w} ${geom.h}` : '0 0 1 1'

  return (
    <>
      {/* Thin lines under cards; no pointer events */}
      <svg
        className="pointer-events-none absolute inset-0 z-[1] h-full w-full select-none"
        viewBox={vb}
        preserveAspectRatio="none"
        style={{ overflow: 'visible' }}
        aria-hidden
      >
        <defs>
          <marker
            id={arrowId}
            markerWidth="8"
            markerHeight="8"
            refX="7"
            refY="4"
            orient="auto"
            markerUnits="userSpaceOnUse"
          >
            <path d="M 0 0 L 8 4 L 0 8 Z" fill={LINK_LINE} />
          </marker>
          <marker
            id={arrowHoverId}
            markerWidth="8"
            markerHeight="8"
            refX="7"
            refY="4"
            orient="auto"
            markerUnits="userSpaceOnUse"
          >
            <path d="M 0 0 L 8 4 L 0 8 Z" fill={LINK_LINE_HOVER} />
          </marker>
        </defs>
        {geom.segments.map(s => {
          const active = hoverKey === s.key
          const endMarker = active ? `url(#${arrowHoverId})` : `url(#${arrowId})`
          return (
            <path
              key={s.key}
              d={s.d}
              fill="none"
              stroke={active ? LINK_LINE_HOVER : LINK_LINE}
              strokeWidth={active ? 3 : 2}
              strokeOpacity={active ? 0.95 : 0.72}
              strokeLinecap="round"
              strokeLinejoin="round"
              markerEnd={endMarker}
            />
          )
        })}
      </svg>

      {children}

      {/* Invisible wide strokes above the grid so hover/click hit the edge, not the card underneath */}
      <svg
        className="absolute inset-0 z-[5] h-full w-full select-none overflow-visible [pointer-events:box-none]"
        viewBox={vb}
        preserveAspectRatio="none"
        aria-hidden
      >
        {geom.segments.map(s => (
          <path
            key={s.key}
            d={s.d}
            fill="none"
            stroke="rgba(0,0,0,0.004)"
            strokeWidth={28}
            strokeLinecap="round"
            strokeLinejoin="round"
            vectorEffect="non-scaling-stroke"
            pointerEvents="stroke"
            className="cursor-pointer"
            onPointerEnter={(e) => {
              if (hoverClearTimer.current) {
                clearTimeout(hoverClearTimer.current)
                hoverClearTimer.current = null
              }
              setHoverKey(s.key)
              setHoverClient({ x: e.clientX, y: e.clientY })
            }}
            onPointerMove={(e) => {
              setHoverClient({ x: e.clientX, y: e.clientY })
            }}
            onPointerLeave={() => {
              hoverClearTimer.current = setTimeout(() => {
                setHoverKey(null)
                setHoverClient(null)
                hoverClearTimer.current = null
              }, 180)
            }}
            onClick={(e) => {
              e.preventDefault()
              e.stopPropagation()
              onLineClick(s.a, s.b)
            }}
          />
        ))}
      </svg>

      {hoverSeg && hoverClient
        ? createPortal(
            <div
              role="tooltip"
              className="pointer-events-none fixed z-[9999] max-w-[240px] rounded-md border border-border bg-popover px-2 py-1 text-center text-[10px] font-medium text-popover-foreground shadow-md"
              style={{
                left: hoverClient.x,
                top: hoverClient.y,
                transform: 'translate(-50%, calc(-100% - 10px))',
              }}
            >
              Unlink {hoverSeg.a} ↔ {hoverSeg.b}
            </div>,
            document.body,
          )
        : null}
    </>
  )
}

export default function ProjectsPage() {
  const navigate = useNavigate()
  const [allProjects, setAllProjects] = useState<ProjectEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [reindexing, setReindexing] = useState<Record<string, boolean>>({})
  const [reindexResult, setReindexResult] = useState<Record<string, string>>({})
  const [deleteTarget, setDeleteTarget] = useState('')
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false)

  const [linkDialogOpen, setLinkDialogOpen] = useState(false)
  const [dialogSource, setDialogSource] = useState('')
  const [dialogTarget, setDialogTarget] = useState('')

  const refresh = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      const { projects } = await callTool<{ projects: Project[] }>('list_projects')
      const entries = await Promise.all(
        projects.map(async (p) => {
          const [schema, linksRes, languages] = await Promise.all([
            callTool<SchemaInfo>('get_graph_schema', { project: p.name }),
            callTool<{ linked_projects: string[] }>('list_project_links', { project: p.name }),
            fetch('/api/languages?project=' + encodeURIComponent(p.name))
              .then(r => r.json())
              .then(normalizeLangInfo)
              .catch(() => normalizeLangInfo(null)),
          ])
          return { project: p, schema, links: linksRes.linked_projects ?? [], languages }
        })
      )
      setAllProjects(entries)
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { refresh() }, [refresh])

  const allLinks = useMemo(() => {
    const linkPairs: { key: string; a: string; b: string }[] = []
    const seen = new Set<string>()
    for (const e of allProjects) {
      for (const l of e.links) {
        const key = [e.project.name, l].sort().join('::')
        if (!seen.has(key)) {
          seen.add(key)
          linkPairs.push({ key, a: e.project.name, b: l })
        }
      }
    }
    return linkPairs
  }, [allProjects])

  const sortedProjects = useMemo(
    () => [...allProjects].sort((a, b) => a.project.name.localeCompare(b.project.name)),
    [allProjects],
  )

  const gridWrapRef = useRef<HTMLDivElement>(null)
  const gridInnerRef = useRef<HTMLDivElement>(null)
  const cardRefs = useRef<Record<string, HTMLDivElement | null>>({})
  const [linkHoverKey, setLinkHoverKey] = useState<string | null>(null)
  const [unlinkPair, setUnlinkPair] = useState<{ a: string; b: string } | null>(null)

  const drawableLinks = useMemo(() => {
    const names = new Set(allProjects.map(e => e.project.name))
    const byLower = new Map(allProjects.map(e => [e.project.name.toLowerCase(), e.project.name]))
    const resolve = (n: string) => byLower.get(n.trim().toLowerCase()) ?? n
    const out: { key: string; a: string; b: string }[] = []
    const seen = new Set<string>()
    for (const l of allLinks) {
      const a = resolve(l.a)
      const b = resolve(l.b)
      if (!names.has(a) || !names.has(b)) continue
      const key = [a, b].sort().join('::')
      if (seen.has(key)) continue
      seen.add(key)
      out.push({ key, a, b })
    }
    return out
  }, [allLinks, allProjects])

  const linkOverlayPadBottom = useMemo(() => {
    const n = drawableLinks.length
    return 40 + LINK_COMPACT_PAD * 2 + Math.max(0, n - 1) * LINK_LANE_SPACING
  }, [drawableLinks.length])

  const dialogTargetOptions = useMemo(() => {
    if (!dialogSource) return allProjects.map(e => e.project.name)
    const src = allProjects.find(e => e.project.name === dialogSource)
    const linked = new Set(src?.links ?? [])
    return allProjects.map(e => e.project.name).filter(n => n !== dialogSource && !linked.has(n))
  }, [allProjects, dialogSource])

  const dialogAlreadyLinked = useMemo(() => {
    if (!dialogSource || !dialogTarget) return false
    const src = allProjects.find(e => e.project.name === dialogSource)
    return src?.links.includes(dialogTarget) ?? false
  }, [allProjects, dialogSource, dialogTarget])

  const doReindex = async (name: string, rootPath: string) => {
    setReindexing(p => ({ ...p, [name]: true }))
    setReindexResult(p => ({ ...p, [name]: '' }))
    try {
      await callTool('index_repository', { path: rootPath })
      setReindexResult(p => ({ ...p, [name]: 'done' }))
      refresh()
    } catch (e: unknown) {
      setReindexResult(p => ({ ...p, [name]: e instanceof Error ? e.message : 'error' }))
    } finally {
      setReindexing(p => ({ ...p, [name]: false }))
    }
  }

  const doDelete = async () => {
    if (!deleteTarget) return
    try {
      await callTool('delete_project', { project: deleteTarget })
      setDeleteDialogOpen(false)
      setDeleteTarget('')
      refresh()
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
      setDeleteDialogOpen(false)
    }
  }

  const doLink = async () => {
    if (!dialogSource || !dialogTarget) return
    try {
      await callTool('link_project', { project: dialogSource, target_project: dialogTarget, action: 'link' })
      setLinkDialogOpen(false)
      setDialogSource('')
      setDialogTarget('')
      refresh()
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const doUnlink = async (a: string, b: string) => {
    try {
      await callTool('link_project', { project: a, target_project: b, action: 'unlink' })
      refresh()
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const renderCard = (entry: ProjectEntry) => {
    const { project: p, schema, languages } = entry
    const techBadges = getTechStackBadges(languages)
    const openGraph = () => navigate('/graph?project=' + encodeURIComponent(p.name))
    const sortedLabels = [...schema.node_labels].sort((a, b) => b.count - a.count)
    const typePreview = sortedLabels.slice(0, GRAPH_TYPE_PREVIEW)
    const typeOverflow = sortedLabels.length - typePreview.length

    return (
      <div
        key={p.name}
        ref={(el) => { cardRefs.current[p.name] = el }}
        role="button"
        tabIndex={0}
        onClick={openGraph}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            openGraph()
          }
        }}
        className={cn(
          'group relative z-10 flex flex-col rounded-xl border border-border/70 bg-card p-3.5 shadow-sm outline-none',
          'transition-[border-color,box-shadow,transform] duration-200',
          'hover:-translate-y-px hover:border-primary/25 hover:shadow-md',
          'focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
        )}
      >
        <div
          className="pointer-events-none absolute right-2.5 top-2.5 flex items-center gap-0.5 text-[10px] font-medium text-muted-foreground opacity-0 transition-opacity duration-200 group-hover:opacity-100"
          aria-hidden
        >
          Graph
          <ArrowRight className="h-3 w-3" />
        </div>

        {/* Header */}
        <div className="flex gap-2.5 pr-14">
          <div className="shrink-0">
            <img
              src={'/api/logo?project=' + encodeURIComponent(p.name)}
              alt=""
              className="h-9 w-9 rounded-lg border border-border/60 bg-muted/30 object-contain p-0.5 shadow-inner"
              onError={(e) => {
                (e.target as HTMLImageElement).style.display = 'none'
                ;(e.target as HTMLImageElement).nextElementSibling?.classList.remove('hidden')
              }}
            />
            <Sparkles className="hidden h-9 w-9 rounded-lg border border-border/60 bg-muted/50 p-1.5 text-muted-foreground" />
          </div>
          <div className="min-w-0 flex-1 space-y-0.5">
            <h3 className="truncate text-sm font-semibold tracking-tight text-foreground">{p.name}</h3>
            <p className="truncate font-mono text-[10px] leading-snug text-muted-foreground" title={p.root_path}>
              {p.root_path}
            </p>
          </div>
        </div>

        {/* Tech stack */}
        {techBadges.length > 0 ? (
          <div className="mt-2.5">
            <div className="mb-1 text-[9px] font-semibold uppercase tracking-wide text-muted-foreground">Stack</div>
            <div className="flex flex-wrap gap-1">
              {techBadges.map(b => (
                <span
                  key={b.key}
                  title={b.title}
                  className="inline-flex max-w-full items-center gap-1 rounded border border-border/80 bg-background px-1.5 py-px text-[10px] font-medium text-foreground"
                >
                  {b.icon ? (
                    <img src={b.icon} alt="" className="h-3 w-3 shrink-0 opacity-90" loading="lazy" decoding="async" />
                  ) : null}
                  <span className="truncate">{b.label}</span>
                </span>
              ))}
            </div>
          </div>
        ) : null}

        {/* Graph size — neutral metric strip */}
        <div className="mt-2.5 grid grid-cols-3 divide-x divide-border/80 overflow-hidden rounded-lg border border-border/70 bg-muted/25">
          <div className="px-1 py-1.5 text-center">
            <p className="text-[9px] font-medium uppercase tracking-wide text-muted-foreground">Nodes</p>
            <p className="text-sm font-semibold tabular-nums leading-tight text-foreground">
              {schema.total_nodes.toLocaleString()}
            </p>
          </div>
          <div className="px-1 py-1.5 text-center">
            <p className="text-[9px] font-medium uppercase tracking-wide text-muted-foreground">Edges</p>
            <p className="text-sm font-semibold tabular-nums leading-tight text-foreground">
              {schema.total_edges.toLocaleString()}
            </p>
          </div>
          <div className="px-1 py-1.5 text-center">
            <p className="text-[9px] font-medium uppercase tracking-wide text-muted-foreground">Types</p>
            <p className="text-sm font-semibold tabular-nums leading-tight text-foreground">
              {schema.node_labels.length}
            </p>
          </div>
        </div>

        {/* Graph entity kinds — compact, single visual language */}
        {typePreview.length > 0 ? (
          <div className="mt-2">
            <div className="mb-1 text-[9px] font-semibold uppercase tracking-wide text-muted-foreground">In graph</div>
            <div className="flex flex-wrap gap-1">
              {typePreview.map(l => (
                <span
                  key={l.label}
                  className="inline-flex items-center gap-1 rounded border border-border/60 bg-muted/40 px-1.5 py-px text-[10px]"
                >
                  <span
                    className="h-1.5 w-1.5 shrink-0 rounded-full"
                    style={{ backgroundColor: LABEL_COLORS[l.label] ?? '#94a3b8' }}
                  />
                  <span className="font-medium tabular-nums text-foreground">{l.count.toLocaleString()}</span>
                  <span className="text-muted-foreground">{l.label}</span>
                </span>
              ))}
              {typeOverflow > 0 ? (
                <span className="inline-flex items-center rounded border border-dashed border-border/80 px-1.5 py-px text-[10px] text-muted-foreground">
                  +{typeOverflow}
                </span>
              ) : null}
            </div>
          </div>
        ) : null}

        <div className="mt-2 flex items-center justify-between border-t border-border/60 pt-2">
          <time className="text-[10px] text-muted-foreground tabular-nums" dateTime={p.indexed_at}>
            {formatDate(p.indexed_at)}
          </time>
          <div className="flex gap-0">
            <Button
              variant="ghost"
              size="icon"
              className="h-7 w-7 text-muted-foreground hover:text-foreground"
              disabled={reindexing[p.name]}
              onClick={(e) => { e.stopPropagation(); doReindex(p.name, p.root_path) }}
              aria-label={`Re-index ${p.name}`}
            >
              {reindexing[p.name] ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RotateCcw className="h-3.5 w-3.5" />}
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="h-7 w-7 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
              onClick={(e) => { e.stopPropagation(); setDeleteTarget(p.name); setDeleteDialogOpen(true) }}
              aria-label={`Delete ${p.name}`}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>

        {reindexResult[p.name] ? (
          <p
            className={cn(
              'mt-1.5 text-[10px]',
              reindexResult[p.name] === 'done' ? 'text-emerald-600 dark:text-emerald-500' : 'text-destructive',
            )}
          >
            {reindexResult[p.name] === 'done' ? 'Re-indexed successfully' : reindexResult[p.name]}
          </p>
        ) : null}
      </div>
    )
  }

  if (loading) {
    return <div className="flex items-center justify-center py-32"><Loader2 className="h-8 w-8 animate-spin text-muted-foreground" /></div>
  }

  if (error) {
    return <Alert variant="destructive" className="max-w-lg mx-auto mt-12"><AlertDescription>{error}</AlertDescription></Alert>
  }

  if (allProjects.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-32 text-muted-foreground gap-4">
        <FolderOpen className="h-16 w-16" />
        <p className="text-lg">No indexed projects yet</p>
        <Button onClick={() => navigate('/config')}>Index a project</Button>
      </div>
    )
  }

  return (
    <div>
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-semibold">Indexed Projects</h2>
        <div className="flex gap-2">
          {allProjects.length >= 2 && (
            <Dialog open={linkDialogOpen} onOpenChange={setLinkDialogOpen}>
              <DialogTrigger render={<Button variant="outline" size="sm" />}>
                <Link2 className="h-4 w-4 mr-1.5" /> Link Projects
              </DialogTrigger>
              <DialogContent className="sm:max-w-md">
                <DialogHeader><DialogTitle>Link Projects</DialogTitle></DialogHeader>
                <div className="space-y-4 pt-2">
                  <Select value={dialogSource} onValueChange={(v) => { setDialogSource(v ?? ''); setDialogTarget('') }}>
                    <SelectTrigger><SelectValue placeholder="Source project" /></SelectTrigger>
                    <SelectContent>{allProjects.map(e => <SelectItem key={e.project.name} value={e.project.name}>{e.project.name}</SelectItem>)}</SelectContent>
                  </Select>

                  <div className="flex items-center justify-center gap-2 text-muted-foreground text-sm">
                    <ArrowDown className="h-4 w-4" /> links to <ArrowDown className="h-4 w-4" />
                  </div>

                  <Select value={dialogTarget} onValueChange={v => setDialogTarget(v ?? '')} disabled={!dialogSource}>
                    <SelectTrigger><SelectValue placeholder="Target project" /></SelectTrigger>
                    <SelectContent>{dialogTargetOptions.map(n => <SelectItem key={n} value={n}>{n}</SelectItem>)}</SelectContent>
                  </Select>

                  {dialogSource && dialogTarget && (
                    <div className="rounded-lg border p-3 text-sm text-center">
                      <span className="font-medium">{dialogSource}</span> ↔ <span className="font-medium">{dialogTarget}</span>
                    </div>
                  )}

                  {dialogAlreadyLinked && (
                    <Alert><AlertTriangle className="h-4 w-4" /><AlertDescription>These projects are already linked.</AlertDescription></Alert>
                  )}

                  {/* Existing links */}
                  {allLinks.length > 0 && (
                    <div className="space-y-1">
                      <p className="text-xs font-medium text-muted-foreground">Existing links</p>
                      {allLinks.map(l => (
                        <div key={l.key} className="flex items-center justify-between rounded border px-3 py-1.5 text-sm">
                          <span>{l.a} ↔ {l.b}</span>
                          <Button variant="ghost" size="icon" className="h-6 w-6" onClick={() => doUnlink(l.a, l.b)}><Unlink className="h-3 w-3" /></Button>
                        </div>
                      ))}
                    </div>
                  )}

                  <div className="flex justify-end gap-2 pt-2">
                    <Button variant="outline" onClick={() => setLinkDialogOpen(false)}>Cancel</Button>
                    <Button disabled={!dialogSource || !dialogTarget || dialogAlreadyLinked} onClick={doLink}>Link</Button>
                  </div>
                </div>
              </DialogContent>
            </Dialog>
          )}
          <Button variant="outline" size="sm" onClick={refresh}><RefreshCw className="h-4 w-4 mr-1.5" /> Refresh</Button>
        </div>
      </div>

      <div
        ref={gridWrapRef}
        className="relative"
        style={{ paddingBottom: linkOverlayPadBottom }}
      >
        <ProjectLinkOverlay
          wrapRef={gridWrapRef}
          gridRef={gridInnerRef}
          cardRefs={cardRefs}
          links={drawableLinks}
          hoverKey={linkHoverKey}
          setHoverKey={setLinkHoverKey}
          onLineClick={(a, b) => setUnlinkPair({ a, b })}
        >
          <div
            ref={gridInnerRef}
            className="relative z-10 grid gap-x-4 gap-y-16 sm:grid-cols-2 lg:grid-cols-3"
          >
            {sortedProjects.map(e => renderCard(e))}
          </div>
        </ProjectLinkOverlay>
      </div>

      {/* Delete dialog */}
      <Dialog open={deleteDialogOpen} onOpenChange={setDeleteDialogOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader><DialogTitle>Delete Project</DialogTitle></DialogHeader>
          <p className="text-sm py-2">
            Are you sure you want to delete <span className="font-semibold">{deleteTarget}</span>? This removes all indexed data.
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setDeleteDialogOpen(false)}>Cancel</Button>
            <Button variant="destructive" onClick={doDelete}>Delete</Button>
          </div>
        </DialogContent>
      </Dialog>

      <Dialog open={unlinkPair !== null} onOpenChange={(open) => { if (!open) setUnlinkPair(null) }}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader><DialogTitle>Unlink projects</DialogTitle></DialogHeader>
          <p className="text-sm py-2">
            Remove the link between{' '}
            <span className="font-semibold">{unlinkPair?.a}</span> and{' '}
            <span className="font-semibold">{unlinkPair?.b}</span>?
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setUnlinkPair(null)}>Cancel</Button>
            <Button
              variant="destructive"
              onClick={async () => {
                if (!unlinkPair) return
                const { a, b } = unlinkPair
                setUnlinkPair(null)
                await doUnlink(a, b)
              }}
            >
              Unlink
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  )
}

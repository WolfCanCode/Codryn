import type { Project, SchemaInfo } from '@/lib/types'

export const LABEL_COLORS: Record<string, string> = {
  Project: '#c62828',
  Folder: '#37474f',
  File: '#546e7a',
  Module: '#7b1fa2',
  Class: '#e64a19',
  Function: '#1976d2',
  Method: '#388e3c',
  Interface: '#f9a825',
}

export const GRAPH_TYPE_PREVIEW = 4

const SKIP_LANGS = new Set([
  'JSON',
  'YAML',
  'Markdown',
  'TOML',
  'XML',
  'INI',
  'Makefile',
  'CSS',
  'SCSS',
  'HTML',
  'GraphQL',
  'SQL',
  'CMake',
  'Meson',
  'Kustomize',
  'VimScript',
  'Unknown',
])

const LANG_MAP: Record<string, [string, string]> = {
  Java: ['java', 'original'],
  Kotlin: ['kotlin', 'original'],
  TypeScript: ['typescript', 'original'],
  TSX: ['react', 'original'],
  JavaScript: ['javascript', 'original'],
  Python: ['python', 'original'],
  Rust: ['rust', 'original'],
  Go: ['go', 'original-wordmark'],
  'C#': ['csharp', 'original'],
  'C++': ['cplusplus', 'original'],
  C: ['c', 'original'],
  Ruby: ['ruby', 'original'],
  PHP: ['php', 'original'],
  Swift: ['swift', 'original'],
  Dart: ['dart', 'original'],
  Scala: ['scala', 'original'],
  Elixir: ['elixir', 'original'],
  Haskell: ['haskell', 'original'],
  Lua: ['lua', 'original'],
  Perl: ['perl', 'original'],
  R: ['r', 'original'],
  Julia: ['julia', 'original'],
  Vue: ['vuejs', 'original'],
  Svelte: ['svelte', 'original'],
  Bash: ['bash', 'original'],
  Zig: ['zig', 'original'],
  Elm: ['elm', 'original'],
  Clojure: ['clojure', 'original'],
  Erlang: ['erlang', 'original'],
  Groovy: ['gradle', 'original'],
  Dockerfile: ['docker', 'original'],
  Nix: ['nixos', 'original'],
  OCaml: ['ocaml', 'original'],
  'F#': ['fsharp', 'original'],
  MATLAB: ['matlab', 'original'],
  HCL: ['terraform', 'original'],
  Fortran: ['fortran', 'original'],
  COBOL: ['cobol', 'original'],
  Verilog: ['verilog', 'original'],
  Protobuf: ['protobuf', 'original'],
  CUDA: ['cuda', 'original'],
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

export interface LangRow {
  language: string
  count: number
}

export interface LangInfo {
  languages?: LangRow[]
  frameworks?: string[]
  libraries?: string[]
}

export interface ProjectEntry {
  project: Project
  schema: SchemaInfo
  links: string[]
  languages: LangInfo
}

export interface ProjectLink {
  key: string
  a: string
  b: string
}

export interface TechBadge {
  key: string
  label: string
  icon?: string
  title: string
}

export function formatDate(iso: string) {
  return new Date(iso).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function normalizeLangInfo(raw: unknown): LangInfo {
  if (!raw || typeof raw !== 'object') return { languages: [], frameworks: [], libraries: [] }
  const objectValue = raw as Record<string, unknown>
  const frameworks = Array.isArray(objectValue.frameworks)
    ? objectValue.frameworks.filter((value): value is string => typeof value === 'string')
    : []
  const libraries = Array.isArray(objectValue.libraries)
    ? objectValue.libraries.filter((value): value is string => typeof value === 'string')
    : []
  const languages: LangRow[] = []
  if (Array.isArray(objectValue.languages)) {
    for (const item of objectValue.languages) {
      if (typeof item === 'string') {
        languages.push({ language: item, count: 0 })
        continue
      }
      if (item && typeof item === 'object' && 'language' in item) {
        const row = item as { language?: unknown; count?: unknown }
        if (typeof row.language === 'string') {
          languages.push({ language: row.language, count: Number(row.count) || 0 })
        }
      }
    }
  }
  return { languages, frameworks, libraries }
}

export function getTechStackBadges(info: LangInfo): TechBadge[] {
  const out: TechBadge[] = []
  const seen = new Set<string>()
  const add = (badge: TechBadge) => {
    if (seen.has(badge.key)) return
    seen.add(badge.key)
    out.push(badge)
  }

  for (const framework of info.frameworks ?? []) {
    const iconMeta = FW_MAP[framework]
    add({
      key: `fw:${framework}`,
      label: framework,
      icon: iconMeta ? deviconUrl(iconMeta[0], iconMeta[1]) : undefined,
      title: iconMeta ? `${framework} (framework)` : `${framework} (framework, no icon mapping)`,
    })
  }

  const librarySet = new Set(info.libraries ?? [])
  for (const library of info.libraries ?? []) {
    const iconMeta = LIB_MAP[library]
    add({
      key: `lib:${library}`,
      label: library,
      icon: iconMeta ? deviconUrl(iconMeta[0], iconMeta[1]) : undefined,
      title: iconMeta ? `${library} (library)` : `${library} (library, no icon mapping)`,
    })
  }

  const langRows = [...(info.languages ?? [])].sort((a, b) => b.count - a.count)
  for (const { language, count } of langRows) {
    if (SKIP_LANGS.has(language)) continue
    if (librarySet.has('React') && language === 'TSX') continue
    const iconMeta = LANG_MAP[language]
    add({
      key: `lang:${language}`,
      label: count > 0 ? `${language} (${count.toLocaleString()})` : language,
      icon: iconMeta ? deviconUrl(iconMeta[0], iconMeta[1]) : undefined,
      title: iconMeta
        ? `${language}: ${count.toLocaleString()} indexed files`
        : `${language}: ${count.toLocaleString()} indexed files (no icon mapping)`,
    })
  }

  return out
}

export function getTopLanguages(info: LangInfo, limit = 3) {
  return getTechStackBadges(info)
    .filter(badge => badge.key.startsWith('lib:') || badge.key.startsWith('lang:') || badge.key.startsWith('fw:'))
    .slice(0, limit)
}

export function sortProjectEntries(entries: ProjectEntry[]) {
  return [...entries].sort((a, b) => {
    const aLinked = a.links.length > 0 ? 1 : 0
    const bLinked = b.links.length > 0 ? 1 : 0
    if (aLinked !== bLinked) return bLinked - aLinked
    if (a.links.length !== b.links.length) return b.links.length - a.links.length
    return a.project.name.localeCompare(b.project.name)
  })
}

export function buildProjectLinks(entries: ProjectEntry[]): ProjectLink[] {
  const names = new Set(entries.map(entry => entry.project.name))
  const byLowerName = new Map(entries.map(entry => [entry.project.name.toLowerCase(), entry.project.name]))
  const seen = new Set<string>()
  const out: ProjectLink[] = []

  for (const entry of entries) {
    for (const linkedName of entry.links) {
      const resolvedSource = byLowerName.get(entry.project.name.toLowerCase()) ?? entry.project.name
      const resolvedTarget = byLowerName.get(linkedName.trim().toLowerCase()) ?? linkedName
      if (!names.has(resolvedSource) || !names.has(resolvedTarget) || resolvedSource === resolvedTarget) continue
      const key = [resolvedSource, resolvedTarget].sort().join('::')
      if (seen.has(key)) continue
      seen.add(key)
      out.push({ key, a: resolvedSource, b: resolvedTarget })
    }
  }

  return out.sort((a, b) => a.key.localeCompare(b.key))
}

export function projectLogoSrc(projectName: string) {
  return `/api/logo?project=${encodeURIComponent(projectName)}`
}

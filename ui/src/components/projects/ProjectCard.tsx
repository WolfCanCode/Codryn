import { ArrowRight, Loader2, RotateCcw, Trash2 } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

import { ProjectAvatar } from './ProjectAvatar'
import {
  GRAPH_TYPE_PREVIEW,
  LABEL_COLORS,
  formatDate,
  getTechStackBadges,
  type ProjectEntry,
} from './shared'

interface ProjectCardProps {
  entry: ProjectEntry
  reindexing: boolean
  reindexResult?: string
  onOpen: () => void
  onReindex: () => void
  onDelete: () => void
}

export function ProjectCard({
  entry,
  reindexing,
  reindexResult,
  onOpen,
  onReindex,
  onDelete,
}: ProjectCardProps) {
  const { project, schema, links, languages } = entry
  const techBadges = getTechStackBadges(languages)
  const sortedLabels = [...schema.node_labels].sort((a, b) => b.count - a.count)
  const typePreview = sortedLabels.slice(0, GRAPH_TYPE_PREVIEW)
  const typeOverflow = sortedLabels.length - typePreview.length

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault()
          onOpen()
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

      <div className="flex gap-2.5 pr-14">
        <ProjectAvatar projectName={project.name} className="h-9 w-9 rounded-lg" />
        <div className="min-w-0 flex-1 space-y-0.5">
          <h3 className="truncate text-sm font-semibold tracking-tight text-foreground">{project.name}</h3>
          <p className="truncate font-mono text-[10px] leading-snug text-muted-foreground" title={project.root_path}>
            {project.root_path}
          </p>
        </div>
      </div>

      {links.length > 0 ? (
        <div className="mt-2">
          <div className="mb-1 text-[9px] font-semibold uppercase tracking-wide text-muted-foreground">Linked</div>
          <div className="flex flex-wrap gap-1">
            {links.map(linkedProject => (
              <Badge
                key={linkedProject}
                variant="outline"
                className="max-w-full bg-emerald-50/80 text-[10px] text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300"
              >
                <span className="truncate">{linkedProject}</span>
              </Badge>
            ))}
          </div>
        </div>
      ) : null}

      {techBadges.length > 0 ? (
        <div className="mt-2.5">
          <div className="mb-1 text-[9px] font-semibold uppercase tracking-wide text-muted-foreground">Stack</div>
          <div className="flex flex-wrap gap-1">
            {techBadges.map(badge => (
              <span
                key={badge.key}
                title={badge.title}
                className="inline-flex max-w-full items-center gap-1 rounded border border-border/80 bg-background px-1.5 py-px text-[10px] font-medium text-foreground"
              >
                {badge.icon ? (
                  <img
                    src={badge.icon}
                    alt=""
                    className="h-3 w-3 shrink-0 opacity-90"
                    loading="lazy"
                    decoding="async"
                  />
                ) : null}
                <span className="truncate">{badge.label}</span>
              </span>
            ))}
          </div>
        </div>
      ) : null}

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

      {typePreview.length > 0 ? (
        <div className="mt-2">
          <div className="mb-1 text-[9px] font-semibold uppercase tracking-wide text-muted-foreground">In graph</div>
          <div className="flex flex-wrap gap-1">
            {typePreview.map(label => (
              <span
                key={label.label}
                className="inline-flex items-center gap-1 rounded border border-border/60 bg-muted/40 px-1.5 py-px text-[10px]"
              >
                <span
                  className="h-1.5 w-1.5 shrink-0 rounded-full"
                  style={{ backgroundColor: LABEL_COLORS[label.label] ?? '#94a3b8' }}
                />
                <span className="font-medium tabular-nums text-foreground">{label.count.toLocaleString()}</span>
                <span className="text-muted-foreground">{label.label}</span>
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
        <time className="text-[10px] text-muted-foreground tabular-nums" dateTime={project.indexed_at}>
          {formatDate(project.indexed_at)}
        </time>
        <div className="flex gap-0">
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7 text-muted-foreground hover:text-foreground"
            disabled={reindexing}
            onClick={(event) => {
              event.stopPropagation()
              onReindex()
            }}
            aria-label={`Re-index ${project.name}`}
          >
            {reindexing ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RotateCcw className="h-3.5 w-3.5" />}
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
            onClick={(event) => {
              event.stopPropagation()
              onDelete()
            }}
            aria-label={`Delete ${project.name}`}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      {reindexResult ? (
        <p
          className={cn(
            'mt-1.5 text-[10px]',
            reindexResult === 'done' ? 'text-emerald-600 dark:text-emerald-500' : 'text-destructive',
          )}
        >
          {reindexResult === 'done' ? 'Re-indexed successfully' : reindexResult}
        </p>
      ) : null}
    </div>
  )
}

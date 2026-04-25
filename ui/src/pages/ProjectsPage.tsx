import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import {
  AlertTriangle,
  ArrowDown,
  FolderOpen,
  Link2,
  Loader2,
  RefreshCw,
  Unlink,
} from 'lucide-react'

import { ProjectCard } from '@/components/projects/ProjectCard'
import { ProjectRelationshipCanvas } from '@/components/projects/ProjectRelationshipCanvas'
import {
  buildProjectLinks,
  normalizeLangInfo,
  sortProjectEntries,
  type ProjectEntry,
} from '@/components/projects/shared'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { callTool } from '@/lib/rpc'
import type { Project, SchemaInfo } from '@/lib/types'

export default function ProjectsPage() {
  const navigate = useNavigate()
  const [allProjects, setAllProjects] = useState<ProjectEntry[]>([])
  const [activeTab, setActiveTab] = useState('projects')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [reindexing, setReindexing] = useState<Record<string, boolean>>({})
  const [reindexResult, setReindexResult] = useState<Record<string, string>>({})
  const [deleteTarget, setDeleteTarget] = useState('')
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false)
  const [linkDialogOpen, setLinkDialogOpen] = useState(false)
  const [dialogSource, setDialogSource] = useState('')
  const [dialogTarget, setDialogTarget] = useState('')
  const [unlinkPair, setUnlinkPair] = useState<{ a: string; b: string } | null>(null)

  const refresh = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      const { projects } = await callTool<{ projects: Project[] }>('list_projects')
      const entries = await Promise.all(
        projects.map(async (project) => {
          const [schema, linksResult, languages] = await Promise.all([
            callTool<SchemaInfo>('get_graph_schema', { project: project.name }),
            callTool<{ linked_projects: string[] }>('list_project_links', { project: project.name }),
            fetch(`/api/languages?project=${encodeURIComponent(project.name)}`)
              .then(response => response.json())
              .then(normalizeLangInfo)
              .catch(() => normalizeLangInfo(null)),
          ])
          return {
            project,
            schema,
            links: linksResult.linked_projects ?? [],
            languages,
          }
        }),
      )
      setAllProjects(entries)
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const allLinks = useMemo(() => buildProjectLinks(allProjects), [allProjects])
  const sortedProjects = useMemo(() => sortProjectEntries(allProjects), [allProjects])

  const dialogTargetOptions = useMemo(() => {
    if (!dialogSource) return allProjects.map(entry => entry.project.name)
    const sourceEntry = allProjects.find(entry => entry.project.name === dialogSource)
    const linked = new Set(sourceEntry?.links ?? [])
    return allProjects.map(entry => entry.project.name).filter(name => name !== dialogSource && !linked.has(name))
  }, [allProjects, dialogSource])

  const dialogAlreadyLinked = useMemo(() => {
    if (!dialogSource || !dialogTarget) return false
    const sourceEntry = allProjects.find(entry => entry.project.name === dialogSource)
    return sourceEntry?.links.includes(dialogTarget) ?? false
  }, [allProjects, dialogSource, dialogTarget])

  const doReindex = async (name: string, rootPath: string) => {
    setReindexing(current => ({ ...current, [name]: true }))
    setReindexResult(current => ({ ...current, [name]: '' }))
    try {
      await callTool('index_repository', { path: rootPath })
      setReindexResult(current => ({ ...current, [name]: 'done' }))
      await refresh()
    } catch (cause: unknown) {
      setReindexResult(current => ({ ...current, [name]: cause instanceof Error ? cause.message : 'error' }))
    } finally {
      setReindexing(current => ({ ...current, [name]: false }))
    }
  }

  const doDelete = async () => {
    if (!deleteTarget) return
    try {
      await callTool('delete_project', { project: deleteTarget })
      setDeleteDialogOpen(false)
      setDeleteTarget('')
      await refresh()
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause))
      setDeleteDialogOpen(false)
    }
  }

  const doLink = async (source = dialogSource, target = dialogTarget) => {
    if (!source || !target) return
    try {
      await callTool('link_project', { project: source, target_project: target, action: 'link' })
      setLinkDialogOpen(false)
      setDialogSource('')
      setDialogTarget('')
      await refresh()
      setActiveTab('relationship')
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const doUnlink = async (a: string, b: string) => {
    try {
      await callTool('link_project', { project: a, target_project: b, action: 'unlink' })
      await refresh()
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-32">
        <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
      </div>
    )
  }

  if (error) {
    return (
      <Alert variant="destructive" className="mx-auto mt-12 max-w-lg">
        <AlertDescription>{error}</AlertDescription>
      </Alert>
    )
  }

  if (allProjects.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-4 py-32 text-muted-foreground">
        <FolderOpen className="h-16 w-16" />
        <p className="text-lg">No indexed projects yet</p>
        <Button onClick={() => navigate('/config')}>Index a project</Button>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
        <div className="space-y-1">
          <h2 className="text-xl font-semibold text-slate-950">Indexed Projects</h2>
          <p className="text-sm text-muted-foreground">
            Browse project cards or switch to the relationship canvas to link and unlink clusters.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {allProjects.length >= 2 ? (
            <Dialog open={linkDialogOpen} onOpenChange={setLinkDialogOpen}>
              <DialogTrigger render={<Button variant="outline" size="sm" />}>
                <Link2 className="mr-1.5 h-4 w-4" />
                Link Projects
              </DialogTrigger>
              <DialogContent className="sm:max-w-md">
                <DialogHeader>
                  <DialogTitle>Link Projects</DialogTitle>
                </DialogHeader>
                <div className="space-y-4 pt-2">
                  <Select
                    value={dialogSource}
                    onValueChange={(value) => {
                      setDialogSource(value ?? '')
                      setDialogTarget('')
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Source project" />
                    </SelectTrigger>
                    <SelectContent>
                      {allProjects.map(entry => (
                        <SelectItem key={entry.project.name} value={entry.project.name}>
                          {entry.project.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>

                  <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
                    <ArrowDown className="h-4 w-4" />
                    links to
                    <ArrowDown className="h-4 w-4" />
                  </div>

                  <Select
                    value={dialogTarget}
                    onValueChange={value => setDialogTarget(value ?? '')}
                    disabled={!dialogSource}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Target project" />
                    </SelectTrigger>
                    <SelectContent>
                      {dialogTargetOptions.map(name => (
                        <SelectItem key={name} value={name}>
                          {name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>

                  {dialogSource && dialogTarget ? (
                    <div className="rounded-lg border p-3 text-center text-sm">
                      <span className="font-medium">{dialogSource}</span> ↔{' '}
                      <span className="font-medium">{dialogTarget}</span>
                    </div>
                  ) : null}

                  {dialogAlreadyLinked ? (
                    <Alert>
                      <AlertTriangle className="h-4 w-4" />
                      <AlertDescription>These projects are already linked.</AlertDescription>
                    </Alert>
                  ) : null}

                  {allLinks.length > 0 ? (
                    <div className="space-y-1">
                      <p className="text-xs font-medium text-muted-foreground">Existing links</p>
                      {allLinks.map(link => (
                        <div key={link.key} className="flex items-center justify-between rounded border px-3 py-1.5 text-sm">
                          <span>
                            {link.a} ↔ {link.b}
                          </span>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-6 w-6"
                            onClick={() => setUnlinkPair({ a: link.a, b: link.b })}
                          >
                            <Unlink className="h-3 w-3" />
                          </Button>
                        </div>
                      ))}
                    </div>
                  ) : null}

                  <div className="flex justify-end gap-2 pt-2">
                    <Button variant="outline" onClick={() => setLinkDialogOpen(false)}>
                      Cancel
                    </Button>
                    <Button disabled={!dialogSource || !dialogTarget || dialogAlreadyLinked} onClick={() => void doLink()}>
                      Link
                    </Button>
                  </div>
                </div>
              </DialogContent>
            </Dialog>
          ) : null}

          <Button variant="outline" size="sm" onClick={() => void refresh()}>
            <RefreshCw className="mr-1.5 h-4 w-4" />
            Refresh
          </Button>
        </div>
      </div>

      <Tabs value={activeTab} onValueChange={setActiveTab} className="gap-4">
        <TabsList className="bg-slate-200/70 p-1">
          <TabsTrigger value="projects" className="min-w-28">
            Projects
          </TabsTrigger>
          <TabsTrigger value="relationship" className="min-w-28">
            Relationship
          </TabsTrigger>
        </TabsList>

        <TabsContent value="projects" className="mt-0">
          <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {sortedProjects.map(entry => (
              <ProjectCard
                key={entry.project.name}
                entry={entry}
                reindexing={Boolean(reindexing[entry.project.name])}
                reindexResult={reindexResult[entry.project.name]}
                onOpen={() => navigate(`/graph?project=${encodeURIComponent(entry.project.name)}`)}
                onReindex={() => void doReindex(entry.project.name, entry.project.root_path)}
                onDelete={() => {
                  setDeleteTarget(entry.project.name)
                  setDeleteDialogOpen(true)
                }}
              />
            ))}
          </div>
        </TabsContent>

        <TabsContent value="relationship" className="mt-0">
          <ProjectRelationshipCanvas
            entries={sortedProjects}
            links={allLinks}
            onLinkProjects={doLink}
            onRequestUnlink={(a, b) => setUnlinkPair({ a, b })}
            onOpenProject={(projectName) => navigate(`/graph?project=${encodeURIComponent(projectName)}`)}
          />
        </TabsContent>
      </Tabs>

      <Dialog open={deleteDialogOpen} onOpenChange={setDeleteDialogOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Delete Project</DialogTitle>
          </DialogHeader>
          <p className="py-2 text-sm">
            Are you sure you want to delete <span className="font-semibold">{deleteTarget}</span>? This removes all
            indexed data.
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setDeleteDialogOpen(false)}>
              Cancel
            </Button>
            <Button variant="destructive" onClick={() => void doDelete()}>
              Delete
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      <Dialog open={unlinkPair !== null} onOpenChange={(open) => (!open ? setUnlinkPair(null) : undefined)}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>Unlink projects</DialogTitle>
          </DialogHeader>
          <p className="py-2 text-sm">
            Remove the link between <span className="font-semibold">{unlinkPair?.a}</span> and{' '}
            <span className="font-semibold">{unlinkPair?.b}</span>?
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setUnlinkPair(null)}>
              Cancel
            </Button>
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

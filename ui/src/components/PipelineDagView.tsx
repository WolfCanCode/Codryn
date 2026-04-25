import type { PipelineDag, JobInfo, StageInfo } from '@/lib/types';
import { Badge } from '@/components/ui/badge';

function synthesizeStages(jobs: JobInfo[], stages: StageInfo[]) {
  if (stages.length > 0) return [...stages].sort((a, b) => a.order - b.order);
  const names = new Set(jobs.map((job) => job.stage || 'jobs'));
  return [...names].map((name, index) => ({ name, order: index }));
}

export function PipelineDagView({ dag }: { dag: PipelineDag | null }) {
  if (!dag) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-zinc-500">
        Select a pipeline
      </div>
    );
  }

  const stages = synthesizeStages(dag.jobs, dag.stages);
  const jobsByStage = new Map<string, JobInfo[]>();
  for (const job of dag.jobs) {
    const stage = job.stage || stages[0]?.name || 'jobs';
    jobsByStage.set(stage, [...(jobsByStage.get(stage) ?? []), job]);
  }

  return (
    <div className="h-full overflow-auto bg-zinc-50 p-4">
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold text-zinc-900">{dag.pipeline.name}</h3>
        {dag.pipeline.ci_system && <Badge variant="secondary">{dag.pipeline.ci_system}</Badge>}
        {dag.pipeline.triggers.map((trigger) => (
          <Badge key={trigger} variant="outline" className="text-[10px]">{trigger}</Badge>
        ))}
        <span className="text-xs text-zinc-500">{dag.pipeline.file_path}</span>
      </div>

      <div className="flex min-w-max gap-4">
        {stages.map((stage) => {
          const jobs = jobsByStage.get(stage.name) ?? [];
          return (
            <section key={stage.name} className="w-64 shrink-0 rounded-md border border-zinc-200 bg-white">
              <div className="border-b border-zinc-200 px-3 py-2">
                <div className="text-xs font-semibold text-zinc-900">{stage.name}</div>
                <div className="text-[10px] text-zinc-500">{jobs.length} jobs</div>
              </div>
              <div className="space-y-2 p-2">
                {jobs.length === 0 ? (
                  <div className="rounded border border-dashed border-zinc-200 p-3 text-center text-[11px] text-zinc-400">
                    No jobs
                  </div>
                ) : jobs.map((job) => (
                  <article key={job.name} className="rounded-md border border-zinc-200 bg-zinc-50 p-2">
                    <div className="truncate text-xs font-medium text-zinc-900">{job.name}</div>
                    {job.image && <div className="mt-1 truncate text-[10px] text-zinc-500">{job.image}</div>}
                    {job.dependencies.length > 0 && (
                      <div className="mt-2 flex flex-wrap gap-1">
                        {job.dependencies.map((dep) => (
                          <Badge key={dep} variant="outline" className="text-[9px]">{dep}</Badge>
                        ))}
                      </div>
                    )}
                  </article>
                ))}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}

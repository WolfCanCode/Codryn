import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import type { FlowNode, SankeyGraph, SankeyLane } from './flowTypes';
import type { FlowMode } from './flowLane';

type Props = {
  mode: FlowMode;
  anchorTitle: string;
  confidence?: number;
  graph: SankeyGraph | null;
  flowNodesBySankeyId: Map<string, FlowNode>;
  className?: string;
};

function laneLabel(lane: SankeyLane): string {
  switch (lane) {
    case 'route':
      return 'Route';
    case 'controller':
      return 'Controller';
    case 'service':
      return 'Service';
    case 'repository':
      return 'Repository';
    case 'dto':
      return 'DTO';
    case 'component':
      return 'Component';
    case 'framework':
      return 'Framework';
    case 'unknown':
      return 'Unknown';
    default:
      return String(lane);
  }
}

function formatConfidencePct(raw: number): string {
  if (!Number.isFinite(raw)) return '';
  const v = raw > 1 ? raw / 100 : raw;
  const pct = Math.round(Math.max(0, Math.min(1, v)) * 100);
  return `${pct}%`;
}

function filePathForSankeyId(
  sankeyId: string,
  flowNodesBySankeyId: Map<string, FlowNode>,
  fallback?: string,
): string | undefined {
  const fp = flowNodesBySankeyId.get(sankeyId)?.file_path ?? fallback;
  const s = (fp ?? '').trim();
  return s ? s : undefined;
}

export function OnboardingExtract({ mode, anchorTitle, confidence, graph, flowNodesBySankeyId, className }: Props) {
  const markdown = useMemo(() => {
    const title = (anchorTitle ?? '').trim() || 'Untitled';
    const lines: string[] = [];

    lines.push(`## Flow: ${title}`);
    lines.push('');

    lines.push('### Summary');
    if (typeof confidence === 'number') {
      const pct = formatConfidencePct(confidence);
      if (pct) lines.push(`- confidence: ${pct}`);
    }
    lines.push(`- mode: ${mode}`);
    lines.push('');

    if (!graph || graph.nodes.length === 0) {
      lines.push('_No flow graph available._');
      lines.push('');
      return lines.join('\n');
    }

    const filePaths = new Set<string>();
    for (const n of graph.nodes) {
      const fp = filePathForSankeyId(n.id, flowNodesBySankeyId, n.file_path);
      if (fp) filePaths.add(fp);
    }

    const sortedFiles = [...filePaths].sort((a, b) => a.localeCompare(b));

    lines.push('### Key files');
    if (sortedFiles.length === 0) {
      lines.push('- (none)');
    } else {
      for (const fp of sortedFiles) lines.push(`- \`${fp}\``);
    }
    lines.push('');

    lines.push('### Key symbols by lane');
    const nodesByLane = new Map<SankeyLane, { title: string; file_path?: string }[]>();
    for (const n of graph.nodes) {
      if (!nodesByLane.has(n.lane)) nodesByLane.set(n.lane, []);
      nodesByLane.get(n.lane)!.push({ title: n.title, file_path: n.file_path });
    }

    const laneOrder: SankeyLane[] = graph.lanes?.length ? [...graph.lanes] : (Array.from(nodesByLane.keys()) as SankeyLane[]);
    for (const lane of laneOrder) {
      const items = nodesByLane.get(lane) ?? [];
      if (items.length === 0) continue;

      const names = items.map((i) => i.title);
      const capped = names.slice(0, 20);
      lines.push(`- **${laneLabel(lane)}**: ${capped.join(', ')}${names.length > capped.length ? '…' : ''}`);
    }
    lines.push('');

    lines.push('### Suggested next reads');
    const suggested = new Set<string>();

    const orderedNodes = [...graph.nodes].filter((n) => n.lane !== 'unknown').sort((a, b) => {
      const ai = laneOrder.indexOf(a.lane);
      const bi = laneOrder.indexOf(b.lane);
      const aIdx = ai === -1 ? Number.MAX_SAFE_INTEGER : ai;
      const bIdx = bi === -1 ? Number.MAX_SAFE_INTEGER : bi;
      if (aIdx !== bIdx) return aIdx - bIdx;
      const t = a.title.localeCompare(b.title);
      if (t !== 0) return t;
      return a.id.localeCompare(b.id);
    });

    for (const n of orderedNodes) {
      if (suggested.size >= 12) break;
      const fp = filePathForSankeyId(n.id, flowNodesBySankeyId, n.file_path);
      if (!fp) continue;
      if (!suggested.has(fp)) suggested.add(fp);
    }

    if (suggested.size === 0) {
      lines.push('- (none)');
    } else {
      for (const fp of suggested) lines.push(`- \`${fp}\``);
    }
    lines.push('');

    return lines.join('\n');
  }, [anchorTitle, confidence, flowNodesBySankeyId, graph, mode]);

  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (timeoutRef.current != null) window.clearTimeout(timeoutRef.current);
    };
  }, []);

  const onCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(markdown);
      setCopied(true);
      if (timeoutRef.current != null) window.clearTimeout(timeoutRef.current);
      timeoutRef.current = window.setTimeout(() => setCopied(false), 900);
    } catch {
      // ignore
    }
  }, [markdown]);

  return (
    <aside className={cn('flex h-full w-full flex-col gap-3 border-l bg-white p-4', className)}>
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="truncate text-sm font-medium text-foreground">Onboarding extract</div>
          <div className="truncate text-xs text-muted-foreground">{anchorTitle}</div>
        </div>
        <Button size="sm" variant="secondary" onClick={onCopy} className="shrink-0">
          {copied ? 'Copied' : 'Copy Markdown'}
        </Button>
      </div>

      <pre className="flex-1 overflow-auto whitespace-pre-wrap rounded-md border bg-background p-3 text-[11px] leading-4 text-foreground">
        {markdown}
      </pre>
    </aside>
  );
}

